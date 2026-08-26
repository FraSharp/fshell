// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::error::BuiltinError;
use chrono::{DateTime, Utc};
use fshell_core::ShellError;
use fshell_core::Val;
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use miette::SourceSpan;
use std::sync::Arc;
use std::time::SystemTime;
use ustr::ustr;
use walkdir::WalkDir;

/// Filter configuration parsed from user arguments.
struct FfConfig {
    path: String,
    name_glob: Option<String>,
    negate_name: bool,
    min_size: Option<u64>,
    max_size: Option<u64>,
    min_modified: Option<i64>, // seconds since now (e.g. "7d" = now - 7*86400)
    max_modified: Option<i64>, // seconds since now
    dirs_only: bool,
    files_only: bool,
    hidden: bool,
    max_depth: Option<usize>,
    max_results: Option<usize>,
}

impl Default for FfConfig {
    fn default() -> Self {
        Self {
            path: ".".to_string(),
            name_glob: None,
            negate_name: false,
            min_size: None,
            max_size: None,
            min_modified: None,
            max_modified: None,
            dirs_only: false,
            files_only: false,
            hidden: false,
            max_depth: None,
            max_results: None,
        }
    }
}

pub fn ff_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    // 1. Check process / file-read capability on the current dir
    let pwd = env.cwd();
    env.enforce_capability("ff", fshell_engine::CapAction::ReadDir(pwd.clone()))?;
    env.track_read(pwd.clone());

    // 2. Parse args
    let mut config = parse_args(&args)?;
    if config.path == "." {
        config.path = pwd.to_string_lossy().to_string();
    }

    // 3. Check capability on the search root
    let search_root = std::path::PathBuf::from(&config.path);
    if !search_root.exists() {
        return Err(BuiltinError::FileNotFound {
            cmd: "ff".into(),
            path: config.path.to_string(),
            span,
        }
        .into());
    }
    env.enforce_capability("ff", fshell_engine::CapAction::ReadDir(search_root.clone()))?;
    env.track_read(search_root);

    // 4. Run the search
    let config_arc = Arc::new(config);
    let tx_clone = tx.clone();
    tokio::spawn(async move {
        if let Err(e) = run_search(&config_arc, tx_clone).await {
            let _ = tx.send(PipelinePayload::Structured(e.into())).await;
        }
    });

    Ok(())
}

async fn run_search(config: &FfConfig, tx: PipeSender) -> Result<(), ShellError> {
    let now = SystemTime::now();
    // Bounded defaults: an unbounded recursive walk from cwd (e.g. `/`) can
    // scan the whole filesystem. 32 levels covers real project layouts and
    // 1000 results bounds output; both are overridable via `-d`/`-l`.
    let max_depth = config.max_depth.unwrap_or(32);
    let max_results = config.max_results.unwrap_or(1000);
    let mut count = 0usize;

    for entry in WalkDir::new(&config.path)
        .max_depth(max_depth)
        .follow_links(false)
    {
        // Yield to tokio periodically to avoid blocking
        if count.is_multiple_of(500) {
            tokio::task::yield_now().await;
        }

        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue, // skip permission-denied etc.
        };

        // Hidden file filter — skip entries starting with '.'
        if !config.hidden
            && let Some(name) = entry.file_name().to_str()
            && name.starts_with('.')
        {
            continue;
        }

        // Type filter
        if config.dirs_only && !entry.file_type().is_dir() {
            continue;
        }
        if config.files_only && !entry.file_type().is_file() {
            continue;
        }

        // Name glob filter
        if let Some(ref glob) = config.name_glob {
            let name = entry.file_name().to_string_lossy();
            let matched = glob_match(glob, &name);
            if config.negate_name == matched {
                continue;
            }
        }

        // Size filter (only for files)
        if (config.min_size.is_some() || config.max_size.is_some())
            && let Ok(meta) = entry.metadata()
        {
            let size = meta.len();
            if let Some(min) = config.min_size
                && size < min
            {
                continue;
            }
            if let Some(max) = config.max_size
                && size > max
            {
                continue;
            }
        }

        // Modified filter
        if (config.min_modified.is_some() || config.max_modified.is_some())
            && let Ok(meta) = entry.metadata()
            && let Ok(mtime) = meta.modified()
        {
            let age = now
                .duration_since(mtime)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            if let Some(min) = config.min_modified
                && age < min
            {
                continue;
            }
            if let Some(max) = config.max_modified
                && age > max
            {
                continue;
            }
        }

        // Build the result record — cache metadata to avoid repeated stat calls
        let path = entry.path().to_string_lossy().to_string();
        let name = entry.file_name().to_string_lossy().to_string();
        let file_type = if entry.file_type().is_dir() {
            "dir"
        } else if entry.file_type().is_symlink() {
            "symlink"
        } else {
            "file"
        };

        let meta = entry.metadata().ok();
        let size = meta.as_ref().map(|m| m.len() as i64).unwrap_or(0);
        let modified_iso = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .map(|t| {
                let dt: DateTime<Utc> = t.into();
                dt.to_rfc3339()
            })
            .unwrap_or_default();

        let mut map = fshell_core::FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        map.insert(ustr("path"), Val::String(path.clone()));
        map.insert(ustr("abs_path"), Val::String(path));
        map.insert(ustr("name"), Val::String(name));
        map.insert(ustr("type"), Val::String(file_type.to_string()));
        map.insert(ustr("size"), Val::Int(size));
        map.insert(ustr("modified"), Val::String(modified_iso));

        count += 1;
        if tx
            .send(PipelinePayload::Data(Arc::new(Val::Map(map))))
            .await
            .is_err()
        {
            break;
        }

        if count >= max_results {
            break;
        }
    }

    Ok(())
}

