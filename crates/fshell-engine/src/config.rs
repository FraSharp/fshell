// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const START_MARKER: &str = "# === fsh managed settings ===";
pub const END_MARKER: &str = "# === end managed settings ===";
pub const MAX_BACKUPS: usize = 5;

/// Global file-system-level lock to prevent concurrent writes to init.fsh
/// from racing within the same process (e.g. alias and setopt called simultaneously).
static INIT_FSH_LOCK: Mutex<()> = Mutex::new(());
// Public API
/// Rewrite the managed settings block inside `init.fsh`, preserving user code.
/// If `set_lines` is empty, the managed block is removed entirely.
pub fn update_managed_settings(set_lines: &[String]) -> Result<(), String> {
    let init_path = init_path()?;

    fshell_core::debug_log!(
        "update_managed_settings: path={:?} set_lines={:?}",
        init_path,
        set_lines
    );

    let _lock = INIT_FSH_LOCK
        .lock()
        .map_err(|e| format!("Lock poisoned: {e}"))?;

    // Read current content on disk (with backup fallback).
    let disk_content = read_with_backup_chain(&init_path);
    let user_content = extract_user_code_from_str(&disk_content);

    if set_lines.is_empty() && user_content.trim().is_empty() {
        let _ = fs::remove_file(&init_path);
        // Also clean up backup files.
        if let Some(dir) = init_path.parent() {
            for b in 1..=MAX_BACKUPS {
                let _ = fs::remove_file(dir.join(format!("init.fsh.bak{b}")));
            }
        }
        return Ok(());
    }

    let new_content = compose_content(set_lines, &user_content);
    safe_write(&init_path, &new_content)
}

/// Add or update an alias line in init.fsh (replaces any existing alias with the same name).
pub fn persist_alias(name: &str, expansion: &str) -> Result<(), String> {
    let init_path = init_path()?;
    let line = format!("alias {} {}\n", name, fshell_quote(expansion));
    let prefix = format!("alias {} ", name);

    let _lock = INIT_FSH_LOCK
        .lock()
        .map_err(|e| format!("Lock poisoned: {e}"))?;

    let existing = read_with_backup_chain(&init_path);
    let new_content = filter_lines(&existing, |l| !l.trim_start().starts_with(&prefix)) + &line;

    safe_write(&init_path, &new_content)
}

/// Remove an alias line from init.fsh.
pub fn remove_alias(name: &str) -> Result<(), String> {
    let init_path = init_path()?;
    let prefix = format!("alias {} ", name);

    let _lock = INIT_FSH_LOCK
        .lock()
        .map_err(|e| format!("Lock poisoned: {e}"))?;

    let existing = read_with_backup_chain(&init_path);
    let new_content = filter_lines(&existing, |l| !l.trim_start().starts_with(&prefix));

    if new_content.trim() == existing.trim() {
        return Ok(()); // nothing changed
    }

    safe_write(&init_path, &new_content)
}

/// Persist a hook registration line in init.fsh (avoids duplicates).
pub fn persist_hook(event: &str, fn_name: &str) -> Result<(), String> {
    let init_path = init_path()?;
    let line = format!("hook {} {}\n", event, fn_name);

    let _lock = INIT_FSH_LOCK
        .lock()
        .map_err(|e| format!("Lock poisoned: {e}"))?;

    let existing = read_with_backup_chain(&init_path);
    if existing.lines().any(|l| l.trim() == line.trim()) {
        return Ok(());
    }
    let new_content = existing + &line;
    safe_write(&init_path, &new_content)
}

/// Remove a hook line from init.fsh.
pub fn remove_hook(event: &str, fn_name: &str) -> Result<(), String> {
    let init_path = init_path()?;
    let prefix = format!("hook {} {}", event, fn_name);

    let _lock = INIT_FSH_LOCK
        .lock()
        .map_err(|e| format!("Lock poisoned: {e}"))?;

    let existing = read_with_backup_chain(&init_path);
    let new_content = filter_lines(&existing, |l| l.trim() != prefix);

    if new_content.trim() == existing.trim() {
        return Ok(());
    }
    safe_write(&init_path, &new_content)
}

