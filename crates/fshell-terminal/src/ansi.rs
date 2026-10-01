// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Raw ANSI terminal command bytes owned by fshell.
//!
//! Byte-for-byte replacement for the subset of `crossterm` commands fshell
//! actually uses. Sequences match `crossterm 0.29.0` Unix output so existing
//! snapshots, PTY tests, and terminal behavior are unchanged.
//!
//! Unix only. This crate targets macOS/Linux; non-Unix builds get stubs.

use std::io::{self, Write};

/// Alternate screen entry (`crossterm EnterAlternateScreen`).
pub const ENTER_ALTERNATE_SCREEN: &str = "\x1b[?1049h";
/// Alternate screen exit (`crossterm LeaveAlternateScreen`).
pub const LEAVE_ALTERNATE_SCREEN: &str = "\x1b[?1049l";

/// Show cursor (`crossterm cursor::Show`).
pub const SHOW_CURSOR: &str = "\x1b[?25h";
/// Hide cursor (`crossterm cursor::Hide`).
pub const HIDE_CURSOR: &str = "\x1b[?25l";
/// Enable cursor blinking (`crossterm cursor::EnableBlinking`).
pub const ENABLE_BLINKING: &str = "\x1b[?12h";
/// Disable cursor blinking (`crossterm cursor::DisableBlinking`).
pub const DISABLE_BLINKING: &str = "\x1b[?12l";

/// Clear entire screen (`crossterm ClearType::All`).
pub const CLEAR_ALL: &str = "\x1b[2J";
/// Clear scrollback plus screen (`crossterm ClearType::Purge`).
pub const CLEAR_PURGE: &str = "\x1b[3J";
/// Clear from cursor down (`crossterm ClearType::FromCursorDown`).
pub const CLEAR_FROM_CURSOR_DOWN: &str = "\x1b[J";
/// Clear from cursor up (`crossterm ClearType::FromCursorUp`).
pub const CLEAR_FROM_CURSOR_UP: &str = "\x1b[1J";
/// Clear current line (`crossterm ClearType::CurrentLine`).
pub const CLEAR_CURRENT_LINE: &str = "\x1b[2K";
/// Clear until new line (`crossterm ClearType::UntilNewLine`).
pub const CLEAR_UNTIL_NEW_LINE: &str = "\x1b[K";

