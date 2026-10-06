// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Unix terminal input source owned by fshell.
//!
//! Reads the terminal file descriptor with `libc::poll`, decodes bytes with
//! [`AnsiParser`](crate::parse::AnsiParser), and reports closure when the
//! descriptor reaches EOF. Resize is observed by comparing the window size on
//! every wake: bounded by [`RESIZE_QUANTUM`] even when no keys arrive, with
//! no signal handlers and no extra file descriptors.
//!
//! Unix only.

use std::collections::VecDeque;
use std::fs::File;
use std::io;
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::input::{EventReader, InputError, InputEvent, InputPoll};
use crate::parse::{AnsiParser, ESC_TIMEOUT, Parse, RawEvent, RawKey, RawModifiers};

/// Upper bound on resize-observation latency while blocked in a read.
///
/// The terminal descriptor is not signalled on resize, so a wait with no
/// input wakes periodically to re-check the window size. Two hundred
/// milliseconds is invisible next to human resize gestures and costs one
/// `poll` + `ioctl` per quantum.
const RESIZE_QUANTUM: Duration = Duration::from_millis(200);

/// TTY input fd. Reads stay blocking: every read follows `poll` reporting
/// readiness, so it returns with the bytes already available. The
/// descriptor is shared with stdout and inherited by children, so it must
/// never carry `O_NONBLOCK`.
struct UnixFd {
    fd: RawFd,
    /// Owned handle when the fd came from `/dev/tty`; `None` for stdin.
    _owned: Option<File>,
}

impl UnixFd {
    /// Take ownership of `fd` without touching its status flags.
    fn open(fd: RawFd, owned: Option<File>) -> Self {
        Self { fd, _owned: owned }
    }

    /// Wait up to `timeout` for readability. Hangup and error conditions
    /// count as readable: the subsequent read resolves them to EOF or error.
    fn wait_readable(&self, timeout: Duration) -> io::Result<bool> {
        let mut fd = libc::pollfd {
            fd: self.fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = timeout.as_millis().min(std::ffi::c_int::MAX as u128) as std::ffi::c_int;
        let ready = unsafe { libc::poll(&mut fd, 1, millis) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            // A signal interrupted the wait without input; the caller's
            // loop re-checks its deadline and waits again.
            if error.kind() == io::ErrorKind::Interrupted {
                return Ok(false);
            }
            return Err(error);
        }
        Ok(ready > 0)
    }

    /// Read into `buf`; callers wait for readability first. A zero-byte
    /// read is terminal EOF.
    fn read_chunk(&self, buf: &mut [u8]) -> io::Result<usize> {
        let count = unsafe { libc::read(self.fd, buf.as_mut_ptr().cast(), buf.len()) };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(count as usize)
    }
}

#[allow(clippy::useless_conversion)]
fn window_size(fd: RawFd) -> Option<(u16, u16)> {
    unsafe {
        let mut size: libc::winsize = std::mem::zeroed();
        if libc::ioctl(fd, libc::TIOCGWINSZ.into(), &mut size) != 0 {
            return None;
        }
        if size.ws_col == 0 && size.ws_row == 0 {
            return None;
        }
        Some((size.ws_col, size.ws_row))
    }
}

/// Synchronous Unix terminal reader: TTY bytes into mapped input events.
///
/// Opening is lazy so construction never fails: the descriptor is acquired
/// on first use and open errors surface as [`InputError::Poll`], exactly
/// where callers already handle input failure.
pub(crate) struct UnixSource {
    fd: Option<UnixFd>,
    parser: AnsiParser,
    pending: VecDeque<InputEvent>,
    last_size: Option<(u16, u16)>,
    size_pending: bool,
    esc_due: bool,
    chunk: Vec<u8>,
}

impl UnixSource {
    pub fn new() -> Self {
        Self {
            fd: None,
            parser: AnsiParser::new(),
            pending: VecDeque::new(),
            last_size: None,
            size_pending: false,
            esc_due: false,
            chunk: vec![0u8; 1024],
        }
    }

    /// Open over an explicit fd (tests, PTY probes).
    #[cfg(test)]
    fn open_fd(fd: RawFd) -> Self {
        Self {
            fd: Some(UnixFd::open(fd, None)),
            parser: AnsiParser::new(),
            pending: VecDeque::new(),
            last_size: window_size(fd),
            size_pending: false,
            esc_due: false,
            chunk: vec![0u8; 1024],
        }
    }

