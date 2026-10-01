// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Unix raw-mode and terminal-size primitives owned by fshell.
//!
//! Replaces `crossterm::terminal::{enable_raw_mode, disable_raw_mode,
//! is_raw_mode_enabled, size}` with explicit, per-fd termios handling.
//! The process-global `enable/disable/is_enabled` trio preserves crossterm's
//! nesting semantics (first enabler owns the restore) so existing call sites
//! migrate without behavior change; new code should prefer the `_fd` variants
//! that carry the original termios explicitly.

use std::io;
#[cfg(unix)]
use std::os::unix::io::RawFd;
#[cfg(unix)]
use std::sync::Mutex;

#[cfg(unix)]
static PRIOR_RAW_MODE: Mutex<Option<(RawFd, libc::termios)>> = Mutex::new(None);

/// Terminal size as `(columns, rows)`.
#[cfg(unix)]
fn window_size_fd(fd: RawFd) -> io::Result<(u16, u16)> {
    unsafe {
        let mut size: libc::winsize = std::mem::zeroed();
        #[allow(clippy::useless_conversion)]
        if libc::ioctl(fd, libc::TIOCGWINSZ.into(), &mut size) != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((size.ws_col, size.ws_row))
    }
}

/// Query terminal size, preferring `/dev/tty` with stdout fallback.
/// Matches `crossterm::terminal::size` Unix behavior (no `tput` fallback:
/// callers already default to `(80, 24)` on error).
#[cfg(unix)]
pub fn size() -> io::Result<(u16, u16)> {
    if let Ok(file) = std::fs::File::open("/dev/tty") {
        use std::os::unix::io::AsRawFd;
        if let Ok(dims) = window_size_fd(file.as_raw_fd())
            && (dims.0 != 0 || dims.1 != 0)
        {
            return Ok(dims);
        }
    }
    let dims = window_size_fd(libc::STDOUT_FILENO)?;
    if dims.0 == 0 && dims.1 == 0 {
        return Err(io::Error::other("terminal size is zero"));
    }
    Ok(dims)
}

#[cfg(not(unix))]
pub fn size() -> io::Result<(u16, u16)> {
    Err(io::Error::other("Unix-only terminal support"))
}

/// Resolve the fd `crossterm::terminal` operates on: stdin when it is a TTY,
/// otherwise `/dev/tty`. Returns an owned file when `/dev/tty` is opened so
/// the fd stays valid for the caller.
#[cfg(unix)]
fn tty_fd() -> io::Result<(RawFd, Option<std::fs::File>)> {
    if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
        return Ok((libc::STDIN_FILENO, None));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")?;
    use std::os::unix::io::AsRawFd;
    let fd = file.as_raw_fd();
    Ok((fd, Some(file)))
}

#[cfg(unix)]
fn get_attr(fd: RawFd) -> io::Result<libc::termios> {
    unsafe {
        let mut termios: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut termios) != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(termios)
    }
}

#[cfg(unix)]
fn set_attr(fd: RawFd, termios: &libc::termios) -> io::Result<()> {
    unsafe {
        if libc::tcsetattr(fd, libc::TCSANOW, termios) != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// Enable raw mode on `fd`, returning the previous termios for explicit restore.
#[cfg(unix)]
pub fn enable_raw_mode_fd(fd: RawFd) -> io::Result<libc::termios> {
    let orig = get_attr(fd)?;
    unsafe {
        let mut raw = orig;
        libc::cfmakeraw(&mut raw);
        if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(orig)
}

/// Restore termios previously saved by [`enable_raw_mode_fd`].
#[cfg(unix)]
pub fn restore_raw_mode_fd(fd: RawFd, orig: &libc::termios) -> io::Result<()> {
    set_attr(fd, orig)
}

/// Returns true when the process-global raw-mode guard holds the terminal.
#[cfg(unix)]
pub fn is_raw_mode_enabled() -> bool {
    PRIOR_RAW_MODE.lock().map(|g| g.is_some()).unwrap_or(false)
}

#[cfg(not(unix))]
pub fn is_raw_mode_enabled() -> bool {
    false
}

/// Enable process-global raw mode (first caller owns the restore).
/// Idempotent: returns `Ok` immediately when already enabled.
#[cfg(unix)]
pub fn enable_raw_mode() -> io::Result<()> {
    let mut guard = PRIOR_RAW_MODE
        .lock()
        .map_err(|_| io::Error::other("raw-mode state lock is poisoned"))?;
    if guard.is_some() {
        return Ok(());
    }
    let (fd, _keep) = tty_fd()?;
    let orig = get_attr(fd)?;
    unsafe {
        let mut raw = orig;
        libc::cfmakeraw(&mut raw);
        if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    *guard = Some((fd, orig));
    Ok(())
}

#[cfg(not(unix))]
pub fn enable_raw_mode() -> io::Result<()> {
    Err(io::Error::other("Unix-only terminal support"))
}

/// Restore the termios saved by [`enable_raw_mode`]. No-op when not enabled.
#[cfg(unix)]
pub fn disable_raw_mode() -> io::Result<()> {
    let saved = PRIOR_RAW_MODE
        .lock()
        .map_err(|_| io::Error::other("raw-mode state lock is poisoned"))?
        .take();
    if let Some((fd, orig)) = saved {
        set_attr(fd, &orig)?;
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn disable_raw_mode() -> io::Result<()> {
    Ok(())
}

/// Returns true when `fd` is a TTY.
pub fn is_tty_fd(fd: RawFd) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::isatty(fd) == 1 }
    }
    #[cfg(not(unix))]
    {
        let _ = fd;
        false
    }
}

/// Returns true when stdin is a TTY. Replaces `crossterm::tty::IsTty`.
pub fn is_stdin_tty() -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}
