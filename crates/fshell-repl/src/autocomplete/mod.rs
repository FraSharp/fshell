// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Native completion subsystem for fshell.

pub mod carapace;
pub mod custom;
pub mod files;
pub mod help;
pub mod types;

pub use self::carapace::*;
pub use self::custom::*;
pub use self::files::*;
pub use self::help::*;
pub use self::types::*;

use crate::fuzzy;
use crate::history;
use fshell_engine::Env;
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct FrecencyCache {
    data: Vec<(String, f64)>,
    loaded_at: Instant,
}

static Z_FRECENCY_CACHE: Mutex<Option<FrecencyCache>> = Mutex::new(None);
const Z_FRECENCY_TTL: Duration = Duration::from_secs(300);

pub struct FshellCompleter {
    pub env: Env,
}

/// Common external commands that should appear in first-word completions.
pub const COMMON_EXTERNAL_COMMANDS: &[(&str, &str)] = &[
    ("cargo", "Rust package manager"),
    ("git", "Distributed version control system"),
    ("npm", "Node.js package manager"),
    ("node", "Node.js runtime"),
    ("python", "Python interpreter"),
    ("python3", "Python interpreter"),
    ("docker", "Container runtime"),
    ("kubectl", "Kubernetes CLI"),
    ("make", "Build automation tool"),
    ("vim", "Text editor"),
    ("nvim", "Text editor"),
    ("code", "VS Code editor"),
    ("ssh", "Secure shell remote login"),
    ("curl", "Transfer data from URLs"),
    ("wget", "Download files from URLs"),
    ("tar", "Archive utility"),
    ("rg", "Recursive search with regex (ripgrep)"),
    ("fd", "Fast file finder"),
    ("jq", "JSON processor"),
    ("sed", "Stream editor"),
    ("awk", "Pattern scanning and processing language"),
    ("chmod", "Change file permissions"),
    ("chown", "Change file owner"),
    ("find", "Search for files in a directory hierarchy"),
    ("xargs", "Build and execute commands from stdin"),
    ("ps", "Report process status"),
    ("top", "Display processes"),
    ("htop", "Interactive process viewer"),
    ("kill", "Send a signal to a process"),
    ("man", "Display manual pages"),
    ("less", "Pager for viewing text"),
    ("more", "Pager for viewing text (basic)"),
    ("diff", "Compare files line by line"),
    ("patch", "Apply a diff file to originals"),
    ("gh", "GitHub CLI"),
    ("brew", "macOS package manager"),
    ("apt", "Debian package manager"),
    ("yum", "RHEL package manager"),
    ("pacman", "Arch package manager"),
    ("systemctl", "Control systemd services"),
    ("journalctl", "Query systemd journal"),
];

/// Rich description for builtins and common external commands.
pub fn command_description(cmd: &str) -> Option<&'static str> {
    let builtin_desc = match cmd {
        "abs" => Some("Return absolute value of a number"),
        "round" => Some("Round a number to nearest integer"),
        "floor" => Some("Round a number down to nearest integer"),
        "ceil" => Some("Round a number up to nearest integer"),
        "pow" => Some("Raise a number to a power"),
        "min" => Some("Return the minimum of two or more values"),
        "max" => Some("Return the maximum of two or more values"),
        "which" => Some("Locate a command on PATH"),
        "graph" => Some("Render a capability graph"),
        "caps-profile" => Some("Show capability profile for current session"),
        "ls" => Some("List directories and files under active capabilities"),
        "watch" => Some("Watch a file or directory for changes"),
        "cd" => Some("Change current working directory and update PWD grants"),
        "z" => Some("Jump to a directory using frecency"),
        "zi" => Some("Interactive frecency directory picker"),
        "extract" => Some("Extract compressed archives"),
        "head" => Some("Show first N lines or items"),
        "tail" => Some("Show last N lines or items"),
        "uniq" => Some("Remove duplicate adjacent lines"),
        "join" => Some("Join lines or CSV fields"),
        "jobs" => Some("Display active background tasks and status"),
        "fg" => Some("Resume background tasks in foreground"),
        "bg" => Some("Resume suspended tasks in background"),
        "export" => Some("Set shell environment variables"),
        "env" => Some("Print shell environment variables"),
        "help" => Some("Display global reference or detailed command information"),
        "pwd" => Some("Print current working directory"),
        "echo" => Some("Print arguments to stdout"),
        "cat" => Some("Concatenate and print files"),
        "touch" => Some("Create empty files or update timestamps"),
        "mkdir" => Some("Create directories"),
        "rm" => Some("Remove files or directories"),
        "cp" => Some("Copy files or directories"),
        "mv" => Some("Move or rename files or directories"),
        "clear" => Some("Clear the terminal screen"),
        "wrap" => Some("Wrap text to a given width"),
        "type" => Some("Display type information for a value"),
        "strict" => Some("Run a command under strict capability enforcement"),
        "reload" => Some("Reload the shell configuration"),
        "alias" => Some("Create or list command aliases"),
        "history" => Some("Search, filter, and query SQLite command history"),
        "hook" => Some("Register or list shell hooks"),
        "group-by" => Some("Group pipeline items by a key"),
        _ => None,
    };
    builtin_desc.or_else(|| {
        COMMON_EXTERNAL_COMMANDS
            .iter()
            .find(|(name, _)| *name == cmd)
            .map(|(_, desc)| *desc)
    })
}