    /// Acquire the terminal descriptor on first use: stdin when it is a TTY,
    /// else `/dev/tty`.
    fn ensure_open(&mut self) -> io::Result<RawFd> {
        if let Some(fd) = &self.fd {
            return Ok(fd.fd);
        }
        let (fd, owned) = if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
            (libc::STDIN_FILENO, None)
        } else {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/tty")?;
            let fd = file.as_raw_fd();
            (fd, Some(file))
        };
        self.last_size = window_size(fd);
        self.fd = Some(UnixFd::open(fd, owned));
        Ok(fd)
    }

    /// Decode every complete event in the buffer into the queue. Returns
    /// true when at least one event is pending; swallowed bytes (focus
    /// reports, undecodable input) drain silently inside.
    fn drain_events(&mut self) -> bool {
        loop {
            match self.parser.try_parse() {
                Parse::Event(raw) => {
                    if let Some(event) = crate::parse::map_raw_event(raw) {
                        self.pending.push_back(event);
                    }
                }
                Parse::Again => {}
                Parse::NeedMore => break,
            }
        }
        !self.pending.is_empty()
    }

    /// Replay input recovered outside this source, in the order it left the
    /// device: raw bytes feed the parser first, and decoded events follow
    /// the events those bytes produce. Returns true when an event is pending.
    fn recover(&mut self) -> bool {
        while let Some(item) = crate::inbox::pop() {
            match item {
                crate::inbox::Recovered::Bytes(bytes) => self.parser.push(&bytes),
                crate::inbox::Recovered::Event(event) => {
                    self.drain_events();
                    self.pending.push_back(event);
                }
            }
        }
        self.drain_events()
    }
}

impl EventReader for UnixSource {
    /// Wait until [`read`](EventReader::read) can make progress: a complete
    /// event is buffered, the window size moved, a pending lone `ESC` aged
    /// out, or the descriptor reached EOF.
    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        let fd = self.ensure_open()?;
        if !self.pending.is_empty() || self.esc_due || self.size_pending {
            return Ok(true);
        }
        // Seed the size baseline on first use so an early resize is caught.
        if self.last_size.is_none() {
            self.last_size = window_size(fd);
        }
        if self.recover() {
            return Ok(true);
        }
        let start = Instant::now();
        loop {
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                // A lone ESC with no continuation is the Escape key, not a
                // timeout: the byte is already in hand.
                if self.parser.take_lone_esc() {
                    self.esc_due = true;
                    return Ok(true);
                }
                return Ok(false);
            }
            let remaining = timeout - elapsed;
            // A lone ESC shortens the wait to its disambiguation window;
            // otherwise the wait is bounded so a lone resize still surfaces.
            let wait = if self.parser.is_lone_esc() {
                remaining.min(ESC_TIMEOUT)
            } else {
                remaining.min(RESIZE_QUANTUM)
            };
            let readable = self
                .fd
                .as_ref()
                .expect("descriptor opened above")
                .wait_readable(wait)?;
            if !readable {
                // A cursor-position query can read input while this source is
                // waiting, then return those bytes through the shared inbox.
                // Replay them on the next wait quantum; otherwise they remain
                // stranded here until an unrelated key makes the fd readable.
                if self.recover() {
                    return Ok(true);
                }
                // Quantum expired with no input: age out a lone ESC within
                // ESC_TIMEOUT and surface a lone resize within the quantum.
                if self.parser.is_lone_esc() && start.elapsed() >= ESC_TIMEOUT {
                    self.parser.take_lone_esc();
                    self.esc_due = true;
                    return Ok(true);
                }
                if let Some(size) = window_size(fd)
                    && Some(size) != self.last_size
                {
                    self.last_size = Some(size);
                    self.size_pending = true;
                    return Ok(true);
                }
                continue;
            }
            // Recovered bytes are older than whatever the device still holds,
            // so replay them first, then read, under the shared device lock
            // that keeps readers from interleaving.
            let read = {
                let _guard = crate::inbox::lock_device();
                if self.recover() {
                    return Ok(true);
                }
                self.fd
                    .as_ref()
                    .expect("descriptor opened above")
                    .read_chunk(&mut self.chunk)
            };
            match read {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "terminal input closed",
                    ));
                }
                Ok(count) => {
                    self.parser.push(&self.chunk[..count]);
                    if self.drain_events() {
                        return Ok(true);
                    }
                    // Partial sequence: loop back to waiting for the rest.
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                    // A signal interrupted the read; nothing was consumed.
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    // Readiness was consumed elsewhere; wait again.
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Decode one event. Succeeds after [`poll`](EventReader::poll) reported
    /// progress; a lone `WouldBlock` is unreachable by construction.
    fn read(&mut self) -> io::Result<Option<InputEvent>> {
        if self.esc_due {
            self.esc_due = false;
            return Ok(Some(
                crate::parse::map_raw_event(RawEvent::Key {
                    key: RawKey::Escape,
                    modifiers: RawModifiers::default(),
                })
                .expect("escape always maps to an event"),
            ));
        }
        if self.size_pending {
            self.size_pending = false;
            if let Some(size) = self.last_size {
                return Ok(Some(InputEvent::Resize {
                    columns: size.0,
                    rows: size.1,
                }));
            }
        }
        if let Some(event) = self.pending.pop_front() {
            return Ok(Some(event));
        }
        // Progress was reported but nothing is buffered: only a spurious
        // wakeup away from the next poll, never a silent wait.
        Err(io::Error::from(io::ErrorKind::WouldBlock))
    }
}

