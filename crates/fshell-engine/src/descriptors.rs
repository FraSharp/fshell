// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! The shell's file-descriptor table.
//!
//! A POSIX shell names its I/O by small integers. A redirection rebinds one,
//! `n>&m` makes two names share one open file description (and therefore one
//! offset), `exec` makes a binding outlive the command, and every child starts
//! with a copy of the table. This module models exactly that.
//!
//! Every binding is a real open file description — a file, a pipe end, a
//! here-document's pipe — never an in-process imitation of one. That is what lets a builtin and a child share one input and consume
//! only what each actually reads (`while read l; do cmd; done < f`), and lets
//! two external stages of a pipeline exchange bytes without the shell copying
//! them.
//!
//! The table belongs to a shell execution environment: [`crate::Env`] owns one
//! and a subshell copies it. Nobody holds it across an `.await`; callers clone
//! the [`Descriptor`] they need, which is cheap and shares the description.

use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::Arc;

/// What one descriptor number refers to.
#[derive(Clone)]
pub enum Descriptor {
    /// A descriptor the shell process itself inherited (its 0, 1, 2).
    Process(RawFd),
    /// An open file description the shell owns: a file, a pipe end, a
    /// here-document. Duplicates share it, as dup(2) duplicates do.
    File(Arc<File>),
}

impl Descriptor {
    pub fn as_raw_fd(&self) -> RawFd {
        match self {
            Descriptor::Process(fd) => *fd,
            Descriptor::File(file) => file.as_raw_fd(),
        }
    }
}

impl std::fmt::Debug for Descriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Descriptor::Process(fd) => write!(f, "Process({fd})"),
            Descriptor::File(file) => write!(f, "File({})", file.as_raw_fd()),
        }
    }
}

/// Descriptor numbers bound in one shell execution environment. A number that
/// is absent is closed.
#[derive(Clone, Debug)]
pub struct DescriptorTable {
    entries: BTreeMap<i32, Descriptor>,
}

impl Default for DescriptorTable {
    fn default() -> Self {
        Self::standard()
    }
}

impl DescriptorTable {
    /// Standard input, output and error bound to the shell process's own.
    pub fn standard() -> Self {
        Self {
            entries: (0..=2).map(|fd| (fd, Descriptor::Process(fd))).collect(),
        }
    }

    pub fn get(&self, fd: i32) -> Option<&Descriptor> {
        self.entries.get(&fd)
    }

    /// Bind `fd`, returning what it was bound to before.
    pub fn set(&mut self, fd: i32, descriptor: Descriptor) -> Option<Descriptor> {
        self.entries.insert(fd, descriptor)
    }

    /// Bind `descriptor` to the lowest number at or above 3 that is free, and
    /// return that number. Child descriptors start at 3, so a name handed to a
    /// command (`/dev/fd/N`) stays out of the way of its standard streams.
    pub fn allocate(&mut self, descriptor: Descriptor) -> i32 {
        let mut fd = 3;
        while self.entries.contains_key(&fd) {
            fd += 1;
        }
        self.entries.insert(fd, descriptor);
        fd
    }

    /// Close `fd`, returning what it was bound to before.
    pub fn close(&mut self, fd: i32) -> Option<Descriptor> {
        self.entries.remove(&fd)
    }

