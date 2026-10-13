//! Owned Unix process execution with nonblocking, bounded byte transport.
//!
//! The deadline covers stdin delivery, child execution and pipe EOF, starting
//! before spawn. Spawn/reap are OS operations, not hard-real-time guarantees.
//! Cleanup owns the direct child and its initial group, not arbitrary descendants
//! that deliberately escape that group. There are no detached I/O threads.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub struct ExecutionLimits {
    pub deadline: Duration,
    /// Maximum retained bytes per stream. Exceeding it fails the invocation.
    pub output_bytes: usize,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            deadline: Duration::from_secs(10),
            output_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug)]
pub enum ProcessErrorKind {
    InvalidInvocation(String),
    Io {
        operation: &'static str,
        source: io::Error,
    },
    InputClosed,
    OutputLimit(OutputStream),
    Deadline,
    Cancelled,
    Cleanup,
}

#[derive(Debug, Default)]
pub struct CapturedOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdin_written: usize,
    pub elapsed: Duration,
}

#[derive(Debug)]
pub struct ProcessOutput {
    pub status: ExitStatus,
    pub capture: CapturedOutput,
}

#[derive(Debug)]
pub struct ProcessError {
    pub kind: ProcessErrorKind,
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub capture: CapturedOutput,
    pub status: Option<ExitStatus>,
    pub cleanup_errors: Vec<String>,
}

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "process infrastructure failure: {:?}\nprogram: {:?}\nargs: {:?}\nstatus: {:?}\nstdin delivered: {} bytes; elapsed: {:?}\nstdout: {:?}\nstderr: {:?}",
            self.kind,
            self.program,
            self.args,
            self.status,
            self.capture.stdin_written,
            self.capture.elapsed,
            &self.capture.stdout[..self.capture.stdout.len().min(4096)],
            &self.capture.stderr[..self.capture.stderr.len().min(4096)],
        )?;
        if !self.cleanup_errors.is_empty() {
            write!(f, "\ncleanup errors: {:?}", self.cleanup_errors)?;
        }
        Ok(())
    }
}

impl std::error::Error for ProcessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ProcessErrorKind::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// An explicit executable, working directory and child-only environment.
/// No environment is inherited. Arguments and streams preserve Unix bytes.
#[derive(Debug)]
pub struct ProcessCommand {
    program: PathBuf,
    cwd: PathBuf,
    args: Vec<OsString>,
    env: BTreeMap<OsString, OsString>,
    stdin: Vec<u8>,
    limits: ExecutionLimits,
    cancellation: Option<Arc<AtomicBool>>,
}

