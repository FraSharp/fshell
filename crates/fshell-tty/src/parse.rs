// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Unix terminal input decoding owned by fshell.
//!
//! Translates raw terminal bytes into [`RawEvent`]s. The accepted grammar is
//! the subset a shell line editor can act on: control bytes, UTF-8 text,
//! CSI/SS3 functional keys, legacy and SGR mouse reports, bracketed paste,
//! and focus reports (recorded for cursor queries, never surfaced as input).
//!
//! Deliberate departures from a strict reading of the input grammar, all
//! documented at the site:
//! - coordinate arithmetic saturates instead of underflowing on `0`;
//! - a double `ESC` yields one `Escape` and keeps the second `ESC` pending
//!   instead of swallowing it, so no keystroke is ever lost;
//! - `CSI R` (`F(3)`) is accepted alongside the SS3 form;
//! - kinds are always `Press`: without the kitty keyboard protocol (which
//!   fshell never enables) terminals never report repeat or release.

use crate::input::{
    InputEvent, Key, KeyAction, KeyEvent, Modifiers, MouseAction, MouseButton, MouseEvent,
};

/// Lone-`ESC` disambiguation window.
///
/// A bare `ESC` byte may be the `Escape` key or the start of an `Alt+` chord
/// or escape sequence. Waiting this long for a continuation byte separates
/// the two without making `Escape` feel laggy.
pub(crate) const ESC_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(50);

/// A decoded key before shell-level normalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RawKey {
    Char(char),
    Enter,
    Escape,
    Backspace,
    Tab,
    BackTab,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    Insert,
    Function(u8),
    Null,
}

/// Raw modifier state as decoded from the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RawModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    /// Super, hyper, meta, or lock bits: representable, but outside fshell's
    /// three-modifier vocabulary, so folded into [`Modifiers::OTHER`].
    pub other: bool,
}

/// A decoded mouse report before shell-level mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RawMouse {
    pub action: MouseAction,
    pub button: Option<MouseButton>,
    pub column: u16,
    pub row: u16,
}

/// One decoded terminal event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RawEvent {
    Key {
        key: RawKey,
        modifiers: RawModifiers,
    },
    Mouse(RawMouse),
    Paste(String),
}

/// Outcome of one parse attempt over the buffered bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Parse {
    /// A complete event; the consumed bytes left the buffer.
    Event(RawEvent),
    /// Bytes were consumed without producing an event (focus reports,
    /// device responses, undecodable bytes); parse again immediately.
    Again,
    /// The buffer holds a prefix that may complete with more bytes.
    NeedMore,
}

/// Stateful ANSI input parser over an accumulating byte buffer.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct AnsiParser {
    buf: Vec<u8>,
}