    /// Restore `fd` to a binding previously returned by [`Self::set`] or
    /// [`Self::close`].
    pub fn restore(&mut self, fd: i32, previous: Option<Descriptor>) {
        match previous {
            Some(descriptor) => {
                self.entries.insert(fd, descriptor);
            }
            None => {
                self.entries.remove(&fd);
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (i32, &Descriptor)> {
        self.entries.iter().map(|(fd, d)| (*fd, d))
    }
}

/// A pipe, both ends close-on-exec.
pub fn pipe() -> io::Result<(File, File)> {
    let (read, write) = crate::create_cloexec_pipe()?;
    // SAFETY: both descriptors are fresh and owned only here.
    Ok(unsafe { (File::from_raw_fd(read), File::from_raw_fd(write)) })
}

/// A here-document: the read end of a pipe that yields `bytes`, as dash and
/// bash provide one. A reader that overshoots consumes what it read, exactly as
/// in those shells, and a command that never reads consumes nothing.
///
/// The write end never reaches a child, so it can be made non-blocking: what
/// fits in the pipe is written now, and a writer thread delivers the rest as
/// the reader drains it (ending with EPIPE if every reader goes away first).
pub fn here_document(bytes: &[u8]) -> io::Result<File> {
    let (read_end, write_end) = pipe()?;
    let fd = write_end.as_raw_fd();
    // SAFETY: fcntl on a descriptor this function owns.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    let mut written = 0;
    while written < bytes.len() {
        // SAFETY: the slice is valid for the remaining length.
        let n = unsafe { libc::write(fd, bytes[written..].as_ptr().cast(), bytes.len() - written) };
        if n >= 0 {
            written += n as usize;
            continue;
        }
        let error = io::Error::last_os_error();
        match error.kind() {
            io::ErrorKind::Interrupted => continue,
            io::ErrorKind::WouldBlock => break,
            _ => return Err(error),
        }
    }
    if written < bytes.len() {
        // SAFETY: as above; the writer thread blocks until the reader drains.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
        }
        let rest = bytes[written..].to_vec();
        std::thread::Builder::new()
            .name("fsh-heredoc".into())
            .spawn(move || {
                let _ = write_raw(write_end.as_raw_fd(), &rest);
            })?;
    }
    Ok(read_end)
}

/// Duplicate `fd` close-on-exec, at or above `minimum`.
fn duplicate_raw(fd: RawFd, minimum: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: F_DUPFD_CLOEXEC creates a fresh descriptor owned by the result.
    let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, minimum) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `duplicate` is a new descriptor nobody else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
}

fn is_regular_file(fd: RawFd) -> bool {
    // SAFETY: fstat writes into a zeroed stat buffer it is given.
    unsafe {
        let mut stat = std::mem::zeroed::<libc::stat>();
        libc::fstat(fd, &mut stat) == 0 && (stat.st_mode & libc::S_IFMT) == libc::S_IFREG
    }
}

fn write_raw(fd: RawFd, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        // SAFETY: the slice is valid for `bytes.len()` bytes.
        let n = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if n < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        bytes = &bytes[n as usize..];
    }
    Ok(())
}

/// Write every byte to `descriptor`.
///
/// A regular file is written directly. A pipe or terminal can block until its
/// reader catches up — possibly another stage of this very shell — so it is
/// written from the blocking pool, never from an async worker.
pub async fn write_all(descriptor: &Descriptor, bytes: &[u8]) -> io::Result<()> {
    let fd = descriptor.as_raw_fd();
    if is_regular_file(fd) {
        return write_raw(fd, bytes);
    }
    let owned = duplicate_raw(fd, 0)?;
    let bytes = bytes.to_vec();
    tokio::task::spawn_blocking(move || write_raw(owned.as_raw_fd(), &bytes))
        .await
        .map_err(io::Error::other)?
}

/// Read one line, including its newline when there is one. `None` is end of
/// input with nothing read.
///
/// Consumes exactly the line, so the rest stays readable by the next command:
/// a seekable input is read in blocks and the overshoot seeked back, anything
/// else a byte at a time — the only way not to consume past the line.
pub async fn read_line(descriptor: &Descriptor) -> io::Result<Option<Vec<u8>>> {
    let owned = duplicate_raw(descriptor.as_raw_fd(), 0)?;
    tokio::task::spawn_blocking(move || read_line_fd(owned.as_raw_fd()))
        .await
        .map_err(io::Error::other)?
}

fn read_line_fd(fd: RawFd) -> io::Result<Option<Vec<u8>>> {
    // SAFETY: lseek on a valid descriptor; failure only reports non-seekable.
    let seekable = unsafe { libc::lseek(fd, 0, libc::SEEK_CUR) } >= 0;
    let block = if seekable { 4096 } else { 1 };
    let mut line = Vec::new();
    let mut buf = vec![0u8; block];
    loop {
        // SAFETY: `buf` is valid for `block` bytes.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), block) };
        if n < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        let n = n as usize;
        if n == 0 {
            return Ok((!line.is_empty()).then_some(line));
        }
        if let Some(newline) = buf[..n].iter().position(|&b| b == b'\n') {
            line.extend_from_slice(&buf[..=newline]);
            let overshoot = (n - newline - 1) as libc::off_t;
            if overshoot > 0 {
                // SAFETY: seeking back over bytes this call just read.
                unsafe { libc::lseek(fd, -overshoot, libc::SEEK_CUR) };
            }
            return Ok(Some(line));
        }
        line.extend_from_slice(&buf[..n]);
    }
}

/// Read a pipe's read end to end of file without blocking an async worker.
///
/// The read end must be private to the shell: it is switched to non-blocking
/// mode, which would otherwise leak into every process sharing the
/// description.
pub async fn read_to_end(read_end: File) -> io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut receiver = tokio::net::unix::pipe::Receiver::from_owned_fd(OwnedFd::from(read_end))?;
    let mut bytes = Vec::new();
    receiver.read_to_end(&mut bytes).await?;
    Ok(bytes)
}

/// The descriptors a child process starts with, duplicated from a table.
pub struct ChildDescriptors {
    fds: Vec<(i32, OwnedFd)>,
}

