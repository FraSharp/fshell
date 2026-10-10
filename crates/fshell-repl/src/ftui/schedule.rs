// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Render scheduling for the inline REPL: poll deadlines and clock dirtiness.
//!
//! Extracted from the REPL loop (`ftui/mod.rs`) so wakeup policy is
//! unit-testable without a terminal. The loop aggregates input state
//! (agent/completion busyness, status-bar visibility) and asks the scheduler
//! when to wake and whether the clock needs a redraw. `update()` diffing plus
//! backend no-op suppression keep idle wakeups at zero bytes.

use super::prompt::PromptManager;
use std::time::Duration;

/// Owns render wakeup policy for one interactive prompt session.
pub struct RenderScheduler {
    last_status_second: Option<i64>,
}

impl RenderScheduler {
    pub fn new() -> Self {
        Self {
            last_status_second: None,
        }
    }

    /// Deadline-based poll timeout: spinner cadence only when referenced,
    /// fast poll while external loaders are busy, second-boundary wakeups for
    /// a visible clock, slow idle poll otherwise.
    pub fn poll_timeout(
        &self,
        prompt_mgr: &PromptManager,
        status_visible: bool,
        external_busy: bool,
    ) -> Duration {
        prompt_mgr.next_poll_timeout(status_visible, external_busy)
    }

    /// Second-boundary clock dirtiness: exactly one redraw per second while
    /// visible, zero otherwise. The first observation only establishes the
    /// baseline. Showing a per-second clock inherently opts into per-second
    /// output (which clears mouse selection); the default has no clock.
    pub fn check_clock(&mut self, status_visible: bool) -> bool {
        self.check_clock_at(status_visible, chrono::Local::now().timestamp())
    }

    /// Pure core of [`RenderScheduler::check_clock`] for deterministic tests.
    pub(crate) fn check_clock_at(&mut self, status_visible: bool, now_sec: i64) -> bool {
        if status_visible {
            if self.last_status_second != Some(now_sec) {
                // First observation establishes the baseline without forcing
                // a redraw; subsequent second changes do.
                if self.last_status_second.is_some() {
                    self.last_status_second = Some(now_sec);
                    return true;
                }
                self.last_status_second = Some(now_sec);
            }
            false
        } else {
            self.last_status_second = None;
            false
        }
    }
}

impl Default for RenderScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_stays_silent_when_hidden() {
        let mut sched = RenderScheduler::new();
        assert!(!sched.check_clock_at(false, 100));
        assert!(!sched.check_clock_at(false, 101));
    }

    #[test]
    fn clock_baseline_then_second_boundary() {
        let mut sched = RenderScheduler::new();
        // First observation establishes the baseline: no redraw.
        assert!(!sched.check_clock_at(true, 100));
        // Same second: silent.
        assert!(!sched.check_clock_at(true, 100));
        // Next second: exactly one redraw.
        assert!(sched.check_clock_at(true, 101));
        // Same second again: silent.
        assert!(!sched.check_clock_at(true, 101));
    }

    #[test]
    fn clock_hiding_resets_baseline() {
        let mut sched = RenderScheduler::new();
        assert!(!sched.check_clock_at(true, 100));
        assert!(!sched.check_clock_at(false, 101));
        // Re-showing re-establishes the baseline without a redraw.
        assert!(!sched.check_clock_at(true, 102));
    }

    #[test]
    fn poll_timeout_delegates_to_prompt_manager() {
        let env = fshell_engine::Env::new();
        let mgr = PromptManager::new(env);
        let sched = RenderScheduler::new();
        assert_eq!(
            sched.poll_timeout(&mgr, false, false),
            Duration::from_millis(1000)
        );
        assert_eq!(
            sched.poll_timeout(&mgr, false, true),
            Duration::from_millis(50)
        );
    }
}
