// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::cpu_dbg;
use crate::{PromptSnapshot, refresh_prompt_snapshot, render_prompt_template};
use fshell_engine::Env;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use std::sync::{Arc, Mutex};
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
}

impl PromptWidget {
    pub fn new(name: &str, command: &str) -> Self {
        Self {
            name: name.to_string(),
            command: command.to_string(),
            cached_output: Arc::new(Mutex::new(String::new())),
            is_running: Arc::new(Mutex::new(false)),
            last_run: Arc::new(Mutex::new(None)),
        }
    }

    pub fn trigger_update(&self, env: &Env) {
        let is_running = self.is_running.clone();
        {
            let mut running = is_running.lock().unwrap_or_else(|e| e.into_inner());
            if *running {
                return;
            }
            *running = true;
        }

        let cmd = self.command.clone();
        let cached_output = self.cached_output.clone();
        let last_run = self.last_run.clone();
        let env_clone = env.clone();

        tokio::spawn(async move {
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

            if let Ok(mut out) = cached_output.lock() {
                *out = output;
            }
            if let Ok(mut lr) = last_run.lock() {
                *lr = Some(Instant::now());
            }
            if let Ok(mut running) = is_running.lock() {
                *running = false;
            }
        });
    }
}

// Spinners / Animations in prompt
pub struct PromptAnimation {
    pub name: String,
    pub frames: Vec<String>,
    pub frame_rate_ms: u64,
}