/// Declarative flag metadata for builtin commands.
pub fn builtin_flags(cmd: &str) -> &'static [(&'static str, &'static str)] {
    match cmd {
        "ls" => &[
            ("-v", "Verbose: include permissions field"),
            ("-a", "Include hidden entries"),
        ],
        "history" => &[
            ("-i", "Explicitly open the interactive search TUI"),
            (
                "--interactive",
                "Explicitly open the interactive search TUI",
            ),
            ("--stats", "Display database command statistics"),
            ("--cwd", "Filter by current directory"),
            ("--session", "Filter by current terminal session ID"),
            (
                "--global",
                "Clear CWD/session filters to query global history",
            ),
            ("--host", "Filter by current machine hostname"),
            ("--exit", "Filter by execution exit code"),
            ("--limit", "Limit number of returned pipeline entries"),
        ],
        "help" => &[
            ("-a", "Display all help topics in full"),
            ("-q", "Compact listing (name + summary per topic)"),
            ("-t", "List topic names only"),
            ("-e", "Show examples only for a topic"),
            ("-v", "Show full detail for a topic"),
            ("--search", "Search help topics by keyword"),
        ],
        "head" => &[("-n", "Number of lines/items to output (default 10)")],
        "tail" => &[("-n", "Number of lines/items to output (default 10)")],
        "reload" => &[
            ("--full", "Full process handoff with state preservation"),
            ("--build", "Rebuild the shell from source before reloading"),
            ("-b", "Rebuild the shell from source before reloading"),
        ],
        _ => &[],
    }
}

impl Completer for FshellCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<CompletionCandidate> {
        let pos = pos.min(line.len());
        let pos = if line.is_char_boundary(pos) {
            pos
        } else {
            line.floor_char_boundary(pos)
        };
        let prefix = &line[..pos];
        let words: Vec<&str> = prefix.split_whitespace().collect();
        let last_word = crate::ftui::completions::extract_quote_aware_token(prefix);
        let starting_new_arg = prefix.ends_with(' ');

        let is_external = if !words.is_empty() {
            let cmd = words[0];
            let is_builtin = self.env.get_all_builtins().iter().any(|b| b == cmd);
            let is_alias = self
                .env
                .get_all_aliases()
                .iter()
                .any(|(name, _)| name == cmd);
            let is_fn = {
                let fns = self.env.fns.read();
                fns.contains_key(cmd)
            };
            let env_path = Some(self.env.vars.read()).and_then(|vars| {
                if let Some(fshell_core::Val::String(s)) = vars.get("PATH") {
                    Some(s.clone())
                } else {
                    None
                }
            });
            !(is_builtin || is_alias || is_fn)
                && fshell_engine::is_external_command_cached(cmd, env_path.as_deref())
        } else {
            false
        };

        if is_external {
            if let Some(cached) = complete_with_carapace_cached(&words, last_word, pos)
                && !cached.is_empty()
            {
                return cached;
            }
            let words_owned: Vec<String> = words.iter().map(|s| s.to_string()).collect();
            spawn_carapace_refresh(words_owned, last_word.to_string());
        }

        let is_cd = words.len() >= 2 && words[words.len() - 2] == "cd" || prefix.trim_end() == "cd";
        let is_z = words.len() >= 2 && words[words.len() - 2] == "z" || prefix.trim_end() == "z";
        let is_path =
            last_word.contains('/') || last_word.starts_with('.') || last_word.starts_with('~');