impl AnsiParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append freshly read bytes.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// True when the buffer holds exactly one pending `ESC` byte.
    pub fn is_lone_esc(&self) -> bool {
        self.buf == [0x1B]
    }

    /// Consume a pending lone `ESC`, reporting whether one was held.
    pub fn take_lone_esc(&mut self) -> bool {
        if self.is_lone_esc() {
            self.buf.clear();
            true
        } else {
            false
        }
    }

    /// Attempt one event. Always makes progress on `Event`/`Again`.
    pub fn try_parse(&mut self) -> Parse {
        if self.buf.is_empty() {
            return Parse::NeedMore;
        }
        if self.buf[0] != 0x1B {
            return self.parse_plain();
        }
        // Lone ESC needs the disambiguation window; the reader applies it.
        if self.buf.len() == 1 {
            return Parse::NeedMore;
        }
        match self.buf[1] {
            b'[' => self.parse_csi(),
            b'O' => self.parse_ss3(),
            0x1B => {
                // A fast double-ESC yields one Escape; the second stays
                // pending so it is never swallowed.
                self.buf.drain(..1);
                Parse::Event(RawEvent::Key {
                    key: RawKey::Escape,
                    modifiers: RawModifiers::default(),
                })
            }
            _ => self.parse_alt(),
        }
    }

    /// Parse a non-escape byte: controls, Tab/Enter/Backspace, text.
    fn parse_plain(&mut self) -> Parse {
        let byte = self.buf[0];
        match byte {
            0x0D => {
                self.buf.drain(..1);
                Parse::Event(key(RawKey::Enter))
            }
            0x09 => {
                self.buf.drain(..1);
                Parse::Event(key(RawKey::Tab))
            }
            0x7F => {
                self.buf.drain(..1);
                Parse::Event(key(RawKey::Backspace))
            }
            0x00 => {
                // NUL arrives as Ctrl+Space.
                self.buf.drain(..1);
                Parse::Event(RawEvent::Key {
                    key: RawKey::Char(' '),
                    modifiers: RawModifiers {
                        control: true,
                        ..RawModifiers::default()
                    },
                })
            }
            0x01..=0x1A => {
                // Ctrl+A..Ctrl+Z. 0x0A is Ctrl+J: kept as-is so the shared
                // line-terminator normalization turns it into Enter.
                self.buf.drain(..1);
                Parse::Event(RawEvent::Key {
                    key: RawKey::Char((byte - 0x01 + b'a') as char),
                    modifiers: RawModifiers {
                        control: true,
                        ..RawModifiers::default()
                    },
                })
            }
            0x1C..=0x1F => {
                // Ctrl+4..Ctrl+7 (`Ctrl+\`, `Ctrl+]`, `Ctrl+^`, `Ctrl+_`),
                // decoded to their shifted digits.
                self.buf.drain(..1);
                Parse::Event(RawEvent::Key {
                    key: RawKey::Char((byte - 0x1C + b'4') as char),
                    modifiers: RawModifiers {
                        control: true,
                        ..RawModifiers::default()
                    },
                })
            }
            _ => match decode_utf8(&self.buf) {
                Utf8::Char(character, len) => {
                    self.buf.drain(..len);
                    let modifiers = RawModifiers {
                        shift: character.is_uppercase(),
                        ..RawModifiers::default()
                    };
                    Parse::Event(RawEvent::Key {
                        key: RawKey::Char(character),
                        modifiers,
                    })
                }
                Utf8::NeedMore => Parse::NeedMore,
                Utf8::Invalid => {
                    // Undecodable bytes carry no key; skip one so decoding
                    // resumes at the next valid byte.
                    self.buf.drain(..1);
                    Parse::Again
                }
            },
        }
    }

    /// Parse `ESC` + byte: `Alt+` chord over whatever the byte alone means.
    fn parse_alt(&mut self) -> Parse {
        // Decode the remainder as a plain key, then add Alt. Mouse and focus
        // sequences never validly follow a bare ESC, so only keys qualify.
        let mut inner = AnsiParser {
            buf: self.buf[1..].to_vec(),
        };
        let before = inner.buf.len();
        match inner.try_parse() {
            Parse::Event(RawEvent::Key { key, mut modifiers }) => {
                modifiers.alt = true;
                let consumed_inner = before - inner.buf.len();
                self.buf.drain(..1 + consumed_inner);
                Parse::Event(RawEvent::Key { key, modifiers })
            }
            Parse::Event(_) => {
                // Non-key events never take Alt; drop the ESC and re-parse.
                self.buf.drain(..1);
                Parse::Again
            }
            Parse::Again => {
                // Inner bytes were ignorable; drop the ESC and continue.
                self.buf.drain(..1);
                Parse::Again
            }
            // A lone byte after ESC may itself need more (multibyte UTF-8).
            Parse::NeedMore => Parse::NeedMore,
        }
    }

    /// Parse `ESC O` (SS3): arrows, Home/End, F1-F4.
    fn parse_ss3(&mut self) -> Parse {
        if self.buf.len() == 2 {
            return Parse::NeedMore;
        }
        let code = match self.buf[2] {
            b'A' => RawKey::Up,
            b'B' => RawKey::Down,
            b'C' => RawKey::Right,
            b'D' => RawKey::Left,
            b'H' => RawKey::Home,
            b'F' => RawKey::End,
            b'P' => RawKey::Function(1),
            b'Q' => RawKey::Function(2),
            b'R' => RawKey::Function(3),
            b'S' => RawKey::Function(4),
            _ => {
                self.buf.drain(..3.min(self.buf.len()));
                return Parse::Again;
            }
        };
        self.buf.drain(..3);
        Parse::Event(key(code))
    }

    /// Parse `ESC [` (CSI), dispatching on the byte after the introducer.
    fn parse_csi(&mut self) -> Parse {
        if self.buf.len() == 2 {
            return Parse::NeedMore;
        }
        match self.buf[2] {
            b'[' => {
                // Linux console `ESC [[ A-E` → F1-F5.
                if self.buf.len() == 3 {
                    return Parse::NeedMore;
                }
                match self.buf[3] {
                    value @ b'A'..=b'E' => {
                        self.buf.drain(..4);
                        Parse::Event(key(RawKey::Function(1 + value - b'A')))
                    }
                    _ => {
                        self.buf.drain(..4.min(self.buf.len()));
                        Parse::Again
                    }
                }
            }
            b'A' => self.simple_csi(RawKey::Up, 3),
            b'B' => self.simple_csi(RawKey::Down, 3),
            b'C' => self.simple_csi(RawKey::Right, 3),
            b'D' => self.simple_csi(RawKey::Left, 3),
            b'H' => self.simple_csi(RawKey::Home, 3),
            b'F' => self.simple_csi(RawKey::End, 3),
            b'Z' => {
                self.buf.drain(..3);
                Parse::Event(RawEvent::Key {
                    key: RawKey::BackTab,
                    modifiers: RawModifiers {
                        shift: true,
                        ..RawModifiers::default()
                    },
                })
            }
            b'M' => self.parse_x10_mouse(),
            b'<' => self.parse_sgr_mouse(),
            // Focus reports are consumed; no fshell screen acts on them.
            b'I' | b'O' => {
                self.buf.drain(..3);
                Parse::Again
            }
            b';' => match csi_final_len(&self.buf) {
                Some(len) => self.parse_modified_key(len),
                None => Parse::NeedMore,
            },
            b'P' => self.simple_csi(RawKey::Function(1), 3),
            b'Q' => self.simple_csi(RawKey::Function(2), 3),
            b'R' => self.simple_csi(RawKey::Function(3), 3),
            b'S' => self.simple_csi(RawKey::Function(4), 3),
            // Device responses carry no input: consume silently so a stray
            // cursor-position report never leaks as keystrokes.
            b'?' | b'=' | b'>' => self.swallow_csi(),
            b'0'..=b'9' => self.parse_numbered_csi(),
            _ => self.drop_csi(),
        }
    }

    /// Consume a fixed-length `ESC [` + final sequence as `key`.
    fn simple_csi(&mut self, code: RawKey, len: usize) -> Parse {
        self.buf.drain(..len.min(self.buf.len()));
        Parse::Event(key(code))
    }

    /// Consume a private/intermediate-led sequence (`ESC [ ? …`, `ESC [ = …`)
    /// through its final byte without producing input.
    fn swallow_csi(&mut self) -> Parse {
        match csi_final_len(&self.buf) {
            Some(len) => {
                self.buf.drain(..len);
                Parse::Again
            }
            None => Parse::NeedMore,
        }
    }

    /// Drop an unrecognized CSI sequence through its final byte, or one byte
    /// when no final byte is buffered yet.
    fn drop_csi(&mut self) -> Parse {
        match csi_final_len(&self.buf) {
            Some(len) => {
                self.buf.drain(..len);
                Parse::Again
            }
            None => {
                self.buf.drain(..1);
                Parse::Again
            }
        }
    }

    /// Parse numbered CSI: `~`-suffixed specials, `u`-encoded keys,
    /// `M`-suffixed rxvt mouse, `R` cursor responses, and modified finals.
    fn parse_numbered_csi(&mut self) -> Parse {
        let len = match csi_final_len(&self.buf) {
            Some(len) => len,
            None => return Parse::NeedMore,
        };
        // Bracketed paste needs its terminator before the text is trusted.
        if self.buf.starts_with(b"\x1B[200~") {
            return self.parse_paste();
        }
        let seq = self.buf[..len].to_vec();
        let last = seq[len - 1];
        match last {
            b'~' => self.parse_special_key(&seq),
            b'u' => self.parse_csi_u(&seq),
            b'M' => self.parse_rxvt_mouse(&seq),
            // Cursor-position responses feed a waiting query and are never
            // surfaced as input.
            b'R' => {
                if let Some((column, row)) = cursor_report(&seq) {
                    crate::inbox::record_cursor_report(column, row);
                }
                self.buf.drain(..len);
                Parse::Again
            }
            _ => self.parse_modified_key(len),
        }
    }

    /// Parse a final byte with an optional `;modifier` parameter
    /// (`ESC [ 1 ; 5 A` is Ctrl+Up). `len` bounds the sequence.
    fn parse_modified_key(&mut self, len: usize) -> Parse {
        let seq = self.buf[..len].to_vec();
        let last = seq[len - 1];
        let key = match last {
            b'A' => RawKey::Up,
            b'B' => RawKey::Down,
            b'C' => RawKey::Right,
            b'D' => RawKey::Left,
            b'F' => RawKey::End,
            b'H' => RawKey::Home,
            b'P' => RawKey::Function(1),
            b'Q' => RawKey::Function(2),
            b'R' => RawKey::Function(3),
            b'S' => RawKey::Function(4),
            _ => {
                self.buf.drain(..len);
                return Parse::Again;
            }
        };
        let modifiers = modifier_in_seq(&seq).unwrap_or_default();
        self.buf.drain(..len);
        Parse::Event(RawEvent::Key { key, modifiers })
    }

    /// Parse `~`-suffixed specials (`ESC [ 3 ~` is Delete) with an optional
    /// `;modifier` parameter.
    fn parse_special_key(&mut self, seq: &[u8]) -> Parse {
        let body = &seq[2..seq.len() - 1];
        let (number, modifiers) = match split_csi_modifier(body) {
            Some(pair) => pair,
            None => {
                self.buf.drain(..seq.len());
                return Parse::Again;
            }
        };
        let key = match number {
            1 | 7 => RawKey::Home,
            2 => RawKey::Insert,
            3 => RawKey::Delete,
            4 | 8 => RawKey::End,
            5 => RawKey::PageUp,
            6 => RawKey::PageDown,
            11 => RawKey::Function(1),
            12 => RawKey::Function(2),
            13 => RawKey::Function(3),
            14 => RawKey::Function(4),
            15 => RawKey::Function(5),
            17 => RawKey::Function(6),
            18 => RawKey::Function(7),
            19 => RawKey::Function(8),
            20 => RawKey::Function(9),
            21 => RawKey::Function(10),
            23 => RawKey::Function(11),
            24 => RawKey::Function(12),
            _ => {
                self.buf.drain(..seq.len());
                return Parse::Again;
            }
        };
        self.buf.drain(..seq.len());
        Parse::Event(RawEvent::Key { key, modifiers })
    }

    /// Parse `CSI … u` (fixterms): a codepoint with an optional `;modifier`.
    /// Functional-range codepoints belong to the kitty protocol, which fshell
    /// never enables, so only direct Unicode scalar values decode.
    fn parse_csi_u(&mut self, seq: &[u8]) -> Parse {
        let body = &seq[2..seq.len() - 1];
        let (codepoint, modifiers) = match split_csi_modifier(body) {
            Some(pair) => pair,
            None => {
                self.buf.drain(..seq.len());
                return Parse::Again;
            }
        };
        let character = match char::from_u32(codepoint) {
            Some(character) => character,
            None => {
                self.buf.drain(..seq.len());
                return Parse::Again;
            }
        };
        let (key, modifiers) = match character {
            '\x1B' => (RawKey::Escape, modifiers),
            '\r' => (RawKey::Enter, modifiers),
            // `\n` in raw mode is Ctrl+J; keep it raw so the shared
            // line-terminator normalization turns it into Enter.
            '\n' => (
                RawKey::Char('j'),
                RawModifiers {
                    control: true,
                    ..RawModifiers::default()
                },
            ),
            '\t' if modifiers.shift => (RawKey::BackTab, modifiers),
            '\t' => (RawKey::Tab, modifiers),
            '\x7F' => (RawKey::Backspace, modifiers),
            '\0' => (RawKey::Null, modifiers),
            other => (RawKey::Char(other), modifiers),
        };
        self.buf.drain(..seq.len());
        Parse::Event(RawEvent::Key { key, modifiers })
    }

    /// Parse bracketed paste (`ESC [ 200 ~ text ESC [ 201 ~`). The opening
    /// marker alone waits for the terminator; the text decodes lossily, so
    /// invalid bytes become replacement characters.
    fn parse_paste(&mut self) -> Parse {
        let terminator = b"\x1B[201~";
        let end = self
            .buf
            .windows(terminator.len())
            .position(|window| window == terminator)
            .map(|index| index + terminator.len());
        match end {
            Some(end) => {
                let text =
                    String::from_utf8_lossy(&self.buf[6..end - terminator.len()]).into_owned();
                self.buf.drain(..end);
                Parse::Event(RawEvent::Paste(text))
            }
            None => Parse::NeedMore,
        }
    }

    /// Parse X10 mouse (`ESC [ M Cb Cx Cy`, six raw bytes).
    fn parse_x10_mouse(&mut self) -> Parse {
        if self.buf.len() < 6 {
            return Parse::NeedMore;
        }
        let button = match self.buf[3].checked_sub(32) {
            Some(button) => button,
            None => {
                self.buf.drain(..6);
                return Parse::Again;
            }
        };
        let Some(action) = mouse_button(button) else {
            self.buf.drain(..6);
            return Parse::Again;
        };
        // Coordinates saturate instead of underflowing on bytes below 32.
        let column = self.buf[4].saturating_sub(33);
        let row = self.buf[5].saturating_sub(33);
        self.buf.drain(..6);
        Parse::Event(RawEvent::Mouse(RawMouse {
            action,
            button: drag_button(&action),
            column: column.into(),
            row: row.into(),
        }))
    }

    /// Parse SGR mouse (`ESC [ < Cb ; Cx ; Cy M|m`, lowercase `m` on release).
    fn parse_sgr_mouse(&mut self) -> Parse {
        let end = match self
            .buf
            .iter()
            .position(|byte| *byte == b'm' || *byte == b'M')
        {
            Some(index) if index >= 6 => index + 1,
            _ => return Parse::NeedMore,
        };
        let release = self.buf[end - 1] == b'm';
        let seq = self.buf[..end].to_vec();
        let body = match std::str::from_utf8(&seq[3..end - 1]) {
            Ok(body) => body,
            Err(_) => {
                self.buf.drain(..end);
                return Parse::Again;
            }
        };
        let mut parts = body.split(';');
        let button: u8 = match parts.next().and_then(|text| text.parse().ok()) {
            Some(button) => button,
            None => {
                self.buf.drain(..end);
                return Parse::Again;
            }
        };
        let column: u16 = match parts.next().and_then(|text| text.parse::<u16>().ok()) {
            Some(value) => value.saturating_sub(1),
            None => {
                self.buf.drain(..end);
                return Parse::Again;
            }
        };
        let row: u16 = match parts.next().and_then(|text| text.parse::<u16>().ok()) {
            Some(value) => value.saturating_sub(1),
            None => {
                self.buf.drain(..end);
                return Parse::Again;
            }
        };
        let mut action = match mouse_button(button) {
            Some(action) => action,
            None => {
                self.buf.drain(..end);
                return Parse::Again;
            }
        };
        // SGR marks release with a lowercase `m`: a press kind becomes its
        // release.
        if release {
            action = match action {
                MouseAction::Down(button) => MouseAction::Up(button),
                other => other,
            };
        }
        self.buf.drain(..end);
        Parse::Event(RawEvent::Mouse(RawMouse {
            action,
            button: drag_button(&action),
            column,
            row,
        }))
    }

    /// Parse rxvt mouse (`ESC [ Cb ; Cx ; Cy M`, decimal button minus 32).
    fn parse_rxvt_mouse(&mut self, seq: &[u8]) -> Parse {
        let body = match std::str::from_utf8(&seq[2..seq.len() - 1]) {
            Ok(body) => body,
            Err(_) => {
                self.buf.drain(..seq.len());
                return Parse::Again;
            }
        };
        let mut parts = body.split(';');
        let button: u8 = match parts
            .next()
            .and_then(|text| text.parse::<u8>().ok())
            .and_then(|value| value.checked_sub(32))
        {
            Some(button) => button,
            None => {
                self.buf.drain(..seq.len());
                return Parse::Again;
            }
        };
        let column: u16 = match parts.next().and_then(|text| text.parse::<u16>().ok()) {
            Some(value) => value.saturating_sub(1),
            None => {
                self.buf.drain(..seq.len());
                return Parse::Again;
            }
        };
        let row: u16 = match parts.next().and_then(|text| text.parse::<u16>().ok()) {
            Some(value) => value.saturating_sub(1),
            None => {
                self.buf.drain(..seq.len());
                return Parse::Again;
            }
        };
        let action = match mouse_button(button) {
            Some(action) => action,
            None => {
                self.buf.drain(..seq.len());
                return Parse::Again;
            }
        };
        self.buf.drain(..seq.len());
        Parse::Event(RawEvent::Mouse(RawMouse {
            action,
            button: drag_button(&action),
            column,
            row,
        }))
    }
}

