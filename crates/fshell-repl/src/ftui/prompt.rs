// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::cpu_dbg;
use crate::{PromptSnapshot, refresh_prompt_snapshot, render_prompt_template};
use fshell_core::lock::Mutex;
use fshell_engine::Env;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use std::sync::Arc;
use std::time::{Duration, Instant};

// Unified SGR→ratatui parser lives in `ftui::ansi` (A4).

fn val_to_string(v: &fshell_core::Val) -> String {
    match v {
        fshell_core::Val::Null => "".to_string(),
        fshell_core::Val::Bool(b) => b.to_string(),
        fshell_core::Val::Int(i) => i.to_string(),
        fshell_core::Val::Float(f) => f.to_string(),
        fshell_core::Val::String(s) => s.clone(),
        fshell_core::Val::Blob(b) => String::from_utf8_lossy(b).to_string(),
        other => format!("{:?}", other),
    }
}

// `ansi_to_spans` now lives in `ftui::ansi` — single source of truth for
// SGR→ratatui (A4). It was duplicated here (~200 lines) and in `mod.rs`;
// both now delegate so the two sites cannot diverge.
pub use crate::ftui::ansi::ansi_to_spans;

// Background Prompt Widget
pub struct PromptWidget {
    pub name: String,
    pub command: String,
    pub cached_output: Arc<Mutex<String>>,
    pub is_running: Arc<Mutex<bool>>,
    pub last_run: Arc<Mutex<Option<Instant>>>,
    active_handle: Option<tokio::task::JoinHandle<()>>,
}

impl PromptWidget {
    pub fn new(name: &str, command: &str) -> Self {
        Self {
            name: name.to_string(),
            command: command.to_string(),
            cached_output: Arc::new(Mutex::new(String::new())),
            is_running: Arc::new(Mutex::new(false)),
            last_run: Arc::new(Mutex::new(None)),
            active_handle: None,
        }
    }

    pub fn trigger_update(&mut self, env: &Env) {
        if let Some(handle) = self.active_handle.take()
            && !handle.is_finished()
        {
            self.active_handle = Some(handle);
            return;
        }

        let is_running = self.is_running.clone();
        {
            let mut running = is_running.lock();
            if *running {
                return;
            }
            *running = true;
        }

        let cmd = self.command.clone();
        let cached_output = self.cached_output.clone();
        let last_run = self.last_run.clone();
        let env_clone = env.clone();

        let handle = tokio::spawn(async move {
            let _running_guard = RunningFlagGuard {
                flag: is_running.clone(),
            };
            let mut parser = fshell_core::Parser::new(&cmd);
            let output = match parser.parse_statements() {
                Ok(stmts) => {
                    if let Some(stmt) = stmts.first() {
                        if let fshell_core::Stmt::Expr(expr) = stmt.unpack() {
                            if let fshell_core::Expr::Pipeline(pipeline) = expr.unpack() {
                                match fshell_engine::collect_pipeline(pipeline, &env_clone).await {
                                    Ok(vals) => vals
                                        .iter()
                                        .map(|v| match v {
                                            fshell_core::Val::String(s) => s.trim().to_string(),
                                            other => val_to_string(other).trim().to_string(),
                                        })
                                        .collect::<Vec<_>>()
                                        .join(" "),
                                    Err(e) => format!("widget-error: {}", e),
                                }
                            } else {
                                "invalid-expr".to_string()
                            }
                        } else {
                            "invalid-stmt".to_string()
                        }
                    } else {
                        "".to_string()
                    }
                }
                Err(e) => format!("parse-error: {}", e),
            };

            *cached_output.lock() = output;
            *last_run.lock() = Some(Instant::now());
        });
        self.active_handle = Some(handle);
    }
}

impl Drop for PromptWidget {
    fn drop(&mut self) {
        if let Some(handle) = self.active_handle.take() {
            handle.abort();
        }
    }
}

struct RunningFlagGuard {
    flag: Arc<Mutex<bool>>,
}

impl Drop for RunningFlagGuard {
    fn drop(&mut self) {
        *self.flag.lock() = false;
    }
}

