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

use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::io::RawFd;
#[cfg(unix)]
use std::sync::Mutex;

/// Saved terminal state for the process-global raw-mode guard: the fd, the
/// owned `/dev/tty` handle when one was opened (kept alive so the descriptor
/// cannot be recycled while the guard holds it), and the original termios.
#[cfg(unix)]
static PRIOR_RAW_MODE: Mutex<Option<(RawFd, Option<std::fs::File>, libc::termios)>> =
    Mutex::new(None);

/// Terminal size as `(columns, rows)`.
#[cfg(unix)]
fn window_size_fd(fd: RawFd) -> io::Result<(u16, u16)> {
    let (columns, rows, _, _) = window_size_fields(fd)?;
    Ok((columns, rows))
}

/// Full window size as `(columns, rows, width_pixels, height_pixels)`.
#[cfg(unix)]
fn window_size_fields(fd: RawFd) -> io::Result<(u16, u16, u16, u16)> {
    unsafe {
        let mut size: libc::winsize = std::mem::zeroed();
        #[allow(clippy::useless_conversion)]
        if libc::ioctl(fd, libc::TIOCGWINSZ.into(), &mut size) != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((size.ws_col, size.ws_row, size.ws_xpixel, size.ws_ypixel))
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

/// Full window size as `(columns, rows, width_pixels, height_pixels)`,
/// preferring `/dev/tty` with stdout fallback. Mirrors
/// `crossterm::terminal::window_size` Unix behavior, including zeroed pixel
/// fields on terminals that do not report them.
#[cfg(unix)]
pub fn window_size() -> io::Result<(u16, u16, u16, u16)> {
    if let Ok(file) = std::fs::File::open("/dev/tty") {
        use std::os::unix::io::AsRawFd;
        if let Ok(size) = window_size_fields(file.as_raw_fd()) {
            return Ok(size);
        }
    }
    window_size_fields(libc::STDOUT_FILENO)
}

#[cfg(not(unix))]
pub fn window_size() -> io::Result<(u16, u16, u16, u16)> {
    Err(io::Error::other("Unix-only terminal support"))
}

/// Query the terminal cursor position as `(column, row)` with a device
/// status report. The request is written to `out`; the reply is read from
/// the terminal device. Raw mode is entered for the duration when it is not
/// already active, because canonical mode would hold the reply back.
///
/// The reply can be consumed by a concurrent input reader; callers that poll
/// the terminal from another thread may see this time out, exactly as with
/// `crossterm::cursor::position`.
#[cfg(unix)]
pub fn cursor_position(out: &mut impl Write) -> io::Result<(u16, u16)> {
    if is_raw_mode_enabled() {
        return read_cursor_position(out);
    }
    enable_raw_mode()?;
    let result = read_cursor_position(out);
    let _ = disable_raw_mode();
    result
}

#[cfg(not(unix))]
pub fn cursor_position(_out: &mut impl Write) -> io::Result<(u16, u16)> {
    Err(io::Error::other("Unix-only terminal support"))
}

/// Write the status report request and wait up to two seconds for its reply.
#[cfg(unix)]
fn read_cursor_position(out: &mut impl Write) -> io::Result<(u16, u16)> {
    out.write_all(b"\x1b[6n")?;
    out.flush()?;
    let (fd, _owned) = tty_fd()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut buffer = Vec::with_capacity(32);
    loop {
        let now = std::time::Instant::now();
        if now >= deadline {
            return Err(io::Error::other(
                "the cursor position could not be read within a normal duration",
            ));
        }
        let millis = (deadline - now)
            .as_millis()
            .min(std::ffi::c_int::MAX as u128) as std::ffi::c_int;
        let mut poll_fd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut poll_fd, 1, millis) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if ready == 0 {
            continue;
        }
        let mut chunk = [0u8; 32];
        let count = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted
                || error.kind() == io::ErrorKind::WouldBlock
            {
                continue;
            }
            return Err(error);
        }
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "terminal input closed",
            ));
        }
        buffer.extend_from_slice(&chunk[..count as usize]);
        if let Some(position) = parse_cursor_position(&buffer) {
            return Ok(position);
        }
        // Only the tail can complete a pending sequence.
        if buffer.len() > 64 {
            buffer.drain(..buffer.len() - 32);
        }
    }
}

/// Scan for a `CSI row ; column R` reply, returning 0-based `(column, row)`.
#[cfg(unix)]
fn parse_cursor_position(bytes: &[u8]) -> Option<(u16, u16)> {
    let mut index = 0;
    while index + 3 <= bytes.len() {
        if bytes[index] != 0x1B || bytes[index + 1] != b'[' {
            index += 1;
            continue;
        }
        let mut cursor = index + 2;
        let row_start = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if cursor == row_start || cursor >= bytes.len() || bytes[cursor] != b';' {
            index += 1;
            continue;
        }
        let row = std::str::from_utf8(&bytes[row_start..cursor])
            .ok()
            .and_then(|text| text.parse::<u16>().ok());
        let Some(row) = row else {
            index += 1;
            continue;
        };
        cursor += 1;
        let column_start = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if cursor == column_start || cursor >= bytes.len() || bytes[cursor] != b'R' {
            index += 1;
            continue;
        }
        let column = std::str::from_utf8(&bytes[column_start..cursor])
            .ok()
            .and_then(|text| text.parse::<u16>().ok());
        let Some(column) = column else {
            index += 1;
            continue;
        };
        return Some((column.saturating_sub(1), row.saturating_sub(1)));
    }
    None
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
    let (fd, owned) = tty_fd()?;
    let orig = get_attr(fd)?;
    unsafe {
        let mut raw = orig;
        libc::cfmakeraw(&mut raw);
        if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    *guard = Some((fd, owned, orig));
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
    if let Some((fd, _owned, orig)) = saved {
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

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn cursor_report_parses_zero_based() {
        assert_eq!(parse_cursor_position(b"\x1b[12;34R"), Some((33, 11)));
    }

    #[test]
    fn cursor_report_ignores_preceding_input() {
        assert_eq!(
            parse_cursor_position(b"\x1b[Ahello\x1b[3;4R"),
            Some((3, 2))
        );
    }

    #[test]
    fn incomplete_or_absent_reports_are_none() {
        assert_eq!(parse_cursor_position(b"\x1b[12;"), None);
        assert_eq!(parse_cursor_position(b"\x1b[12;34"), None);
        assert_eq!(parse_cursor_position(b"plain text"), None);
    }

    #[test]
    fn oversized_parameters_do_not_abort_the_scan() {
        assert_eq!(
            parse_cursor_position(b"\x1b[999999999;1R\x1b[2;3R"),
            Some((2, 1))
        );
    }
}
