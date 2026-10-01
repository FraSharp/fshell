// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Unix raw-mode, terminal-size, and cursor-query primitives owned by fshell.
//!
//! Termios handling is explicit and per-fd. The process-global
//! `enable/disable/is_enabled` trio follows nesting semantics: the first
//! enabler owns the restore and later enablers are no-ops. New code should
//! prefer the `_fd` variants that carry the original termios explicitly.

use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::io::{AsRawFd, RawFd};
#[cfg(unix)]
use std::sync::Mutex;
#[cfg(unix)]
use std::time::{Duration, Instant};

/// Saved terminal state for the process-global raw-mode guard: the fd and
/// its original termios. The descriptor belongs to [`controlling_tty`],
/// which owns it for the process lifetime.
#[cfg(unix)]
static PRIOR_RAW_MODE: Mutex<Option<(RawFd, libc::termios)>> = Mutex::new(None);

/// The controlling terminal, opened once and kept for the process lifetime.
///
/// Opening `/dev/tty` per query cost a descriptor cycle on every redraw; the
/// handle is created on first use, closed on exec, and never inherited.
#[cfg(unix)]
fn controlling_tty() -> Option<&'static std::fs::File> {
    static TTY: std::sync::OnceLock<Option<std::fs::File>> = std::sync::OnceLock::new();
    TTY.get_or_init(|| {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .ok()
    })
    .as_ref()
}

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

/// Measure the terminal with the window-size ioctl, preferring the
/// controlling terminal over stdout. Zeroed dimensions mean the ioctl was
/// answered by something that is not a terminal, so they do not count.
#[cfg(unix)]
fn ioctl_size() -> Option<(u16, u16)> {
    if let Some(file) = controlling_tty()
        && let Ok(size) = window_size_fd(file.as_raw_fd())
        && (size.0 != 0 || size.1 != 0)
    {
        return Some(size);
    }
    if let Ok(size) = window_size_fd(libc::STDOUT_FILENO)
        && (size.0 != 0 || size.1 != 0)
    {
        return Some(size);
    }
    None
}

/// Ask `tput` for a dimension, used when the ioctl cannot measure the
/// terminal at all.
#[cfg(unix)]
fn tput_dimension(name: &str) -> Option<u16> {
    let output = std::process::Command::new("tput").arg(name).output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_tput_output(&output.stdout)
}

/// Parse a `tput` dimension reply.
#[cfg(unix)]
fn parse_tput_output(bytes: &[u8]) -> Option<u16> {
    std::str::from_utf8(bytes).ok()?.trim().parse().ok()
}

/// Query terminal size: the window-size ioctl first, then `tput` for
/// environments where the ioctl is unavailable.
#[cfg(unix)]
pub fn size() -> io::Result<(u16, u16)> {
    if let Some(size) = ioctl_size() {
        return Ok(size);
    }
    match (tput_dimension("cols"), tput_dimension("lines")) {
        (Some(columns), Some(rows)) => Ok((columns, rows)),
        _ => Err(io::Error::other("terminal size is unavailable")),
    }
}

#[cfg(not(unix))]
pub fn size() -> io::Result<(u16, u16)> {
    Err(io::Error::other("Unix-only terminal support"))
}

/// Full window size as `(columns, rows, width_pixels, height_pixels)`,
/// preferring the controlling terminal with stdout fallback. Pixel fields
/// stay zero on terminals that do not report them.
#[cfg(unix)]
pub fn window_size() -> io::Result<(u16, u16, u16, u16)> {
    if let Some(file) = controlling_tty()
        && let Ok(size) = window_size_fields(file.as_raw_fd())
    {
        return Ok(size);
    }
    window_size_fields(libc::STDOUT_FILENO)
}

#[cfg(not(unix))]
pub fn window_size() -> io::Result<(u16, u16, u16, u16)> {
    Err(io::Error::other("Unix-only terminal support"))
}

/// Time allowed for a cursor report to arrive.
#[cfg(unix)]
const CURSOR_REPORT_TIMEOUT: Duration = Duration::from_secs(2);

/// Longest a query waits on the device between checks for a report that a
/// concurrent event source recorded.
#[cfg(unix)]
const CURSOR_REPORT_SLICE: Duration = Duration::from_millis(20);

/// Query the terminal cursor position as `(column, row)` with a device
/// status report. The request is written to `out`; the reply is read from
/// the terminal device. Raw mode is entered for the duration when it is not
/// already active, because canonical mode would hold the reply back.
///
/// When an event source polls concurrently it consumes the reply and records
/// it, so the query completes without reading the device. Anything the query
/// reads that is not the report is pushed to the input inbox for the next
/// source to replay; no keystroke is lost either way.
#[cfg(unix)]
pub fn cursor_position(out: &mut impl Write) -> io::Result<(u16, u16)> {
    if is_raw_mode_enabled() {
        let fd = tty_fd()?;
        return query_cursor_position(out, fd, CURSOR_REPORT_TIMEOUT);
    }
    enable_raw_mode()?;
    let result = tty_fd().and_then(|fd| query_cursor_position(out, fd, CURSOR_REPORT_TIMEOUT));
    let _ = disable_raw_mode();
    result
}

#[cfg(not(unix))]
pub fn cursor_position(_out: &mut impl Write) -> io::Result<(u16, u16)> {
    Err(io::Error::other("Unix-only terminal support"))
}