        // z: frecency jump completions
        if is_z {
            let scored = {
                let mut cache = Z_FRECENCY_CACHE.lock().unwrap_or_else(|e| e.into_inner());
                let needs_rebuild = match &*cache {
                    None => true,
                    Some(c) => c.loaded_at.elapsed() >= Z_FRECENCY_TTL,
                };
                if needs_rebuild {
                    *cache = None;
                    if let Some(db_path) = fshell_builtins::get_frecency_db_path()
                        && let Ok(content) = std::fs::read_to_string(&db_path)
                    {
                        let db: fshell_builtins::FrecencyDb =
                            serde_json::from_str(&content).unwrap_or_default();
                        let mut scored: Vec<(String, f64)> = db
                            .paths
                            .iter()
                            .map(|(path, entry)| (path.clone(), entry.frequency))
                            .collect();
                        scored.sort_by(|a, b| {
                            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                        });
                        *cache = Some(FrecencyCache {
                            data: scored,
                            loaded_at: Instant::now(),
                        });
                    }
                }
                cache.as_ref().map(|c| c.data.clone()).unwrap_or_default()
            };

            if !scored.is_empty() {
                let last_word_lower = last_word.to_lowercase();
                let mut results = Vec::new();
                for (path, _) in scored {
                    let path_lower = path.to_lowercase();
                    if last_word.is_empty() || path_lower.contains(&last_word_lower) {
                        results.push(
                            CompletionCandidate::new(
                                path.clone(),
                                CompletionKind::Directory,
                                TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                            )
                            .with_description("[dir] frecency"),
                        );
                    }
                }
                if !results.is_empty() {
                    return results;
                }
            }
            let mut results = complete_files(if starting_new_arg { "" } else { last_word }, pos);
            results.retain(|s| s.value.ends_with('/'));
            return results;
        }

        // cd: directory-only completion
        if is_cd {
            let file_word = if starting_new_arg { "" } else { last_word };
            let mut results = complete_files(file_word, pos);
            results.retain(|s| s.value.ends_with('/'));
            return results;
        }

        // General path completion
        if is_path {
            return complete_files(last_word, pos);
        }

        // Job ID completion for fg/bg
        let is_fg_bg = words.len() >= 2 && matches!(words[words.len() - 2], "fg" | "bg")
            || prefix.trim_end() == "fg"
            || prefix.trim_end() == "bg";
        if is_fg_bg || last_word.starts_with('%') {
            let jobs = self.env.job_control.jobs.read();
            if !jobs.is_empty() {
                let typed_job = last_word.strip_prefix('%').unwrap_or("");
                let results: Vec<CompletionCandidate> = jobs
                    .iter()
                    .filter(|(_, job)| {
                        typed_job.is_empty() || job.id.to_string().starts_with(typed_job)
                    })
                    .map(|(_, job)| {
                        CompletionCandidate::new(
                            format!("%{}", job.id),
                            CompletionKind::Job,
                            TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                        )
                        .with_description(format!("Job {}: {}", job.id, job.cmd))
                    })
                    .collect();
                return results;
            }
        }

        // Pipe completion
        if let Some(pipe_idx) = prefix.rfind('|') {
            let upstream = &prefix[..pipe_idx];
            let downstream = &prefix[pipe_idx + 1..];
            let downstream_trimmed = downstream.trim_start();

            if downstream_trimmed.is_empty() {
                let pipe_ops = [
                    ("filter ", "Filter pipeline items"),
                    ("map ", "Map/transform pipeline items"),
                    ("sort ", "Sort pipeline items"),
                    ("grep ", "Grep text within pipeline items"),
                    ("count ", "Count pipeline items"),
                    ("limit ", "Limit pipeline output"),
                    ("@json ", "Serialize/Deserialize JSON boundary"),
                    ("@yaml ", "Serialize/Deserialize YAML boundary"),
                    ("@msgpack ", "Serialize/Deserialize MsgPack boundary"),
                    ("@text ", "Serialize to text boundary"),
                    ("@csv ", "Parse or emit CSV data"),
                    ("@table ", "Render structured data as an ASCII table"),
                    ("@bar ", "Render numeric data as a bar chart"),
                ];
                return pipe_ops
                    .into_iter()
                    .map(|(op, desc)| {
                        CompletionCandidate::new(
                            op,
                            CompletionKind::PipeOperator,
                            TextSpan::new(pos, pos),
                        )
                        .with_description(desc)
                    })
                    .collect();
            }

            let operators = ["filter", "map", "sort", "grep", "limit"];
            for op in &operators {
                if let Some(op_suffix) = downstream_trimmed.strip_prefix(op)
                    && (op_suffix.trim_start().is_empty() || op_suffix.ends_with(' '))
                {
                    let keys = get_upstream_keys(upstream, &self.env);
                    return keys
                        .into_iter()
                        .map(|k| {
                            CompletionCandidate::new(
                                k.clone(),
                                CompletionKind::Variable,
                                TextSpan::new(pos, pos),
                            )
                            .with_description(format!("Upstream property: {}", k))
                        })
                        .collect();
                }
            }

            let all_ops = [
                "filter", "map", "sort", "grep", "count", "limit", "@json", "@yaml", "@msgpack",
                "@text", "@csv", "@table", "@bar",
            ];
            if !downstream_trimmed.is_empty() {
                let partial = all_ops
                    .iter()
                    .filter(|op| op.starts_with(downstream_trimmed))
                    .collect::<Vec<_>>();
                if !partial.is_empty() {
                    return partial
                        .into_iter()
                        .map(|op| {
                            CompletionCandidate::new(
                                *op,
                                CompletionKind::PipeOperator,
                                TextSpan::new(pos, pos),
                            )
                            .with_description(*op)
                        })
                        .collect();
                }
            }
        }