/// A plain key with no modifiers.
fn key(code: RawKey) -> RawEvent {
    RawEvent::Key {
        key: code,
        modifiers: RawModifiers::default(),
    }
}

/// Map a decoded event into shell semantics. Returns `None` for undecodable
/// input, which the source skips without disturbing the wait.
pub(crate) fn map_raw_event(event: RawEvent) -> Option<InputEvent> {
    match event {
        RawEvent::Key { key, modifiers } => {
            let (code, modifiers) = normalize_line_terminator(key, modifiers);
            Some(InputEvent::Key(KeyEvent {
                key: map_key(code),
                modifiers: Modifiers::from_raw(modifiers),
                action: KeyAction::Press,
            }))
        }
        RawEvent::Mouse(mouse) => Some(InputEvent::Mouse(MouseEvent {
            action: mouse.action,
            column: mouse.column,
            row: mouse.row,
        })),
        RawEvent::Paste(text) => Some(InputEvent::Paste(text)),
    }
}

/// Treat a bare line feed as Enter.
///
/// In raw mode the Enter key arrives as CR and decodes to [`RawKey::Enter`].
/// A bare LF (`\n`) — how coding agents, `tmux send-keys`, `expect`, and
/// piped scripts terminate a line — instead decodes as `Ctrl+J`. A shell must
/// treat that byte as "run this line": bash and zsh both bind `\C-j` to
/// accept-line, and the two are indistinguishable at the byte level in a raw
/// terminal. Without this normalization such input can never be submitted
/// from the editor — it just keeps adding newlines.
fn normalize_line_terminator(code: RawKey, modifiers: RawModifiers) -> (RawKey, RawModifiers) {
    if code == RawKey::Char('j')
        && modifiers
            == (RawModifiers {
                control: true,
                ..RawModifiers::default()
            })
    {
        (RawKey::Enter, RawModifiers::default())
    } else {
        (code, modifiers)
    }
}