// Spinners / Animations in prompt
pub struct PromptAnimation {
    pub name: String,
    pub frames: Vec<String>,
    pub frame_rate_ms: u64,
    started_at: Instant,
}

impl PromptAnimation {
    pub fn get_frame(&self) -> String {
        if self.frames.is_empty() {
            return String::new();
        }
        self.frame_at(self.started_at.elapsed())
    }

    fn frame_at(&self, elapsed: Duration) -> String {
        if self.frames.is_empty() {
            return String::new();
        }
        let frame_rate_ms = self.frame_rate_ms.max(1);
        let index =
            ((elapsed.as_millis() as u64 / frame_rate_ms) % self.frames.len() as u64) as usize;
        self.frames[index].clone()
    }
}

fn interpolate_template(
    tpl: &str,
    snapshot: &PromptSnapshot,
    animations: &[PromptAnimation],
    widgets: &[PromptWidget],
) -> String {
    let user = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
    let branch = snapshot.branch.clone();
    let duration_color = snapshot.duration_color.as_deref().unwrap_or("\x1b[37m");
    let mut resolved = render_prompt_template(
        tpl,
        &user,
        &snapshot.pwd,
        branch.as_deref(),
        snapshot.exit_code,
        snapshot.duration,
        snapshot.job_count,
        duration_color,
    );
    for anim in animations {
        let placeholder = format!("{{{}}}", anim.name);
        if resolved.contains(&placeholder) {
            resolved = resolved.replace(&placeholder, &anim.get_frame());
        }
    }
    for widget in widgets {
        let placeholder = format!("{{{}}}", widget.name);
        if resolved.contains(&placeholder) {
            let out = widget.cached_output.lock().clone();
            resolved = resolved.replace(&placeholder, &out);
        }
    }
    resolved
}

pub struct PromptManager {
    pub env: Env,
    pub widgets: Vec<PromptWidget>,
    pub animations: Vec<PromptAnimation>,
    pub last_snapshot_update: Instant,
    pub cached_snapshot: PromptSnapshot,
    pub last_known_widget_outputs: Vec<String>,
    last_animation_frames: Vec<String>,
    last_osc7_pwd: Option<String>,
    /// Last pwd the shared git service was pointed at. The service itself
    /// owns the watcher, cache and dirty flag; this just avoids redundant
    /// `ensure_watching` discovery on every input-loop iteration.
    watched_pwd: Option<String>,
}