        if let Some(custom_suggestions) = get_custom_completions(line, pos, &self.env)
            && !custom_suggestions.is_empty()
        {
            return custom_suggestions;
        }

        if is_external && !last_word.starts_with('-') && (starting_new_arg || is_path) {
            return complete_files(if starting_new_arg { "" } else { last_word }, pos);
        }

        let mut suggestions = Vec::new();
        let last_word_lower = last_word.to_lowercase();
        let builtins = self.env.get_all_builtins();

        // Flag completions for builtins
        if last_word.starts_with('-') && words.len() >= 2 {
            let cmd = words[0];
            let flags = builtin_flags(cmd);
            if !flags.is_empty() {
                suggestions.extend(
                    flags
                        .iter()
                        .filter(|(flag, _)| flag.starts_with(last_word))
                        .map(|(flag, desc)| {
                            CompletionCandidate::new(
                                *flag,
                                CompletionKind::Flag,
                                TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                            )
                            .with_description(*desc)
                        }),
                );
            }
        }

        // Auto-generated completions for external commands via --help parsing
        if suggestions.is_empty() && last_word.starts_with('-') && words.len() >= 2 {
            let cmd = words[0];
            let is_builtin = self.env.get_all_builtins().iter().any(|b| b == cmd);
            if !is_builtin {
                if let Some(flags) = get_completions(cmd) {
                    for flag in &flags {
                        let flag_str = flag.long.as_deref().or(flag.short.as_deref()).unwrap_or("");
                        if flag_str.starts_with(last_word) {
                            let value = flag
                                .long
                                .as_deref()
                                .or(flag.short.as_deref())
                                .unwrap_or("")
                                .to_string();
                            let final_val = if flag.has_arg {
                                format!("{} ", value)
                            } else {
                                value
                            };
                            let mut cand = CompletionCandidate::new(
                                final_val,
                                CompletionKind::Flag,
                                TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                            );
                            if let Some(d) = &flag.desc {
                                cand = cand.with_description(d.clone());
                            }
                            suggestions.push(cand);
                        }
                    }
                } else {
                    queue_background_parse(cmd);
                }
            }
        }

        // Help argument: suggest topics AND categories
        if words.len() >= 2 && words[words.len() - 2] == "help" && !last_word.starts_with('-') {
            let topics = fshell_builtins::help::help_topics();
            for topic in topics {
                if topic.name.starts_with(&last_word_lower) {
                    suggestions.push(
                        CompletionCandidate::new(
                            topic.name.to_string(),
                            CompletionKind::HelpTopic,
                            TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                        )
                        .with_description(topic.summary),
                    );
                }
            }
            let categories = [
                ("builtins", "Browse built-in commands"),
                ("pipeline", "Browse pipeline operators"),
                ("language", "Browse language constructs"),
                ("security", "Browse security topics"),
                ("concepts", "Browse shell concepts"),
            ];
            for (name, desc) in &categories {
                if name.starts_with(&last_word_lower) {
                    suggestions.push(
                        CompletionCandidate::new(
                            *name,
                            CompletionKind::HelpTopic,
                            TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                        )
                        .with_description(*desc),
                    );
                }
            }
            return suggestions;
        }