/// Persist a formatted function definition (appended to init.fsh inside user-code section).
pub fn persist_function(formatted: &str) -> Result<(), String> {
    let init_path = init_path()?;

    let _lock = INIT_FSH_LOCK
        .lock()
        .map_err(|e| format!("Lock poisoned: {e}"))?;

    let mut existing = read_with_backup_chain(&init_path);
    if !existing.is_empty() && !existing.ends_with('\n') {
        existing.push('\n');
    }
    existing.push_str(formatted);
    if !existing.ends_with('\n') {
        existing.push('\n');
    }
    safe_write(&init_path, &existing)
}

// Internal helpers
/// Returns the path to init.fsh.
fn init_path() -> Result<PathBuf, String> {
    let cfg_dir =
        crate::config_dir().ok_or_else(|| "Could not determine config directory".to_string())?;
    Ok(cfg_dir.join("init.fsh"))
}

/// Read init.fsh from disk, falling back through the backup chain if the main file
/// is missing, empty, or fails to read. Returns the content of the first readable
/// file found, or an empty string if nothing works.
pub fn read_with_backup_chain(path: &Path) -> String {
    // Try main file first.
    if let Ok(content) = fs::read_to_string(path)
        && !content.is_empty()
    {
        return content;
    }
    // Fall back through backups.
    let dir = match path.parent() {
        Some(d) => d,
        None => return String::new(),
    };
    for b in 1..=MAX_BACKUPS {
        let bak = dir.join(format!("init.fsh.bak{b}"));
        if let Ok(content) = fs::read_to_string(&bak)
            && !content.is_empty()
        {
            fshell_core::debug_log!("read_with_backup_chain: recovered from {:?}", bak);
            return content;
        }
    }
    String::new()
}

/// Atomically write content to `path` with backup rotation.
/// Validates that the write doesn't destroy meaningful content.
fn safe_write(path: &Path, content: &str) -> Result<(), String> {
    // === Guard 1: never silently truncate a non-empty file to empty ===
    // Only allow empty writes if the file doesn't exist or is already empty.
    let exists_and_nonempty =
        path.exists() && fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false);
    if content.is_empty() && exists_and_nonempty {
        // This would destroy content — refuse.
        fshell_core::debug_log!(
            "safe_write: refusing to truncate {:?} to empty (file has content)",
            path
        );
        return Err(
            "Refusing to write empty content over existing init.fsh — no changes applied. "
                .to_string()
                + "If you intended to clear the file, use `config reload` or remove it manually.",
        );
    }

    // === Guard 2: rotate backups BEFORE writing ===
    rotate_backups(path);

    // === Guard 3: atomic write via temp + rename ===
    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, content)
        .map_err(|e| format!("Failed to write {}: {e}", tmp_path.display()))?;
    fs::rename(&tmp_path, path).map_err(|e| {
        format!(
            "Failed to rename {} -> {}: {e}",
            tmp_path.display(),
            path.display()
        )
    })?;

    // === Guard 4: verify the written content ===
    let verify = fs::read_to_string(path).unwrap_or_default();
    if verify != content {
        // Try to restore from backup.
        if let Some(dir) = path.parent()
            && let Ok(bak_content) = fs::read_to_string(dir.join("init.fsh.bak1"))
        {
            let _ = fs::write(path, &bak_content);
        }
        return Err(format!(
            "Write verification failed for {:?} — content mismatch, restored from backup.",
            path
        ));
    }

    Ok(())
}

/// Rotate backup files: .bak1 → .bak2 → .bak3 → ... → .bak{MAX_BACKUPS}, then copy current → .bak1.
fn rotate_backups(path: &Path) {
    let dir = match path.parent() {
        Some(d) => d,
        None => return,
    };
    let file_name = match path.file_name().and_then(|s| s.to_str()) {
        Some(n) => n,
        None => return,
    };

    // Shift backups down the chain.
    for b in (1..MAX_BACKUPS).rev() {
        let src = dir.join(format!("{}.bak{b}", file_name));
        let dst = dir.join(format!("{}.bak{}", file_name, b + 1));
        if src.exists() {
            let _ = fs::rename(&src, &dst);
        }
    }

    // Copy current file to .bak1.
    if path.exists() {
        let bak1 = dir.join(format!("{}.bak1", file_name));
        let _ = fs::copy(path, &bak1);
    }
}

