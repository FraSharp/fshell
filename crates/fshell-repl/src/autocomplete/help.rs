// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Dynamic `--help` flag extraction, parsing, and caching.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct HelpFlag {
    pub short: Option<String>,
    pub long: Option<String>,
    pub has_arg: bool,
    pub desc: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct CachedHelp {
    pub command: String,
    pub mtime: u64,
    pub fetched_at: u64,
    pub flags: Vec<HelpFlag>,
}

fn cache_path_for(cmd: &str) -> Option<PathBuf> {
    let dir = fshell_engine::cache_dir()?.join("completions");
    let safe: String = cmd
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if safe.is_empty() {
        return None;
    }
    Some(dir.join(format!("{safe}.json")))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

static BACKGROUND_PARSE_DEBOUNCE: Mutex<Option<(String, Instant)>> = Mutex::new(None);
const BACKGROUND_PARSE_DEBOUNCE_MS: u64 = 2000;

static PARSE_TX: OnceLock<mpsc::SyncSender<String>> = OnceLock::new();
static RESOLVED_PATHS: Mutex<Option<(u64, HashMap<String, PathBuf>)>> = Mutex::new(None);

fn path_hash() -> u64 {
    std::env::var("PATH")
        .map(|p| {
            let mut h = fshell_hash::FhashHasher::new();
            p.hash(&mut h);
            h.finish()
        })
        .unwrap_or(0)
}

fn resolve_binary_path(name: &str) -> Option<PathBuf> {
    for base in &["/usr/local/bin", "/opt/homebrew/bin", "/usr/bin", "/bin"] {
        let p = std::path::Path::new(base).join(name);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            let p = std::path::Path::new(dir).join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn binary_mtime(name: &str) -> Option<u64> {
    let cached_path = {
        let mut guard = RESOLVED_PATHS.lock().unwrap_or_else(|e| e.into_inner());
        let current_hash = path_hash();
        let should_clear = match guard.as_ref() {
            Some((old_hash, _)) => *old_hash != current_hash,
            None => false,
        };
        if should_clear {
            *guard = None;
        }
        if guard.is_none() {
            *guard = Some((current_hash, HashMap::new()));
        }
        guard.as_ref().and_then(|(_, map)| map.get(name).cloned())
    };

    if let Some(path) = cached_path {
        return stat_mtime(&path);
    }

    let path = resolve_binary_path(name)?;
    {
        let mut guard = RESOLVED_PATHS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, map)) = guard.as_mut() {
            map.entry(name.to_string()).or_insert(path.clone());
        }
    }

    stat_mtime(&path)
}

fn stat_mtime(path: &std::path::Path) -> Option<u64> {
    path.metadata()
        .ok()?
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

pub fn parse_help_output(text: &str) -> Vec<HelpFlag> {
    let mut flags = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let parsed = if trimmed.starts_with('[') {
            parse_bracket_notation(trimmed)
        } else if trimmed.starts_with('-') {
            parse_dash_notation(trimmed)
        } else {
            continue;
        };

        let Some(parsed) = parsed else { continue };

        flags.push(HelpFlag {
            short: parsed.short,
            long: parsed.long,
            has_arg: parsed.has_arg,
            desc: parsed.desc,
        });
    }

    if flags.is_empty() {
        for line in text.lines() {
            if let Some(flag) = find_bracket_anywhere(line.trim()) {
                flags.push(HelpFlag {
                    short: None,
                    long: Some(flag),
                    has_arg: false,
                    desc: None,
                });
            }
        }
    }

    if flags.is_empty() {
        for line in text.lines() {
            if let Some(flag) = find_flag_anywhere(line) {
                flags.push(HelpFlag {
                    short: None,
                    long: Some(flag),
                    has_arg: false,
                    desc: None,
                });
            }
        }
    }

    flags
}

fn parse_bracket_notation(s: &str) -> Option<ParsedFlag> {
    let close = match s.find(']') {
        Some(c) if c > 1 => c,
        _ => return None,
    };

    let inner = s[1..close].trim();
    if inner.is_empty() || !inner.contains('-') {
        return None;
    }

    let parts: Vec<&str> = inner.splitn(2, |c: char| c.is_whitespace()).collect();
    let flag_part = parts[0].trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-');

    let (short, long) = if flag_part.starts_with("--") {
        (
            None,
            Some(format!("--{}", flag_part.trim_start_matches("--"))),
        )
    } else if flag_part.starts_with('-') && flag_part.len() == 2 {
        (Some(flag_part.to_string()), None)
    } else {
        return None;
    };

    let has_arg = parts.len() > 1 && !parts[1].trim().is_empty();

    let desc = {
        let rest = s[close + 1..].trim();
        if rest.is_empty() || is_metavar_text(rest) {
            None
        } else {
            Some(rest.to_string())
        }
    };

    Some(ParsedFlag {
        short,
        long,
        has_arg,
        desc,
    })
}

struct ParsedFlag {
    short: Option<String>,
    long: Option<String>,
    has_arg: bool,
    desc: Option<String>,
}

fn parse_dash_notation(s: &str) -> Option<ParsedFlag> {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut i = 0;

    let mut dash_count = 0;
    while i < len && chars[i] == '-' {
        dash_count += 1;
        i += 1;
    }
    if dash_count == 0 {
        return None;
    }

    let mut short: Option<String> = None;
    let mut long: Option<String> = None;
    let mut has_arg = false;
    let mut parsed = false;

    if dash_count == 2 {
        let mut long_name = String::new();
        while i < len
            && !chars[i].is_whitespace()
            && chars[i] != '='
            && chars[i] != '<'
            && chars[i] != '['
        {
            long_name.push(chars[i]);
            i += 1;
        }
        if !long_name.is_empty() {
            if chars.get(i) == Some(&'=')
                || chars.get(i) == Some(&'<')
                || chars.get(i) == Some(&'[')
            {
                has_arg = true;
            }
            if has_arg {
                skip_past_word(&chars, &mut i, len);
            } else {
                let saved = i;
                skip_whitespace(&chars, &mut i, len);
                if i < len
                    && (chars[i] == '<' || chars[i] == '[' || is_uppercase_metavar(&chars, i, len))
                {
                    has_arg = true;
                    skip_past_word(&chars, &mut i, len);
                } else {
                    i = saved;
                }
            }
            let cleaned = long_name
                .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-')
                .to_string();
            long = Some(format!("--{cleaned}"));
            parsed = true;
        }
    } else if dash_count == 1 {
        let mut name = String::new();
        while i < len && !chars[i].is_whitespace() && chars[i] != ',' {
            name.push(chars[i]);
            i += 1;
        }
        if !name.is_empty() {
            if name.len() == 1 {
                short = Some(format!("-{}", name));
                skip_whitespace_comma(&chars, &mut i, len);
                if i + 1 < len && chars[i] == '-' && chars[i + 1] == '-' {
                    if let Some((l, arg, new_i)) = consume_long_from(&chars, i, len) {
                        long = l;
                        has_arg = arg;
                        i = new_i;
                    }
                } else if i < len
                    && (chars[i] == '<' || chars[i] == '[' || is_uppercase_metavar(&chars, i, len))
                {
                    has_arg = true;
                    skip_past_word(&chars, &mut i, len);
                }
                parsed = true;
            } else {
                let cleaned = name
                    .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-')
                    .to_string();
                long = Some(format!("-{cleaned}"));
                skip_whitespace(&chars, &mut i, len);
                if i < len
                    && (chars[i] == '<' || chars[i] == '[' || is_uppercase_metavar(&chars, i, len))
                {
                    has_arg = true;
                    skip_past_word(&chars, &mut i, len);
                }
                parsed = true;
            }
        }
    }

    if !parsed {
        return None;
    }

    let desc = if i < len {
        let rest = s[i..].trim();
        if rest.is_empty() || is_metavar_text(rest) {
            None
        } else {
            Some(rest.to_string())
        }
    } else {
        None
    };

    Some(ParsedFlag {
        short,
        long,
        has_arg,
        desc,
    })
}

fn skip_whitespace(chars: &[char], i: &mut usize, len: usize) {
    while *i < len && chars[*i].is_whitespace() {
        *i += 1;
    }
}

fn skip_whitespace_comma(chars: &[char], i: &mut usize, len: usize) {
    while *i < len && (chars[*i].is_whitespace() || chars[*i] == ',') {
        *i += 1;
    }
}

fn skip_past_word(chars: &[char], i: &mut usize, len: usize) {
    while *i < len && !chars[*i].is_whitespace() {
        *i += 1;
    }
}

fn is_uppercase_metavar(chars: &[char], start: usize, len: usize) -> bool {
    if start >= len {
        return false;
    }
    let mut count = 0;
    let mut pos = start;
    while pos < len
        && (chars[pos].is_uppercase() || chars[pos] == '_' || chars[pos].is_ascii_digit())
    {
        count += 1;
        pos += 1;
        if count > 10 {
            return false;
        }
    }
    count >= 2
        && (pos >= len || chars[pos].is_whitespace() || chars[pos] == ',' || chars[pos] == '.')
}

fn consume_long_from(
    chars: &[char],
    start: usize,
    len: usize,
) -> Option<(Option<String>, bool, usize)> {
    let mut i = start;
    let mut dc = 0;
    while i < len && chars[i] == '-' {
        dc += 1;
        i += 1;
    }
    if dc != 2 {
        return None;
    }
    let mut long_name = String::new();
    while i < len && !chars[i].is_whitespace() && chars[i] != '=' {
        long_name.push(chars[i]);
        i += 1;
    }
    if long_name.is_empty() {
        return None;
    }
    let has_arg = chars.get(i) == Some(&'=');
    let cleaned = long_name
        .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-')
        .to_string();
    Some((Some(format!("--{cleaned}")), has_arg, i))
}

fn is_metavar_text(s: &str) -> bool {
    let s = s.trim();
    if s.starts_with('<') || s.starts_with('[') {
        return true;
    }
    let first_word = s.split_whitespace().next().unwrap_or("");
    if first_word.len() >= 2
        && first_word.len() <= 8
        && first_word.chars().all(|c| c.is_uppercase() || c == '_')
    {
        return true;
    }
    false
}

fn find_bracket_anywhere(s: &str) -> Option<String> {
    if let Some(start) = s.find("[--") {
        let after = &s[start + 3..];
        let flag = after
            .split(|c: char| c == ']' || c.is_whitespace())
            .next()?;
        let cleaned = flag.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-');
        if !cleaned.is_empty() {
            return Some(format!("--{}", cleaned));
        }
    }
    if let Some(start) = s.find("[-") {
        let after = &s[start + 2..];
        let flag = after.split(']').next()?;
        if flag.len() == 1 {
            return Some(format!("-{}", flag));
        }
    }
    None
}

fn find_flag_anywhere(s: &str) -> Option<String> {
    let pos = s.find("--")?;
    let after = &s[pos + 2..];
    let flag = after.split_whitespace().next()?;
    if flag.is_empty() {
        return None;
    }
    let cleaned = flag.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-');
    if cleaned.is_empty() {
        return None;
    }
    Some(format!("--{}", cleaned))
}

pub fn get_completions(name: &str) -> Option<Vec<HelpFlag>> {
    let path = cache_path_for(name)?;
    if !path.exists() {
        queue_background_parse(name);
        return None;
    }

    let content = std::fs::read_to_string(&path).ok()?;
    let cached: CachedHelp = serde_json::from_str(&content).ok()?;

    if now_secs() > cached.fetched_at + 3600 {
        queue_background_parse(name);
        return None;
    }

    if let Some(mtime) = binary_mtime(name)
        && mtime > cached.fetched_at
    {
        queue_background_parse(name);
        return None;
    }

    Some(cached.flags)
}

pub fn queue_background_parse(name: &str) {
    {
        let mut debounce = BACKGROUND_PARSE_DEBOUNCE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some((prev_name, prev_time)) = &*debounce
            && prev_name == name
            && prev_time.elapsed() < Duration::from_millis(BACKGROUND_PARSE_DEBOUNCE_MS)
        {
            return;
        }
        *debounce = Some((name.to_string(), Instant::now()));
    }

    let tx = PARSE_TX.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<String>(32);
        std::thread::spawn(move || {
            while let Ok(name) = rx.recv() {
                process_parse_request(&name);
            }
        });
        tx
    });

    let _ = tx.try_send(name.to_string());
}