impl PromptManager {
    pub fn new(env: Env) -> Self {
        let current_pwd = env.cwd().to_string_lossy().to_string();

        let snapshot = refresh_prompt_snapshot(&env, &current_pwd);

        let animations = vec![PromptAnimation {
            name: "SPINNER".to_string(),
            frames: vec!["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]
                .into_iter()
                .map(|s| s.to_string())
                .collect(),
            frame_rate_ms: 80,
            started_at: Instant::now(),
        }];

        crate::prompt::git_ensure_watching(&current_pwd);

        Self {
            env,
            widgets: Vec::new(),
            animations,
            last_snapshot_update: Instant::now(),
            cached_snapshot: snapshot,
            last_known_widget_outputs: Vec::new(),
            last_animation_frames: Vec::new(),
            last_osc7_pwd: None,
            watched_pwd: Some(current_pwd),
        }
    }

    /// Keep the manager's view of the world in sync with the live REPL env.
    /// `Env` is Arc-backed so this clone is cheap; without it theme/vars
    /// changes made by commands would never reach the prompt.
    pub fn sync_env(&mut self, env: &Env) {
        self.env = env.clone();
    }

    /// Emit OSC7 only when the directory actually changed. Called outside
    /// frame draws so the side-channel never interleaves with prompt rows.
    /// Returns true when an emission was attempted.
    pub fn maybe_emit_osc7(&mut self, pwd: &str) -> bool {
        if self.last_osc7_pwd.as_deref() == Some(pwd) {
            return false;
        }
        self.last_osc7_pwd = Some(pwd.to_string());
        crate::emit_osc7(pwd);
        true
    }

    pub fn refresh_snapshot(&mut self, cwd: &str) {
        self.cached_snapshot = refresh_prompt_snapshot(&self.env, cwd);
        self.last_snapshot_update = Instant::now();
        // Explicit refresh (command end, cd): re-point the shared watcher and
        // consume any pending dirty flag so the next idle `update()` stays silent.
        if self.watched_pwd.as_deref() != Some(cwd) {
            crate::prompt::git_ensure_watching(cwd);
            self.watched_pwd = Some(cwd.to_string());
        }
        let _ = crate::prompt::git_take_dirty();
    }

    /// Returns true if any prompt state changed (warranting a redraw).
    /// Idle-selection guarantee: returns false (zero bytes) when the snapshot,
    /// widget outputs and animation frames are all identical. Git refreshes
    /// are event-driven (fs watcher, pwd change, explicit refresh) with a
    /// slow safety-net fallback; there is no 1s poll.
    pub fn update(&mut self) -> bool {
        let mut changed = false;
        let current_pwd = self.env.cwd().to_string_lossy().to_string();

        // Re-point the shared fs watcher on directory change (cheap string
        // compare; `ensure_watching` does discovery only then).
        if self.watched_pwd.as_deref() != Some(&current_pwd) {
            crate::prompt::git_ensure_watching(&current_pwd);
            self.watched_pwd = Some(current_pwd.clone());
        }

        let pwd_changed = current_pwd != self.cached_snapshot.pwd;
        let git_event = crate::prompt::git_take_dirty();
        let fallback_due =
            self.last_snapshot_update.elapsed() > crate::prompt::GIT_FALLBACK_INTERVAL;

        if pwd_changed || git_event || fallback_due {
            cpu_dbg!(
                "PromptManager::update snapshot refresh (pwd_changed={} git_event={} fallback={})",
                pwd_changed,
                git_event,
                fallback_due
            );
            let fresh = refresh_prompt_snapshot(&self.env, &current_pwd);
            self.last_snapshot_update = Instant::now();
            if fresh != self.cached_snapshot {
                self.cached_snapshot = fresh;
                changed = true;
            }
        }

        // Trigger updates for widgets that have expired (e.g. run every 5s or if empty).
        // Independent of git refreshes so widgets stay fresh on long idle.
        for widget in &mut self.widgets {
            let should_run = {
                let lr = widget.last_run.lock();
                match *lr {
                    None => true,
                    Some(last_run) => last_run.elapsed() > Duration::from_secs(5),
                }
            };
            if should_run {
                widget.trigger_update(&self.env);
            }
        }

        // Animation frames change over time, but only matter when the template
        // actually references them. Otherwise an idle spinner definition must
        // never wake the renderer.
        if self.spinner_in_use() {
            let frames: Vec<String> = self.animations.iter().map(|a| a.get_frame()).collect();
            if frames != self.last_animation_frames {
                self.last_animation_frames = frames;
                changed = true;
            }
        }

        // Initialize or update widget output caches to detect background execution completion
        if self.last_known_widget_outputs.len() != self.widgets.len() {
            self.last_known_widget_outputs = self
                .widgets
                .iter()
                .map(|w| w.cached_output.lock().clone())
                .collect();
            changed = true;
        } else {
            for (i, w) in self.widgets.iter().enumerate() {
                let current = w.cached_output.lock().clone();
                if current != self.last_known_widget_outputs[i] {
                    self.last_known_widget_outputs[i] = current;
                    changed = true;
                }
            }
        }

        cpu_dbg!("PromptManager::update returning changed={}", changed);
        changed
    }

    /// True when the current prompt templates reference a time-based animation.
    fn spinner_in_use(&self) -> bool {
        if self.animations.is_empty() {
            return false;
        }
        let vars = self.env.vars.read();
        if let Some(fshell_core::Val::String(tpl)) = vars.get("FSH_PROMPT")
            && (tpl.contains("{SPINNER}") || tpl.contains("{spinner}"))
        {
            return true;
        }
        if let Some(fshell_core::Val::String(tpl)) = vars.get("FSH_PROMPT_RIGHT")
            && (tpl.contains("{SPINNER}") || tpl.contains("{spinner}"))
        {
            return true;
        }
        false
    }

    /// Returns true if any animation or widget is active (for poll timeout calculation).
    /// Kept for compatibility; prefer `next_poll_timeout` for deadline-based
    /// scheduling which wakes exactly when the next dynamic source needs it.
    pub fn has_active_animations(&self) -> bool {
        if self.spinner_in_use() {
            cpu_dbg!("has_active_animations=TRUE: spinner in use");
            return true;
        }

        // Check async widgets
        let widgets_busy = self.widgets.iter().any(|w| *w.is_running.lock());
        if widgets_busy {
            cpu_dbg!("has_active_animations=TRUE: widget(s) busy");
        }
        widgets_busy
    }

    /// Fastest poll interval required by prompt-owned dynamic sources.
    /// Returns None when the prompt itself needs no timed wakeups.
    fn prompt_poll_interval(&self) -> Option<Duration> {
        // Async widgets completing in background need a fast poll to pick up
        // output changes via `update()`.
        if self.widgets.iter().any(|w| *w.is_running.lock()) {
            return Some(Duration::from_millis(50));
        }
        // Spinner advances every `frame_rate_ms` but only when referenced.
        if self.spinner_in_use() {
            let fastest = self
                .animations
                .iter()
                .map(|a| a.frame_rate_ms.max(1))
                .min()
                .unwrap_or(80);
            return Some(Duration::from_millis(fastest));
        }
        None
    }

    /// Deadline-based poll timeout. Wakes exactly when the next dynamic
    /// source needs it, otherwise falls back to a slow idle poll so external
    /// git changes are eventually picked up (until the fs watcher lands).
    /// `status_visible` enables second-boundary wakeups for the clock;
    /// `external_busy` covers agent/completion loaders owned by the loop.
    pub fn next_poll_timeout(&self, status_visible: bool, external_busy: bool) -> Duration {
        if external_busy {
            return Duration::from_millis(50);
        }
        if let Some(interval) = self.prompt_poll_interval() {
            return interval;
        }
        if status_visible {
            // Wake on the next second boundary so the clock ticks exactly
            // once per second instead of drifting on a fixed 1000ms poll.
            let now_ms = chrono::Local::now().timestamp_millis();
            let until_next = 1000 - (now_ms.rem_euclid(1000));
            return Duration::from_millis(until_next.clamp(50, 1000) as u64);
        }
        // Idle: no animation, no clock, no busy widgets. Slow poll as safety
        // net for external changes; `update()` diffing keeps it silent.
        Duration::from_millis(1000)
    }

    /// Single prompt pipeline: every prompt surface (live `Line`, committed
    /// ANSI transcript, right side, transient final) resolves through an ANSI
    /// string first and converts once via `ansi_to_spans`. Previously the live
    /// path used direct segment→ratatui rendering while transcripts used ANSI
    /// strings, so the same prompt could diverge between screen and scrollback.
    /// Value formats are intentionally frozen per source (template
    /// interpolation keeps its backward-compatible `{duration_human}` etc.;
    /// segments keep their typed formatters); what is unified here is the
    /// resolution pipeline, not the value dialects. A template→segment
    /// compiler would be needed for full semantic unification.
    fn left_template(&self) -> Option<String> {
        self.env
            .vars
            .read()
            .get("FSH_PROMPT")
            .and_then(|v| match v {
                fshell_core::Val::String(s) => Some(s.clone()),
                _ => None,
            })
    }

    fn right_template(&self) -> Option<String> {
        self.env
            .vars
            .read()
            .get("FSH_PROMPT_RIGHT")
            .and_then(|v| match v {
                fshell_core::Val::String(s) => Some(s.clone()),
                _ => None,
            })
    }

    fn segment_list_ansi(&self, left: bool) -> String {
        let config = self.env.prompt_config.read();
        let theme = self.env.active_theme();
        let list = if left { &config.left } else { &config.right };
        crate::prompt::render_segment_list(
            list,
            &config,
            &self.cached_snapshot.pwd,
            &self.cached_snapshot.git_status,
            self.cached_snapshot.exit_code,
            self.cached_snapshot.duration,
            self.cached_snapshot.job_count,
            left,
            &theme,
        )
    }

    /// Resolve the left prompt to an ANSI string without a trailing reset.
    fn resolve_left_text(&self) -> String {
        if let Some(tpl) = self.left_template() {
            interpolate_template(&tpl, &self.cached_snapshot, &self.animations, &self.widgets)
        } else {
            self.segment_list_ansi(true)
        }
    }

    /// Resolve the right prompt to an ANSI string without a trailing reset.
    fn resolve_right_text(&self) -> String {
        if let Some(tpl) = self.right_template() {
            interpolate_template(&tpl, &self.cached_snapshot, &self.animations, &self.widgets)
        } else {
            self.segment_list_ansi(false)
        }
    }

    /// Live prompt contract: the inline viewport reserves exactly one row for
    /// each prompt side, so the live `Line` is the first visual line. The full
    /// (possibly multiline) text is preserved in committed transcripts.
    fn first_line(text: &str) -> &str {
        text.split('\n').next().unwrap_or(text)
    }

    pub fn render_prompt_left(&self, continuation: bool) -> Line<'static> {
        if continuation {
            return Line::from(vec![Span::styled(
                "> ",
                Style::default().fg(Color::DarkGray),
            )]);
        }

        Line::from(ansi_to_spans(Self::first_line(&self.resolve_left_text())))
    }

