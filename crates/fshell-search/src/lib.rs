// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Search execution without shell values, runtime tasks, or terminal formatting.
//! Callers own authorization, cancellation, and bounded output transport.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::panic))]

pub mod content;
pub mod framing;
pub mod traversal;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("invalid search plan: {0}")]
    Plan(String),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Walk(#[from] ignore::Error),
    #[error("{0}")]
    Denied(String),
}

/// A stop request is successful early termination, not an I/O failure.
/// Workers check the caller's cancellation predicate even before the first match.
#[derive(Default)]
pub struct Control {
    stopped: AtomicBool,
}

impl Control {
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Access {
    Directory,
    File,
}