        // Git branch/tag completions
        if git_branch_context(&words) {
            let branches = git_branches_cached(&self.env);
            let tags = git_tags_cached();
            for branch in branches.iter().chain(tags.iter()) {
                if branch.starts_with(last_word) {
                    suggestions.push(
                        CompletionCandidate::new(
                            branch.clone(),
                            CompletionKind::GitBranch,
                            TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                        )
                        .with_description("git branch/tag"),
                    );
                }
            }
            if !suggestions.is_empty() {
                return suggestions;
            }
        }

        // Past word 1 of a recognized command -> default to files
        let on_arg = words.len() >= 2 || (words.len() == 1 && starting_new_arg);
        if on_arg
            && !last_word.starts_with('-')
            && !last_word.starts_with('$')
            && !last_word.starts_with('%')
            && !prefix.contains('|')
        {
            let cmd = words[0];
            let env_path = Some(self.env.vars.read()).and_then(|vars| {
                if let Some(fshell_core::Val::String(s)) = vars.get("PATH") {
                    Some(s.clone())
                } else {
                    None
                }
            });
            let is_cmd = builtins.iter().any(|b| *b == cmd)
                || self.env.get_all_aliases().iter().any(|(n, _)| n == cmd)
                || {
                    let fns = self.env.fns.read();
                    fns.contains_key(cmd)
                }
                || fshell_engine::is_external_command_cached(cmd, env_path.as_deref());
            if is_cmd {
                let file_word = if starting_new_arg { "" } else { last_word };
                return complete_files(file_word, pos);
            }
        }

        // Alias completions (first word)
        if words.len() <= 1 {
            let aliases = self.env.get_all_aliases();
            for (name, expansion) in &aliases {
                if name.starts_with(&last_word_lower) {
                    suggestions.push(
                        CompletionCandidate::new(
                            name.clone(),
                            CompletionKind::UserFunction,
                            TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                        )
                        .with_description(format!("-> {}", expansion)),
                    );
                }
            }
        }

        // Registered builtins
        for b in builtins {
            if b.starts_with(&last_word_lower) {
                let desc = command_description(&b).unwrap_or("Built-in command");
                suggestions.push(
                    CompletionCandidate::new(
                        b.clone(),
                        CompletionKind::Builtin,
                        TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                    )
                    .with_description(desc),
                );
            }
        }

        // Common external commands
        if words.len() <= 1 {
            for (cmd, desc) in COMMON_EXTERNAL_COMMANDS {
                if cmd.starts_with(&last_word_lower) {
                    if suggestions.iter().any(|s| s.value == *cmd) {
                        continue;
                    }
                    suggestions.push(
                        CompletionCandidate::new(
                            *cmd,
                            CompletionKind::ExternalCommand,
                            TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                        )
                        .with_description(*desc),
                    );
                }
            }
        }

        // PATH executables
        if words.len() <= 1 && !last_word_lower.is_empty() {
            let env_path = {
                let vars = self.env.vars.read();
                vars.get("PATH").and_then(|v| match v {
                    fshell_core::Val::String(s) => Some(s.clone()),
                    _ => None,
                })
            };
            let candidates = fshell_engine::get_path_executables(env_path.as_deref());
            for exe in candidates {
                if exe.to_lowercase().starts_with(&last_word_lower) {
                    if suggestions.iter().any(|s| s.value == exe) {
                        continue;
                    }
                    let desc = command_description(&exe).unwrap_or("[ext] executable");
                    suggestions.push(
                        CompletionCandidate::new(
                            exe,
                            CompletionKind::ExternalCommand,
                            TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                        )
                        .with_description(desc),
                    );
                }
            }
        }

        // Frequent commands from history
        if words.len() <= 1
            && !last_word_lower.is_empty()
            && let Ok(entries) = history::query_frequent_by_prefix(&last_word_lower, 10)
        {
            for (cmd, freq) in &entries {
                if cmd == last_word || suggestions.iter().any(|s| s.value == *cmd) {
                    continue;
                }
                suggestions.push(
                    CompletionCandidate::new(
                        cmd.clone(),
                        CompletionKind::Custom("history"),
                        TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                    )
                    .with_description(format!(
                        "{} use{}",
                        freq,
                        if *freq == 1 { "" } else { "s" }
                    )),
                );
            }
        }