/// Parse user arguments into a `FfConfig`.
fn parse_args(args: &[Val]) -> Result<FfConfig, String> {
    let mut config = FfConfig::default();
    let mut raw_args: Vec<String> = Vec::new();

    for arg in args {
        let s = match arg {
            Val::String(st) => st.clone(),
            Val::Int(i) => i.to_string(),
            Val::Float(f) => f.to_string(),
            other => other.to_text(),
        };
        raw_args.push(s);
    }

    // First positional arg (before any filter keyword) is the path
    let mut positional = Vec::new();
    let mut i = 0;
    while i < raw_args.len() {
        match raw_args[i].as_str() {
            // Ignore "and" as syntactic sugar
            "and" => {
                i += 1;
                continue;
            }
            "name" => {
                if i + 2 < raw_args.len() {
                    let op = &raw_args[i + 1];
                    let pattern = &raw_args[i + 2];
                    if op == "=" {
                        config.name_glob = Some(pattern.clone());
                        config.negate_name = false;
                        i += 3;
                        continue;
                    } else if op == "!=" {
                        config.name_glob = Some(pattern.clone());
                        config.negate_name = true;
                        i += 3;
                        continue;
                    }
                }
                return Err(
                    "ff: expected name = \"<pattern>\" or name != \"<pattern>\"".to_string()
                );
            }
            "size" => {
                if i + 2 < raw_args.len() {
                    let op = &raw_args[i + 1];
                    let val_raw = &raw_args[i + 2];
                    let bytes = parse_size(val_raw)?;
                    match op.as_str() {
                        ">" => config.min_size = Some(bytes + 1),
                        ">=" => config.min_size = Some(bytes),
                        "<" => config.max_size = Some(bytes.saturating_sub(1)),
                        "<=" => config.max_size = Some(bytes),
                        "=" => {
                            config.min_size = Some(bytes);
                            config.max_size = Some(bytes);
                        }
                        _ => {
                            return Err(BuiltinError::UnexpectedArgument {
                                cmd: "ff".into(),
                                arg: format!("unknown size operator '{op}'"),
                                span: None,
                            }
                            .to_string());
                        }
                    }
                    i += 3;
                    continue;
                }
                return Err("ff: expected size <op> <value> (e.g. size > 1mb)".to_string());
            }
            "modified" => {
                if i + 2 < raw_args.len() {
                    let op = &raw_args[i + 1];
                    let val_raw = &raw_args[i + 2];
                    let seconds = parse_duration(val_raw)?;
                    match op.as_str() {
                        // "modified > 7d" = older than 7 days → min age = 7 days
                        ">" => config.min_modified = Some(seconds + 1),
                        ">=" => config.min_modified = Some(seconds),
                        // "modified < 7d" = newer than 7 days → max age = 7 days
                        "<" => config.max_modified = Some(seconds.saturating_sub(1)),
                        "<=" => config.max_modified = Some(seconds),
                        "=" => {
                            config.min_modified = Some(seconds);
                            config.max_modified = Some(seconds);
                        }
                        _ => {
                            return Err(BuiltinError::UnexpectedArgument {
                                cmd: "ff".into(),
                                arg: format!("unknown modified operator '{op}'"),
                                span: None,
                            }
                            .to_string());
                        }
                    }
                    i += 3;
                    continue;
                }
                return Err("ff: expected modified <op> <value> (e.g. modified > 7d)".to_string());
            }
            "type" => {
                if i + 2 < raw_args.len() {
                    let op = &raw_args[i + 1];
                    let ty = &raw_args[i + 2];
                    if op != "=" {
                        return Err(BuiltinError::UnexpectedArgument {
                            cmd: "ff".into(),
                            arg: format!("expected type = \"dir\" or type = \"file\", got '{op}'"),
                            span: None,
                        }
                        .to_string());
                    }
                    match ty.as_str() {
                        "dir" => config.dirs_only = true,
                        "file" => config.files_only = true,
                        _ => {
                            return Err(BuiltinError::UnexpectedArgument {
                                cmd: "ff".into(),
                                arg: format!("unknown type '{ty}', expected 'dir' or 'file'"),
                                span: None,
                            }
                            .to_string());
                        }
                    }
                    i += 3;
                    continue;
                }
                return Err("ff: expected type = \"dir\" or type = \"file\"".to_string());
            }
            "hidden" => {
                config.hidden = true;
                i += 1;
                continue;
            }
            "-n" | "--max-results" => {
                if i + 1 < raw_args.len() {
                    let n = raw_args[i + 1]
                        .parse::<usize>()
                        .map_err(|_| format!("ff: invalid number for -n: {}", raw_args[i + 1]))?;
                    config.max_results = Some(n);
                    i += 2;
                    continue;
                }
                return Err("ff: -n requires a number".to_string());
            }
            "--depth" => {
                if i + 1 < raw_args.len() {
                    let d = raw_args[i + 1].parse::<usize>().map_err(|_| {
                        format!("ff: invalid number for --depth: {}", raw_args[i + 1])
                    })?;
                    config.max_depth = Some(d);
                    i += 2;
                    continue;
                }
                return Err("ff: --depth requires a number".to_string());
            }
            other => {
                positional.push(other.to_string());
                i += 1;
            }
        }
    }

    // First positional is the search path
    if let Some(path) = positional.first() {
        config.path = path.clone();
    }

    Ok(config)
}

