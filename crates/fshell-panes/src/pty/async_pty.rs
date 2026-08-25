// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Async PTY wrapper using `tokio::io::unix::AsyncFd`.
//!
//! Wraps a `portable-pty` master fd in non-blocking mode for async
//! read/write via epoll/kqueue. Handles `EAGAIN` transparently so
//! no bytes are dropped under PTY pressure.
//!
//! ## Safety notes
//!
//! - `AsyncPtyFd` does NOT close the fd on drop — `master: Box<dyn MasterPty>`
//!   owns the lifecycle. This avoids double-close.
//! - Field drop order matters: `async_fd` must drop before `master` so the fd
//!   is unregistered from epoll/kqueue before it is closed. Rust drops fields
//!   in declaration order.

use std::io;
use std::os::unix::io::{AsRawFd, RawFd};

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use tokio::io::unix::AsyncFd;

// AsyncPtyFd

/// Borrow-only wrapper around a raw fd that implements `AsRawFd` for
/// `AsyncFd`. Does **not** own the fd — `master` handles lifecycle.
struct AsyncPtyFd(RawFd);

impl AsRawFd for AsyncPtyFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

// NOTE: No Drop impl — `master: Box<dyn MasterPty>` handles fd close.

// AsyncPty

/// Non-blocking async PTY wrapper.
///
/// Spawns a shell process via `portable-pty`, extracts the master fd,
/// sets it to `O_NONBLOCK`, and wraps it in `tokio::io::unix::AsyncFd`
/// for non-blocking async I/O.
pub struct AsyncPty {
    /// Must drop before `master` so the fd is unregistered from epoll
    /// before it is closed. Declaration order = drop order in Rust.
    async_fd: AsyncFd<AsyncPtyFd>,
    child: Box<dyn portable_pty::Child>,
    master: Box<dyn portable_pty::MasterPty>,
}

impl AsyncPty {
    /// Spawn a shell and return the async PTY handle.
    ///
    /// `shell` is the path to the shell binary (e.g. `"/bin/bash"`).
    /// `cols` / `rows` set the initial terminal size.
    pub fn spawn(shell: &str, cols: u16, rows: u16) -> io::Result<Self> {
        let pty_system = NativePtySystem::default();

        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(io::Error::other)?;

        let mut cmd = CommandBuilder::new(shell);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("INSIDE_FSHELL_PANES", "1");
        cmd.env("FSHELL_PANES", "1");

        let socket_path = crate::proto::get_socket_path();
        cmd.env("FSH_PANES_SOCKET", socket_path.to_string_lossy().as_ref());

        if let Ok(exe) = std::env::var("FSHELL_BIN") {
            cmd.env("FSHELL_BIN", exe);
        } else {
            cmd.env("FSHELL_BIN", shell);
        }

        let child = pair.slave.spawn_command(cmd).map_err(io::Error::other)?;

        // Extract raw fd and set non-blocking
        let raw_fd = pair
            .master
            .as_raw_fd()
            .ok_or_else(|| io::Error::other("no raw fd available"))?;

        set_non_blocking(raw_fd)?;

        let async_fd = AsyncFd::new(AsyncPtyFd(raw_fd)).map_err(io::Error::other)?;

        Ok(Self {
            async_fd,
            child,
            master: pair.master,
        })
    }

    // Async read

    /// Read from the PTY master. Yields to the tokio runtime when no
    /// data is available; retries on `EAGAIN`/`EWOULDBLOCK`.
    pub async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.async_fd.readable_mut().await?;

            match guard.try_io(|inner| {
                let fd = inner.as_raw_fd();
                let res =
                    unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
                if res < 0 {
                    let err = io::Error::last_os_error();
                    if err.kind() == io::ErrorKind::WouldBlock {
                        Err(err) // retry — try_io returns Err(_would_block)
                    } else {
                        Ok(Err(err)) // real error
                    }
                } else {
                    Ok(Ok(res as usize))
                }
            }) {
                Ok(Ok(Ok(0))) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "PTY closed (EOF)",
                    ));
                }
                Ok(Ok(Ok(n))) => return Ok(n),
                Ok(Ok(Err(e))) => return Err(e),
                Ok(Err(_)) => continue,        // EAGAIN from try_io wrapper
                Err(_would_block) => continue, // spurious wake, retry
            }
        }
    }

    // Async write

    /// Write to the PTY master. Under PTY pressure the underlying fd
    /// returns `EAGAIN`; this method retries via `writable().await`
    /// so no bytes are dropped.
    pub async fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.async_fd.writable().await?;

            match guard.try_io(|inner| {
                let fd = inner.as_raw_fd();
                let res =
                    unsafe { libc::write(fd, data.as_ptr() as *const libc::c_void, data.len()) };
                if res < 0 {
                    let err = io::Error::last_os_error();
                    if err.kind() == io::ErrorKind::WouldBlock {
                        Err(err) // retry — try_io returns Err(_would_block)
                    } else {
                        Ok(Err(err)) // real error
                    }
                } else {
                    Ok(Ok(res as usize))
                }
            }) {
                Ok(Ok(Ok(n))) => return Ok(n),
                Ok(Ok(Err(e))) => return Err(e),
                Ok(Err(_)) => continue,        // EAGAIN from try_io wrapper
                Err(_would_block) => continue, // spurious wake, retry
            }
        }
    }

    // Resize

    /// Resize the PTY (and underlying shell window).
    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        self.master.resize(size).map_err(io::Error::other)
    }

    // Kill

    /// Kill the child shell process.
    pub fn kill(&mut self) -> io::Result<()> {
        self.child.kill().map_err(io::Error::other)
    }
}

// Helpers

fn set_non_blocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }

    let result = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}
