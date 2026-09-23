use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event as CrosstermEvent};
use thiserror::Error;

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
/// Super, Hyper, Meta, and any future Crossterm modifier bits are folded into
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

    fn from_crossterm(modifiers: event::KeyModifiers) -> Self {
        let mut result = Self::empty();
        if modifiers.contains(event::KeyModifiers::SHIFT) {
            result |= Self::SHIFT;
        }
        if modifiers.contains(event::KeyModifiers::CONTROL) {
            result |= Self::CONTROL;
        }
        if modifiers.contains(event::KeyModifiers::ALT) {
            result |= Self::ALT;
        }
        if modifiers.intersects(
            event::KeyModifiers::SUPER | event::KeyModifiers::HYPER | event::KeyModifiers::META,
        ) {
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

/// Errors from terminal input, retaining which public Crossterm operation
/// surfaced the error for useful diagnostics.
#[derive(Debug, Error)]
pub enum InputError {
    #[error("Crossterm event poll failed: {0}")]
    Poll(#[source] io::Error),
    #[error("Crossterm event read failed: {0}")]
    Read(#[source] io::Error),
    #[error("Crossterm event stream failed: {0}")]
    Stream(#[source] io::Error),
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

trait EventReader: Send {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool>;
    fn read(&mut self) -> io::Result<CrosstermEvent>;
}

struct CrosstermReader;

impl EventReader for CrosstermReader {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        event::poll(timeout)
    }

    fn read(&mut self) -> io::Result<CrosstermEvent> {
        event::read()
    }
}

/// Crossterm-backed source for blocking/polling interfaces.
pub struct CrosstermEventSource {
    source: InputSource<CrosstermReader>,
}

impl CrosstermEventSource {
    pub fn new() -> Self {
        Self {
            source: InputSource::new(CrosstermReader),
        }
    }

    pub fn poll(&mut self, timeout: Duration) -> Result<InputPoll, InputError> {
        self.source.poll(timeout)
    }
}

impl Default for CrosstermEventSource {
    fn default() -> Self {
        Self::new()
    }
}

impl EventSource for CrosstermEventSource {
    fn poll(&mut self, timeout: Duration) -> Result<InputPoll, InputError> {
        CrosstermEventSource::poll(self, timeout)
    }
}

struct InputSource<R> {
    reader: R,
    closed: bool,
}

impl<R> InputSource<R> {
    fn new(reader: R) -> Self {
        Self {
            reader,
            closed: false,
        }
    }
}

impl<R: EventReader> InputSource<R> {
    fn poll(&mut self, timeout: Duration) -> Result<InputPoll, InputError> {
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
                // Crossterm's public poll API reports an interrupted source
                // read as `Ok(false)`, so an exposed Interrupted error has the
                // same non-fatal meaning at this boundary.
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

            if let Some(event) = map_event(event) {
                return Ok(InputPoll::Event(event));
            }

            if start.elapsed() >= timeout {
                return Ok(InputPoll::Timeout);
            }
        }
    }
}

/// Async adapter for interactive clients that consume Crossterm's event
/// stream. Closure is sticky; `next()` returns `Closed` on every later call.
pub struct CrosstermEventStream {
    stream: crossterm::event::EventStream,
    closed: bool,
}

impl Default for CrosstermEventStream {
    fn default() -> Self {
        Self::new()
    }
}

impl CrosstermEventStream {
    pub fn new() -> Self {
        Self {
            stream: crossterm::event::EventStream::new(),
            closed: false,
        }
    }

    pub async fn next(&mut self) -> Result<InputPoll, InputError> {
        use futures::StreamExt;

        if self.closed {
            return Ok(InputPoll::Closed);
        }

        loop {
            match self.stream.next().await {
                None => {
                    self.closed = true;
                    return Ok(InputPoll::Closed);
                }
                Some(Err(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                    self.closed = true;
                    return Ok(InputPoll::Closed);
                }
                Some(Err(error)) if error.kind() == io::ErrorKind::Interrupted => continue,
                Some(Err(error)) => return Err(InputError::Stream(error)),
                Some(Ok(event)) => {
                    if let Some(event) = map_event(event) {
                        return Ok(InputPoll::Event(event));
                    }
                }
            }
        }
    }
}

fn map_event(event: CrosstermEvent) -> Option<InputEvent> {
    match event {
        CrosstermEvent::Key(key) => Some(InputEvent::Key(KeyEvent {
            key: map_key(key.code),
            modifiers: Modifiers::from_crossterm(key.modifiers),
            action: match key.kind {
                event::KeyEventKind::Press => KeyAction::Press,
                event::KeyEventKind::Repeat => KeyAction::Repeat,
                event::KeyEventKind::Release => KeyAction::Release,
            },
        })),
        CrosstermEvent::Mouse(mouse) => Some(InputEvent::Mouse(MouseEvent {
            action: match mouse.kind {
                event::MouseEventKind::Down(button) => MouseAction::Down(map_button(button)),
                event::MouseEventKind::Up(button) => MouseAction::Up(map_button(button)),
                event::MouseEventKind::Drag(button) => MouseAction::Drag(map_button(button)),
                event::MouseEventKind::Moved => MouseAction::Moved,
                event::MouseEventKind::ScrollDown => MouseAction::ScrollDown,
                event::MouseEventKind::ScrollUp => MouseAction::ScrollUp,
                event::MouseEventKind::ScrollLeft => MouseAction::ScrollLeft,
                event::MouseEventKind::ScrollRight => MouseAction::ScrollRight,
            },
            column: mouse.column,
            row: mouse.row,
        })),
        CrosstermEvent::Resize(columns, rows) => Some(InputEvent::Resize { columns, rows }),
        CrosstermEvent::Paste(text) => Some(InputEvent::Paste(text)),
        // No current fshell screen consumes focus changes. They are enabled
        // only as part of the current terminal mode and are ignored here.
        CrosstermEvent::FocusGained | CrosstermEvent::FocusLost => None,
    }
}

fn map_key(key: event::KeyCode) -> Key {
    match key {
        event::KeyCode::Char(character) => Key::Character(character),
        event::KeyCode::Enter => Key::Enter,
        event::KeyCode::Esc => Key::Escape,
        event::KeyCode::Backspace => Key::Backspace,
        event::KeyCode::Tab => Key::Tab,
        event::KeyCode::BackTab => Key::BackTab,
        event::KeyCode::Up => Key::Up,
        event::KeyCode::Down => Key::Down,
        event::KeyCode::Left => Key::Left,
        event::KeyCode::Right => Key::Right,
        event::KeyCode::Home => Key::Home,
        event::KeyCode::End => Key::End,
        event::KeyCode::PageUp => Key::PageUp,
        event::KeyCode::PageDown => Key::PageDown,
        event::KeyCode::Delete => Key::Delete,
        event::KeyCode::Insert => Key::Insert,
        event::KeyCode::F(number) => Key::Function(number),
        event::KeyCode::Null => Key::Null,
        _ => Key::Other,
    }
}

fn map_button(button: event::MouseButton) -> MouseButton {
    match button {
        event::MouseButton::Left => MouseButton::Left,
        event::MouseButton::Middle => MouseButton::Middle,
        event::MouseButton::Right => MouseButton::Right,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    #[derive(Default)]
    struct FakeReader {
        polls: VecDeque<io::Result<bool>>,
        reads: VecDeque<io::Result<CrosstermEvent>>,
        poll_count: usize,
    }

    impl EventReader for FakeReader {
        fn poll(&mut self, _timeout: Duration) -> io::Result<bool> {
            self.poll_count += 1;
            self.polls
                .pop_front()
                .expect("test must provide a poll result")
        }

        fn read(&mut self) -> io::Result<CrosstermEvent> {
            self.reads
                .pop_front()
                .expect("test must provide a read result")
        }
    }

    fn source(
        polls: &[io::Result<bool>],
        reads: &[io::Result<CrosstermEvent>],
    ) -> InputSource<FakeReader> {
        InputSource::new(FakeReader {
            polls: polls.iter().map(clone_io_result).collect(),
            reads: reads.iter().map(clone_io_result).collect(),
            poll_count: 0,
        })
    }

    fn clone_io_result<T: Clone>(result: &io::Result<T>) -> io::Result<T> {
        result
            .as_ref()
            .map(Clone::clone)
            .map_err(|error| io::Error::new(error.kind(), error.to_string()))
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
                Ok(CrosstermEvent::Key(event::KeyEvent::new(
                    event::KeyCode::Enter,
                    event::KeyModifiers::empty(),
                ))),
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
    fn crossterm_events_map_to_fshell_semantics() {
        let key = map_event(CrosstermEvent::Key(event::KeyEvent::new_with_kind(
            event::KeyCode::Char('x'),
            event::KeyModifiers::CONTROL | event::KeyModifiers::ALT,
            event::KeyEventKind::Repeat,
        )))
        .unwrap();
        assert_eq!(
            key,
            InputEvent::Key(KeyEvent {
                key: Key::Character('x'),
                modifiers: Modifiers::CONTROL | Modifiers::ALT,
                action: KeyAction::Repeat,
            })
        );

        let mouse = map_event(CrosstermEvent::Mouse(event::MouseEvent {
            kind: event::MouseEventKind::Drag(event::MouseButton::Left),
            column: 7,
            row: 3,
            modifiers: event::KeyModifiers::empty(),
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
            map_event(CrosstermEvent::Resize(80, 24)),
            Some(InputEvent::Resize {
                columns: 80,
                rows: 24
            })
        );
        assert_eq!(
            map_event(CrosstermEvent::Paste("text".into())),
            Some(InputEvent::Paste("text".into()))
        );
        assert_eq!(map_event(CrosstermEvent::FocusGained), None);
    }

    #[test]
    fn unsupported_modifiers_do_not_become_plain_character_input() {
        let modifiers = Modifiers::from_crossterm(event::KeyModifiers::SUPER);
        assert!(!modifiers.is_empty());
        assert!(!modifiers.contains(Modifiers::CONTROL));
    }
}