impl ProcessCommand {
    pub fn new(program: impl AsRef<Path>, cwd: impl AsRef<Path>) -> Self {
        Self {
            program: program.as_ref().to_path_buf(),
            cwd: cwd.as_ref().to_path_buf(),
            args: Vec::new(),
            env: BTreeMap::new(),
            stdin: Vec::new(),
            limits: ExecutionLimits::default(),
            cancellation: None,
        }
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|s| s.as_ref().to_owned()));
        self
    }

    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.env
            .insert(key.as_ref().to_owned(), value.as_ref().to_owned());
        self
    }

    pub fn stdin(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.stdin = bytes.into();
        self
    }

    pub fn limits(mut self, limits: ExecutionLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn cancellation(mut self, flag: Arc<AtomicBool>) -> Self {
        self.cancellation = Some(flag);
        self
    }

    /// Execute once. A completed nonzero exit/signal is an observation, not an
    /// infrastructure failure. All I/O, deadline and cleanup failures are errors.
    pub fn run(&self) -> Result<ProcessOutput, Box<ProcessError>> {
        let started = Instant::now();
        let mut capture = CapturedOutput::default();
        if let Err(kind) = self.validate() {
            return Err(self.failure(kind, capture, None, Vec::new(), started));
        }
        if self.cancelled() {
            return Err(self.failure(
                ProcessErrorKind::Cancelled,
                capture,
                None,
                Vec::new(),
                started,
            ));
        }
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .current_dir(&self.cwd)
            .env_clear()
            .envs(&self.env);
        command.process_group(0);
        command.stdin(if self.stdin.is_empty() {
            Stdio::null()
        } else {
            Stdio::piped()
        });
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let child = command.spawn().map_err(|source| {
            self.failure(
                ProcessErrorKind::Io {
                    operation: "spawn",
                    source,
                },
                CapturedOutput::default(),
                None,
                Vec::new(),
                started,
            )
        })?;
        let mut owner = OwnedChild::new(child);
        let mut input = owner.child.stdin.take();
        let mut stdout = owner.child.stdout.take();
        let mut stderr = owner.child.stderr.take();
        let result = (|| {
            for fd in [
                input.as_ref().map(AsRawFd::as_raw_fd),
                stdout.as_ref().map(AsRawFd::as_raw_fd),
                stderr.as_ref().map(AsRawFd::as_raw_fd),
            ]
            .into_iter()
            .flatten()
            {
                nonblocking(fd).map_err(|source| ProcessErrorKind::Io {
                    operation: "set nonblocking",
                    source,
                })?;
            }
            loop {
                if self.cancelled() {
                    return Err(ProcessErrorKind::Cancelled);
                }
                if started.elapsed() >= self.limits.deadline {
                    return Err(ProcessErrorKind::Deadline);
                }
                if !owner.exited {
                    owner.exited =
                        child_exited(owner.child.id()).map_err(|source| ProcessErrorKind::Io {
                            operation: "poll child",
                            source,
                        })?;
                }
                drain(
                    &mut stdout,
                    &mut capture.stdout,
                    self.limits.output_bytes,
                    OutputStream::Stdout,
                )?;
                drain(
                    &mut stderr,
                    &mut capture.stderr,
                    self.limits.output_bytes,
                    OutputStream::Stderr,
                )?;
                if let Some(writer) = input.as_mut() {
                    let remaining = &self.stdin[capture.stdin_written..];
                    match writer.write(&remaining[..remaining.len().min(64 * 1024)]) {
                        Ok(0) => return Err(ProcessErrorKind::InputClosed),
                        Ok(n) => capture.stdin_written += n,
                        Err(e)
                            if e.kind() == io::ErrorKind::WouldBlock
                                || e.kind() == io::ErrorKind::Interrupted => {}
                        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                            return Err(ProcessErrorKind::InputClosed);
                        }
                        Err(source) => {
                            return Err(ProcessErrorKind::Io {
                                operation: "write stdin",
                                source,
                            });
                        }
                    }
                    if capture.stdin_written == self.stdin.len() {
                        input = None; // Deliver EOF immediately, not after waiting for exit.
                    }
                }
                if owner.exited && stdout.is_none() && stderr.is_none() && input.is_none() {
                    return Ok(());
                }
                let mut fds = Vec::with_capacity(3);
                for (fd, events) in [
                    (input.as_ref().map(AsRawFd::as_raw_fd), libc::POLLOUT),
                    (stdout.as_ref().map(AsRawFd::as_raw_fd), libc::POLLIN),
                    (stderr.as_ref().map(AsRawFd::as_raw_fd), libc::POLLIN),
                ] {
                    if let Some(fd) = fd {
                        fds.push(libc::pollfd {
                            fd,
                            events,
                            revents: 0,
                        });
                    }
                }
                let remaining = self.limits.deadline.saturating_sub(started.elapsed());
                let wait = remaining.as_millis().clamp(1, 20) as i32;
                // SAFETY: fds owns valid pollfd elements for all still-owned pipes.
                let rc = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, wait) };
                if rc < 0 {
                    let source = io::Error::last_os_error();
                    if source.kind() != io::ErrorKind::Interrupted {
                        return Err(ProcessErrorKind::Io {
                            operation: "poll pipes",
                            source,
                        });
                    }
                }
            }
        })();
        drop(input);
        drop(stdout);
        drop(stderr);
        let cleanup_errors = owner.cleanup();
        capture.elapsed = started.elapsed();
        match result {
            Err(kind) => Err(self.failure(kind, capture, owner.status, cleanup_errors, started)),
            Ok(()) if !cleanup_errors.is_empty() => Err(self.failure(
                ProcessErrorKind::Cleanup,
                capture,
                owner.status,
                cleanup_errors,
                started,
            )),
            Ok(()) => match owner.status {
                Some(status) => Ok(ProcessOutput { status, capture }),
                None => Err(self.failure(
                    ProcessErrorKind::Cleanup,
                    capture,
                    None,
                    vec!["child status missing after cleanup".into()],
                    started,
                )),
            },
        }
    }

    fn cancelled(&self) -> bool {
        self.cancellation
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
    }

    fn validate(&self) -> Result<(), ProcessErrorKind> {
        let invalid = |reason: &str| ProcessErrorKind::InvalidInvocation(reason.into());
        if !self.program.is_absolute() || !self.cwd.is_absolute() {
            return Err(invalid("executable and cwd must be absolute"));
        }
        if self.limits.deadline.is_zero() || self.limits.output_bytes == 0 {
            return Err(invalid("deadline and output limit must be positive"));
        }
        let metadata = std::fs::metadata(&self.program).map_err(|source| ProcessErrorKind::Io {
            operation: "validate executable",
            source,
        })?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(invalid("executable must be an executable regular file"));
        }
        if !self.cwd.is_dir() {
            return Err(invalid("cwd must be an existing directory"));
        }
        Ok(())
    }

    fn failure(
        &self,
        kind: ProcessErrorKind,
        mut capture: CapturedOutput,
        status: Option<ExitStatus>,
        cleanup_errors: Vec<String>,
        started: Instant,
    ) -> Box<ProcessError> {
        capture.elapsed = started.elapsed();
        Box::new(ProcessError {
            kind,
            program: self.program.clone(),
            args: self.args.clone(),
            capture,
            status,
            cleanup_errors,
        })
    }
}

