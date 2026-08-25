// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::RwLock;
use fshell_hash::FxHashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProfilerCategory {
    Init,
    Source,
    FnCall,
}

impl ProfilerCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProfilerCategory::Init => "Init",
            ProfilerCategory::Source => "Source",
            ProfilerCategory::FnCall => "FnCall",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProfilerEntry {
    pub label: String,
    pub category: ProfilerCategory,
    pub total_time: Duration,
    pub children_total_time: Duration,
    pub call_count: u64,
}

impl ProfilerEntry {
    pub fn self_time(&self) -> Duration {
        self.total_time - std::cmp::min(self.total_time, self.children_total_time)
    }
}

#[derive(Debug, Clone)]
pub struct ProfilerEdge {
    pub caller_idx: usize,
    pub callee_idx: usize,
    pub total_time: Duration,
    pub call_count: u64,
}

#[derive(Debug, Clone)]
struct CallFrame {
    entry_idx: usize,
}

#[derive(Debug)]
pub struct ProfilerState {
    enabled: bool,
    entries: Vec<ProfilerEntry>,
    entry_map: FxHashMap<(String, ProfilerCategory), usize>,
    call_stack: Vec<CallFrame>,
    pub edges: Vec<ProfilerEdge>,
}

impl ProfilerState {
    pub fn new(enabled: bool) -> Self {
        ProfilerState {
            enabled,
            entries: Vec::new(),
            entry_map: FxHashMap::default(),
            call_stack: Vec::new(),
            edges: Vec::new(),
        }
    }

    fn entry_index(&mut self, label: &str, category: ProfilerCategory) -> usize {
        *self
            .entry_map
            .entry((label.to_string(), category))
            .or_insert_with(|| {
                let idx = self.entries.len();
                self.entries.push(ProfilerEntry {
                    label: label.to_string(),
                    category,
                    total_time: Duration::ZERO,
                    children_total_time: Duration::ZERO,
                    call_count: 0,
                });
                idx
            })
    }

    fn record(&mut self, entry_idx: usize, elapsed: Duration) {
        let popped = self.call_stack.pop();
        debug_assert_eq!(popped.as_ref().map(|f| f.entry_idx), Some(entry_idx));

        if let Some(entry) = self.entries.get_mut(entry_idx) {
            entry.total_time += elapsed;
            entry.call_count += 1;
        }

        if let Some(parent) = self.call_stack.last() {
            if let Some(parent_entry) = self.entries.get_mut(parent.entry_idx) {
                parent_entry.children_total_time += elapsed;
            }
            for edge in &mut self.edges {
                if edge.caller_idx == parent.entry_idx && edge.callee_idx == entry_idx {
                    edge.total_time += elapsed;
                    break;
                }
            }
        }
    }

    fn record_edge(&mut self, caller_idx: usize, callee_idx: usize) {
        for edge in &mut self.edges {
            if edge.caller_idx == caller_idx && edge.callee_idx == callee_idx {
                edge.call_count += 1;
                return;
            }
        }
        self.edges.push(ProfilerEdge {
            caller_idx,
            callee_idx,
            total_time: Duration::ZERO,
            call_count: 1,
        });
    }

