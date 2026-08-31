// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Schema and metadata definitions for fshell configurable options.

use fshell_core::Val;
use fshell_engine::{Env, ShellOptions, SuggestionMode};
use fshell_render::RenderFormat;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionKind {
    Bool(bool),
    Choice {
        current: String,
        choices: &'static [&'static str],
    },
    Integer {
        current: usize,
        min: usize,
        max: usize,
        unit: &'static str,
        examples: &'static [&'static str],
        higher_meaning: &'static str,
        lower_meaning: &'static str,
    },
    Theme {
        current: String,
    },
    Keybinding {
        current: String,
    },
}

#[derive(Debug, Clone)]
pub struct OptionItem {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub section: &'static str,
    pub kind: OptionKind,
}

impl OptionItem {
    pub fn load_all(env: &Env) -> Vec<OptionItem> {
        let opts = env.options.read();
        let vars = env.vars.read();

        let error_format_str = match opts.error_format {
            RenderFormat::Auto => "auto",
            RenderFormat::Graphical => "graphical",
            RenderFormat::Compact => "compact",
            RenderFormat::Explain => "explain",
            RenderFormat::Json => "json",
        };

        let suggestion_mode_str = match opts.suggestion_mode {
            SuggestionMode::Blocking => "blocking",
            SuggestionMode::Deferred => "deferred",
        };

        let keybinding_str = match vars.get("FSH_KEYBINDING_MODE") {
            Some(Val::String(s)) => s.as_str(),
            _ => "emacs",
        };

        vec![
            // --- Execution & Pipelines ---
            OptionItem {
                key: "autocd",
                label: "Auto CD",
                description: "Typing a directory path automatically changes directory into it",
                section: "Execution & Navigation",
                kind: OptionKind::Bool(opts.autocd),
            },
            OptionItem {
                key: "autopushd",
                label: "Auto Pushd",
                description: "Automatically push directory onto stack on cd",
                section: "Execution & Navigation",
                kind: OptionKind::Bool(opts.autopushd),
            },
            OptionItem {
                key: "cdable_vars",
                label: "CD-able Variables",
                description: "Treat variable names containing valid paths as directories for cd",
                section: "Execution & Navigation",
                kind: OptionKind::Bool(opts.cdable_vars),
            },
            OptionItem {
                key: "pipefail",
                label: "Pipefail",
                description: "Pipeline return code is that of the last failing stage",
                section: "Execution & Navigation",
                kind: OptionKind::Bool(opts.pipefail),
            },
            OptionItem {
                key: "errexit",
                label: "Exit on Error (errexit)",
                description: "Exit script or pipeline immediately when a command exits non-zero",
                section: "Execution & Navigation",
                kind: OptionKind::Bool(opts.errexit),
            },
            OptionItem {
                key: "nounset",
                label: "Error on Unset (nounset)",
                description: "Treat references to unset variables as an evaluation error",
                section: "Execution & Navigation",
                kind: OptionKind::Bool(opts.nounset),
            },
            OptionItem {
                key: "noexec",
                label: "No Execution (noexec)",
                description: "Parse and validate commands without executing them",
                section: "Execution & Navigation",
                kind: OptionKind::Bool(opts.noexec),
            },
            OptionItem {
                key: "xtrace",
                label: "Execution Trace (xtrace)",
                description: "Print each command and arguments before executing",
                section: "Execution & Navigation",
                kind: OptionKind::Bool(opts.xtrace),
            },
            OptionItem {
                key: "verbose",
                label: "Verbose",
                description: "Print shell input lines as they are read",
                section: "Execution & Navigation",
                kind: OptionKind::Bool(opts.verbose),
            },
            OptionItem {
                key: "json_auto_parse",
                label: "JSON Auto Parse",
                description: "Automatically parse external process stdout as structured JSON when possible",
                section: "Execution & Navigation",
                kind: OptionKind::Bool(opts.json_auto_parse),
            },
            OptionItem {
                key: "clear_on_reload",
                label: "Clear on Reload",
                description: "Screen clear behavior when reloading config/session",
                section: "Execution & Navigation",
                kind: OptionKind::Choice {
                    current: opts.clear_on_reload.clone(),
                    choices: &["ask", "always", "never"],
                },
            },
            OptionItem {
                key: "session_restore",
                label: "Session Restore",
                description: "Behavior when restoring previous interactive shell sessions",
                section: "Execution & Navigation",
                kind: OptionKind::Choice {
                    current: opts.session_restore.clone(),
                    choices: &["none", "auto", "picker", "ask"],
                },
            },
            // --- Security & Sandboxing ---
            OptionItem {
                key: "confirm_destructive",
                label: "Confirm Destructive Commands",
                description: "Prompt for confirmation before running dangerous commands (e.g. rm -rf /)",
                section: "Security & Sandboxing",
                kind: OptionKind::Bool(opts.confirm_destructive),
            },
            OptionItem {
                key: "sandbox_all",
                label: "Sandbox All Commands",
                description: "Enforce sandbox policies on all external process executions",
                section: "Security & Sandboxing",
                kind: OptionKind::Bool(opts.sandbox_all),
            },
            OptionItem {
                key: "sandbox_mode",
                label: "Sandbox Mode",
                description: "OS-level subprocess security mode (Landlock on Linux, SBPL on macOS)",
                section: "Security & Sandboxing",
                kind: OptionKind::Choice {
                    current: opts.sandbox_mode.clone(),
                    choices: &["off", "monitor", "prompt", "deny-all"],
                },
            },
            // --- Globbing & File Matching ---
            OptionItem {
                key: "nullglob",
                label: "Null Glob",
                description: "Patterns that match no files expand to nothing instead of literal pattern",
                section: "Globbing & File Matching",
                kind: OptionKind::Bool(opts.nullglob),
            },
            OptionItem {
                key: "nocaseglob",
                label: "Case-Insensitive Glob",
                description: "Perform case-insensitive filename matching during glob expansion",
                section: "Globbing & File Matching",
                kind: OptionKind::Bool(opts.nocaseglob),
            },
            OptionItem {
                key: "noclobber",
                label: "No Clobber",
                description: "Prevent shell output redirection from overwriting existing files",
                section: "Globbing & File Matching",
                kind: OptionKind::Bool(opts.noclobber),
            },
            // --- REPL & Interactive Experience ---
            OptionItem {
                key: "did_you_mean",
                label: "Did You Mean (DYM)",
                description: "Suggest corrections for mistyped command names",
                section: "REPL & Editing",
                kind: OptionKind::Bool(opts.did_you_mean),
            },
            OptionItem {
                key: "suggestion_mode",
                label: "Suggestion Mode",
                description: "Interactive suggestion timing in REPL (blocking vs background deferred)",
                section: "REPL & Editing",
                kind: OptionKind::Choice {
                    current: suggestion_mode_str.to_string(),
                    choices: &["deferred", "blocking"],
                },
            },
            OptionItem {
                key: "keybinding",
                label: "Keybinding Mode",
                description: "Command-line editing mode (Emacs vs Vi navigation)",
                section: "REPL & Editing",
                kind: OptionKind::Keybinding {
                    current: keybinding_str.to_string(),
                },
            },
            OptionItem {
                key: "ignoreeof",
                label: "Ignore EOF (Ctrl-D)",
                description: "Prevent accidental shell exit on EOF / Ctrl-D",
                section: "REPL & Editing",
                kind: OptionKind::Bool(opts.ignoreeof),
            },
            OptionItem {
                key: "histignoredups",
                label: "Ignore Duplicate History",
                description: "Do not save consecutive duplicate commands in history",
                section: "REPL & Editing",
                kind: OptionKind::Bool(opts.histignoredups),
            },
            OptionItem {
                key: "quiet_aliases",
                label: "Quiet Aliases",
                description: "Suppress warnings when an alias shadows a builtin or executable",
                section: "REPL & Editing",
                kind: OptionKind::Bool(opts.quiet_aliases),
            },
            // --- Display, Styling & Output ---
            OptionItem {
                key: "theme",
                label: "Color Theme",
                description: "Active visual theme for prompt, syntax highlighting, and chrome",
                section: "Display & Theme",
                kind: OptionKind::Theme {
                    current: opts.theme.clone(),
                },
            },
            OptionItem {
                key: "error_color",
                label: "Error Colors",
                description: "Enable rich ANSI colors in diagnostic and error reports",
                section: "Display & Theme",
                kind: OptionKind::Bool(opts.error_color),
            },
            OptionItem {
                key: "error_format",
                label: "Error Format",
                description: "Default diagnostic renderer formatting style",
                section: "Display & Theme",
                kind: OptionKind::Choice {
                    current: error_format_str.to_string(),
                    choices: &["auto", "graphical", "compact", "explain", "json"],
                },
            },
            OptionItem {
                key: "notify",
                label: "Job Notifications",
                description: "Send notifications when long-running background tasks complete",
                section: "Display & Theme",
                kind: OptionKind::Bool(opts.notify),
            },
            // --- Channels & Performance Buffers ---
            OptionItem {
                key: "pipeline_channel_size",
                label: "Pipeline Buffer Size",
                description: "Capacity of bounded async pipeline queues between stages",
                section: "Performance & Limits",
                kind: OptionKind::Integer {
                    current: opts.pipeline_channel_size,
                    min: 1,
                    max: 100_000,
                    unit: "elements / queue",
                    examples: &[
                        "10 (low RAM)",
                        "100 (default, balanced)",
                        "1000 (high throughput streaming)",
                    ],
                    higher_meaning: "Buffers more stream elements in memory between pipeline stages, improving throughput for bursts but increasing peak RSS.",
                    lower_meaning: "Applies immediate backpressure to upstream stages, minimizing memory usage at the cost of potential stage context switches.",
                },
            },
            OptionItem {
                key: "notify_threshold",
                label: "Notification Duration",
                description: "Threshold in seconds before background job completion triggers notification",
                section: "Performance & Limits",
                kind: OptionKind::Integer {
                    current: opts.notify_threshold as usize,
                    min: 1,
                    max: 3600,
                    unit: "seconds",
                    examples: &[
                        "5 (quick alerts for builds)",
                        "10 (default)",
                        "30 (long-running batch jobs only)",
                    ],
                    higher_meaning: "Only sends system notifications for lengthy background tasks, reducing notification noise for short or medium-length commands.",
                    lower_meaning: "Notifies sooner when jobs complete; setting below 3s may generate frequent notification popups during normal shell usage.",
                },
            },
            OptionItem {
                key: "stderr_max_bytes",
                label: "Max Stderr Buffer",
                description: "Maximum captured stderr size before truncate in diagnostics",
                section: "Performance & Limits",
                kind: OptionKind::Integer {
                    current: opts.stderr_max_bytes,
                    min: 1024,
                    max: 104_857_600,
                    unit: "bytes",
                    examples: &[
                        "65536 (64 KB, minimal)",
                        "1048576 (1 MB, default)",
                        "10485760 (10 MB, verbose debug logs)",
                    ],
                    higher_meaning: "Preserves extensive build / compiler output in diagnostic error spans without truncation, using more memory.",
                    lower_meaning: "Caps stderr buffer aggressively to prevent memory spikes if an external program floods error streams.",
                },
            },
            OptionItem {
                key: "sort_max_items",
                label: "Max Sort Buffer Items",
                description: "Maximum item count for in-memory pipeline sort stages",
                section: "Performance & Limits",
                kind: OptionKind::Integer {
                    current: opts.sort_max_items,
                    min: 100,
                    max: 10_000_000,
                    unit: "items",
                    examples: &[
                        "10000 (lightweight)",
                        "100000 (default)",
                        "1000000 (large dataset pipelines)",
                    ],
                    higher_meaning: "Permits sorting very large structured tables or object streams in memory without truncation.",
                    lower_meaning: "Protects the shell against out-of-memory crashes when piping unbounded infinite streams into sort without limit.",
                },
            },
        ]
    }