/// Ask for a cursor report on `fd` and wait for it.
#[cfg(unix)]
fn query_cursor_position(
    out: &mut impl Write,
    fd: RawFd,
    timeout: Duration,
) -> io::Result<(u16, u16)> {
    // A report left over from an earlier exchange is not this answer.
    crate::inbox::clear_cursor_report();
    out.write_all(b"\x1b[6n")?;
    out.flush()?;
    let deadline = Instant::now() + timeout;
    let mut buffer = Vec::with_capacity(32);
    loop {
        if let Some(position) = crate::inbox::take_cursor_report() {
            return Ok(position);
        }
        let now = Instant::now();
        if now >= deadline {
            // Whatever was read is input for the next source, not ours.
            crate::inbox::push_bytes(&buffer);
            return Err(io::Error::other(
                "the cursor position could not be read within a normal duration",
            ));
        }
        let wait = (deadline - now).min(CURSOR_REPORT_SLICE);
        if !wait_readable(fd, wait)? {
            continue;
        }
        let read = {
            // Reads are serialized with the event source so recovered bytes
            // keep the order the device released them in.
            let _guard = crate::inbox::lock_device();
            read_device(fd, &mut buffer)
        };
        match read {
            Ok(true) => {
                if let Some((start, end, column, row)) = crate::parse::find_cursor_report(&buffer) {
                    let mut leftovers = Vec::with_capacity(buffer.len() - (end - start));
                    leftovers.extend_from_slice(&buffer[..start]);
                    leftovers.extend_from_slice(&buffer[end..]);
                    crate::inbox::push_bytes(&leftovers);
                    return Ok((column, row));
                }
                // Only the tail can complete a pending report; older bytes
                // are user input and belong to the next source.
                if buffer.len() > 64 {
                    let tail = buffer.split_off(buffer.len() - 32);
                    crate::inbox::push_bytes(&buffer);
                    buffer = tail;
                }
            }
            Ok(false) => {}
            Err(error) => return Err(error),
        }
    }
}

/// Wait up to `timeout` for `fd` to become readable.
#[cfg(unix)]
fn wait_readable(fd: RawFd, timeout: Duration) -> io::Result<bool> {
    let millis = timeout.as_millis().min(std::ffi::c_int::MAX as u128) as std::ffi::c_int;
    let mut poll_fd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut poll_fd, 1, millis) };
    if ready < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok(false);
        }
        return Err(error);
    }
    Ok(ready > 0)
}

/// Read one chunk from `fd` into `buffer`. Returns false when the read was
/// interrupted or would block and the caller should wait again.
#[cfg(unix)]
fn read_device(fd: RawFd, buffer: &mut Vec<u8>) -> io::Result<bool> {
    let mut chunk = [0u8; 64];
    let count = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
    if count < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted || error.kind() == io::ErrorKind::WouldBlock {
            return Ok(false);
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
    Ok(true)
}

/// Resolve the fd terminal queries operate on: stdin when it is a TTY,
/// otherwise the process-wide controlling terminal.
#[cfg(unix)]
fn tty_fd() -> io::Result<RawFd> {
    if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
        return Ok(libc::STDIN_FILENO);
    }
    controlling_tty()
        .map(|file| file.as_raw_fd())
        .ok_or_else(|| io::Error::other("/dev/tty is unavailable"))
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
    let fd = tty_fd()?;
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

/// Returns true when stdin is a TTY.
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
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::inbox::Recovered;
    use crate::test_support::{Pipe, lock};

    #[test]
    fn query_returns_the_report_and_keeps_surrounding_input() {
        let _guard = lock();
        let mut pipe = Pipe::new().unwrap();
        pipe.write(b"ab\x1B[4;7Rcd");
        let mut out = Vec::new();
        assert_eq!(
            query_cursor_position(&mut out, pipe.reader, Duration::from_secs(5)).unwrap(),
            (6, 3)
        );
        assert_eq!(out.as_slice(), b"\x1B[6n");
        // The keystrokes around the report survive for the next reader.
        assert!(matches!(
            crate::inbox::pop(),
            Some(Recovered::Bytes(bytes)) if bytes == b"abcd"
        ));
        assert!(crate::inbox::pop().is_none());
    }

    #[test]
    fn query_timeout_returns_input_to_the_inbox() {
        let _guard = lock();
        let mut pipe = Pipe::new().unwrap();
        pipe.write(b"xy");
        let mut out = Vec::new();
        assert!(query_cursor_position(&mut out, pipe.reader, Duration::from_millis(60)).is_err());
        assert!(matches!(
            crate::inbox::pop(),
            Some(Recovered::Bytes(bytes)) if bytes == b"xy"
        ));
    }

    #[test]
    fn tput_output_parses_dimensions() {
        assert_eq!(parse_tput_output(b"120\n"), Some(120));
        assert_eq!(parse_tput_output(b" 40 "), Some(40));
        assert_eq!(parse_tput_output(b""), None);
        assert_eq!(parse_tput_output(b"abc"), None);
    }

    #[test]
    fn report_recorded_elsewhere_answers_the_query() {
        let _guard = lock();
        let pipe = Pipe::new().unwrap();
        let recorder = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(40));
            crate::inbox::record_cursor_report(4, 5);
        });
        let mut out = Vec::new();
        assert_eq!(
            query_cursor_position(&mut out, pipe.reader, Duration::from_secs(2)).unwrap(),
            (4, 5)
        );
        recorder.join().unwrap();
        assert!(crate::inbox::pop().is_none());
    }
}
