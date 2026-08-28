// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::{Stmt, Val};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct ExternalCache {
    items: Vec<String>,
    loaded_at: Instant,
}
struct GitTagsCache {
    tags: Vec<String>,
    loaded_at: Instant,
    head_mtime: SystemTime,
}

static DOCKER_CONTAINERS_CACHE: Mutex<Option<ExternalCache>> = Mutex::new(None);
static DOCKER_IMAGES_CACHE: Mutex<Option<ExternalCache>> = Mutex::new(None);
static KUBE_PODS_CACHE: Mutex<Option<ExternalCache>> = Mutex::new(None);
static BREW_CACHE: Mutex<Option<ExternalCache>> = Mutex::new(None);
static GIT_TAGS_CACHE: Mutex<Option<GitTagsCache>> = Mutex::new(None);

static DOCKER_CONTAINERS_UPDATING: AtomicBool = AtomicBool::new(false);
static DOCKER_IMAGES_UPDATING: AtomicBool = AtomicBool::new(false);
static KUBE_PODS_UPDATING: AtomicBool = AtomicBool::new(false);
static BREW_UPDATING: AtomicBool = AtomicBool::new(false);
static GIT_TAGS_UPDATING: AtomicBool = AtomicBool::new(false);

const DOCKER_CACHE_TTL: Duration = Duration::from_secs(300);
const KUBE_CACHE_TTL: Duration = Duration::from_secs(300);
const BREW_CACHE_TTL: Duration = Duration::from_secs(300);

fn get_cached_external(
    cache: &'static Mutex<Option<ExternalCache>>,
    updating: &'static AtomicBool,
    ttl: Duration,
    loader: impl FnOnce() -> Vec<String> + Send + 'static,
) -> Vec<String> {
    let mut needs_update = false;
    let mut cached_val = None;
    if let Ok(guard) = cache.lock() {
        if let Some(ref c) = *guard {
            if c.loaded_at.elapsed() < ttl {
                return c.items.clone();
            }
            cached_val = Some(c.items.clone());
            needs_update = true;
        } else {
            needs_update = true;
        }
    }
    if needs_update
        && updating
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        std::thread::spawn(move || {
            let items = loader();
            if let Ok(mut guard) = cache.lock() {
                *guard = Some(ExternalCache {
                    items,
                    loaded_at: Instant::now(),
                });
            }
            updating.store(false, Ordering::SeqCst);
        });
    }
    cached_val.unwrap_or_default()
}

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

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Instant;

static BACKGROUND_PARSE_DEBOUNCE: Mutex<Option<(String, Instant)>> = Mutex::new(None);
const BACKGROUND_PARSE_DEBOUNCE_MS: u64 = 2000;

static PARSE_TX: std::sync::OnceLock<mpsc::SyncSender<String>> = std::sync::OnceLock::new();

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
    // Phase 1: cache lookup under the lock only — no I/O
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
        guard
            .as_mut()
            .expect("guard was just set to Some")
            .1
            .get(name)
            .cloned()
    };

    if let Some(path) = cached_path {
        // Cache hit — stat outside the lock
        return stat_mtime(&path);
    }

    // Phase 2: cache miss — resolve binary path (I/O outside the lock)
    let path = resolve_binary_path(name)?;

    // Phase 3: update cache under the lock (short)
    {
        let mut guard = RESOLVED_PATHS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, map)) = guard.as_mut() {
            map.entry(name.to_string()).or_insert(path.clone());
        }
    }

    // Phase 4: stat outside the lock
    stat_mtime(&path)
}