/// Compose the final content from managed settings lines and user code.
fn compose_content(set_lines: &[String], user_content: &str) -> String {
    let mut new_content = String::new();

    if !set_lines.is_empty() {
        new_content.push_str(START_MARKER);
        new_content.push('\n');
        for line in set_lines {
            new_content.push_str(line);
            new_content.push('\n');
        }
        new_content.push_str(END_MARKER);
        new_content.push_str("\n\n");
    }

    let trimmed = user_content.trim();
    if !trimmed.is_empty() {
        new_content.push_str(trimmed);
        new_content.push('\n');
    }

    new_content
}

/// Extract everything outside the managed block markers from a string.
pub fn extract_user_code_from_str(content: &str) -> String {
    // Strip trailing newline for consistent matching.
    let content = content.trim_end();
    if let Some(start) = content.find(START_MARKER) {
        let before = &content[..start];
        if let Some(end) = content[start..].find(END_MARKER) {
            let after_start = start + end + END_MARKER.len();
            let after = &content[after_start..];
            return format!("{}{}", before.trim_end(), after.trim_start());
        }
    }
    content.to_string()
}

/// Extract everything outside the managed block markers (file-backed).
pub fn extract_user_code(init_path: &std::path::Path) -> String {
    if !init_path.exists() {
        return String::new();
    }
    let content = match fs::read_to_string(init_path) {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    extract_user_code_from_str(&content)
}

/// Filter lines of a string, keeping those for which `pred` returns true.
fn filter_lines(content: &str, pred: impl Fn(&str) -> bool) -> String {
    content
        .lines()
        .filter(|l| pred(l))
        .map(|l| format!("{}\n", l))
        .collect()
}

/// Quote a string for use as an fsh string literal.
pub fn fshell_quote(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{}\"", escaped)
}

/// Snapshot of shell settings for serialization.
pub struct SettingsSnapshot {
    pub autocd: bool,
    pub pipefail: bool,
    pub notify: bool,
    pub json_auto_parse: bool,
    pub did_you_mean: bool,
    pub sandbox_mode: String,
    pub pipeline_channel_size: usize,
    pub prompt: String,
    pub prompt_right: String,
    pub keybinding: String,
    pub errexit: bool,
    pub nounset: bool,
    pub nullglob: bool,
    pub nocaseglob: bool,
    pub noclobber: bool,
    pub noexec: bool,
    pub xtrace: bool,
    pub verbose: bool,
    pub ignoreeof: bool,
    pub autopushd: bool,
    pub histignoredups: bool,
    pub cdable_vars: bool,
    pub quiet_aliases: bool,
    pub clear_on_reload: String,
    pub session_restore: String,
    pub theme: String,
    pub disabled_builtins: Vec<String>,
    pub command_binaries: std::collections::HashMap<String, String>,
    pub confirm_destructive: bool,
    pub sandbox_all: bool,
}

