// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Shared scaffolding for tests that read a terminal stand-in.

use std::os::unix::io::RawFd;
use std::sync::{Mutex, MutexGuard};

/// Serialize tests that touch the process-wide inbox or cursor report.
pub(crate) fn lock() -> MutexGuard<'static, ()> {
    static TEST_LOCK: Mutex<()> = Mutex::new(());
    TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A pipe standing in for the terminal: tests write input bytes, readers
/// consume them through the real `poll`/`read` path.
pub(crate) struct Pipe {
    pub reader: RawFd,
    writer: Option<std::fs::File>,
}

impl Pipe {
    pub fn new() -> std::io::Result<Self> {
        let mut fds = [0; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let writer = unsafe {
            use std::os::unix::io::FromRawFd;
            std::fs::File::from_raw_fd(fds[1])
        };
        Ok(Self {
            reader: fds[0],
            writer: Some(writer),
        })
    }

    pub fn write(&mut self, bytes: &[u8]) {
        use std::io::Write;
        let writer = self.writer.as_mut().expect("writer is open");
        writer.write_all(bytes).expect("test pipe write");
        writer.flush().expect("test pipe flush");
    }

    pub fn close_writer(&mut self) {
        drop(self.writer.take());
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        unsafe { libc::close(self.reader) };
    }
}