    pub fn render_prompt_left_ansi(&self) -> String {
        let mut resolved = self.resolve_left_text();
        // Template path historically terminates with an explicit reset.
        if self.left_template().is_some() {
            resolved.push_str("\x1b[0m");
        }
        resolved
    }

    pub fn render_prompt_right(&self) -> Line<'static> {
        Line::from(ansi_to_spans(Self::first_line(&self.resolve_right_text())))
    }

    // Erase multi-line components and render transient prompts on submit
    pub fn render_prompt_final(&self) -> Line<'static> {
        let rendered = self.render_prompt_final_ansi();
        Line::from(ansi_to_spans(&rendered))
    }

    pub fn render_prompt_final_ansi(&self) -> String {
        let vars = self.env.vars.read();
        let final_tpl = vars.get("PS1_FINAL").and_then(|v| match v {
            fshell_core::Val::String(s) => Some(s),
            _ => None,
        });

        let result = if let Some(tpl) = final_tpl {
            interpolate_template(tpl, &self.cached_snapshot, &self.animations, &self.widgets)
        } else {
            // Default: use the full PS1 prompt
            self.render_prompt_left_ansi()
        };

        // Always ensure ANSI reset at the end (Bug 4.1)
        if !result.ends_with("\x1b[0m") {
            format!("{}\x1b[0m", result)
        } else {
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ansi_to_spans_simple() {
        let spans = ansi_to_spans("hello");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "hello");
    }

    #[test]
    fn test_ansi_to_spans_colored() {
        let spans = ansi_to_spans("\x1b[31mred\x1b[0mnormal");
        // Span 0: "red" in red
        // Span 1: "normal" in default (reset from \x1b[0m)
        assert_eq!(spans.len(), 2, "got {spans:#?}");
        assert_eq!(spans[0].content, "red");
        assert_eq!(spans[1].content, "normal");
    }

    #[test]
    fn test_ansi_to_spans_reset_at_end() {
        let spans = ansi_to_spans("\x1b[31mno reset");
        // Should have: "no reset" in red, then empty reset span
        assert!(spans.len() >= 2, "got {} spans: {:#?}", spans.len(), spans);
        assert_eq!(spans[1].content, "");
        assert_eq!(spans[1].style, Style::default());
    }

    #[test]
    fn test_ansi_to_spans_skips_non_sgr_csi() {
        // Cursor movement sequences should be silently stripped.
        // The text is split into spans at CSI boundaries but all have default style.
        let spans = ansi_to_spans("before\x1b[2Kafter");
        assert_eq!(spans.len(), 2, "got {} spans: {:#?}", spans.len(), spans);
        assert_eq!(spans[0].content, "before");
        assert_eq!(spans[1].content, "after");
    }

    #[test]
    fn test_ansi_to_spans_consecutive_sgr() {
        let spans = ansi_to_spans("\x1b[31m\x1b[32mtext");
        // "text" in green + empty reset span
        assert_eq!(spans.len(), 2, "got {} spans: {:#?}", spans.len(), spans);
        assert_eq!(spans[0].content, "text");
    }

    #[test]
    fn test_ansi_to_spans_256_color() {
        let spans = ansi_to_spans("\x1b[38;5;166morange\x1b[0m");
        // "orange" in color 166 + empty trailing reset (from \x1b[0m restoring default)
        // After \x1b[0m, style is default, so no extra reset is appended.
        assert_eq!(spans.len(), 1, "got {} spans: {:#?}", spans.len(), spans);
        assert_eq!(spans[0].content, "orange");
    }

    #[test]
    fn test_ansi_to_spans_24bit_color() {
        let spans = ansi_to_spans("\x1b[38;2;255;128;0mcoral\x1b[0m");
        assert_eq!(spans.len(), 1, "got {} spans: {:#?}", spans.len(), spans);
        assert_eq!(spans[0].content, "coral");
    }

    #[test]
    fn test_prompt_animation_uses_stable_clock() {
        let animation = PromptAnimation {
            name: "test".to_string(),
            frames: vec!["a".to_string(), "b".to_string()],
            frame_rate_ms: 1,
            started_at: Instant::now(),
        };

        assert_eq!(animation.frame_at(Duration::ZERO), "a");
        assert_eq!(animation.frame_at(Duration::from_millis(1)), "b");

        let zero_rate = PromptAnimation {
            frame_rate_ms: 0,
            ..animation
        };
        assert_eq!(zero_rate.frame_at(Duration::ZERO), "a");
    }

    #[test]
    fn test_maybe_emit_osc7_only_on_change() {
        let env = fshell_engine::Env::new();
        let mut mgr = super::PromptManager::new(env);
        assert!(mgr.maybe_emit_osc7("/tmp/a"));
        assert!(!mgr.maybe_emit_osc7("/tmp/a"));
        assert!(mgr.maybe_emit_osc7("/tmp/b"));
    }

    #[test]
    fn test_next_poll_timeout_deadlines() {
        let env = fshell_engine::Env::new();
        let mgr = super::PromptManager::new(env);
        // Idle, no clock, no busy: slow poll.
        assert_eq!(
            mgr.next_poll_timeout(false, false),
            Duration::from_millis(1000)
        );
        // External busy (agent/completions): fast poll.
        assert_eq!(
            mgr.next_poll_timeout(false, true),
            Duration::from_millis(50)
        );
        // Clock visible: wake on second boundary (50..=1000ms).
        let clock = mgr.next_poll_timeout(true, false);
        assert!(clock >= Duration::from_millis(50) && clock <= Duration::from_millis(1000));
    }

    /// Strip SGR escape sequences for visible-text comparison.
    fn strip_sgr(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' && chars.peek() == Some(&'[') {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn test_live_line_and_ansi_transcript_agree() {
        // Pipeline unity: the live `Line` and the committed ANSI transcript
        // must carry the same visible text in both template and segment modes.
        let env = fshell_engine::Env::new();
        {
            let mut vars = env.vars.write();
            vars.insert(
                "FSH_PROMPT".to_string(),
                fshell_core::Val::String("{user}@{pwd}> ".to_string()),
            );
        }
        let mgr = super::PromptManager::new(env.clone());
        let line_text: String = mgr
            .render_prompt_left(false)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        let ansi_text = strip_sgr(&mgr.render_prompt_left_ansi());
        assert_eq!(line_text, ansi_text);

        {
            env.vars.write().remove("FSH_PROMPT");
        }
        let mgr = super::PromptManager::new(env);
        let line_text: String = mgr
            .render_prompt_left(false)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        let ansi_text = strip_sgr(&mgr.render_prompt_left_ansi());
        assert_eq!(line_text, ansi_text);
    }
}