    pub fn cycle(&mut self, env: &Env) -> Result<String, String> {
        match &mut self.kind {
            OptionKind::Bool(b) => {
                *b = !*b;
                let mut opts = env.options.write();
                opts.set_bool(self.key, *b)?;
                drop(opts);
                env.sync_options_map()
                    .map_err(|e| format!("Failed to sync options: {e}"))?;
                Ok(format!(
                    "{} = {}",
                    self.label,
                    if *b { "ON" } else { "OFF" }
                ))
            }
            OptionKind::Choice { current, choices } => {
                let idx = choices.iter().position(|&c| c == current).unwrap_or(0);
                let next_idx = (idx + 1) % choices.len();
                let next_val = choices[next_idx].to_string();
                *current = next_val.clone();
                self.apply_value(env, &next_val)?;
                Ok(format!("{} = {}", self.label, next_val))
            }
            OptionKind::Keybinding { current } => {
                let next = if current == "emacs" { "vi" } else { "emacs" };
                *current = next.to_string();
                let mut vars = env.vars.write();
                vars.insert("FSH_KEYBINDING_MODE".into(), Val::String(next.to_string()));
                drop(vars);
                Ok(format!("Keybinding = {}", next))
            }
            OptionKind::Theme { current } => {
                let config_dir =
                    fshell_engine::config_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
                let available = fshell_core::theme::Theme::available(&config_dir);
                let idx = available.iter().position(|c| c == current).unwrap_or(0);
                let next_idx = if available.is_empty() {
                    0
                } else {
                    (idx + 1) % available.len()
                };
                let next_theme = available
                    .get(next_idx)
                    .cloned()
                    .unwrap_or_else(|| "default".into());
                *current = next_theme.clone();
                self.apply_value(env, &next_theme)?;
                Ok(format!("Theme = {}", next_theme))
            }
            OptionKind::Integer { .. } => Err("Press Enter or 'e' to edit numeric value".into()),
        }
    }