/// Stat the file and return its mtime in epoch seconds.
fn stat_mtime(path: &std::path::Path) -> Option<u64> {
    path.metadata()
        .ok()?
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

/// Parse --help output into flag suggestions using simple line scanning.
/// Handles GNU-style flags plus many common non-GNU formats:
/// `-f, --flag=<arg>`, `--flag <arg>`, `[--flag]`, `-flag`, `-f --flag`,
/// `--flag ARG`, `--flag [arg]`, `--flag=<VALUE>`.
/// Also extracts descriptions when possible.
fn parse_help_output(text: &str) -> Vec<HelpFlag> {
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

    // Lenient pass 1: bracket notation anywhere in a line
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

    // Lenient pass 2: any line containing `--`
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

/// Parse bracket notation: `[--flag]`, `[--flag ARG]`, `[-f]`, `[-f ARG]`.
/// Returns (short, long, has_arg, description).
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

/// Parse dash-prefixed flag lines. Handles GNU and non-GNU formats.
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
        // --flag, --flag=ARG, --flag <arg>, --flag [arg], --flag ARG
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
        // -f, --flag or -f --flag or -flag or -f <arg> or -f ARG
        let mut name = String::new();
        while i < len && !chars[i].is_whitespace() && chars[i] != ',' {
            name.push(chars[i]);
            i += 1;
        }
        if !name.is_empty() {
            if name.len() == 1 {
                // Classic short flag: -f
                short = Some(format!("-{}", name));
                skip_whitespace_comma(&chars, &mut i, len);
                // Check for --flag (short+long pair)
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
                // Multi-char single-dash: long flag with single dash (e.g. -flag)
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

    // Extract description from remaining text
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

// --- Helpers ---

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

/// Skip past a word token (non-whitespace characters).
fn skip_past_word(chars: &[char], i: &mut usize, len: usize) {
    while *i < len && !chars[*i].is_whitespace() {
        *i += 1;
    }
}

/// Check if the text at position `start` looks like an uppercase metavar token (e.g. FILE, PATH).
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

/// Consume a `--flag` token starting at `start`. Returns (long_flag, has_arg, new_position).
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

/// Check if text looks like a metavar: `<...>`, `[...]`, or a short all-uppercase word.
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

/// Find `[--flag]` or `[-f]` bracket notation anywhere in a line.
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

/// Find `--flag` within a line (last-resort lenient extraction).
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

/// Get cached completions for an external command.
/// Returns None if cache is missing, stale, or binary changed — and
/// triggers a background parse.
pub fn get_completions(name: &str) -> Option<Vec<HelpFlag>> {
    let path = cache_path_for(name)?;
    if !path.exists() {
        queue_background_parse(name);
        return None;
    }

    let content = std::fs::read_to_string(&path).ok()?;
    let cached: CachedHelp = serde_json::from_str(&content).ok()?;

    // 1-hour TTL
    if now_secs() > cached.fetched_at + 3600 {
        queue_background_parse(name);
        return None;
    }

    // Binary mtime changed (re-installed/updated)
    if let Some(mtime) = binary_mtime(name)
        && mtime > cached.fetched_at
    {
        queue_background_parse(name);
        return None;
    }

    Some(cached.flags)
}

/// Queue a background parse of --help output for the given command.
/// Debounces: skips if the same command was queued within 2s.
/// Uses a single worker thread with a bounded channel (cap 32).
/// If the channel is full, the request is silently dropped.
pub fn queue_background_parse(name: &str) {
    {
        let mut debounce = BACKGROUND_PARSE_DEBOUNCE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some((prev_name, prev_time)) = &*debounce
            && prev_name == name
            && prev_time.elapsed() < std::time::Duration::from_millis(BACKGROUND_PARSE_DEBOUNCE_MS)
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
            let start = std::time::Instant::now();
            let status = loop {
                if start.elapsed() >= std::time::Duration::from_secs(5) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                match child.try_wait() {
                    Ok(Some(status)) => break status,
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
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
        // Some CLIs only respond to the short form.
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

// Git branch/tag completions

/// Get git branches for the specified directory.
pub fn git_branches_for_path(pwd: &std::path::Path) -> Vec<String> {
    let repo = match fshell_git::repo::Repository::discover(pwd) {
        Ok(r) => r,
        Err(_) => return vec![],
    };

    repo.list_refs("refs/heads/")
        .into_iter()
        .map(|r| {
            r.name
                .strip_prefix("refs/heads/")
                .unwrap_or(&r.name)
                .to_string()
        })
        .filter(|l| !l.is_empty())
        .collect()
}

/// Get git branches for the current repository.
pub fn git_branches() -> Vec<String> {
    let pwd = std::env::current_dir().unwrap_or_default();
    git_branches_for_path(&pwd)
}

/// Get cached git branches (uses Env cache, invalidated on chpwd or .git/HEAD change).
pub fn git_branches_cached(env: &fshell_engine::Env) -> Vec<String> {
    let cwd = env.cwd();
    let head_path = cwd.join(".git/HEAD");
    // Fast path: check cache with read lock. Only stat .git/HEAD when TTL
    // has expired — avoids a syscall on every keystroke.
    {
        let cache = env.prompt.git_branch_cache.read();
        if let Some((time, branches, _head_mtime)) = cache.as_ref()
            && time.elapsed() < fshell_engine::GIT_CACHE_TTL
        {
            return branches.clone();
        }
        // TTL expired: validate against .git/HEAD mtime before falling through
        if let Some((_, branches, head_mtime)) = cache.as_ref()
            && let Ok(current_mtime) = std::fs::metadata(&head_path).and_then(|m| m.modified())
            && current_mtime == *head_mtime
        {
            // mtime unchanged — refresh TTL and return cached
            let branches = branches.clone();
            let mtime = *head_mtime;
            drop(cache);
            let mut cache = env.prompt.git_branch_cache.write();
            *cache = Some((std::time::Instant::now(), branches.clone(), mtime));
            return branches;
        }
    }
    // Cache miss: acquire write lock to populate
    let branches = git_branches_for_path(&cwd);
    let head_mtime = std::fs::metadata(&head_path)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    {
        let mut cache = env.prompt.git_branch_cache.write();
        *cache = Some((std::time::Instant::now(), branches.clone(), head_mtime));
    }
    branches
}

/// Get git tags for the current repository.
pub fn git_tags() -> Vec<String> {
    let output = std::process::Command::new("git")
        .args(["tag", "--sort=-version:refname"])
        .output();
    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .take(50)
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        _ => vec![],
    }
}

pub fn git_tags_cached() -> Vec<String> {
    let mut needs_update = false;
    let mut cached_val = None;

    if let Ok(cache) = GIT_TAGS_CACHE.lock() {
        if let Some(ref c) = *cache {
            let matches_mtime = std::fs::metadata(".git/HEAD")
                .and_then(|m| m.modified())
                .map(|current_mtime| current_mtime == c.head_mtime)
                .unwrap_or(false);
            if matches_mtime && c.loaded_at.elapsed() < Duration::from_secs(30) {
                return c.tags.clone();
            } else {
                cached_val = Some(c.tags.clone());
                needs_update = true;
            }
        } else {
            needs_update = true;
        }
    }

    if needs_update
        && GIT_TAGS_UPDATING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        std::thread::spawn(move || {
            let tags = git_tags();
            let head_mtime = std::fs::metadata(".git/HEAD")
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            if let Ok(mut cache) = GIT_TAGS_CACHE.lock() {
                *cache = Some(GitTagsCache {
                    tags,
                    loaded_at: Instant::now(),
                    head_mtime,
                });
            }
            GIT_TAGS_UPDATING.store(false, Ordering::SeqCst);
        });
    }

    cached_val.unwrap_or_default()
}

/// Detect if we're completing a git subcommand that expects a branch/tag.
pub fn git_branch_context(words: &[&str]) -> bool {
    if words.len() < 2 {
        return false;
    }
    matches!(
        (words[0], words[1]),
        (
            "git",
            "checkout" | "switch" | "merge" | "rebase" | "branch" | "push" | "pull"
        )
    )
}

fn evaluate_fsh_completions_sync(expr_str: &str, env: &fshell_engine::Env) -> Vec<String> {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        tokio::task::block_in_place(|| {
            handle.block_on(async {
                let mut parser = fshell_core::Parser::new(expr_str);
                if let Ok(stmts) = parser.parse_statements() {
                    let mut last_results = Vec::new();
                    for stmt in stmts {
                        if let Stmt::Expr(expr) = stmt {
                            if let Ok(res) = fshell_engine::eval_expr(&expr, env).await {
                                match res {
                                    Val::List(list) => {
                                        last_results =
                                            list.into_iter().map(|v| v.to_text()).collect();
                                    }
                                    Val::String(s) => {
                                        last_results =
                                            s.split_whitespace().map(|s| s.to_string()).collect();
                                    }
                                    other => {
                                        last_results = vec![other.to_text()];
                                    }
                                }
                            }
                        } else {
                            let _ = fshell_engine::eval_stmt(&stmt, env, false).await;
                        }
                    }
                    last_results
                } else {
                    vec![]
                }
            })
        })
    } else {
        vec![]
    }
}

fn resolve_choices(choices_str: &str, env: &fshell_engine::Env) -> Vec<String> {
    let choices_trimmed = choices_str.trim();
    if choices_trimmed.starts_with('(') && choices_trimmed.ends_with(')') {
        let inner = &choices_trimmed[1..choices_trimmed.len() - 1];
        evaluate_fsh_completions_sync(inner, env)
    } else {
        let sep = if choices_trimmed.contains(',') {
            ','
        } else {
            ' '
        };
        choices_trimmed
            .split(sep)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

pub fn get_registry_completions(
    line: &str,
    pos: usize,
    env: &fshell_engine::Env,
) -> Option<Vec<reedline::Suggestion>> {
    let completions_guard = env.completions.read();
    let pos = pos.min(line.len());
    let pos = if line.is_char_boundary(pos) {
        pos
    } else {
        line.floor_char_boundary(pos)
    };
    let prefix = &line[..pos];
    let words: Vec<&str> = prefix.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let cmd = words[0];
    let comp = completions_guard.get(cmd)?;

    let last_word = if prefix.ends_with(' ') {
        ""
    } else {
        words.last().copied().unwrap_or("")
    };
    let starting_new_arg = prefix.ends_with(' ');
    let filter = last_word.to_lowercase();

    // Determine current subcommands context
    let mut typed_subcmds = Vec::new();
    let words_after_cmd = &words[1..];
    if starting_new_arg {
        for w in words_after_cmd {
            if !w.starts_with('-') {
                typed_subcmds.push(w.to_string());
            }
        }
    } else {
        if words_after_cmd.len() > 1 {
            for w in &words_after_cmd[..words_after_cmd.len() - 1] {
                if !w.starts_with('-') {
                    typed_subcmds.push(w.to_string());
                }
            }
        }
    }

    let mut suggestions = Vec::new();

    // 1. Check if we are completing flag choices/arguments
    let prev_word = if starting_new_arg {
        words.last().copied()
    } else if words.len() > 1 {
        Some(words[words.len() - 2])
    } else {
        None
    };

    if let Some(pw) = prev_word
        && pw.starts_with('-')
    {
        let flag_name = pw.trim_start_matches('-');
        for flag in &comp.flags {
            if flag.parent_subcmds == typed_subcmds
                && (flag.short.as_deref() == Some(flag_name)
                    || flag.long.as_deref() == Some(flag_name))
                && let Some(ref choices_str) = flag.choices
            {
                let choices = resolve_choices(choices_str, env);
                for val in choices {
                    if val.to_lowercase().starts_with(&filter) {
                        suggestions.push(reedline::Suggestion {
                            value: val.clone(),
                            description: None,
                            extra: None,
                            span: reedline::Span::new(pos - last_word.len(), pos),
                            append_whitespace: true,
                            style: None,
                            display_override: None,
                            match_indices: None,
                        });
                    }
                }
                return Some(suggestions);
            }
        }
    }

    // 2. Add subcommand suggestions
    for sub in &comp.subcommands {
        if sub.parent_subcmds == typed_subcmds && sub.name.to_lowercase().starts_with(&filter) {
            suggestions.push(reedline::Suggestion {
                value: sub.name.clone(),
                description: sub.desc.clone().map(|d| format!("[{}] {}", cmd, d)),
                extra: None,
                span: reedline::Span::new(pos - last_word.len(), pos),
                append_whitespace: true,
                style: None,
                display_override: None,
                match_indices: None,
            });
        }
    }

    // 3. Add flag suggestions
    for flag in &comp.flags {
        if flag.parent_subcmds == typed_subcmds {
            if let Some(ref s) = flag.short {
                let s_with_dash = format!("-{}", s);
                if s_with_dash.to_lowercase().starts_with(&filter) {
                    suggestions.push(reedline::Suggestion {
                        value: s_with_dash,
                        description: flag.desc.clone().map(|d| format!("[flag] {}", d)),
                        extra: None,
                        span: reedline::Span::new(pos - last_word.len(), pos),
                        append_whitespace: true,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    });
                }
            }
            if let Some(ref l) = flag.long {
                let l_with_dash = format!("--{}", l);
                if l_with_dash.to_lowercase().starts_with(&filter) {
                    suggestions.push(reedline::Suggestion {
                        value: l_with_dash,
                        description: flag.desc.clone().map(|d| format!("[flag] {}", d)),
                        extra: None,
                        span: reedline::Span::new(pos - last_word.len(), pos),
                        append_whitespace: true,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    });
                }
            }
        }
    }

    // 4. Add dynamic provider / wordlist suggestions
    for prov in &comp.dynamic_providers {
        if prov.parent_subcmds == typed_subcmds {
            let choices = resolve_choices(&prov.command, env);
            for val in choices {
                if val.to_lowercase().starts_with(&filter) {
                    suggestions.push(reedline::Suggestion {
                        value: val.clone(),
                        description: Some(format!("[{}] dynamic", cmd)),
                        extra: None,
                        span: reedline::Span::new(pos - last_word.len(), pos),
                        append_whitespace: true,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    });
                }
            }
        }
    }

    if suggestions.is_empty() {
        None
    } else {
        Some(suggestions)
    }
}

/// Get custom completions for common external commands (git, cargo, npm, docker, kubectl, ssh, make, rustup, brew).
pub fn get_custom_completions(
    line: &str,
    pos: usize,
    env: &fshell_engine::Env,
) -> Option<Vec<reedline::Suggestion>> {
    if let Some(reg_suggs) = get_registry_completions(line, pos, env) {
        return Some(reg_suggs);
    }
    let pos = pos.min(line.len());
    let pos = if line.is_char_boundary(pos) {
        pos
    } else {
        line.floor_char_boundary(pos)
    };
    let prefix = &line[..pos];
    let words: Vec<&str> = prefix.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }

    let cmd = words[0];
    let last_word = if prefix.ends_with(' ') {
        ""
    } else {
        words.last().copied().unwrap_or("")
    };
    let starting_new_arg = prefix.ends_with(' ');

    let supported = [
        "git", "cargo", "npm", "docker", "kubectl", "ssh", "make", "rustup", "brew",
    ];
    if !supported.contains(&cmd) {
        return None;
    }

    let filter = last_word.to_lowercase();
    let mut suggestions = Vec::new();

    let add_suggestions = |items: Vec<(&str, &str)>,
                           suggestions: &mut Vec<reedline::Suggestion>| {
        for (val, desc) in items {
            if val.to_lowercase().starts_with(&filter) {
                suggestions.push(reedline::Suggestion {
                    value: val.to_string(),
                    description: Some(format!("[{}] {}", cmd, desc)),
                    extra: None,
                    span: reedline::Span::new(pos - last_word.len(), pos),
                    append_whitespace: true,
                    style: None,
                    display_override: None,
                    match_indices: None,
                });
            }
        }
    };

    let add_dynamic_suggestions =
        |items: Vec<String>, desc: &str, suggestions: &mut Vec<reedline::Suggestion>| {
            for val in items {
                if val.to_lowercase().starts_with(&filter) {
                    suggestions.push(reedline::Suggestion {
                        value: val.clone(),
                        description: Some(format!("[{}] {}", cmd, desc)),
                        extra: None,
                        span: reedline::Span::new(pos - last_word.len(), pos),
                        append_whitespace: true,
                        style: None,
                        display_override: None,
                        match_indices: None,
                    });
                }
            }
        };

    match cmd {
        "git" => {
            if words.len() == 1 || (words.len() == 2 && !starting_new_arg) {
                let subcmds = vec![
                    ("checkout", "Switch branches or restore working tree files"),
                    ("switch", "Switch branches"),
                    ("status", "Show the working tree status"),
                    ("log", "Show commit logs"),
                    ("branch", "List, create, or delete branches"),
                    (
                        "diff",
                        "Show changes between commits, commit and working tree, etc",
                    ),
                    ("commit", "Record changes to the repository"),
                    ("push", "Update remote refs along with associated objects"),
                    (
                        "pull",
                        "Fetch from and integrate with another repository or a local branch",
                    ),
                    ("fetch", "Download objects and refs from another repository"),
                    ("add", "Add file contents to the index"),
                    (
                        "rm",
                        "Remove files from the working tree and from the index",
                    ),
                    ("clone", "Clone a repository into a new directory"),
                    (
                        "init",
                        "Create an empty Git repository or reinitialize an existing one",
                    ),
                    ("rebase", "Reapply commits on top of another base tip"),
                    ("merge", "Join two or more development histories together"),
                    ("reset", "Reset current HEAD to the specified state"),
                    (
                        "stash",
                        "Stash the changes in a dirty working directory away",
                    ),
                    ("remote", "Manage set of tracked repositories"),
                ];
                add_suggestions(subcmds, &mut suggestions);
            } else if words.len() >= 2 {
                let sub = words[1];
                if ["checkout", "switch", "merge", "rebase", "branch"].contains(&sub) {
                    let branches = git_branches_cached(env);
                    let tags = git_tags_cached();
                    add_dynamic_suggestions(branches, "git branch", &mut suggestions);
                    add_dynamic_suggestions(tags, "git tag", &mut suggestions);
                } else if sub == "log" {
                    let flags = vec![
                        ("--oneline", "Commit show on one line"),
                        (
                            "--graph",
                            "Draw text-based graphical representation of commit history",
                        ),
                        ("--all", "Show all commits in history"),
                        ("--decorate", "Show ref names of commits"),
                        ("--stat", "Show stat of changed files"),
                    ];
                    add_suggestions(flags, &mut suggestions);
                } else if ["add", "rm", "diff", "status", "restore", "reset"].contains(&sub) {
                    return None; // Fallback to standard file autocomplete
                }
            }
        }
        "cargo" => {
            if words.len() == 1 || (words.len() == 2 && !starting_new_arg) {
                let subcmds = vec![
                    ("build", "Compile the current package"),
                    ("test", "Run the tests"),
                    ("run", "Run a binary or example of the local package"),
                    (
                        "clippy",
                        "Checks a package to catch common mistakes and improve your Rust code",
                    ),
                    (
                        "fmt",
                        "Formats all bin and lib files of the current package",
                    ),
                    (
                        "check",
                        "Analyze the current package and report errors, but don't build object files",
                    ),
                    ("bench", "Execute all benchmarks of a local package"),
                    (
                        "doc",
                        "Build this package's and its dependencies' documentation",
                    ),
                    ("new", "Create a new cargo package"),
                    (
                        "init",
                        "Create a new cargo package in an existing directory",
                    ),
                    (
                        "update",
                        "Update dependencies as recorded in the local Lock file",
                    ),
                    (
                        "clean",
                        "Remove artifacts that cargo has generated in the past",
                    ),
                ];
                add_suggestions(subcmds, &mut suggestions);
            } else if words.len() >= 2 {
                let sub = words[1];
                if ["build", "check", "clippy", "rustc", "test", "bench", "run"].contains(&sub) {
                    let flags = vec![
                        (
                            "--release",
                            "Build artifacts in release mode, with optimizations",
                        ),
                        ("--workspace", "Build all packages in the workspace"),
                        (
                            "--all-targets",
                            "Equivalent to specifying --lib --bins --tests --benches --examples",
                        ),
                        ("--lib", "Build only this package's library"),
                        ("--verbose", "Use verbose output"),
                        ("-v", "Use verbose output"),
                    ];
                    add_suggestions(flags, &mut suggestions);
                }
            }
        }
        "npm" => {
            if words.len() == 1 || (words.len() == 2 && !starting_new_arg) {
                let subcmds = vec![
                    ("install", "Install package dependencies"),
                    ("run", "Run a package script"),
                    ("test", "Run package tests"),
                    ("start", "Start the package application"),
                    ("stop", "Stop the package application"),
                    ("uninstall", "Uninstall a package"),
                    ("update", "Update packages"),
                    ("init", "Initialize a package.json file"),
                    ("publish", "Publish package to registry"),
                ];
                add_suggestions(subcmds, &mut suggestions);
            } else if words.len() >= 2 {
                let sub = words[1];
                if sub == "run" {
                    let scripts = npm_scripts();
                    add_dynamic_suggestions(scripts, "npm script", &mut suggestions);
                }
            }
        }
        "docker" => {
            if words.len() == 1 || (words.len() == 2 && !starting_new_arg) {
                let subcmds = vec![
                    ("ps", "List containers"),
                    ("images", "List images"),
                    ("run", "Create and run a new container from an image"),
                    ("exec", "Execute a command in a running container"),
                    ("stop", "Stop one or more running containers"),
                    ("start", "Start one or more stopped containers"),
                    ("restart", "Restart one or more containers"),
                    ("rm", "Remove one or more containers"),
                    ("rmi", "Remove one or more images"),
                    ("build", "Build an image from a Dockerfile"),
                    ("pull", "Download an image from a registry"),
                    ("push", "Upload an image to a registry"),
                    ("logs", "Fetch the logs of a container"),
                    ("compose", "Docker Compose CLI"),
                ];
                add_suggestions(subcmds, &mut suggestions);
            } else if words.len() >= 2 {
                let sub = words[1];
                if ["exec", "stop", "start", "restart", "logs", "rm"].contains(&sub) {
                    let containers = docker_containers();
                    add_dynamic_suggestions(containers, "docker container", &mut suggestions);
                } else if sub == "rmi" {
                    let images = docker_images();
                    add_dynamic_suggestions(images, "docker image", &mut suggestions);
                } else if sub == "compose" {
                    let compose_subcmds = vec![
                        ("up", "Create and start containers"),
                        ("down", "Stop and remove containers, networks"),
                        ("ps", "List containers"),
                        ("logs", "View output from containers"),
                        ("build", "Build or rebuild services"),
                        ("exec", "Execute a command in a running service container"),
                        ("restart", "Restart service containers"),
                    ];
                    add_suggestions(compose_subcmds, &mut suggestions);
                }
            }
        }
        "kubectl" => {
            if words.len() == 1 || (words.len() == 2 && !starting_new_arg) {
                let subcmds = vec![
                    ("get", "Display one or many resources"),
                    (
                        "describe",
                        "Show details of a specific resource or group of resources",
                    ),
                    ("logs", "Print the logs for a container in a pod"),
                    ("exec", "Execute a command in a container"),
                    ("apply", "Apply a configuration to a resource"),
                    ("delete", "Delete resources"),
                    ("edit", "Edit a resource on the server"),
                ];
                add_suggestions(subcmds, &mut suggestions);
            } else if words.len() >= 2 {
                let sub = words[1];
                if ["get", "describe", "delete", "edit"].contains(&sub) {
                    let resources = vec![
                        ("pods", "Kubernetes Pods"),
                        ("services", "Kubernetes Services"),
                        ("deployments", "Kubernetes Deployments"),
                        ("statefulsets", "Kubernetes StatefulSets"),
                        ("configmaps", "Kubernetes ConfigMaps"),
                        ("secrets", "Kubernetes Secrets"),
                        ("namespaces", "Kubernetes Namespaces"),
                        ("ingresses", "Kubernetes Ingresses"),
                        ("nodes", "Kubernetes Nodes"),
                    ];
                    add_suggestions(resources, &mut suggestions);
                } else if ["logs", "exec"].contains(&sub) {
                    let pods = kube_pods();
                    add_dynamic_suggestions(pods, "k8s pod", &mut suggestions);
                }
            }
        }
        "ssh" => {
            let hosts = ssh_hosts();
            add_dynamic_suggestions(hosts, "ssh host", &mut suggestions);
        }
        "make" => {
            let targets = make_targets();
            add_dynamic_suggestions(targets, "makefile target", &mut suggestions);
        }
        "rustup" => {
            if words.len() == 1 || (words.len() == 2 && !starting_new_arg) {
                let subcmds = vec![
                    ("show", "Show active toolchains"),
                    ("update", "Update toolchains"),
                    ("default", "Set default toolchain"),
                    ("toolchain", "Manage toolchains"),
                    ("target", "Manage targets"),
                    ("component", "Manage components"),
                    ("override", "Manage overrides"),
                    ("run", "Run command in toolchain env"),
                    ("which", "Resolve toolchain binary"),
                    ("doc", "View rust documentation"),
                ];
                add_suggestions(subcmds, &mut suggestions);
            } else if words.len() >= 2 {
                let sub = words[1];
                if sub == "toolchain" {
                    let subsub = vec![
                        ("list", "List toolchains"),
                        ("install", "Install toolchain"),
                        ("uninstall", "Uninstall toolchain"),
                        ("link", "Link custom toolchain"),
                    ];
                    add_suggestions(subsub, &mut suggestions);
                } else if ["target", "component"].contains(&sub) {
                    let subsub = vec![
                        ("list", "List items"),
                        ("add", "Add item"),
                        ("remove", "Remove item"),
                    ];
                    add_suggestions(subsub, &mut suggestions);
                }
            }
        }
        "brew" => {
            if words.len() == 1 || (words.len() == 2 && !starting_new_arg) {
                let subcmds = vec![
                    ("install", "Install formula or cask"),
                    ("uninstall", "Uninstall formula or cask"),
                    ("update", "Update Homebrew and packages"),
                    ("upgrade", "Upgrade packages"),
                    ("list", "List packages"),
                    ("info", "Display package info"),
                    ("search", "Search packages"),
                    ("cleanup", "Clean stale lockfiles and downloads"),
                    ("doctor", "Diagnose system health"),
                ];
                add_suggestions(subcmds, &mut suggestions);
            } else if words.len() >= 2 {
                let sub = words[1];
                if ["uninstall", "upgrade", "info"].contains(&sub) {
                    let formulas = brew_installed();
                    add_dynamic_suggestions(formulas, "installed package", &mut suggestions);
                }
            }
        }
        _ => {}
    }

    Some(suggestions)
}

fn npm_scripts() -> Vec<String> {
    let mut path = std::env::current_dir().unwrap_or_default();
    for _ in 0..5 {
        let pkg = path.join("package.json");
        if pkg.exists() {
            if let Ok(content) = std::fs::read_to_string(&pkg)
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&content)
                && let Some(scripts) = json.get("scripts").and_then(|s| s.as_object())
            {
                return scripts.keys().cloned().collect();
            }
            break;
        }
        if !path.pop() {
            break;
        }
    }
    vec![]
}

fn docker_containers() -> Vec<String> {
    get_cached_external(
        &DOCKER_CONTAINERS_CACHE,
        &DOCKER_CONTAINERS_UPDATING,
        DOCKER_CACHE_TTL,
        || {
            let output = std::process::Command::new("docker")
                .args(["ps", "-a", "--format", "{{.Names}}"])
                .output();
            match output {
                Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect(),
                _ => vec![],
            }
        },
    )
}

fn docker_images() -> Vec<String> {
    get_cached_external(
        &DOCKER_IMAGES_CACHE,
        &DOCKER_IMAGES_UPDATING,
        DOCKER_CACHE_TTL,
        || {
            let output = std::process::Command::new("docker")
                .args(["images", "--format", "{{.Repository}}:{{.Tag}}"])
                .output();
            match output {
                Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty() && l != "<none>:<none>")
                    .collect(),
                _ => vec![],
            }
        },
    )
}

fn kube_pods() -> Vec<String> {
    get_cached_external(
        &KUBE_PODS_CACHE,
        &KUBE_PODS_UPDATING,
        KUBE_CACHE_TTL,
        || {
            let output = std::process::Command::new("kubectl")
                .args(["get", "pods", "-o", "jsonpath={.items[*].metadata.name}"])
                .output();
            match output {
                Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
                _ => vec![],
            }
        },
    )
}

pub fn prewarm_completions() {
    if fshell_engine::is_external_command_cached("docker", None) {
        docker_containers();
        docker_images();
    }
    if fshell_engine::is_external_command_cached("kubectl", None) {
        kube_pods();
    }
    if fshell_engine::is_external_command_cached("brew", None) {
        brew_installed();
    }
}

fn ssh_hosts() -> Vec<String> {
    let mut hosts = Vec::new();
    let home = std::env::var("HOME").unwrap_or_default();

    // Parse ~/.ssh/config
    let config_path = std::path::Path::new(&home).join(".ssh/config");
    if config_path.exists()
        && let Ok(content) = std::fs::read_to_string(&config_path)
    {
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(host_line) = trimmed.strip_prefix("Host ") {
                let host = host_line.trim();
                if !host.contains('*') && !host.contains('?') {
                    hosts.push(host.to_string());
                }
            }
        }
    }

    // Parse ~/.ssh/known_hosts
    let kh_path = std::path::Path::new(&home).join(".ssh/known_hosts");
    if kh_path.exists()
        && let Ok(content) = std::fs::read_to_string(&kh_path)
    {
        for line in content.lines() {
            if let Some(host_part) = line.split_whitespace().next() {
                let host = host_part.split(',').next().unwrap_or(host_part);
                if !host.starts_with('|') && !host.is_empty() {
                    hosts.push(host.to_string());
                }
            }
        }
    }

    hosts.sort();
    hosts.dedup();
    hosts
}

fn make_targets() -> Vec<String> {
    let mut targets = Vec::new();
    for name in &["Makefile", "makefile"] {
        let path = std::path::Path::new(name);
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(path) {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with('#') || trimmed.starts_with('.') {
                        continue;
                    }
                    if let Some((target_part, _)) = trimmed.split_once(':') {
                        let target = target_part.trim();
                        if !target.is_empty()
                            && !target.contains('=')
                            && !target.contains('$')
                            && !target.contains('/')
                        {
                            targets.push(target.to_string());
                        }
                    }
                }
            }
            break;
        }
    }
    targets
}

fn brew_installed() -> Vec<String> {
    get_cached_external(&BREW_CACHE, &BREW_UPDATING, BREW_CACHE_TTL, || {
        let output = std::process::Command::new("brew")
            .args(["list", "--formula"])
            .output();
        let mut formulas = match output {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect(),
            _ => vec![],
        };
        let output_cask = std::process::Command::new("brew")
            .args(["list", "--cask"])
            .output();
        if let Ok(out) = output_cask
            && out.status.success()
        {
            formulas.extend(
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty()),
            );
        }
        formulas.sort();
        formulas.dedup();
        formulas
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_branch_context() {
        assert!(git_branch_context(&["git", "checkout"]));
        assert!(git_branch_context(&["git", "switch"]));
        assert!(!git_branch_context(&["git", "status"]));
        assert!(!git_branch_context(&["ls"]));
    }

    #[test]
    fn test_git_branches() {
        let branches = git_branches();
        let _ = branches; // Should not panic
    }

    #[tokio::test]
    async fn test_custom_completions() {
        let env = fshell_engine::Env::new();

        // 1. git checkout subcommand completions
        let suggs = get_custom_completions("git ch", 6, &env).unwrap();
        assert!(!suggs.is_empty());
        assert_eq!(suggs[0].value, "checkout");

        // 2. cargo build subcommand completions
        let suggs = get_custom_completions("cargo bu", 8, &env).unwrap();
        assert!(!suggs.is_empty());
        assert_eq!(suggs[0].value, "build");

        // 3. git checkout branch completions
        let suggs = get_custom_completions("git checkout ", 13, &env).unwrap();
        let _ = suggs;

        // 4. Unsupported command returns None
        let suggs = get_custom_completions("ls -l", 5, &env);
        assert!(suggs.is_none());
    }
}