impl PromptAnimation {
    pub fn get_frame(&self) -> String {
        if self.frames.is_empty() {
            return String::new();
        }
        let total_ms = Instant::now().elapsed().as_millis() as u64;
        let index = ((total_ms / self.frame_rate_ms) % self.frames.len() as u64) as usize;
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
            let out = widget
                .cached_output
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
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
        }];

        Self {
            env,
            widgets: Vec::new(),
            animations,
            last_snapshot_update: Instant::now(),
            cached_snapshot: snapshot,
            last_known_widget_outputs: Vec::new(),
        }
    }

    pub fn refresh_snapshot(&mut self, cwd: &str) {
        self.cached_snapshot = refresh_prompt_snapshot(&self.env, cwd);
        self.last_snapshot_update = Instant::now();
    }

    /// Returns true if any prompt state changed (warranting a redraw).
    pub fn update(&mut self) -> bool {
        let mut changed = false;
        let current_pwd = self.env.cwd().to_string_lossy().to_string();

        // Update database/environment status snapshot every 1 second
        if self.last_snapshot_update.elapsed() > Duration::from_millis(1000) {
            cpu_dbg!("PromptManager::update snapshot refresh (every 1s)");
            self.cached_snapshot = refresh_prompt_snapshot(&self.env, &current_pwd);
            self.last_snapshot_update = Instant::now();
            changed = true;

            // Trigger updates for widgets that have expired (e.g. run every 5s or if empty)
            for widget in &self.widgets {
                let should_run = {
                    let lr = widget.last_run.lock().unwrap_or_else(|e| e.into_inner());
                    match *lr {
                        None => true,
                        Some(last_run) => last_run.elapsed() > Duration::from_secs(5),
                    }
                };
                if should_run {
                    widget.trigger_update(&self.env);
                }
            }
        }

        // Initialize or update widget output caches to detect background execution completion
        if self.last_known_widget_outputs.len() != self.widgets.len() {
            self.last_known_widget_outputs = self
                .widgets
                .iter()
                .map(|w| {
                    w.cached_output
                        .lock()
                        .map(|g| g.clone())
                        .unwrap_or_default()
                })
                .collect();
            changed = true;
        } else {
            for (i, w) in self.widgets.iter().enumerate() {
                let current = w
                    .cached_output
                    .lock()
                    .map(|g| g.clone())
                    .unwrap_or_default();
                if current != self.last_known_widget_outputs[i] {
                    self.last_known_widget_outputs[i] = current;
                    changed = true;
                }
            }
        }

        cpu_dbg!("PromptManager::update returning changed={}", changed);
        changed
    }

    /// Returns true if any animation or widget is active (for poll timeout calculation).
    pub fn has_active_animations(&self) -> bool {
        // Check animations — spinner is always "active" in the sense that it changes over time.
        // Instead of checking if any animation is defined, check if any animation's frame
        // depends on time (which is always true). More practically: check if any animation
        // references exist in the prompt configuration.
        if !self.animations.is_empty() {
            // The speed at which the animation frame changes is exactly frame_rate_ms.
            // Since animation frames change every 80ms, we need fast polls when animations
            // are referenced in the prompt template.
            let vars = self.env.vars.read();
            if let Some(fshell_core::Val::String(tpl)) = vars.get("FSH_PROMPT")
                && (tpl.contains("{SPINNER}") || tpl.contains("{spinner}"))
            {
                cpu_dbg!("has_active_animations=TRUE: FSH_PROMPT contains spinner");
                return true;
            }
            // Also check right prompt
            if let Some(fshell_core::Val::String(tpl)) = vars.get("FSH_PROMPT_RIGHT")
                && (tpl.contains("{SPINNER}") || tpl.contains("{spinner}"))
            {
                cpu_dbg!("has_active_animations=TRUE: FSH_PROMPT_RIGHT contains spinner");
                return true;
            }
        }

        // Check async widgets
        let widgets_busy = self
            .widgets
            .iter()
            .any(|w| *w.is_running.lock().unwrap_or_else(|e| e.into_inner()));
        if widgets_busy {
            cpu_dbg!("has_active_animations=TRUE: widget(s) busy");
        }
        widgets_busy
    }

    pub fn render_prompt_left(&self, continuation: bool) -> Line<'static> {
        if continuation {
            return Line::from(vec![Span::styled(
                "> ",
                Style::default().fg(Color::DarkGray),
            )]);
        }

        // Check if user has a custom string template override
        let vars = self.env.vars.read();
        let prompt_tpl = vars.get("FSH_PROMPT").and_then(|v| match v {
            fshell_core::Val::String(s) => Some(s),
            _ => None,
        });

        if let Some(tpl) = prompt_tpl {
            let resolved =
                interpolate_template(tpl, &self.cached_snapshot, &self.animations, &self.widgets);
            Line::from(ansi_to_spans(&resolved))
        } else {
            // High-performance direct AST/segment -> Ratatui Span path (zero ANSI string roundtrip)
            let config = self.env.prompt_config.read();
            let theme = self.env.active_theme();
            let lines = crate::prompt::render_segment_list_to_ratatui_lines(
                &config.left,
                &config,
                &self.cached_snapshot.pwd,
                &self.cached_snapshot.git_status,
                self.cached_snapshot.exit_code,
                self.cached_snapshot.duration,
                self.cached_snapshot.job_count,
                true,
                &theme,
            );
            lines.into_iter().next().unwrap_or_default()
        }
    }

    pub fn render_prompt_left_ansi(&self) -> String {
        let config = self.env.prompt_config.read();
        let theme = self.env.active_theme();
        let rendered_ansi = crate::prompt::render_segment_list(
            &config.left,
            &config,
            &self.cached_snapshot.pwd,
            &self.cached_snapshot.git_status,
            self.cached_snapshot.exit_code,
            self.cached_snapshot.duration,
            self.cached_snapshot.job_count,
            true,
            &theme,
        );

        let vars = self.env.vars.read();
        let prompt_tpl = vars.get("FSH_PROMPT").and_then(|v| match v {
            fshell_core::Val::String(s) => Some(s),
            _ => None,
        });

        if let Some(tpl) = prompt_tpl {
            let mut resolved =
                interpolate_template(tpl, &self.cached_snapshot, &self.animations, &self.widgets);
            resolved.push_str("\x1b[0m");
            resolved
        } else {
            rendered_ansi
        }
    }

    pub fn render_prompt_right(&self) -> Line<'static> {
        let vars = self.env.vars.read();
        let right_tpl = vars.get("FSH_PROMPT_RIGHT").and_then(|v| match v {
            fshell_core::Val::String(s) => Some(s),
            _ => None,
        });

        if let Some(tpl) = right_tpl {
            let resolved =
                interpolate_template(tpl, &self.cached_snapshot, &self.animations, &self.widgets);
            Line::from(ansi_to_spans(&resolved))
        } else {
            let config = self.env.prompt_config.read();
            let theme = self.env.active_theme();
            let lines = crate::prompt::render_segment_list_to_ratatui_lines(
                &config.right,
                &config,
                &self.cached_snapshot.pwd,
                &self.cached_snapshot.git_status,
                self.cached_snapshot.exit_code,
                self.cached_snapshot.duration,
                self.cached_snapshot.job_count,
                false,
                &theme,
            );
            lines.into_iter().next().unwrap_or_default()
        }
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
}