/// Parse a human-readable size string (e.g., "1mb", "500kb", "2gb") into bytes.
fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim().to_lowercase();
    if s.is_empty() {
        return Err("ff: empty size value".to_string());
    }

    let (num_str, unit) = if s.ends_with("kib") {
        (&s[..s.len() - 3], 1024u64)
    } else if s.ends_with("mib") {
        (&s[..s.len() - 3], 1024u64.pow(2))
    } else if s.ends_with("gib") {
        (&s[..s.len() - 3], 1024u64.pow(3))
    } else if s.ends_with("kb") {
        (&s[..s.len() - 2], 1000u64)
    } else if s.ends_with("mb") {
        (&s[..s.len() - 2], 1000u64.pow(2))
    } else if s.ends_with("gb") {
        (&s[..s.len() - 2], 1000u64.pow(3))
    } else if s.ends_with("b") {
        (&s[..s.len() - 1], 1u64)
    } else if s.ends_with('k') {
        (&s[..s.len() - 1], 1000u64)
    } else if s.ends_with('m') {
        (&s[..s.len() - 1], 1000u64.pow(2))
    } else if s.ends_with('g') {
        (&s[..s.len() - 1], 1000u64.pow(3))
    } else {
        (s.as_str(), 1u64)
    };

    let num: f64 = num_str
        .parse()
        .map_err(|_| format!("ff: invalid size number '{}'", num_str))?;

    Ok((num * unit as f64) as u64)
}

/// Parse a human-readable duration string (e.g., "7d", "30m", "2h") into seconds.
fn parse_duration(s: &str) -> Result<i64, String> {
    let s = s.trim().to_lowercase();
    if s.is_empty() {
        return Err("ff: empty duration value".to_string());
    }

    let (num_str, multiplier) = if s.ends_with('d') {
        (&s[..s.len() - 1], 86400i64)
    } else if s.ends_with('h') {
        (&s[..s.len() - 1], 3600i64)
    } else if s.ends_with('m') && s.len() > 1 {
        // Could be minutes. Avoid matching just "m"
        if s.ends_with("min") {
            (&s[..s.len() - 3], 60i64)
        } else {
            (&s[..s.len() - 1], 60i64)
        }
    } else if s.ends_with('s') {
        (&s[..s.len() - 1], 1i64)
    } else {
        (s.as_str(), 1i64)
    };

    let num: i64 = num_str
        .parse()
        .map_err(|_| format!("ff: invalid duration number '{}'", num_str))?;

    Ok(num * multiplier)
}