/// Mouse capture enable (`crossterm EnableMouseCapture`, all modes, in order).
pub const ENABLE_MOUSE_CAPTURE: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1015h\x1b[?1006h";
/// Mouse capture disable (inverse, reverse order).
pub const DISABLE_MOUSE_CAPTURE: &str = "\x1b[?1006l\x1b[?1015l\x1b[?1003l\x1b[?1002l\x1b[?1000l";

/// Focus-change enable (`crossterm EnableFocusChange`).
pub const ENABLE_FOCUS_CHANGE: &str = "\x1b[?1004h";
/// Focus-change disable (`crossterm DisableFocusChange`).
pub const DISABLE_FOCUS_CHANGE: &str = "\x1b[?1004l";

/// Bracketed-paste enable (`crossterm EnableBracketedPaste`).
pub const ENABLE_BRACKETED_PASTE: &str = "\x1b[?2004h";
/// Bracketed-paste disable (`crossterm DisableBracketedPaste`).
pub const DISABLE_BRACKETED_PASTE: &str = "\x1b[?2004l";

/// Save cursor position (`crossterm SavePosition`, SCO form).
pub const SAVE_POSITION: &str = "\x1b7";
/// Restore cursor position (`crossterm RestorePosition`, SCO form).
pub const RESTORE_POSITION: &str = "\x1b8";

/// Kitty keyboard enhancement reset written by the emergency restore path.
pub const RESET_KEYBOARD_ENHANCEMENTS: &str = "\x1b[=0u";

fn write_all(out: &mut impl Write, bytes: &str) -> io::Result<()> {
    out.write_all(bytes.as_bytes())
}

/// Write `MoveTo(column, row)` (`crossterm cursor::MoveTo`, 0-based).
pub fn move_to(out: &mut impl Write, column: u16, row: u16) -> io::Result<()> {
    write!(
        out,
        "\x1b[{};{}H",
        row.saturating_add(1),
        column.saturating_add(1)
    )
}

/// Write `MoveToColumn(column)` (`crossterm cursor::MoveToColumn`, 0-based).
pub fn move_to_column(out: &mut impl Write, column: u16) -> io::Result<()> {
    write!(out, "\x1b[{}G", column.saturating_add(1))
}

/// Write `MoveToRow(row)` (`crossterm cursor::MoveToRow`, 0-based).
pub fn move_to_row(out: &mut impl Write, row: u16) -> io::Result<()> {
    write!(out, "\x1b[{}d", row.saturating_add(1))
}

/// Write raw bytes (`crossterm style::Print`).
pub fn print(out: &mut impl Write, text: &str) -> io::Result<()> {
    out.write_all(text.as_bytes())
}

/// Enter alternate screen and flush.
pub fn enter_alternate_screen(out: &mut impl Write) -> io::Result<()> {
    write_all(out, ENTER_ALTERNATE_SCREEN)?;
    out.flush()
}

/// Leave alternate screen and flush.
pub fn leave_alternate_screen(out: &mut impl Write) -> io::Result<()> {
    write_all(out, LEAVE_ALTERNATE_SCREEN)?;
    out.flush()
}

/// Show cursor and flush.
pub fn show_cursor(out: &mut impl Write) -> io::Result<()> {
    write_all(out, SHOW_CURSOR)?;
    out.flush()
}

/// Hide cursor and flush.
pub fn hide_cursor(out: &mut impl Write) -> io::Result<()> {
    write_all(out, HIDE_CURSOR)?;
    out.flush()
}

/// Enable cursor blinking and flush.
pub fn enable_blinking(out: &mut impl Write) -> io::Result<()> {
    write_all(out, ENABLE_BLINKING)?;
    out.flush()
}

/// Disable cursor blinking and flush.
pub fn disable_blinking(out: &mut impl Write) -> io::Result<()> {
    write_all(out, DISABLE_BLINKING)?;
    out.flush()
}

/// Clear current line and flush.
pub fn clear_current_line(out: &mut impl Write) -> io::Result<()> {
    write_all(out, CLEAR_CURRENT_LINE)?;
    out.flush()
}

/// Clear from cursor down and flush.
pub fn clear_from_cursor_down(out: &mut impl Write) -> io::Result<()> {
    write_all(out, CLEAR_FROM_CURSOR_DOWN)?;
    out.flush()
}

/// Clear entire screen and flush.
pub fn clear_all(out: &mut impl Write) -> io::Result<()> {
    write_all(out, CLEAR_ALL)?;
    out.flush()
}

/// Enable mouse capture and flush.
pub fn enable_mouse_capture(out: &mut impl Write) -> io::Result<()> {
    write_all(out, ENABLE_MOUSE_CAPTURE)?;
    out.flush()
}

/// Disable mouse capture and flush.
pub fn disable_mouse_capture(out: &mut impl Write) -> io::Result<()> {
    write_all(out, DISABLE_MOUSE_CAPTURE)?;
    out.flush()
}

/// Enable bracketed paste and flush.
pub fn enable_bracketed_paste(out: &mut impl Write) -> io::Result<()> {
    write_all(out, ENABLE_BRACKETED_PASTE)?;
    out.flush()
}

/// Disable bracketed paste and flush.
pub fn disable_bracketed_paste(out: &mut impl Write) -> io::Result<()> {
    write_all(out, DISABLE_BRACKETED_PASTE)?;
    out.flush()
}

/// Enable focus-change reporting and flush.
pub fn enable_focus_change(out: &mut impl Write) -> io::Result<()> {
    write_all(out, ENABLE_FOCUS_CHANGE)?;
    out.flush()
}

/// Disable focus-change reporting and flush.
pub fn disable_focus_change(out: &mut impl Write) -> io::Result<()> {
    write_all(out, DISABLE_FOCUS_CHANGE)?;
    out.flush()
}

/// Save cursor position and flush.
pub fn save_position(out: &mut impl Write) -> io::Result<()> {
    write_all(out, SAVE_POSITION)?;
    out.flush()
}

/// Restore cursor position and flush.
pub fn restore_position(out: &mut impl Write) -> io::Result<()> {
    write_all(out, RESTORE_POSITION)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(f: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> Vec<u8> {
        let mut out = Vec::new();
        f(&mut out).expect("write to Vec never fails");
        out
    }

    #[test]
    fn sequences_match_crossterm_unix_bytes() {
        assert_eq!(ENTER_ALTERNATE_SCREEN.as_bytes(), b"\x1b[?1049h");
        assert_eq!(LEAVE_ALTERNATE_SCREEN.as_bytes(), b"\x1b[?1049l");
        assert_eq!(SHOW_CURSOR.as_bytes(), b"\x1b[?25h");
        assert_eq!(HIDE_CURSOR.as_bytes(), b"\x1b[?25l");
        assert_eq!(ENABLE_BLINKING.as_bytes(), b"\x1b[?12h");
        assert_eq!(DISABLE_BLINKING.as_bytes(), b"\x1b[?12l");
        assert_eq!(CLEAR_ALL.as_bytes(), b"\x1b[2J");
        assert_eq!(CLEAR_PURGE.as_bytes(), b"\x1b[3J");
        assert_eq!(CLEAR_FROM_CURSOR_DOWN.as_bytes(), b"\x1b[J");
        assert_eq!(CLEAR_FROM_CURSOR_UP.as_bytes(), b"\x1b[1J");
        assert_eq!(CLEAR_CURRENT_LINE.as_bytes(), b"\x1b[2K");
        assert_eq!(CLEAR_UNTIL_NEW_LINE.as_bytes(), b"\x1b[K");
        assert_eq!(
            ENABLE_MOUSE_CAPTURE.as_bytes(),
            b"\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1015h\x1b[?1006h"
        );
        assert_eq!(
            DISABLE_MOUSE_CAPTURE.as_bytes(),
            b"\x1b[?1006l\x1b[?1015l\x1b[?1003l\x1b[?1002l\x1b[?1000l"
        );
        assert_eq!(ENABLE_FOCUS_CHANGE.as_bytes(), b"\x1b[?1004h");
        assert_eq!(DISABLE_FOCUS_CHANGE.as_bytes(), b"\x1b[?1004l");
        assert_eq!(ENABLE_BRACKETED_PASTE.as_bytes(), b"\x1b[?2004h");
        assert_eq!(DISABLE_BRACKETED_PASTE.as_bytes(), b"\x1b[?2004l");
        assert_eq!(SAVE_POSITION.as_bytes(), b"\x1b7");
        assert_eq!(RESTORE_POSITION.as_bytes(), b"\x1b8");
    }

    #[test]
    fn cursor_addressing_is_one_based() {
        assert_eq!(buf(|o| move_to(o, 0, 0)), b"\x1b[1;1H");
        assert_eq!(buf(|o| move_to(o, 10, 5)), b"\x1b[6;11H");
        assert_eq!(buf(|o| move_to_column(o, 0)), b"\x1b[1G");
        assert_eq!(buf(|o| move_to_row(o, 0)), b"\x1b[1d");
    }
}