fn map_key(key: RawKey) -> Key {
    match key {
        RawKey::Char(character) => Key::Character(character),
        RawKey::Enter => Key::Enter,
        RawKey::Escape => Key::Escape,
        RawKey::Backspace => Key::Backspace,
        RawKey::Tab => Key::Tab,
        RawKey::BackTab => Key::BackTab,
        RawKey::Up => Key::Up,
        RawKey::Down => Key::Down,
        RawKey::Left => Key::Left,
        RawKey::Right => Key::Right,
        RawKey::Home => Key::Home,
        RawKey::End => Key::End,
        RawKey::PageUp => Key::PageUp,
        RawKey::PageDown => Key::PageDown,
        RawKey::Delete => Key::Delete,
        RawKey::Insert => Key::Insert,
        RawKey::Function(number) => Key::Function(number),
        RawKey::Null => Key::Null,
    }
}

/// Decode a complete `CSI row ; column R` sequence into 0-based
/// `(column, row)`.
pub(crate) fn cursor_report(sequence: &[u8]) -> Option<(u16, u16)> {
    let body = sequence.strip_prefix(b"\x1B[")?.strip_suffix(b"R")?;
    let body = std::str::from_utf8(body).ok()?;
    let (row, column) = body.split_once(';')?;
    let row: u16 = row.parse().ok()?;
    let column: u16 = column.parse().ok()?;
    Some((column.saturating_sub(1), row.saturating_sub(1)))
}