        // Pipeline operators standalone
        let pipe_operators = [
            "filter", "map", "sort", "grep", "count", "limit", "@json", "@yaml", "@msgpack",
            "@text", "@csv", "@table", "@bar",
        ];
        for op in &pipe_operators {
            if op.starts_with(&last_word_lower) {
                let desc = match *op {
                    "filter" => "Filter pipeline items",
                    "map" => "Map/transform pipeline items",
                    "sort" => "Sort pipeline items",
                    "grep" => "Grep text within pipeline items",
                    "count" => "Count pipeline items",
                    "limit" => "Limit pipeline output",
                    _ => "Pipeline boundary format",
                };
                suggestions.push(
                    CompletionCandidate::new(
                        *op,
                        CompletionKind::PipeOperator,
                        TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                    )
                    .with_description(desc),
                );
            }
        }

        let common_keywords = [
            "let", "fn", "match", "try", "catch", "with", "caps", "true", "false", "null", "unsafe",
        ];
        for kw in &common_keywords {
            if kw.starts_with(&last_word_lower) {
                suggestions.push(
                    CompletionCandidate::new(
                        *kw,
                        CompletionKind::Keyword,
                        TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                    )
                    .with_description("Keyword"),
                );
            }
        }

        // User-defined functions
        {
            let fns = self.env.fns.read();
            for name in fns.keys() {
                if name.starts_with(&last_word_lower) {
                    suggestions.push(
                        CompletionCandidate::new(
                            name.clone(),
                            CompletionKind::UserFunction,
                            TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                        )
                        .with_description("User-defined function"),
                    );
                }
            }
        }

        // Variables
        {
            let vars = self.env.vars.read();
            let target_prefix = last_word_lower
                .strip_prefix('$')
                .unwrap_or(&last_word_lower);
            for k in (*vars).keys() {
                if k.to_lowercase().starts_with(target_prefix) {
                    suggestions.push(
                        CompletionCandidate::new(
                            format!("${}", k),
                            CompletionKind::Variable,
                            TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                        )
                        .with_description("Environment variable"),
                    );
                }
            }
        }

        if suggestions.is_empty() {
            if starting_new_arg {
                return complete_files("", pos);
            }
            if !last_word.is_empty() {
                return complete_files(last_word, pos);
            }
        }

        let recent_commands = history::get_recent_commands_cached();
        rank_suggestions(&mut suggestions, last_word, &recent_commands);
        suggestions
    }
}

pub fn compute_match_indices(query: &str, candidate: &str) -> Option<Vec<usize>> {
    if query.is_empty() {
        return None;
    }
    let query_lower = query.to_lowercase();
    let candidate_lower = candidate.to_lowercase();

    if let Some(pos) = candidate_lower.find(&query_lower) {
        let indices: Vec<usize> = (pos..pos + query_lower.chars().count()).collect();
        return Some(indices);
    }

    let mut indices = Vec::new();
    let mut query_chars = query_lower.chars();
    let mut next_char = query_chars.next();
    for (grapheme_idx, c) in candidate_lower.chars().enumerate() {
        if let Some(qc) = next_char {
            if c == qc {
                indices.push(grapheme_idx);
                next_char = query_chars.next();
            }
        } else {
            break;
        }
    }
    if next_char.is_none() && !indices.is_empty() {
        Some(indices)
    } else {
        None
    }
}

fn rank_suggestions(
    suggestions: &mut Vec<CompletionCandidate>,
    query: &str,
    recent_commands: &std::collections::HashSet<String>,
) {
    if suggestions.is_empty() {
        return;
    }

    let kind = fuzzy::choose_kind(suggestions.len());
    let prepared = fuzzy::PreparedQuery::new(query);
    let mut scored: Vec<(isize, usize, CompletionCandidate)> = suggestions
        .drain(..)
        .enumerate()
        .filter_map(|(i, s)| {
            let val_for_match = s.value.trim_end_matches(' ').to_owned();
            fuzzy::fuzzy_score_prepared(&prepared, &val_for_match, kind).map(|score| {
                let boost = if recent_commands.contains(&val_for_match) {
                    500
                } else {
                    0
                };
                (score + boost, i, s)
            })
        })
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    {
        let mut seen = Vec::<String>::new();
        scored.retain(|(_, _, s)| {
            let trimmed = s.value.trim_end_matches(' ');
            if seen.iter().any(|v| v == trimmed) {
                false
            } else {
                seen.push(trimmed.to_owned());
                true
            }
        });
    }
    scored.truncate(fuzzy::MAX_RESULTS);

    scored.sort_by_key(|a| a.2.kind);

    *suggestions = scored
        .into_iter()
        .map(|(_, _, mut s)| {
            let val_for_match = s.value.trim_end_matches(' ');
            s.match_indices = compute_match_indices(query, val_for_match);
            s
        })
        .collect();
}