    pub fn apply_value(&self, env: &Env, val_str: &str) -> Result<(), String> {
        let bare_key = self.key;
        if ShellOptions::bool_keys().contains(&bare_key) {
            let b = match val_str {
                "true" | "on" | "yes" | "1" => true,
                "false" | "off" | "no" | "0" => false,
                _ => return Err(format!("Expected boolean for {bare_key}")),
            };
            let mut opts = env.options.write();
            opts.set_bool(bare_key, b)?;
        } else {
            match bare_key {
                "sandbox_mode" => {
                    if !["prompt", "deny-all", "monitor", "off"].contains(&val_str) {
                        return Err("Must be 'prompt', 'deny-all', 'monitor', or 'off'".into());
                    }
                    let mut opts = env.options.write();
                    opts.sandbox_mode = val_str.to_string();
                }
                "clear_on_reload" => {
                    if !["ask", "always", "never"].contains(&val_str) {
                        return Err("Must be 'ask', 'always', or 'never'".into());
                    }
                    let mut opts = env.options.write();
                    opts.clear_on_reload = val_str.to_string();
                }
                "session_restore" => {
                    if !["none", "auto", "picker", "ask"].contains(&val_str) {
                        return Err("Must be 'none', 'auto', 'picker', or 'ask'".into());
                    }
                    let mut opts = env.options.write();
                    opts.session_restore = val_str.to_string();
                }
                "error_format" => {
                    let fmt = match val_str {
                        "compact" => RenderFormat::Compact,
                        "json" => RenderFormat::Json,
                        "explain" => RenderFormat::Explain,
                        "graphical" => RenderFormat::Graphical,
                        _ => RenderFormat::Auto,
                    };
                    let mut opts = env.options.write();
                    opts.error_format = fmt;
                }
                "suggestion_mode" => {
                    let mode = match val_str {
                        "blocking" => SuggestionMode::Blocking,
                        _ => SuggestionMode::Deferred,
                    };
                    let mut opts = env.options.write();
                    opts.suggestion_mode = mode;
                }
                "theme" => {
                    let config_dir = fshell_engine::config_dir()
                        .unwrap_or_else(|| std::path::PathBuf::from("."));
                    let theme = fshell_core::theme::Theme::load(val_str, &config_dir)
                        .map_err(|e| format!("Failed to load theme {val_str}: {e}"))?;
                    env.set_theme(std::sync::Arc::new(theme));
                    let mut opts = env.options.write();
                    opts.theme = val_str.to_string();
                }
                "pipeline_channel_size" => {
                    let v: usize = val_str.parse().map_err(|_| "Must be a positive integer")?;
                    if v == 0 {
                        return Err("Channel size must be >= 1".into());
                    }
                    let mut opts = env.options.write();
                    opts.pipeline_channel_size = v;
                }
                "notify_threshold" => {
                    let v: u64 = val_str.parse().map_err(|_| "Must be a positive integer")?;
                    let mut opts = env.options.write();
                    opts.notify_threshold = v;
                }
                "stderr_max_bytes" => {
                    let v: usize = val_str.parse().map_err(|_| "Must be a positive integer")?;
                    let mut opts = env.options.write();
                    opts.stderr_max_bytes = v;
                }
                "sort_max_items" => {
                    let v: usize = val_str.parse().map_err(|_| "Must be a positive integer")?;
                    let mut opts = env.options.write();
                    opts.sort_max_items = v;
                }
                "keybinding" => {
                    if !["emacs", "vi"].contains(&val_str) {
                        return Err("Keybinding must be 'emacs' or 'vi'".into());
                    }
                    let mut vars = env.vars.write();
                    vars.insert(
                        "FSH_KEYBINDING_MODE".into(),
                        Val::String(val_str.to_string()),
                    );
                }
                _ => return Err(format!("Unknown option key '{bare_key}'")),
            }
        }

        env.sync_options_map()
            .map_err(|e| format!("Failed to sync options map: {e}"))?;

        Ok(())
    }
}