/// Find a `CSI row ; column R` report in `bytes`, returning its span and
/// 0-based `(column, row)`.
///
/// The scanner tolerates surrounding input, because a query can read
/// keystrokes before the report arrives, and oversized parameters are
/// skipped rather than aborting the search.
pub(crate) fn find_cursor_report(bytes: &[u8]) -> Option<(usize, usize, u16, u16)> {
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
        return Some((
            index,
            cursor + 1,
            column.saturating_sub(1),
            row.saturating_sub(1),
        ));
    }
    None
}

/// Length of the CSI sequence starting the buffer, through its final byte.
/// Returns `None` when no final byte (range `0x40..=0x7E`, excluding the
/// `0x30..=0x3F` parameter and `0x20..=0x2F` intermediate ranges) is buffered
/// yet.
fn csi_final_len(buf: &[u8]) -> Option<usize> {
    if buf.len() < 3 || buf[0] != 0x1B || buf[1] != b'[' {
        return None;
    }
    buf.iter()
        .enumerate()
        .skip(2)
        .find(|item| !(0x30..=0x3F).contains(item.1) && !(0x20..=0x2F).contains(item.1))
        .map(|(index, _)| index + 1)
}

/// Modifier state from the `;mask` parameter of `seq` (`ESC [ … ; mask final`).
/// Masks follow the terminal convention with bit 0 = shift, 1 = alt,
/// 2 = control; super, hyper, meta, and lock bits fold into `other`.
fn modifier_in_seq(seq: &[u8]) -> Option<RawModifiers> {
    let body = std::str::from_utf8(&seq[2..seq.len() - 1]).ok()?;
    let mut parts = body.split(';');
    parts.next()?;
    match parts.next() {
        None => Some(RawModifiers::default()),
        Some(mask_text) => {
            let mask_text = mask_text.split(':').next().unwrap_or("");
            if mask_text.is_empty() {
                return None;
            }
            let mask: u8 = mask_text.parse().ok()?;
            Some(decode_modifier_mask(mask))
        }
    }
}

