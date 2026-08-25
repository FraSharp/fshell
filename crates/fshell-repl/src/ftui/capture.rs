// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::os::fd::FromRawFd;

// Gate: set FSH_REPL_ANCHOR_DEBUG=1 for capture debug logs
fn anchor_debug(msg: impl std::fmt::Display) {
    if std::env::var("FSH_REPL_ANCHOR_DEBUG").as_deref() == Ok("1")
        && let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty")
    {
        use std::io::Write;
        let _ = writeln!(tty, "[capture] {}", msg);
    }
}

/// Outcome of capture — callers must check `ok()` before using `lines`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureStatus {
    Ok,
    DupFailed,
    PipeFailed,
}

/// A RAII guard that redirects stdout/stderr to separate pipes and reads
/// captured output. Uses independent pipes for stdout and stderr to prevent
/// deadlock: if one pipe buffer fills up (e.g. stderr with debug output),
/// the other (stdout) can still drain independently.
///
/// On drop, stdout and stderr are restored to their original descriptors.
pub(crate) struct CaptureGuard {
    saved_stdout: Option<i32>,
    saved_stderr: Option<i32>,
    stdout_reader: Option<std::thread::JoinHandle<Vec<String>>>,
    stderr_reader: Option<std::thread::JoinHandle<Vec<String>>>,
    finished: bool,
    status: CaptureStatus,
    error_msg: Option<String>,
}

impl CaptureGuard {
    #[allow(dead_code)]
    pub(crate) fn status(&self) -> CaptureStatus {
        self.status
    }
    pub(crate) fn error(&self) -> Option<&str> {
        self.error_msg.as_deref()
    }
    pub(crate) fn ok(&self) -> bool {
        self.status == CaptureStatus::Ok
    }
}

/// Create a pipe and return (reader_fd, write_fd).
fn make_pipe() -> Option<(i32, i32)> {
    let mut fds: [i32; 2] = [0, 0];
    let ret = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if ret != 0 {
        None
    } else {
        Some((fds[0], fds[1]))
    }
}