impl ChildDescriptors {
    pub fn prepare(table: &DescriptorTable) -> io::Result<Self> {
        let mut fds = Vec::new();
        for (fd, descriptor) in table.iter() {
            match duplicate_raw(descriptor.as_raw_fd(), 0) {
                Ok(owned) => fds.push((fd, owned)),
                // The shell itself has it closed, so the child does too.
                Err(error) if error.raw_os_error() == Some(libc::EBADF) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(Self { fds })
    }

    /// Bind the prepared descriptors into `cmd`.
    ///
    /// Standard streams go through std's own stdio. A closed standard stream
    /// becomes `/dev/null`: std cannot start a child with 0–2 closed without a
    /// `pre_exec` hook, and a program finding them closed would reuse the
    /// numbers for its own files. Descriptors 3 and above need a hook, which
    /// makes std fork rather than `posix_spawn`; only such commands pay that.
    pub fn attach(self, cmd: &mut std::process::Command) -> io::Result<()> {
        use std::process::Stdio;
        let mut standard: [Option<OwnedFd>; 3] = [None, None, None];
        let mut extra = Vec::new();
        for (fd, owned) in self.fds {
            match fd {
                0..=2 => standard[fd as usize] = Some(owned),
                _ => extra.push((fd, owned)),
            }
        }
        let [stdin, stdout, stderr] = standard;
        cmd.stdin(stdin.map_or_else(Stdio::null, Stdio::from));
        cmd.stdout(stdout.map_or_else(Stdio::null, Stdio::from));
        cmd.stderr(stderr.map_or_else(Stdio::null, Stdio::from));
        if extra.is_empty() {
            return Ok(());
        }
        // Lift every source above every target first, so no dup2 in the child
        // can overwrite a source still waiting to be duplicated.
        let ceiling = extra.iter().map(|(fd, _)| *fd).max().unwrap_or(2) + 1;
        let lifted: Vec<(i32, OwnedFd)> = extra
            .into_iter()
            .map(|(fd, owned)| Ok((fd, duplicate_raw(owned.as_raw_fd(), ceiling)?)))
            .collect::<io::Result<_>>()?;
        let pairs: Vec<(i32, RawFd)> = lifted.iter().map(|(t, o)| (*t, o.as_raw_fd())).collect();
        use std::os::unix::process::CommandExt;
        // SAFETY: between fork and exec; dup2 is async-signal-safe and the
        // closure only reads data allocated before the fork. dup2 clears
        // close-on-exec on the target, so exactly these numbers are inherited.
        unsafe {
            cmd.pre_exec(move || {
                let _keep_alive = &lifted;
                for &(target, source) in &pairs {
                    if libc::dup2(source, target) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pipe_lines_are_consumed_one_at_a_time() {
        let (read, write) = pipe().expect("pipe");
        let input = Descriptor::File(Arc::new(read));
        let output = Descriptor::File(Arc::new(write));
        write_all(&output, b"one\ntwo\nthree").await.expect("write");
        drop(output);
        assert_eq!(
            read_line(&input).await.expect("line"),
            Some(b"one\n".to_vec())
        );
        assert_eq!(
            read_line(&input).await.expect("line"),
            Some(b"two\n".to_vec())
        );
        assert_eq!(
            read_line(&input).await.expect("line"),
            Some(b"three".to_vec())
        );
        assert_eq!(read_line(&input).await.expect("eof"), None);
    }

    #[tokio::test]
    async fn file_lines_share_one_offset_across_duplicates() {
        use std::io::{Seek, Write};
        let mut file = tempfile::tempfile().expect("temporary file");
        file.write_all(b"alpha\nbeta\n").expect("write");
        file.rewind().expect("rewind");
        let first = Descriptor::File(Arc::new(file));
        let duplicate = first.clone();
        assert_eq!(
            read_line(&first).await.expect("line"),
            Some(b"alpha\n".to_vec())
        );
        assert_eq!(
            read_line(&duplicate).await.expect("line"),
            Some(b"beta\n".to_vec())
        );
        assert_eq!(read_line(&first).await.expect("eof"), None);
    }

    #[tokio::test]
    async fn a_here_document_larger_than_a_pipe_is_delivered_whole() {
        let body = vec![b'x'; 1 << 20];
        let read_end = here_document(&body).expect("here-document");
        assert_eq!(read_to_end(read_end).await.expect("drain"), body);
    }

    #[tokio::test]
    async fn a_pipe_drains_to_end_of_file_once_every_writer_closes() {
        let (read, write) = pipe().expect("pipe");
        let writer = Descriptor::File(Arc::new(write));
        let second = writer.clone();
        write_all(&writer, b"ab").await.expect("write");
        write_all(&second, b"cd").await.expect("write");
        drop((writer, second));
        assert_eq!(read_to_end(read).await.expect("drain"), b"abcd");
    }
}