    pub fn entries(&self) -> &[ProfilerEntry] {
        &self.entries
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn reset(&mut self) {
        self.entries.clear();
        self.entry_map.clear();
        self.call_stack.clear();
        self.edges.clear();
    }

    pub fn merge(&mut self, other: &mut ProfilerState) {
        self.entries.append(&mut other.entries);
        for (idx, entry) in self.entries.iter().enumerate() {
            self.entry_map
                .entry((entry.label.clone(), entry.category))
                .or_insert(idx);
        }
        other.entry_map.clear();
        other.call_stack.clear();
        other.edges.clear();
    }

    pub fn guard(
        profiler: &Arc<RwLock<ProfilerState>>,
        label: &str,
        category: ProfilerCategory,
    ) -> Option<ProfilerGuard> {
        let mut state = profiler.write();
        if !state.enabled {
            return None;
        }
        let entry_idx = state.entry_index(label, category);
        let parent_idx = state.call_stack.last().map(|f| f.entry_idx);
        if let Some(pidx) = parent_idx {
            state.record_edge(pidx, entry_idx);
        }
        let start = Instant::now();
        state.call_stack.push(CallFrame { entry_idx });
        drop(state);
        Some(ProfilerGuard {
            profiler: Some(Arc::clone(profiler)),
            entry_idx,
            start,
        })
    }
}

#[must_use]
pub struct ProfilerGuard {
    profiler: Option<Arc<RwLock<ProfilerState>>>,
    entry_idx: usize,
    start: Instant,
}

impl Drop for ProfilerGuard {
    fn drop(&mut self) {
        if let Some(profiler) = &self.profiler {
            let elapsed = self.start.elapsed();
            {
                let mut state = profiler.write();
                state.record(self.entry_idx, elapsed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_guard_records_time() {
        let profiler = Arc::new(RwLock::new(ProfilerState::new(true)));
        let guard = ProfilerState::guard(&profiler, "test_op", ProfilerCategory::Init);
        assert!(guard.is_some());
        std::thread::sleep(std::time::Duration::from_millis(1));
        drop(guard);
        let state = profiler.read();
        assert_eq!(state.entries.len(), 1);
        assert_eq!(state.entries[0].call_count, 1);
        assert!(state.entries[0].total_time >= Duration::from_millis(1));
        assert_eq!(state.entries[0].label, "test_op");
    }

    #[test]
    fn test_disabled_returns_none() {
        let profiler = Arc::new(RwLock::new(ProfilerState::new(false)));
        let guard = ProfilerState::guard(&profiler, "test", ProfilerCategory::Init);
        assert!(guard.is_none());
    }

    #[test]
    fn test_same_label_aggregates() {
        let profiler = Arc::new(RwLock::new(ProfilerState::new(true)));
        let g1 = ProfilerState::guard(&profiler, "op", ProfilerCategory::Init);
        drop(g1);
        let g2 = ProfilerState::guard(&profiler, "op", ProfilerCategory::Init);
        drop(g2);
        let state = profiler.read();
        assert_eq!(state.entries.len(), 1);
        assert_eq!(state.entries[0].call_count, 2);
    }

    #[test]
    fn test_parent_child_time() {
        let profiler = Arc::new(RwLock::new(ProfilerState::new(true)));
        let parent = ProfilerState::guard(&profiler, "parent", ProfilerCategory::Init);
        {
            let child = ProfilerState::guard(&profiler, "child", ProfilerCategory::FnCall);
            std::thread::sleep(std::time::Duration::from_millis(2));
            drop(child);
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
        drop(parent);
        let state = profiler.read();
        assert_eq!(state.entries.len(), 2);
        let p = &state.entries[0];
        let c = &state.entries[1];
        // parent total >= 3ms (includes child's 2ms + 1ms of own time)
        assert!(p.total_time >= Duration::from_millis(3));
        assert!(p.total_time < Duration::from_millis(50));
        // children_total_time should include the child's 2ms
        assert!(p.children_total_time >= Duration::from_millis(2));
        // self_time = total - children_total_time
        let self_time = p.self_time();
        assert!(self_time > Duration::ZERO);
        assert!(self_time <= p.total_time);
        // child total >= 2ms
        assert!(c.total_time >= Duration::from_millis(2));
        // edge recorded parent -> child
        assert_eq!(state.edges.len(), 1);
        assert_eq!(state.edges[0].call_count, 1);
        assert!(state.edges[0].total_time >= Duration::from_millis(2));
    }

    #[test]
    fn test_reset_clears() {
        let profiler = Arc::new(RwLock::new(ProfilerState::new(true)));
        let g = ProfilerState::guard(&profiler, "op", ProfilerCategory::Init);
        drop(g);
        profiler.write().reset();
        let state = profiler.read();
        assert!(state.entries.is_empty());
        assert!(state.edges.is_empty());
    }
}