/// Simple glob matching — supports `*`, `?`, and `[chars]`.
/// This is a basic pattern matcher, not a full regex.
pub(crate) fn glob_match(pattern: &str, name: &str) -> bool {
    let pat_chars: Vec<char> = pattern.chars().collect();
    let name_chars: Vec<char> = name.chars().collect();
    let mut pi = 0;
    let mut ni = 0;
    let mut star_pi = None;
    let mut star_ni = 0;

    while ni < name_chars.len() {
        if pi < pat_chars.len() && (pat_chars[pi] == '?' || pat_chars[pi] == name_chars[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < pat_chars.len() && pat_chars[pi] == '*' {
            star_pi = Some(pi);
            star_ni = ni + 1;
            pi += 1;
        } else if let Some(spi) = star_pi {
            pi = spi + 1;
            ni = star_ni;
            star_ni += 1;
        } else {
            return false;
        }
    }

    while pi < pat_chars.len() && pat_chars[pi] == '*' {
        pi += 1;
    }

    pi == pat_chars.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("1kb").unwrap(), 1000);
        assert_eq!(parse_size("1mb").unwrap(), 1_000_000);
        assert_eq!(parse_size("1gb").unwrap(), 1_000_000_000);
        assert_eq!(parse_size("1kib").unwrap(), 1024);
        assert_eq!(parse_size("1mib").unwrap(), 1_048_576);
        assert_eq!(parse_size("500b").unwrap(), 500);
        assert_eq!(parse_size("100").unwrap(), 100);
        assert_eq!(parse_size("2k").unwrap(), 2000);
        assert_eq!(parse_size("1.5mb").unwrap(), 1_500_000);
    }

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("7d").unwrap(), 7 * 86400);
        assert_eq!(parse_duration("30m").unwrap(), 30 * 60);
        assert_eq!(parse_duration("2h").unwrap(), 2 * 3600);
        assert_eq!(parse_duration("60s").unwrap(), 60);
    }

    #[test]
    fn test_glob_match() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("*.rs", "lib.rs"));
        assert!(!glob_match("*.rs", "main.rs.bak"));
        assert!(glob_match("test*", "test_ff.rs"));
        assert!(glob_match("src/*.rs", "src/main.rs"));
        assert!(!glob_match("src/*.rs", "src/main.c"));
        assert!(glob_match("?", "a"));
        assert!(!glob_match("?", "ab"));
        assert!(glob_match("*.txt", "file.txt"));
        assert!(!glob_match("*.txt", "file.md"));
    }

    #[test]
    fn test_parse_args_basic() {
        let args: Vec<Val> = vec![
            Val::String("name".to_string()),
            Val::String("=".to_string()),
            Val::String("*.rs".to_string()),
        ];
        let config = parse_args(&args).unwrap();
        assert_eq!(config.name_glob, Some("*.rs".to_string()));
        assert!(!config.negate_name);
    }

    #[test]
    fn test_parse_args_with_path() {
        let args: Vec<Val> = vec![
            Val::String("src".to_string()),
            Val::String("name".to_string()),
            Val::String("=".to_string()),
            Val::String("*.rs".to_string()),
        ];
        let config = parse_args(&args).unwrap();
        assert_eq!(config.path, "src");
        assert_eq!(config.name_glob, Some("*.rs".to_string()));
    }

    #[test]
    fn test_parse_size_operator() {
        let args: Vec<Val> = vec![
            Val::String("size".to_string()),
            Val::String(">".to_string()),
            Val::String("1mb".to_string()),
        ];
        let config = parse_args(&args).unwrap();
        // 1mb = 1_000_000 bytes, min_size should be 1_000_001 (strictly greater)
        assert_eq!(config.min_size, Some(1_000_001));
    }

    #[test]
    fn test_parse_hidden() {
        let args: Vec<Val> = vec![Val::String("hidden".to_string())];
        let config = parse_args(&args).unwrap();
        assert!(config.hidden);
    }

    #[test]
    fn test_parse_type_dir() {
        let args: Vec<Val> = vec![
            Val::String("type".to_string()),
            Val::String("=".to_string()),
            Val::String("dir".to_string()),
        ];
        let config = parse_args(&args).unwrap();
        assert!(config.dirs_only);
    }

    #[test]
    fn test_parse_limits() {
        let args: Vec<Val> = vec![
            Val::String("-n".to_string()),
            Val::String("50".to_string()),
            Val::String("--depth".to_string()),
            Val::String("3".to_string()),
        ];
        let config = parse_args(&args).unwrap();
        assert_eq!(config.max_results, Some(50));
        assert_eq!(config.max_depth, Some(3));
    }
}