/// Synchronous Unix terminal input over [`UnixSource`].
pub struct UnixEventSource {
    source: crate::input::InputSource<UnixSource>,
}

impl UnixEventSource {
    pub fn new() -> Self {
        Self {
            source: crate::input::InputSource::new(UnixSource::new()),
        }
    }

    pub fn poll(&mut self, timeout: Duration) -> Result<InputPoll, InputError> {
        self.source.poll(timeout)
    }
}

impl Default for UnixEventSource {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::input::EventSource for UnixEventSource {
    fn poll(&mut self, timeout: Duration) -> Result<InputPoll, InputError> {
        UnixEventSource::poll(self, timeout)
    }
}

/// Async Unix terminal input: decoded events without blocking the executor.
///
/// A worker thread owns a [`UnixEventSource`] and forwards its outcomes; the
/// [`futures::Stream`] impl serves them from a queue. Closure is sticky, the
/// worker exits on its own within [`RESIZE_QUANTUM`] once the stream drops,
/// and non-EOF input errors end the stream as closure, mirroring the
/// previous stream contract.
pub struct UnixEventStream {
    events: std::sync::mpsc::Receiver<StreamMessage>,
    waker: Arc<std::sync::Mutex<Option<std::task::Waker>>>,
    closed: bool,
    shutdown: Arc<AtomicBool>,
}

enum StreamMessage {
    Event(InputEvent),
    Closed,
}

impl UnixEventStream {
    fn spawn(mut source: UnixEventSource) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let waker: Arc<std::sync::Mutex<Option<std::task::Waker>>> =
            Arc::new(std::sync::Mutex::new(None));
        let worker_waker = waker.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = shutdown.clone();
        let wake = move || {
            if let Ok(mut guard) = worker_waker.lock()
                && let Some(waker) = guard.take()
            {
                waker.wake();
            }
        };
        let _ = std::thread::Builder::new()
            .name("fsh-terminal-input".to_string())
            .spawn(move || {
                while !worker_shutdown.load(Ordering::Relaxed) {
                    match source.poll(Duration::from_secs(60 * 60)) {
                        Ok(InputPoll::Event(event)) => match tx.send(StreamMessage::Event(event)) {
                            Ok(()) => wake(),
                            Err(error) => {
                                // The consumer is gone; keep the event for
                                // whichever source reads next.
                                if let StreamMessage::Event(event) = error.0 {
                                    crate::inbox::push_event(event);
                                }
                                break;
                            }
                        },
                        Ok(InputPoll::Closed) => {
                            let _ = tx.send(StreamMessage::Closed);
                            wake();
                            break;
                        }
                        Ok(InputPoll::Timeout) => {}
                        Err(_) => {
                            let _ = tx.send(StreamMessage::Closed);
                            wake();
                            break;
                        }
                    }
                }
            });
        Self {
            events: rx,
            waker,
            closed: false,
            shutdown,
        }
    }

    pub fn new() -> Self {
        Self::spawn(UnixEventSource::new())
    }

    /// Open a stream over an explicit fd (tests).
    #[cfg(test)]
    fn open_fd(fd: RawFd) -> Self {
        let source = UnixEventSource {
            source: crate::input::InputSource::new(UnixSource::open_fd(fd)),
        };
        Self::spawn(source)
    }

    /// Next decoded event, waiting as long as needed. Closure is sticky:
    /// once the stream reports [`InputPoll::Closed`] it keeps reporting it.
    pub async fn next(&mut self) -> InputPoll {
        use futures::StreamExt;
        match StreamExt::next(self).await {
            Some(event) => InputPoll::Event(event),
            None => InputPoll::Closed,
        }
    }
}

