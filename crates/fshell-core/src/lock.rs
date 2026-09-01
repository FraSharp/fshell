// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Unified synchronization primitives for fshell.
//! Backed by `parking_lot` for 1-byte, non-poisoning, futex-driven locks.

pub use parking_lot::lock_api::{RawMutex, RawRwLock};
pub use parking_lot::{
    Condvar, MappedMutexGuard, MappedRwLockReadGuard, MappedRwLockWriteGuard, Mutex, MutexGuard,
    Once, OnceState, RwLock, RwLockReadGuard, RwLockWriteGuard,
};
