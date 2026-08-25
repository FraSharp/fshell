// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::RwLock;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::ReactiveEvent;

/// Reactive cells, dependency tracking, and file/cell tracking.
pub struct Reactivity {
    #[allow(clippy::type_complexity)]
    pub cells: Arc<
        RwLock<fshell_hash::FxHashMap<String, tokio::sync::watch::Receiver<Arc<Vec<crate::Val>>>>>,
    >,
    pub tx: Arc<tokio::sync::mpsc::Sender<ReactiveEvent>>,
    pub pipelines: Arc<RwLock<fshell_hash::FxHashMap<String, String>>>,
    pub deps: Arc<RwLock<fshell_hash::FxHashMap<String, fshell_hash::FxHashSet<String>>>>,
    pub tracked_reads: Arc<RwLock<Option<HashSet<PathBuf>>>>,
    pub tracked_cells: Arc<RwLock<Option<HashSet<String>>>>,
    /// Fast check: set before reactive evaluation starts, cleared after.
    /// Lets track_read/track_cell skip expensive lock acquisitions in the common case.
    pub tracking_active: Arc<AtomicBool>,
    /// Fast check: set to true if any reactive cells are registered to avoid cells lock overhead.
    pub has_cells: Arc<AtomicBool>,
}

impl Clone for Reactivity {
    fn clone(&self) -> Self {
        Self {
            cells: self.cells.clone(),
            tx: self.tx.clone(),
            pipelines: self.pipelines.clone(),
            deps: self.deps.clone(),
            tracked_reads: self.tracked_reads.clone(),
            tracked_cells: self.tracked_cells.clone(),
            tracking_active: self.tracking_active.clone(),
            has_cells: self.has_cells.clone(),
        }
    }
}

impl std::fmt::Debug for Reactivity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reactivity")
            .field("cells", &self.cells)
            .field("pipelines", &self.pipelines)
            .field("deps", &self.deps)
            .field("tracked_reads", &self.tracked_reads)
            .field("tracked_cells", &self.tracked_cells)
            .field("tracking_active", &self.tracking_active)
            .field("has_cells", &self.has_cells)
            .finish()
    }
}