fn nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: caller retains ownership of the live descriptor throughout setup.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn drain<R: Read>(
    reader: &mut Option<R>,
    output: &mut Vec<u8>,
    limit: usize,
    stream: OutputStream,
) -> Result<(), ProcessErrorKind> {
    let Some(pipe) = reader.as_mut() else {
        return Ok(());
    };
    // Bounded work per cycle: an infinite producer cannot starve cancellation,
    // the other stream, stdin, child polling, or the deadline.
    let mut bytes = [0; 8192];
    for _ in 0..8 {
        match pipe.read(&mut bytes) {
            Ok(0) => {
                *reader = None;
                return Ok(());
            }
            Ok(n) => {
                let retain = n.min(limit - output.len());
                output.extend_from_slice(&bytes[..retain]);
                if retain < n {
                    return Err(ProcessErrorKind::OutputLimit(stream));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(source) => {
                return Err(ProcessErrorKind::Io {
                    operation: "read captured stream",
                    source,
                });
            }
        }
    }
    Ok(())
}

// Observe exit without reaping. Keeping the leader's PID reserved until group
// cleanup prevents a recycled PID from identifying an unrelated process group.
fn child_exited(pid: u32) -> io::Result<bool> {
    // SAFETY: initialized siginfo is a valid output buffer; WNOWAIT retains the
    // direct child for the single owner to reap after launch-group cleanup.
    let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
    let rc = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if rc < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok(false);
        }
        return Err(error);
    }
    // SAFETY: a successful waitid with WEXITED populates the child-status fields.
    Ok(unsafe { info.si_pid() } != 0)
}

struct OwnedChild {
    child: Child,
    exited: bool,
    status: Option<ExitStatus>,
    cleaned: bool,
}

impl OwnedChild {
    fn new(child: Child) -> Self {
        Self {
            child,
            exited: false,
            status: None,
            cleaned: false,
        }
    }

    fn cleanup(&mut self) -> Vec<String> {
        if self.cleaned {
            return Vec::new();
        }
        let mut errors = Vec::new();
        // SAFETY: process_group(0) creates a dedicated group led by this child.
        // This never signals the caller's group. ESRCH means the group is gone.
        let rc = unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
        if rc < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                errors.push(format!("kill launch group: {error}"));
            }
        }
        if !self.exited {
            // Also terminate the direct child if it changed its own group.
            if let Err(error) = self.child.kill() {
                // A child that exited between polling and kill is still reaped.
                if error.kind() != io::ErrorKind::InvalidInput {
                    errors.push(format!("kill child: {error}"));
                }
            }
        }
        loop {
            match self.child.wait() {
                Ok(status) => {
                    self.status = Some(status);
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    errors.push(format!("reap child: {error}"));
                    break;
                }
            }
        }
        self.cleaned = true;
        errors
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_read_error_is_not_empty_output() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("injected read failure"))
            }
        }
        let mut output = b"partial".to_vec();
        let error = drain(&mut Some(Broken), &mut output, 100, OutputStream::Stdout)
            .expect_err("read must fail");
        assert!(matches!(
            error,
            ProcessErrorKind::Io {
                operation: "read captured stream",
                ..
            }
        ));
        assert_eq!(output, b"partial");
    }
}