impl Default for UnixEventStream {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for UnixEventStream {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

impl futures::Stream for UnixEventStream {
    type Item = InputEvent;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        if self.closed {
            return std::task::Poll::Ready(None);
        }
        match self.events.try_recv() {
            Ok(StreamMessage::Event(event)) => std::task::Poll::Ready(Some(event)),
            Ok(StreamMessage::Closed) => {
                self.closed = true;
                std::task::Poll::Ready(None)
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                if let Ok(mut waker) = self.waker.lock() {
                    *waker = Some(cx.waker().clone());
                }
                // A send racing the lock above queued an event already.
                match self.events.try_recv() {
                    Ok(StreamMessage::Event(event)) => std::task::Poll::Ready(Some(event)),
                    Ok(StreamMessage::Closed) => {
                        self.closed = true;
                        std::task::Poll::Ready(None)
                    }
                    Err(_) => std::task::Poll::Pending,
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.closed = true;
                std::task::Poll::Ready(None)
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::input::{Key, KeyEvent, Modifiers};
    use crate::test_support::{Pipe, lock};

    fn source_on(pipe: &Pipe) -> UnixEventSource {
        UnixEventSource {
            source: crate::input::InputSource::new(UnixSource::open_fd(pipe.reader)),
        }
    }

    fn expect_key(poll: Result<InputPoll, InputError>) -> (Key, crate::input::Modifiers) {
        match poll {
            Ok(InputPoll::Event(InputEvent::Key(key))) => (key.key, key.modifiers),
            other => panic!("expected a key event, got {other:?}"),
        }
    }

    #[test]
    fn pipe_roundtrip_preserves_order() {
        let _guard = lock();
        let mut pipe = Pipe::new().unwrap();
        let mut source = source_on(&pipe);
        pipe.write(b"a\x1B[A");
        assert_eq!(
            expect_key(source.poll(Duration::from_secs(5))),
            (Key::Character('a'), crate::input::Modifiers::empty())
        );
        assert_eq!(
            expect_key(source.poll(Duration::from_secs(5))),
            (Key::Up, crate::input::Modifiers::empty())
        );
    }

    #[test]
    fn fragmented_writes_reassemble_before_return() {
        let _guard = lock();
        let mut pipe = Pipe::new().unwrap();
        let mut source = source_on(&pipe);
        // A six-byte mouse report split mid-sequence must not surface
        // as an error or a partial key.
        pipe.write(b"\x1B[<0;");
        std::thread::sleep(Duration::from_millis(50));
        pipe.write(b"8;4M");
        match source.poll(Duration::from_secs(5)) {
            Ok(InputPoll::Event(InputEvent::Mouse(mouse))) => {
                assert_eq!(mouse.column, 7);
                assert_eq!(mouse.row, 3);
            }
            other => panic!("expected a mouse event, got {other:?}"),
        }
    }

    #[test]
    fn lone_escape_becomes_escape_not_timeout() {
        let _guard = lock();
        let mut pipe = Pipe::new().unwrap();
        let mut source = source_on(&pipe);
        pipe.write(b"\x1B");
        assert_eq!(
            expect_key(source.poll(Duration::from_secs(5))),
            (Key::Escape, crate::input::Modifiers::empty())
        );
    }

    #[test]
    fn escape_plus_byte_is_alt_not_escape() {
        let _guard = lock();
        let mut pipe = Pipe::new().unwrap();
        let mut source = source_on(&pipe);
        pipe.write(b"\x1Bx");
        let (key, modifiers) = expect_key(source.poll(Duration::from_secs(5)));
        assert_eq!(key, Key::Character('x'));
        assert!(modifiers.contains(crate::input::Modifiers::ALT));
    }

    #[test]
    fn idle_pipe_times_out() {
        let _guard = lock();
        let pipe = Pipe::new().unwrap();
        let mut source = source_on(&pipe);
        assert!(matches!(
            source.poll(Duration::from_millis(50)),
            Ok(InputPoll::Timeout)
        ));
    }

    #[test]
    fn closed_write_end_is_sticky_closed() {
        let _guard = lock();
        let mut pipe = Pipe::new().unwrap();
        let mut source = source_on(&pipe);
        pipe.close_writer();
        assert!(matches!(
            source.poll(Duration::from_secs(5)),
            Ok(InputPoll::Closed)
        ));
        assert!(matches!(
            source.poll(Duration::from_secs(5)),
            Ok(InputPoll::Closed)
        ));
    }

    #[test]
    fn stream_serves_events_then_closure() {
        let _guard = lock();
        let mut pipe = Pipe::new().unwrap();
        let mut stream = UnixEventStream::open_fd(pipe.reader);
        pipe.write(b"q");
        let first = futures::executor::block_on(stream.next());
        assert!(matches!(first, InputPoll::Event(InputEvent::Key(_))));
        pipe.close_writer();
        // EOF arrives once the worker drains the queued key.
        let mut saw_closed = false;
        for _ in 0..100 {
            if matches!(
                futures::executor::block_on(stream.next()),
                InputPoll::Closed
            ) {
                saw_closed = true;
                break;
            }
        }
        assert!(saw_closed, "stream must report closure after EOF");
        assert_eq!(
            futures::executor::block_on(stream.next()),
            InputPoll::Closed
        );
    }

    #[test]
    fn recovered_bytes_replay_before_the_device() {
        let _guard = lock();
        let pipe = Pipe::new().unwrap();
        let mut source = source_on(&pipe);
        crate::inbox::push_bytes(b"a");
        assert_eq!(
            expect_key(source.poll(Duration::from_secs(5))),
            (Key::Character('a'), Modifiers::empty())
        );
    }

    #[test]
    fn recovered_bytes_arriving_while_waiting_need_no_new_key() {
        let _guard = lock();
        let pipe = Pipe::new().unwrap();
        let mut source = source_on(&pipe);
        let producer = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
            crate::inbox::push_bytes(b"e");
        });

        assert_eq!(
            expect_key(source.poll(Duration::from_secs(1))),
            (Key::Character('e'), Modifiers::empty())
        );
        producer.join().unwrap();
    }

    #[test]
    fn recovered_events_keep_their_order_between_bytes() {
        let _guard = lock();
        let pipe = Pipe::new().unwrap();
        let mut source = source_on(&pipe);
        crate::inbox::push_bytes(b"a");
        crate::inbox::push_event(InputEvent::Key(KeyEvent::new(
            Key::Enter,
            Modifiers::empty(),
        )));
        crate::inbox::push_bytes(b"b");
        assert_eq!(
            expect_key(source.poll(Duration::from_secs(5))),
            (Key::Character('a'), Modifiers::empty())
        );
        assert_eq!(
            expect_key(source.poll(Duration::from_secs(5))),
            (Key::Enter, Modifiers::empty())
        );
        assert_eq!(
            expect_key(source.poll(Duration::from_secs(5))),
            (Key::Character('b'), Modifiers::empty())
        );
    }

    #[test]
    fn device_bytes_follow_recovered_ones() {
        let _guard = lock();
        let mut pipe = Pipe::new().unwrap();
        let mut source = source_on(&pipe);
        crate::inbox::push_bytes(b"a");
        pipe.write(b"b");
        assert_eq!(
            expect_key(source.poll(Duration::from_secs(5))),
            (Key::Character('a'), Modifiers::empty())
        );
        assert_eq!(
            expect_key(source.poll(Duration::from_secs(5))),
            (Key::Character('b'), Modifiers::empty())
        );
    }

    #[test]
    fn reports_consumed_by_the_source_answer_a_query() {
        let _guard = lock();
        let mut pipe = Pipe::new().unwrap();
        let mut source = source_on(&pipe);
        crate::inbox::clear_cursor_report();
        pipe.write(b"\x1B[3;9R");
        assert!(matches!(
            source.poll(Duration::from_millis(80)),
            Ok(InputPoll::Timeout)
        ));
        assert_eq!(crate::inbox::take_cursor_report(), Some((8, 2)));
    }

    #[test]
    fn bracketed_paste_reaches_the_source_whole() {
        let _guard = lock();
        let mut pipe = Pipe::new().unwrap();
        let mut source = source_on(&pipe);
        pipe.write(b"\x1B[200~pasted text\x1B[201~");
        match source.poll(Duration::from_secs(5)) {
            Ok(InputPoll::Event(InputEvent::Paste(text))) => assert_eq!(text, "pasted text"),
            other => panic!("expected a paste event, got {other:?}"),
        }
    }

    #[test]
    fn pty_resize_surfaces_as_an_event() {
        let _guard = lock();
        let pty = portable_pty::native_pty_system();
        let pair = pty
            .openpty(portable_pty::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let fd = pair.master.as_raw_fd().expect("master PTY descriptor");
        let mut source = UnixEventSource {
            source: crate::input::InputSource::new(UnixSource::open_fd(fd)),
        };
        pair.master
            .resize(portable_pty::PtySize {
                rows: 40,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        match source.poll(Duration::from_secs(5)) {
            Ok(InputPoll::Event(InputEvent::Resize { columns, rows })) => {
                assert_eq!((columns, rows), (120, 40));
            }
            other => panic!("expected a resize event, got {other:?}"),
        }
    }
}