/// Collect serializable settings from current env state into "set" / "setopt" lines.
pub fn collect_settings_lines(s: &SettingsSnapshot) -> Vec<String> {
    let mut lines = Vec::new();
    if !s.autocd {
        lines.push("unsetopt autocd".into());
    }
    if s.pipefail {
        lines.push("setopt pipefail".into());
    }
    if s.notify {
        lines.push("setopt notify".into());
    }
    if !s.json_auto_parse {
        lines.push("unsetopt json_auto_parse".into());
    }
    if !s.did_you_mean {
        lines.push("unsetopt did_you_mean".into());
    }
    if !s.confirm_destructive {
        lines.push("unsetopt confirm_destructive".into());
    }
    if s.sandbox_all {
        lines.push("setopt sandbox_all".into());
    }

    if s.errexit {
        lines.push("setopt errexit".into());
    }
    if !s.nounset {
        lines.push("unsetopt nounset".into());
    }
    if s.nullglob {
        lines.push("setopt nullglob".into());
    }
    if s.nocaseglob {
        lines.push("setopt nocaseglob".into());
    }
    if s.noclobber {
        lines.push("setopt noclobber".into());
    }
    if s.noexec {
        lines.push("setopt noexec".into());
    }
    if s.xtrace {
        lines.push("setopt xtrace".into());
    }
    if s.verbose {
        lines.push("setopt verbose".into());
    }
    if s.ignoreeof {
        lines.push("setopt ignoreeof".into());
    }
    if s.autopushd {
        lines.push("setopt autopushd".into());
    }
    if s.histignoredups {
        lines.push("setopt histignoredups".into());
    }
    if s.cdable_vars {
        lines.push("setopt cdable_vars".into());
    }
    if s.quiet_aliases {
        lines.push("setopt quiet_aliases".into());
    }

    if s.sandbox_mode != "prompt" {
        lines.push(format!("set sandbox_mode {}", s.sandbox_mode));
    }
    if s.pipeline_channel_size != 100 {
        lines.push(format!(
            "set pipeline_channel_size {}",
            s.pipeline_channel_size
        ));
    }
    if !s.prompt.is_empty() {
        lines.push(format!("set prompt {}", fshell_quote(&s.prompt)));
    }
    if !s.prompt_right.is_empty() {
        lines.push(format!(
            "set prompt_right {}",
            fshell_quote(&s.prompt_right)
        ));
    }
    if !s.keybinding.is_empty() && s.keybinding != "emacs" {
        lines.push(format!("set keybinding {}", s.keybinding));
    }
    if s.clear_on_reload != "ask" {
        lines.push(format!("set clear_on_reload {}", s.clear_on_reload));
    }
    if s.session_restore != "none" {
        lines.push(format!("set session_restore {}", s.session_restore));
    }
    if !s.theme.is_empty() && s.theme != "default" {
        lines.push(format!("set theme {}", s.theme));
    }
    if !s.disabled_builtins.is_empty() {
        let quoted: Vec<String> = s
            .disabled_builtins
            .iter()
            .map(|b| fshell_quote(b))
            .collect();
        lines.push(format!("set disabled_builtins [{}]", quoted.join(", ")));
    }
    if !s.command_binaries.is_empty() {
        let mut pairs = Vec::new();
        let mut keys: Vec<&String> = s.command_binaries.keys().collect();
        keys.sort();
        for k in keys {
            let v = &s.command_binaries[k];
            pairs.push(format!("{}: {}", fshell_quote(k), fshell_quote(v)));
        }
        lines.push(format!("set command_binaries {{{}}}", pairs.join(", ")));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_preserves_user_code_outside_block() {
        let dir = std::env::temp_dir().join("fsh_config_test_uc");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let init_path = dir.join("init.fsh");
        fs::write(
            &init_path,
            "alias ll \"ls -l\"\n# === fsh managed settings ===\nsetopt autocd\n# === end managed settings ===\nfn my_func { echo hello }\n",
        )
        .unwrap();

        let user = extract_user_code(&init_path);
        assert!(user.contains("alias ll"), "user code missing alias");
        assert!(user.contains("fn my_func"), "user code missing fn");
        assert!(!user.contains("setopt autocd"), "managed code leaked");
    }

    #[test]
    fn test_update_managed_settings_writes_block() {
        let dir = std::env::temp_dir().join("fsh_cfg_mig_update");
        let _ = fs::remove_dir_all(&dir);
        let cfg_dir = dir.join(".config/fsh");
        fs::create_dir_all(&cfg_dir).unwrap();

        // Override FSH_CONFIG_DIR so update_managed_settings uses our temp dir
        let orig = std::env::var("FSH_CONFIG_DIR").ok();
        fshell_core::set_var("FSH_CONFIG_DIR", cfg_dir.to_str().unwrap());

        let lines = vec!["setopt autocd".into(), "set prompt \"> \"".into()];
        update_managed_settings(&lines).unwrap();

        let content = fs::read_to_string(cfg_dir.join("init.fsh")).unwrap();
        assert!(content.contains(START_MARKER));
        assert!(content.contains("setopt autocd"));
        assert!(content.contains("set prompt \"> \""));
        assert!(content.contains(END_MARKER));

        if let Some(v) = orig {
            fshell_core::set_var("FSH_CONFIG_DIR", &v);
        } else {
            fshell_core::remove_var("FSH_CONFIG_DIR");
        }
    }

    #[test]
    fn test_collect_settings_lines_defaults_empty() {
        let lines = collect_settings_lines(&SettingsSnapshot {
            autocd: true,
            pipefail: false,
            notify: false,
            json_auto_parse: true,
            did_you_mean: true,
            sandbox_mode: "prompt".into(),
            pipeline_channel_size: 100,
            prompt: String::new(),
            prompt_right: String::new(),
            keybinding: "emacs".into(),
            errexit: false,
            nounset: true,
            nullglob: false,
            nocaseglob: false,
            noclobber: false,
            noexec: false,
            xtrace: false,
            verbose: false,
            ignoreeof: false,
            autopushd: false,
            histignoredups: false,
            cdable_vars: false,
            quiet_aliases: false,
            clear_on_reload: "ask".into(),
            session_restore: "none".into(),
            theme: "default".into(),
            disabled_builtins: vec![],
            command_binaries: std::collections::HashMap::new(),
            confirm_destructive: true,
            sandbox_all: false,
        });
        assert!(
            lines.is_empty(),
            "expected empty for defaults, got {lines:?}"
        );
    }

    #[test]
    fn test_collect_settings_lines_nondefault() {
        let lines = collect_settings_lines(&SettingsSnapshot {
            autocd: false,
            pipefail: true,
            notify: true,
            json_auto_parse: false,
            did_you_mean: false,
            sandbox_mode: "deny-all".into(),
            pipeline_channel_size: 200,
            prompt: "> ".into(),
            prompt_right: String::new(),
            keybinding: "vi".into(),
            errexit: true,
            nounset: false,
            nullglob: true,
            nocaseglob: true,
            noclobber: true,
            noexec: true,
            xtrace: true,
            verbose: true,
            ignoreeof: true,
            autopushd: true,
            histignoredups: true,
            cdable_vars: true,
            quiet_aliases: true,
            clear_on_reload: "always".into(),
            session_restore: "picker".into(),
            theme: "dracula".into(),
            disabled_builtins: vec![],
            command_binaries: std::collections::HashMap::new(),
            confirm_destructive: false,
            sandbox_all: true,
        });
        assert!(lines.contains(&"unsetopt autocd".into()));
        assert!(lines.contains(&"setopt pipefail".into()));
        assert!(lines.contains(&"setopt notify".into()));
        assert!(lines.contains(&"unsetopt json_auto_parse".into()));
        assert!(lines.contains(&"unsetopt did_you_mean".into()));
        assert!(lines.contains(&"setopt errexit".into()));
        assert!(lines.contains(&"unsetopt nounset".into()));
        assert!(lines.contains(&"setopt nullglob".into()));
        assert!(lines.contains(&"setopt nocaseglob".into()));
        assert!(lines.contains(&"setopt noclobber".into()));
        assert!(lines.contains(&"setopt noexec".into()));
        assert!(lines.contains(&"setopt xtrace".into()));
        assert!(lines.contains(&"setopt verbose".into()));
        assert!(lines.contains(&"setopt ignoreeof".into()));
        assert!(lines.contains(&"setopt autopushd".into()));
        assert!(lines.contains(&"setopt histignoredups".into()));
        assert!(lines.contains(&"setopt cdable_vars".into()));
        assert!(lines.contains(&"setopt quiet_aliases".into()));
        assert!(lines.contains(&"set sandbox_mode deny-all".into()));
        assert!(lines.contains(&"set pipeline_channel_size 200".into()));
        assert!(lines.contains(&"set prompt \"> \"".into()));
        assert!(lines.contains(&"set keybinding vi".into()));
        assert!(lines.contains(&"set clear_on_reload always".into()));
        assert!(lines.contains(&"set session_restore picker".into()));
        assert!(lines.contains(&"set theme dracula".into()));
    }
}
