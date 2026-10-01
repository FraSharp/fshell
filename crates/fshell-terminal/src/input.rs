// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Terminal input vocabulary and source state machine.
//!
//! [`InputEvent`] and friends are the only input language fshell's
//! interactive interfaces speak. Decoding lives in [`crate::parse`];
//! transport (blocking poll, async stream) lives in [`crate::unix`].

use std::io;
use std::time::{Duration, Instant};

use thiserror::Error;

#[cfg(unix)]
pub use crate::unix::{UnixEventSource, UnixEventStream};

use crate::parse::RawModifiers;

/// A terminal event understood by fshell's interactive interfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize { columns: u16, rows: u16 },
    Paste(String),
}

/// A key fshell can act on. Backend-specific keys outside this vocabulary are
/// represented by [`Key::Other`] and can be safely ignored by applications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Character(char),
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
    Other,
}

/// The modifier keys relevant to fshell's input behavior.
///
/// Super, Hyper, Meta, and any future modifier bits are folded into
/// `OTHER`. This preserves the important semantic distinction between an
/// unmodified character and a character with an unsupported modifier.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const SHIFT: Self = Self(0b0001);
    pub const CONTROL: Self = Self(0b0010);
    pub const ALT: Self = Self(0b0100);
    const OTHER: Self = Self(0b1000);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub(crate) fn from_raw(modifiers: RawModifiers) -> Self {
        let mut result = Self::empty();
        if modifiers.shift {
            result |= Self::SHIFT;
        }
        if modifiers.control {
            result |= Self::CONTROL;
        }
        if modifiers.alt {
            result |= Self::ALT;
        }
        if modifiers.other {
            result |= Self::OTHER;
        }
        result
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Modifiers {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyAction {
    Press,
    Repeat,
    Release,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyEvent {
    pub key: Key,
    pub modifiers: Modifiers,
    pub action: KeyAction,
}

impl KeyEvent {
    /// Constructs a key press, convenient for application-level tests.
    pub const fn new(key: Key, modifiers: Modifiers) -> Self {
        Self {
            key,
            modifiers,
            action: KeyAction::Press,
        }
    }

    pub const fn with_action(mut self, action: KeyAction) -> Self {
        self.action = action;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseAction {
    Down(MouseButton),
    Up(MouseButton),
    Drag(MouseButton),
    Moved,
    ScrollDown,
    ScrollUp,
    ScrollLeft,
    ScrollRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MouseEvent {
    pub action: MouseAction,
    pub column: u16,
    pub row: u16,
}

/// Outcome of polling a terminal input source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputPoll {
    Event(InputEvent),
    Timeout,
    /// The terminal input lifetime ended. This outcome is sticky per source.
    Closed,
}

/// Errors from terminal input, retaining which read operation surfaced the
/// error for useful diagnostics.
#[derive(Debug, Error)]
pub enum InputError {
    #[error("terminal event poll failed: {0}")]
    Poll(#[source] io::Error),
    #[error("terminal event read failed: {0}")]
    Read(#[source] io::Error),
    #[error("terminal input worker failed: {0}")]
    Worker(String),
}

/// Input source used by synchronous interactive loops.
///
/// Implementations must return `Closed` after terminal EOF and must not
/// translate unrelated I/O failures into closure.
pub trait EventSource: Send {
    fn poll(&mut self, timeout: Duration) -> Result<InputPoll, InputError>;
}

pub(crate) trait EventReader: Send {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool>;
    fn read(&mut self) -> io::Result<Option<InputEvent>>;
}

pub(crate) struct InputSource<R> {
    reader: R,
    closed: bool,
}

impl<R> InputSource<R> {
    pub(crate) fn new(reader: R) -> Self {
        Self {
            reader,
            closed: false,
        }
    }
}

impl<R: EventReader> InputSource<R> {
    pub(crate) fn poll(&mut self, timeout: Duration) -> Result<InputPoll, InputError> {
        if self.closed {
            return Ok(InputPoll::Closed);
        }

        let start = Instant::now();
        loop {
            let remaining = timeout.saturating_sub(start.elapsed());
            let available = match self.reader.poll(remaining) {
                Ok(available) => available,
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                    self.closed = true;
                    return Ok(InputPoll::Closed);
                }
                // An interrupted wait carries no input either way, so it has
                // the same non-fatal meaning at this boundary as a timeout.
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                    return Ok(InputPoll::Timeout);
                }
                Err(error) => return Err(InputError::Poll(error)),
            };

            if !available {
                return Ok(InputPoll::Timeout);
            }

            let event = match self.reader.read() {
                Ok(event) => event,
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                    self.closed = true;
                    return Ok(InputPoll::Closed);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                    if start.elapsed() >= timeout {
                        return Ok(InputPoll::Timeout);
                    }
                    continue;
                }
                Err(error) => return Err(InputError::Read(error)),
            };

            if let Some(event) = event {
                return Ok(InputPoll::Event(event));
            }

            if start.elapsed() >= timeout {
                return Ok(InputPoll::Timeout);
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::parse::{
        RawEvent, RawKey, RawModifiers as WireModifiers, RawMouse, map_raw_event,
    };

    #[derive(Default)]
    struct FakeReader {
        polls: VecDeque<io::Result<bool>>,
        reads: VecDeque<io::Result<Option<InputEvent>>>,
        poll_count: usize,
    }

    impl EventReader for FakeReader {
        fn poll(&mut self, _timeout: Duration) -> io::Result<bool> {
            self.poll_count += 1;
            self.polls
                .pop_front()
                .expect("test must provide a poll result")
        }

        fn read(&mut self) -> io::Result<Option<InputEvent>> {
            self.reads
                .pop_front()
                .expect("test must provide a read result")
        }
    }

    fn source(
        polls: &[io::Result<bool>],
        reads: &[io::Result<Option<InputEvent>>],
    ) -> InputSource<FakeReader> {
        InputSource::new(FakeReader {
            polls: polls.iter().map(clone_io_result).collect(),
            reads: reads.iter().map(clone_io_result).collect(),
            poll_count: 0,
        })
    }

    fn clone_io_result<T: Clone>(result: &io::Result<T>) -> io::Result<T> {
        match result {
            Ok(val) => Ok(val.clone()),
            Err(error) => Err(io::Error::new(error.kind(), error.to_string())),
        }
    }

    #[test]
    fn timeout_is_distinct_from_closed_and_errors() {
        let mut source = source(&[Ok(false)], &[]);
        assert_eq!(source.poll(Duration::ZERO).unwrap(), InputPoll::Timeout);
    }

    #[test]
    fn eof_from_poll_becomes_sticky_closed() {
        let mut source = source(&[Err(io::Error::from(io::ErrorKind::UnexpectedEof))], &[]);
        assert_eq!(
            source.poll(Duration::from_secs(1)).unwrap(),
            InputPoll::Closed
        );
        assert_eq!(
            source.poll(Duration::from_secs(1)).unwrap(),
            InputPoll::Closed
        );
        assert_eq!(source.reader.poll_count, 1);
    }

    #[test]
    fn eof_from_read_becomes_sticky_closed() {
        let mut source = source(
            &[Ok(true)],
            &[Err(io::Error::from(io::ErrorKind::UnexpectedEof))],
        );
        assert_eq!(
            source.poll(Duration::from_secs(1)).unwrap(),
            InputPoll::Closed
        );
        assert_eq!(
            source.poll(Duration::from_secs(1)).unwrap(),
            InputPoll::Closed
        );
        assert_eq!(source.reader.poll_count, 1);
    }

    #[test]
    fn unexpected_poll_and_read_errors_remain_errors() {
        let mut poll_source = source(
            &[Err(io::Error::from(io::ErrorKind::PermissionDenied))],
            &[],
        );
        assert!(matches!(
            poll_source.poll(Duration::from_secs(1)),
            Err(InputError::Poll(error)) if error.kind() == io::ErrorKind::PermissionDenied
        ));

        let mut read_source = source(
            &[Ok(true)],
            &[Err(io::Error::from(io::ErrorKind::PermissionDenied))],
        );
        assert!(matches!(
            read_source.poll(Duration::from_secs(1)),
            Err(InputError::Read(error)) if error.kind() == io::ErrorKind::PermissionDenied
        ));
    }

    #[test]
    fn interrupted_poll_is_a_timeout_and_interrupted_read_retries() {
        let mut poll_source = source(&[Err(io::Error::from(io::ErrorKind::Interrupted))], &[]);
        assert_eq!(
            poll_source.poll(Duration::from_secs(1)).unwrap(),
            InputPoll::Timeout
        );

        let mut read_source = source(
            &[Ok(true), Ok(true)],
            &[
                Err(io::Error::from(io::ErrorKind::Interrupted)),
                Ok(Some(InputEvent::Key(KeyEvent::new(
                    Key::Enter,
                    Modifiers::empty(),
                )))),
            ],
        );
        assert_eq!(
            read_source.poll(Duration::from_secs(1)).unwrap(),
            InputPoll::Event(InputEvent::Key(KeyEvent::new(
                Key::Enter,
                Modifiers::empty(),
            )))
        );
    }

    #[test]
    fn skipped_events_keep_waiting() {
        // A swallowed decode (focus, undecodable bytes) surfaces as `None`
        // and the source waits for the next event within the same call.
        let mut read_source = source(
            &[Ok(true), Ok(true)],
            &[
                Ok(None),
                Ok(Some(InputEvent::Key(KeyEvent::new(
                    Key::Enter,
                    Modifiers::empty(),
                )))),
            ],
        );
        assert_eq!(
            read_source.poll(Duration::from_secs(1)).unwrap(),
            InputPoll::Event(InputEvent::Key(KeyEvent::new(
                Key::Enter,
                Modifiers::empty(),
            )))
        );
    }

    #[test]
    fn bare_line_feed_maps_to_enter() {
        // A raw `\n` decodes as Ctrl+J; a shell must treat it as Enter so
        // agent/tool input can submit a line (bash and zsh bind `\C-j` to
        // accept-line).
        assert_eq!(
            map_raw_event(RawEvent::Key {
                key: RawKey::Char('j'),
                modifiers: WireModifiers {
                    control: true,
                    ..WireModifiers::default()
                },
            }),
            Some(InputEvent::Key(KeyEvent::new(
                Key::Enter,
                Modifiers::empty(),
            )))
        );

        // A genuine Ctrl+J is the same byte and therefore also submits.
        // Modified variants (e.g. Ctrl+Alt+J) are left alone.
        assert_eq!(
            map_raw_event(RawEvent::Key {
                key: RawKey::Char('j'),
                modifiers: WireModifiers {
                    control: true,
                    alt: true,
                    ..WireModifiers::default()
                },
            }),
            Some(InputEvent::Key(KeyEvent::new(
                Key::Character('j'),
                Modifiers::CONTROL | Modifiers::ALT,
            )))
        );
    }

    #[test]
    fn wire_events_map_to_fshell_semantics() {
        let key = map_raw_event(RawEvent::Key {
            key: RawKey::Char('x'),
            modifiers: WireModifiers {
                control: true,
                alt: true,
                ..WireModifiers::default()
            },
        })
        .unwrap();
        assert_eq!(
            key,
            InputEvent::Key(KeyEvent {
                key: Key::Character('x'),
                modifiers: Modifiers::CONTROL | Modifiers::ALT,
                action: KeyAction::Press,
            })
        );

        let mouse = map_raw_event(RawEvent::Mouse(RawMouse {
            action: crate::input::MouseAction::Drag(MouseButton::Left),
            button: Some(MouseButton::Left),
            column: 7,
            row: 3,
        }))
        .unwrap();
        assert_eq!(
            mouse,
            InputEvent::Mouse(MouseEvent {
                action: MouseAction::Drag(MouseButton::Left),
                column: 7,
                row: 3,
            })
        );

        assert_eq!(
            map_raw_event(RawEvent::Paste("text".into())),
            Some(InputEvent::Paste("text".into()))
        );
    }

    #[test]
    fn unsupported_modifiers_do_not_become_plain_character_input() {
        let modifiers = Modifiers::from_raw(WireModifiers {
            other: true,
            ..WireModifiers::default()
        });
        assert!(!modifiers.is_empty());
        assert!(!modifiers.contains(Modifiers::CONTROL));
    }
}