/// Spawn a reader thread that reads lines from `read_fd` and collects them.
fn spawn_reader_thread(read_fd: i32) -> std::thread::JoinHandle<Vec<String>> {
    let mut reader = BufReader::new(unsafe { File::from_raw_fd(read_fd) });
    std::thread::spawn(move || {
        let mut collected = Vec::new();
        let mut buf = String::new();
        loop {
            buf.clear();
            match reader.read_line(&mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = strip_ansi_escapes(buf.trim_end_matches('\n'));
                    if !trimmed.is_empty() {
                        collected.push(trimmed);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        collected
    })
}

impl CaptureGuard {
    pub(crate) fn new() -> Self {
        anchor_debug("CaptureGuard::new() start");

        // Save original stdout/stderr
        let saved_stdout = unsafe { libc::dup(libc::STDOUT_FILENO) };
        let saved_stderr = unsafe { libc::dup(libc::STDERR_FILENO) };
        if saved_stdout < 0 || saved_stderr < 0 {
            let err = std::io::Error::last_os_error();
            anchor_debug(format!("dup() failed: {}", err));
            if saved_stdout >= 0 {
                unsafe {
                    libc::close(saved_stdout);
                }
            }
            if saved_stderr >= 0 {
                unsafe {
                    libc::close(saved_stderr);
                }
            }
            return Self {
                saved_stdout: None,
                saved_stderr: None,
                stdout_reader: None,
                stderr_reader: None,
                finished: false,
                status: CaptureStatus::DupFailed,
                error_msg: Some(format!("capture dup failed: {err}")),
            };
        }

        // Create separate pipes for stdout and stderr to prevent deadlock
        let (stdout_read, stdout_write) = match make_pipe() {
            Some(p) => p,
            None => {
                let err = std::io::Error::last_os_error();
                unsafe {
                    libc::close(saved_stdout);
                    libc::close(saved_stderr);
                }
                return Self {
                    saved_stdout: None,
                    saved_stderr: None,
                    stdout_reader: None,
                    stderr_reader: None,
                    finished: false,
                    status: CaptureStatus::PipeFailed,
                    error_msg: Some(format!("capture pipe failed: {err}")),
                };
            }
        };
        let (stderr_read, stderr_write) = match make_pipe() {
            Some(p) => p,
            None => {
                let err = std::io::Error::last_os_error();
                unsafe {
                    libc::close(stdout_read);
                    libc::close(stdout_write);
                    libc::close(saved_stdout);
                    libc::close(saved_stderr);
                }
                return Self {
                    saved_stdout: None,
                    saved_stderr: None,
                    stdout_reader: None,
                    stderr_reader: None,
                    finished: false,
                    status: CaptureStatus::PipeFailed,
                    error_msg: Some(format!("capture pipe failed: {err}")),
                };
            }
        };

        // Dup the read ends to stable fds so they survive dup2 redirecting 1/2
        let stdout_reader_fd = unsafe { libc::dup(stdout_read) };
        let stderr_reader_fd = unsafe { libc::dup(stderr_read) };
        unsafe {
            libc::close(stdout_read);
            libc::close(stderr_read);
        }

        if stdout_reader_fd < 0 || stderr_reader_fd < 0 {
            let err = std::io::Error::last_os_error();
            anchor_debug(format!("dup() of pipe read ends failed: {err}"));
            unsafe {
                libc::close(stdout_write);
                libc::close(stderr_write);
                libc::close(saved_stdout);
                libc::close(saved_stderr);
            }
            if stdout_reader_fd >= 0 {
                unsafe {
                    libc::close(stdout_reader_fd);
                }
            }
            if stderr_reader_fd >= 0 {
                unsafe {
                    libc::close(stderr_reader_fd);
                }
            }
            return Self {
                saved_stdout: None,
                saved_stderr: None,
                stdout_reader: None,
                stderr_reader: None,
                finished: false,
                status: CaptureStatus::DupFailed,
                error_msg: Some(format!("capture dup failed: {err}")),
            };
        }

        // Redirect stdout → stdout_write, stderr → stderr_write
        unsafe {
            libc::dup2(stdout_write, libc::STDOUT_FILENO);
            libc::dup2(stderr_write, libc::STDERR_FILENO);
            libc::close(stdout_write);
            libc::close(stderr_write);
        }

        // Spawn reader threads
        let stdout_reader = Some(spawn_reader_thread(stdout_reader_fd));
        let stderr_reader = Some(spawn_reader_thread(stderr_reader_fd));

        Self {
            saved_stdout: Some(saved_stdout),
            saved_stderr: Some(saved_stderr),
            stdout_reader,
            stderr_reader,
            finished: false,
            status: CaptureStatus::Ok,
            error_msg: None,
        }
    }

    /// Finish capture: close the pipe write ends, restore original descriptors,
    /// and collect remaining lines from both reader threads.
    pub(crate) fn finish(&mut self) -> Vec<String> {
        self.finished = true;
        anchor_debug("finish() start");
        if let (Some(saved_stdout), Some(saved_stderr)) =
            (self.saved_stdout.take(), self.saved_stderr.take())
        {
            unsafe {
                libc::close(libc::STDOUT_FILENO);
                libc::close(libc::STDERR_FILENO);
                libc::dup2(saved_stdout, libc::STDOUT_FILENO);
                libc::close(saved_stdout);
                libc::dup2(saved_stderr, libc::STDERR_FILENO);
                libc::close(saved_stderr);
            }
        }

        let mut all_lines = Vec::new();

        if let Some(handle) = self.stdout_reader.take()
            && let Ok(lines) = handle.join()
        {
            all_lines.extend(lines);
        }
        if let Some(handle) = self.stderr_reader.take()
            && let Ok(lines) = handle.join()
        {
            all_lines.extend(lines);
        }

        all_lines
    }
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        if !self.finished {
            anchor_debug("CaptureGuard::drop() restoring fds");
            if let (Some(saved_stdout), Some(saved_stderr)) =
                (self.saved_stdout.take(), self.saved_stderr.take())
            {
                unsafe {
                    libc::close(libc::STDOUT_FILENO);
                    libc::close(libc::STDERR_FILENO);
                    libc::dup2(saved_stdout, libc::STDOUT_FILENO);
                    libc::close(saved_stdout);
                    libc::dup2(saved_stderr, libc::STDERR_FILENO);
                    libc::close(saved_stderr);
                }
            }
            if let Some(handle) = self.stdout_reader.take() {
                let _ = handle.join();
            }
            if let Some(handle) = self.stderr_reader.take() {
                let _ = handle.join();
            }
        }
    }
}

fn strip_ansi_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            while let Some(&c) = chars.peek() {
                match c {
                    'A'..='Z' | 'a'..='z' => {
                        chars.next();
                        break;
                    }
                    _ => {
                        chars.next();
                    }
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}