fn process_parse_request(name: &str) {
    let output = match std::process::Command::new(name)
        .arg("--help")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            let start = Instant::now();
            let status = loop {
                if start.elapsed() >= Duration::from_secs(5) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                match child.try_wait() {
                    Ok(Some(status)) => break status,
                    Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                    Err(_) => {
                        let _ = child.wait();
                        return;
                    }
                }
            };
            if !status.success() {
                let _ = child.wait();
                return;
            }
            child.wait_with_output().ok()
        }
        Err(_) => return,
    };

    let output = match output {
        Some(o) => o,
        None => return,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let flags = parse_help_output(&stdout);
    let flags = if flags.is_empty() {
        let alt_output = std::process::Command::new(name)
            .arg("-h")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();
        match alt_output {
            Ok(o) if o.status.success() => parse_help_output(&String::from_utf8_lossy(&o.stdout)),
            _ => flags,
        }
    } else {
        flags
    };

    if flags.is_empty() {
        return;
    }

    let cached = CachedHelp {
        command: name.to_string(),
        mtime: binary_mtime(name).unwrap_or(0),
        fetched_at: now_secs(),
        flags,
    };

    if let Some(path) = cache_path_for(name) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(content) = serde_json::to_string(&cached) {
            let tmp_path = path.with_extension("json.tmp");
            if std::fs::write(&tmp_path, &content).is_ok() {
                let _ = std::fs::rename(&tmp_path, &path);
            }
        }
    }
}