/// Split a `~`/`u` body (`number[;mask[:kind]]`) into its number and
/// modifiers.
fn split_csi_modifier(body: &[u8]) -> Option<(u32, RawModifiers)> {
    let text = std::str::from_utf8(body).ok()?;
    let mut parts = text.split(';');
    let number: u32 = parts.next()?.parse().ok()?;
    let modifiers = match parts.next() {
        None => RawModifiers::default(),
        Some(mask_text) => {
            let mask_text = mask_text.split(':').next().unwrap_or("");
            let mask: u8 = mask_text.parse().ok()?;
            decode_modifier_mask(mask)
        }
    };
    Some((number, modifiers))
}

fn decode_modifier_mask(mask: u8) -> RawModifiers {
    let bits = mask.saturating_sub(1);
    RawModifiers {
        shift: bits & 1 != 0,
        alt: bits & 2 != 0,
        control: bits & 4 != 0,
        other: bits & 0b1111_1000 != 0,
    }
}

/// Button code to mouse action: the low two bits plus the high bits form the
/// button number, bit 5 marks dragging, 3 is release-as-Left, and 4/5/6/7
/// unpressed are wheel and horizontal scroll.
fn mouse_button(code: u8) -> Option<MouseAction> {
    let button = (code & 0b0000_0011) | ((code & 0b1100_0000) >> 4);
    let dragging = code & 0b0010_0000 == 0b0010_0000;
    match (button, dragging) {
        (0, false) => Some(MouseAction::Down(MouseButton::Left)),
        (1, false) => Some(MouseAction::Down(MouseButton::Middle)),
        (2, false) => Some(MouseAction::Down(MouseButton::Right)),
        (0, true) => Some(MouseAction::Drag(MouseButton::Left)),
        (1, true) => Some(MouseAction::Drag(MouseButton::Middle)),
        (2, true) => Some(MouseAction::Drag(MouseButton::Right)),
        (3, false) => Some(MouseAction::Up(MouseButton::Left)),
        (3, true) | (4, true) | (5, true) => Some(MouseAction::Moved),
        (4, false) => Some(MouseAction::ScrollUp),
        (5, false) => Some(MouseAction::ScrollDown),
        (6, false) => Some(MouseAction::ScrollLeft),
        (7, false) => Some(MouseAction::ScrollRight),
        _ => None,
    }
}

/// The button carried by press, release, and drag actions; movement and
/// wheel actions carry none.
fn drag_button(action: &MouseAction) -> Option<MouseButton> {
    match action {
        MouseAction::Down(button) | MouseAction::Up(button) | MouseAction::Drag(button) => {
            Some(*button)
        }
        _ => None,
    }
}

/// UTF-8 decode of a buffer head, validating continuation bytes strictly.
enum Utf8 {
    Char(char, usize),
    NeedMore,
    Invalid,
}

