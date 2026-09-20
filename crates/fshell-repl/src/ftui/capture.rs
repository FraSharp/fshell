// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::fs::File;
use std::io::Read;
use std::os::fd::FromRawFd;

// Gate: set FSH_REPL_ANCHOR_DEBUG=1 for capture debug logs
fn anchor_debug(msg: impl std::fmt::Display) {
    if std::env::var("FSH_REPL_ANCHOR_DEBUG").as_deref() != Ok("1") {
        return;
    }

    let path = format!(
        "{}/fsh_anchor_debug.log",
        std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string())
    );
    if let Ok(mut log) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(log, "[capture] {}", msg);
    }
}

/// Outcome of capture — callers must check `ok()` before using `lines`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureStatus {
    Ok,
    DupFailed,
    PipeFailed,
}

/// A RAII guard that redirects stdout and stderr to one shared pipe.
///
/// A single continuously drained pipe is sufficient to avoid deadlock and,
/// unlike separate pipes, preserves the kernel's write order between the two
/// streams. This is the only ordering that a terminal user could observe.
///
/// On drop, stdout and stderr are restored to their original descriptors.
pub(crate) struct CaptureGuard {
    saved_stdout: Option<i32>,
    saved_stderr: Option<i32>,
    reader: Option<std::thread::JoinHandle<Vec<String>>>,
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

/// Spawn a reader thread that drains the shared pipe until all writers close.
///
/// Reading bytes to completion before splitting lines preserves empty lines
/// and handles output that does not end with a newline. The capture consumer
/// owns presentation policy; the transport must not silently discard data.
fn spawn_reader_thread(read_fd: i32) -> std::thread::JoinHandle<Vec<String>> {
    let mut reader = unsafe { File::from_raw_fd(read_fd) };
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        loop {
            match reader.read_to_end(&mut bytes) {
                Ok(_) => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Vec::new(),
            }
        }

        let mut lines = bytes
            .split(|byte| *byte == b'\n')
            .map(|line| strip_ansi_escapes(&String::from_utf8_lossy(line)))
            .collect::<Vec<_>>();
        if bytes.last() == Some(&b'\n') {
            lines.pop();
        }
        lines
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
                reader: None,
                finished: false,
                status: CaptureStatus::DupFailed,
                error_msg: Some(format!("capture dup failed: {err}")),
            };
        }

        // One shared pipe preserves stdout/stderr ordering while the reader
        // thread drains it continuously, so neither stream can fill a pipe
        // buffer and deadlock the command.
        let (reader_fd, writer_fd) = match make_pipe() {
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
                    reader: None,
                    finished: false,
                    status: CaptureStatus::PipeFailed,
                    error_msg: Some(format!("capture pipe failed: {err}")),
                };
            }
        };

        // Redirect both descriptors to the same writer. Check every dup2 so a
        // partially installed capture cannot be reported as successful.
        let stdout_redirected = unsafe { libc::dup2(writer_fd, libc::STDOUT_FILENO) } >= 0;
        let stderr_redirected =
            stdout_redirected && unsafe { libc::dup2(writer_fd, libc::STDERR_FILENO) } >= 0;
        unsafe {
            libc::close(writer_fd);
        }
        if !stdout_redirected || !stderr_redirected {
            let err = std::io::Error::last_os_error();
            anchor_debug(format!("dup2() capture redirect failed: {err}"));
            unsafe {
                libc::close(reader_fd);
                libc::dup2(saved_stdout, libc::STDOUT_FILENO);
                libc::dup2(saved_stderr, libc::STDERR_FILENO);
                libc::close(saved_stdout);
                libc::close(saved_stderr);
            }
            return Self {
                saved_stdout: None,
                saved_stderr: None,
                reader: None,
                finished: false,
                status: CaptureStatus::DupFailed,
                error_msg: Some(format!("capture redirect failed: {err}")),
            };
        }

        let reader = Some(spawn_reader_thread(reader_fd));

        Self {
            saved_stdout: Some(saved_stdout),
            saved_stderr: Some(saved_stderr),
            reader,
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

        self.reader
            .take()
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default()
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
            if let Some(handle) = self.reader.take() {
                let _ = handle.join();
            }
        }
    }
}

fn strip_ansi_escapes(s: &str) -> String {
    String::from_utf8_lossy(&strip_ansi_escapes::strip(s.as_bytes())).into_owned()
}

#[cfg(test)]
mod tests {
    use super::strip_ansi_escapes;

    #[test]
    fn strips_sgr_and_preserves_empty_lines() {
        let captured = "\x1b[31mred\x1b[0m\n\nplain";
        assert_eq!(
            captured
                .split('\n')
                .map(strip_ansi_escapes)
                .collect::<Vec<_>>(),
            ["red", "", "plain"]
        );
    }
}
