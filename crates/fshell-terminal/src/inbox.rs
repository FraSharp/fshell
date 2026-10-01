// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Process-wide channel for input recovered outside the event source.
//!
//! The terminal device normally has exactly one reader, the event source.
//! Two paths read it anyway: a cursor-position query must read the device
//! when no source is active, and a stream worker may hold an event whose
//! consumer vanished. Both hand what they read here, in read order, so the
//! next source replays it before touching the device and no keystroke is
//! dropped.
//!
//! Cursor reports travel separately: whichever parser sees one records it,
//! so a query whose reply was consumed by a concurrent source still
//! completes instead of timing out.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};

use crate::input::InputEvent;

/// Recovered input, in the order it was read from the device.
#[derive(Debug)]
pub(crate) enum Recovered {
    /// Raw device bytes no parser has seen yet.
    Bytes(Vec<u8>),
    /// A decoded event whose consumer vanished before delivery.
    Event(InputEvent),
}

static INBOX: Mutex<VecDeque<Recovered>> = Mutex::new(VecDeque::new());
static CURSOR_REPORT: Mutex<Option<(u16, u16)>> = Mutex::new(None);
static DEVICE_READ: Mutex<()> = Mutex::new(());

/// Take a lock, ignoring poisoning: the guarded values are queues and slots
/// that are always left consistent, so a panicking writer cannot corrupt
/// them.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Queue bytes read outside the event source.
pub(crate) fn push_bytes(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    lock(&INBOX).push_back(Recovered::Bytes(bytes.to_vec()));
}

/// Queue a decoded event whose consumer is gone.
pub(crate) fn push_event(event: InputEvent) {
    lock(&INBOX).push_back(Recovered::Event(event));
}

/// Oldest recovered item, if any.
pub(crate) fn pop() -> Option<Recovered> {
    lock(&INBOX).pop_front()
}

/// Record a cursor report for a waiting query.
pub(crate) fn record_cursor_report(column: u16, row: u16) {
    *lock(&CURSOR_REPORT) = Some((column, row));
}

/// Take the recorded cursor report, if one is waiting.
pub(crate) fn take_cursor_report() -> Option<(u16, u16)> {
    lock(&CURSOR_REPORT).take()
}

/// Discard a cursor report left over from an earlier exchange.
pub(crate) fn clear_cursor_report() {
    let _ = lock(&CURSOR_REPORT).take();
}

/// Serialize device reads between the event source and a cursor query, so
/// recovered bytes keep the order in which the device released them.
pub(crate) fn lock_device() -> MutexGuard<'static, ()> {
    lock(&DEVICE_READ)
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::input::{Key, KeyEvent, Modifiers};

    #[test]
    fn items_keep_their_order() {
        let _guard = crate::test_support::lock();
        push_bytes(b"ab");
        push_event(InputEvent::Key(KeyEvent::new(
            Key::Enter,
            Modifiers::empty(),
        )));
        match pop() {
            Some(Recovered::Bytes(bytes)) => assert_eq!(bytes, b"ab"),
            other => panic!("expected bytes first, got {other:?}"),
        }
        assert!(matches!(pop(), Some(Recovered::Event(_))));
        assert!(pop().is_none());
    }

    #[test]
    fn cursor_reports_are_taken_once() {
        let _guard = crate::test_support::lock();
        clear_cursor_report();
        record_cursor_report(7, 3);
        assert_eq!(take_cursor_report(), Some((7, 3)));
        assert_eq!(take_cursor_report(), None);
    }
}