fn decode_utf8(buf: &[u8]) -> Utf8 {
    match std::str::from_utf8(buf) {
        Ok(text) => match text.chars().next() {
            Some(character) => Utf8::Char(character, character.len_utf8()),
            None => Utf8::NeedMore,
        },
        Err(_) => {
            let required = match buf[0] {
                0x00..=0x7F => 1,
                0xC0..=0xDF => 2,
                0xE0..=0xEF => 3,
                0xF0..=0xF7 => 4,
                _ => return Utf8::Invalid,
            };
            if required > 1 {
                for byte in &buf[1..] {
                    if byte & !0b0011_1111 != 0b1000_0000 {
                        return Utf8::Invalid;
                    }
                }
            }
            if buf.len() < required {
                Utf8::NeedMore
            } else {
                Utf8::Invalid
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Feed bytes and drain every event the buffer yields.
    fn events(bytes: &[u8]) -> (Vec<RawEvent>, AnsiParser) {
        let mut parser = AnsiParser::new();
        parser.push(bytes);
        let mut out = Vec::new();
        loop {
            match parser.try_parse() {
                Parse::Event(event) => out.push(event),
                Parse::Again => {}
                Parse::NeedMore => break,
            }
        }
        (out, parser)
    }

    fn key_event(key: RawKey) -> RawEvent {
        RawEvent::Key {
            key,
            modifiers: RawModifiers::default(),
        }
    }

    #[test]
    fn plain_bytes_decode() {
        assert_eq!(
            events(b"a"),
            (vec![key_event(RawKey::Char('a'))], AnsiParser::new())
        );
        assert_eq!(
            events(b"\r"),
            (vec![key_event(RawKey::Enter)], AnsiParser::new())
        );
        assert_eq!(
            events(b"\t"),
            (vec![key_event(RawKey::Tab)], AnsiParser::new())
        );
        assert_eq!(
            events(b"\x7F"),
            (vec![key_event(RawKey::Backspace)], AnsiParser::new())
        );
        // Ctrl+C is the byte 0x03, decoded as a controlled character.
        let (decoded, _) = events(b"\x03");
        assert_eq!(
            decoded,
            vec![RawEvent::Key {
                key: RawKey::Char('c'),
                modifiers: RawModifiers {
                    control: true,
                    ..RawModifiers::default()
                },
            }]
        );
        // Ctrl+J stays raw so line-terminator normalization owns Enter.
        let (decoded, _) = events(b"\n");
        assert_eq!(
            decoded,
            vec![RawEvent::Key {
                key: RawKey::Char('j'),
                modifiers: RawModifiers {
                    control: true,
                    ..RawModifiers::default()
                },
            }]
        );
        // Uppercase carries Shift.
        let (decoded, _) = events(b"A");
        assert_eq!(
            decoded,
            vec![RawEvent::Key {
                key: RawKey::Char('A'),
                modifiers: RawModifiers {
                    shift: true,
                    ..RawModifiers::default()
                },
            }]
        );
    }

    #[test]
    fn multibyte_utf8_needs_all_bytes() {
        let mut parser = AnsiParser::new();
        parser.push(b"\xC3");
        assert_eq!(parser.try_parse(), Parse::NeedMore);
        parser.push(b"\xA9");
        assert_eq!(
            parser.try_parse(),
            Parse::Event(key_event(RawKey::Char('\u{e9}')))
        );
    }

    #[test]
    fn invalid_bytes_are_skipped_without_events() {
        // A lone continuation byte carries no key and must not stall.
        assert_eq!(events(b"\x80"), (vec![], AnsiParser::new()));
        // Skipping resumes at the next valid byte.
        assert_eq!(
            events(b"\x80a"),
            (vec![key_event(RawKey::Char('a'))], AnsiParser::new())
        );
    }

    #[test]
    fn csi_arrows_home_end_and_modifiers() {
        assert_eq!(
            events(b"\x1B[A"),
            (vec![key_event(RawKey::Up)], AnsiParser::new())
        );
        assert_eq!(
            events(b"\x1B[H"),
            (vec![key_event(RawKey::Home)], AnsiParser::new())
        );
        // Ctrl+Up carries its modifier mask.
        let (decoded, _) = events(b"\x1B[1;5A");
        assert_eq!(
            decoded,
            vec![RawEvent::Key {
                key: RawKey::Up,
                modifiers: RawModifiers {
                    control: true,
                    ..RawModifiers::default()
                },
            }]
        );
        // Shift+Tab is BackTab with Shift held.
        let (decoded, _) = events(b"\x1B[Z");
        assert_eq!(
            decoded,
            vec![RawEvent::Key {
                key: RawKey::BackTab,
                modifiers: RawModifiers {
                    shift: true,
                    ..RawModifiers::default()
                },
            }]
        );
    }

    #[test]
    fn tilde_specials_and_function_keys() {
        assert_eq!(
            events(b"\x1B[3~"),
            (vec![key_event(RawKey::Delete)], AnsiParser::new())
        );
        assert_eq!(
            events(b"\x1B[5~"),
            (vec![key_event(RawKey::PageUp)], AnsiParser::new())
        );
        assert_eq!(
            events(b"\x1B[15~"),
            (vec![key_event(RawKey::Function(5))], AnsiParser::new())
        );
        // Modified Delete keeps its key with Shift held.
        let (decoded, _) = events(b"\x1B[3;2~");
        assert_eq!(
            decoded,
            vec![RawEvent::Key {
                key: RawKey::Delete,
                modifiers: RawModifiers {
                    shift: true,
                    ..RawModifiers::default()
                },
            }]
        );
    }

    #[test]
    fn alt_chords_and_double_escape() {
        // Alt+x is ESC followed by the key.
        let (decoded, _) = events(b"\x1Bx");
        assert_eq!(
            decoded,
            vec![RawEvent::Key {
                key: RawKey::Char('x'),
                modifiers: RawModifiers {
                    alt: true,
                    ..RawModifiers::default()
                },
            }]
        );
        // A fast double-ESC yields one Escape and keeps the second pending.
        let (decoded, rest) = events(b"\x1B\x1B");
        assert_eq!(decoded, vec![key_event(RawKey::Escape)]);
        assert!(rest.is_lone_esc());
    }

    #[test]
    fn lone_escape_waits_for_continuation() {
        let mut parser = AnsiParser::new();
        parser.push(b"\x1B");
        assert_eq!(parser.try_parse(), Parse::NeedMore);
        assert!(parser.is_lone_esc());
    }

    #[test]
    fn sgr_mouse_press_drag_release_and_scroll() {
        // Left press at (7,3): terminal coordinates are 1-based on the wire.
        let (decoded, _) = events(b"\x1B[<0;8;4M");
        assert_eq!(
            decoded,
            vec![RawEvent::Mouse(RawMouse {
                action: MouseAction::Down(MouseButton::Left),
                button: Some(MouseButton::Left),
                column: 7,
                row: 3,
            })]
        );
        // Release arrives lowercase and converts the press kind.
        let (decoded, _) = events(b"\x1B[<0;8;4m");
        assert_eq!(
            decoded,
            vec![RawEvent::Mouse(RawMouse {
                action: MouseAction::Up(MouseButton::Left),
                button: Some(MouseButton::Left),
                column: 7,
                row: 3,
            })]
        );
        // Drag adds bit 5 to the button code.
        let (decoded, _) = events(b"\x1B[<32;8;4M");
        assert_eq!(
            decoded,
            vec![RawEvent::Mouse(RawMouse {
                action: MouseAction::Drag(MouseButton::Left),
                button: Some(MouseButton::Left),
                column: 7,
                row: 3,
            })]
        );
        // Wheel reports carry no button.
        let (decoded, _) = events(b"\x1B[<64;8;4M");
        assert_eq!(
            decoded,
            vec![RawEvent::Mouse(RawMouse {
                action: MouseAction::ScrollUp,
                button: None,
                column: 7,
                row: 3,
            })]
        );
    }

    #[test]
    fn x10_mouse_decodes_raw_coordinates() {
        // ESC [ M, button 32 (left press), coords 40/41 → (7,8) zero-based.
        let (decoded, _) = events(b"\x1B[M\x20\x28\x29");
        assert_eq!(
            decoded,
            vec![RawEvent::Mouse(RawMouse {
                action: MouseAction::Down(MouseButton::Left),
                button: Some(MouseButton::Left),
                column: 7,
                row: 8,
            })]
        );
    }

    #[test]
    fn bracketed_paste_waits_for_its_terminator() {
        let mut parser = AnsiParser::new();
        parser.push(b"\x1B[200~hello");
        assert_eq!(parser.try_parse(), Parse::NeedMore);
        parser.push(b" \x1B[201~");
        assert_eq!(
            parser.try_parse(),
            Parse::Event(RawEvent::Paste("hello ".into()))
        );
    }

    #[test]
    fn focus_and_device_responses_are_silent() {
        assert_eq!(events(b"\x1B[I"), (vec![], AnsiParser::new()));
        assert_eq!(events(b"\x1B[O"), (vec![], AnsiParser::new()));
        // A cursor-position report never leaks as keystrokes.
        assert_eq!(events(b"\x1B[24;80R"), (vec![], AnsiParser::new()));
    }

    #[test]
    fn cursor_reports_are_recorded_for_a_waiting_query() {
        let _guard = crate::test_support::lock();
        crate::inbox::clear_cursor_report();
        let (decoded, _) = events(b"\x1B[24;80R");
        assert!(decoded.is_empty());
        assert_eq!(crate::inbox::take_cursor_report(), Some((79, 23)));
    }

    #[test]
    fn cursor_report_scans_anywhere_in_the_buffer() {
        assert_eq!(find_cursor_report(b"\x1B[12;34R"), Some((0, 8, 33, 11)));
        assert_eq!(
            find_cursor_report(b"\x1B[Ahello\x1B[3;4R"),
            Some((8, 14, 3, 2))
        );
    }

    #[test]
    fn incomplete_or_absent_reports_are_none() {
        assert_eq!(find_cursor_report(b"\x1B[12;"), None);
        assert_eq!(find_cursor_report(b"\x1B[12;34"), None);
        assert_eq!(find_cursor_report(b"plain text"), None);
    }

    #[test]
    fn oversized_parameters_do_not_abort_the_scan() {
        assert_eq!(
            find_cursor_report(b"\x1B[999999999;1R\x1B[2;3R"),
            Some((14, 20, 2, 1))
        );
    }

    #[test]
    fn fragmented_sequences_reassemble() {
        // A six-byte mouse report split across reads still decodes whole.
        let mut parser = AnsiParser::new();
        parser.push(b"\x1B[<0;");
        assert_eq!(parser.try_parse(), Parse::NeedMore);
        parser.push(b"8;4M");
        assert_eq!(
            parser.try_parse(),
            Parse::Event(RawEvent::Mouse(RawMouse {
                action: MouseAction::Down(MouseButton::Left),
                button: Some(MouseButton::Left),
                column: 7,
                row: 3,
            }))
        );
    }

    #[test]
    fn csi_u_decodes_codepoints() {
        // fixterms `a` with no modifiers.
        let (decoded, _) = events(b"\x1B[97u");
        assert_eq!(decoded, vec![key_event(RawKey::Char('a'))]);
        // `A` (65) with shift mask decodes shifted.
        let (decoded, _) = events(b"\x1B[65;2u");
        assert_eq!(
            decoded,
            vec![RawEvent::Key {
                key: RawKey::Char('A'),
                modifiers: RawModifiers {
                    shift: true,
                    ..RawModifiers::default()
                },
            }]
        );
    }
}
