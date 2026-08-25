// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::prompt_config::{
    ColorSpec, PromptConfig, SegmentConfig, SegmentType, SeparatorStyle,
};
use fshell_git::repo::Repository;
use nu_ansi_term::{Color, Style};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

struct GitStatusCacheEntry {
    pwd: String,
    status: Option<RichGitStatus>,
    head_mtime: Option<std::time::SystemTime>,
    index_mtime: Option<std::time::SystemTime>,
}

static GIT_STATUS_CACHE: Mutex<Option<GitStatusCacheEntry>> = Mutex::new(None);

pub fn clear_git_status_cache() {
    if let Ok(mut cache) = GIT_STATUS_CACHE.lock() {
        *cache = None;
    }
}

struct CachedSegment {
    value: String,
    cached_at: Instant,
}

struct CustomSegmentCacheEntry {
    value: Option<String>,
    loaded_at: Instant,
}

const KUBE_CACHE_TTL: Duration = Duration::from_secs(30);
const HOST_CACHE_TTL: Duration = Duration::from_secs(300);
const CUSTOM_CACHE_TTL: Duration = Duration::from_secs(10);

static SEGMENT_CACHE: Mutex<Option<CachedSegment>> = Mutex::new(None);
static SEGMENT_HOST_CACHE: Mutex<Option<CachedSegment>> = Mutex::new(None);
static CUSTOM_SEGMENT_CACHE: Mutex<Option<HashMap<String, CustomSegmentCacheEntry>>> =
    Mutex::new(None);

static KUBE_UPDATING: AtomicBool = AtomicBool::new(false);
static HOST_UPDATING: AtomicBool = AtomicBool::new(false);
static CUSTOM_SEGMENT_UPDATING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

pub fn clear_segment_cache() {
    if let Ok(mut cache) = SEGMENT_CACHE.lock() {
        *cache = None;
    }
    if let Ok(mut cache) = SEGMENT_HOST_CACHE.lock() {
        *cache = None;
    }
    if let Ok(mut cache) = CUSTOM_SEGMENT_CACHE.lock() {
        *cache = None;
    }
    if let Ok(mut updating) = CUSTOM_SEGMENT_UPDATING.lock() {
        *updating = None;
    }
}

pub struct PromptSegment {
    pub content: String,
    pub fg: Option<Color>,
    pub bold: bool,
    pub italic: bool,
}

#[derive(Clone)]
pub struct RichGitStatus {
    pub branch: String,
    pub ahead: usize,
    pub behind: usize,
    pub modified: usize,
    pub untracked: usize,
    pub clean: bool,
}

static GIT_UPDATING: AtomicBool = AtomicBool::new(false);

pub fn get_rich_git_status(pwd: &str) -> Option<RichGitStatus> {
    let _timer = std::time::Instant::now();
    let mut git_dir = None;
    let mut path = std::path::Path::new(pwd);
    loop {
        let candidate = path.join(".git");
        if candidate.is_dir() {
            git_dir = Some(candidate);
            break;
        }
        match path.parent() {
            Some(parent) => path = parent,
            None => break,
        }
    }

    let git_dir = match git_dir {
        Some(d) => d,
        None => {
            if let Ok(mut cache) = GIT_STATUS_CACHE.lock() {
                *cache = None;
            }
            let elapsed = _timer.elapsed();
            if elapsed > std::time::Duration::from_millis(1)
                && std::env::var("FSH_DBG_CPU_USG").as_deref() == Ok("1")
            {
                eprintln!(
                    "[cpu_dbg] [prompt] get_rich_git_status: no git dir, took {:?} for pwd={:?}",
                    elapsed, pwd
                );
            }
            return None;
        }
    };

    let head_mtime = std::fs::metadata(git_dir.join("HEAD"))
        .ok()
        .and_then(|m| m.modified().ok());
    let index_mtime = std::fs::metadata(git_dir.join("index"))
        .ok()
        .and_then(|m| m.modified().ok());

    // Fast path: cache hit with matching mtimes
    if let Ok(cache) = GIT_STATUS_CACHE.lock()
        && let Some(ref entry) = *cache
        && entry.pwd == pwd
        && entry.head_mtime == head_mtime
        && entry.index_mtime == index_mtime
    {
        let elapsed = _timer.elapsed();
        if elapsed > std::time::Duration::from_millis(1)
            && std::env::var("FSH_DBG_CPU_USG").as_deref() == Ok("1")
        {
            eprintln!(
                "[cpu_dbg] [prompt] get_rich_git_status: cache HIT, took {:?}",
                elapsed
            );
        }
        return entry.status.clone();
    }

    // Cache miss: read stale status if available
    let stale_status = GIT_STATUS_CACHE
        .lock()
        .ok()
        .and_then(|cache| cache.as_ref().map(|e| e.status.clone()))
        .flatten();

    // Trigger non-blocking async update if not currently fetching
    if !GIT_UPDATING.swap(true, Ordering::SeqCst) {
        let pwd_owned = pwd.to_string();
        std::thread::spawn(move || {
            let result = get_rich_git_status_uncached(&pwd_owned);
            if let Ok(mut cache) = GIT_STATUS_CACHE.lock() {
                *cache = Some(GitStatusCacheEntry {
                    pwd: pwd_owned,
                    status: result,
                    head_mtime,
                    index_mtime,
                });
            }
            GIT_UPDATING.store(false, Ordering::SeqCst);
        });
    }

    if let Some(status) = stale_status {
        return Some(status);
    }

    // First load in session: compute synchronously once
    let result = get_rich_git_status_uncached(pwd);
    if let Ok(mut cache) = GIT_STATUS_CACHE.lock() {
        *cache = Some(GitStatusCacheEntry {
            pwd: pwd.to_string(),
            status: result.clone(),
            head_mtime,
            index_mtime,
        });
    }
    result
}

fn get_rich_git_status_uncached(pwd: &str) -> Option<RichGitStatus> {
    let _timer = std::time::Instant::now();
    let repo = Repository::discover(std::path::Path::new(pwd)).ok()?;
    let discover_elapsed = _timer.elapsed();
    if discover_elapsed > std::time::Duration::from_millis(5)
        && std::env::var("FSH_DBG_CPU_USG").as_deref() == Ok("1")
    {
        eprintln!(
            "[cpu_dbg] [prompt] get_rich_git_status_uncached: discover took {:?} for pwd={:?}",
            discover_elapsed, pwd
        );
    }

    let head = repo.head().ok()?;
    let branch = head
        .branch
        .clone()
        .unwrap_or_else(|| format!("{}...", &hex::encode(head.oid)[..7]));

    let (ahead, behind) = repo.ahead_behind().unwrap_or((0, 0));

    let status_timer = std::time::Instant::now();
    let statuses = repo.status().unwrap_or_default();
    let status_elapsed = status_timer.elapsed();
    if status_elapsed > std::time::Duration::from_millis(5)
        && std::env::var("FSH_DBG_CPU_USG").as_deref() == Ok("1")
    {
        eprintln!(
            "[cpu_dbg] [prompt] get_rich_git_status_uncached: repo.status() took {:?}, found {} entries",
            status_elapsed,
            statuses.len()
        );
    }
    let mut modified = 0u32;
    let mut untracked = 0u32;
    for status in statuses.values() {
        match status {
            fshell_git::status::Status::Modified => modified += 1,
            fshell_git::status::Status::Added => untracked += 1,
            fshell_git::status::Status::Deleted => modified += 1,
            fshell_git::status::Status::TypeChange => modified += 1,
            fshell_git::status::Status::Conflicted => modified += 1,
            _ => {}
        }
    }

    let clean = modified == 0 && untracked == 0;
    let total_elapsed = _timer.elapsed();
    if total_elapsed > std::time::Duration::from_millis(10)
        && std::env::var("FSH_DBG_CPU_USG").as_deref() == Ok("1")
    {
        eprintln!(
            "[cpu_dbg] [prompt] get_rich_git_status_uncached: TOTAL {:?} for pwd={:?} branch={}",
            total_elapsed, pwd, branch
        );
    }

    Some(RichGitStatus {
        branch,
        ahead: ahead as usize,
        behind: behind as usize,
        modified: modified as usize,
        untracked: untracked as usize,
        clean,
    })
}

fn resolve_color(
    spec: &Option<ColorSpec>,
    exit_ok: bool,
    theme: &fshell_core::theme::Theme,
) -> Option<nu_ansi_term::Color> {
    spec.as_ref().map(|s| {
        let (r, g, b) = s.resolve_to_rgb(exit_ok, theme);
        nu_ansi_term::Color::Rgb(r, g, b)
    })
}

pub fn resolve_color_from_str(
    s: &str,
    theme: &fshell_core::theme::Theme,
) -> Option<nu_ansi_term::Color> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    if let Ok(fixed) = s.parse::<u8>() {
        return Some(nu_ansi_term::Color::Fixed(fixed));
    }

    let (r, g, b) = theme.resolve_color(s);
    Some(nu_ansi_term::Color::Rgb(r, g, b))
}

pub fn format_duration(duration: Duration) -> String {
    let nanos = duration.as_nanos();
    if nanos == 0 {
        "0µs".to_string()
    } else if nanos < 1000 {
        format!("{}ns", nanos)
    } else if nanos < 1_000_000 {
        format!("{}µs", nanos / 1000)
    } else if nanos < 1_000_000_000 {
        let total_us = nanos / 1000;
        let ms = total_us / 1000;
        let remainder_us = total_us % 1000;
        if remainder_us == 0 {
            format!("{}ms", ms)
        } else {
            format!("{}.{:03}ms", ms, remainder_us)
        }
    } else if nanos < 60_000_000_000_000 {
        let total_ms = nanos / 1_000_000;
        let secs = total_ms / 1000;
        let millis = total_ms % 1000;
        format!("{}.{:02}s", secs, millis / 10)
    } else {
        let total_secs = duration.as_secs();
        let mins = total_secs / 60;
        let remaining_secs = total_secs % 60;
        format!("{}m{}s", mins, remaining_secs)
    }
}

fn compute_segment_value(
    seg_cfg: &SegmentConfig,
    _config: &PromptConfig,
    pwd: &str,
    git: &Option<RichGitStatus>,
    exit_code: i64,
    duration: Duration,
    job_count: usize,
) -> Option<String> {
    match &seg_cfg.r#type {
        SegmentType::User => {
            let user = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
            Some(user)
        }
        SegmentType::Host => {
            if let Some(host) = std::env::var("HOSTNAME")
                .ok()
                .or_else(|| std::env::var("HOST").ok())
            {
                Some(host)
            } else {
                let mut needs_update = false;
                let mut cached_val = None;

                if let Ok(cache) = SEGMENT_HOST_CACHE.lock() {
                    if let Some(ref entry) = *cache {
                        if entry.cached_at.elapsed() < HOST_CACHE_TTL {
                            return Some(entry.value.clone());
                        } else {
                            cached_val = Some(entry.value.clone());
                            needs_update = true;
                        }
                    } else {
                        needs_update = true;
                    }
                }

                if needs_update
                    && HOST_UPDATING
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                {
                    std::thread::spawn(move || {
                        let host = std::process::Command::new("hostname")
                            .output()
                            .ok()
                            .and_then(|o| String::from_utf8(o.stdout).ok())
                            .map(|s| s.trim().to_string());
                        if let Ok(mut cache) = SEGMENT_HOST_CACHE.lock() {
                            *cache = host.map(|h| CachedSegment {
                                value: h,
                                cached_at: Instant::now(),
                            });
                        }
                        HOST_UPDATING.store(false, Ordering::SeqCst);
                    });
                }

                cached_val
            }
        }
        SegmentType::Pwd => {
            if seg_cfg.shorten {
                Some(shorten_pwd(pwd))
            } else {
                Some(pwd.to_string())
            }
        }
        SegmentType::GitBranch => git.as_ref().map(|g| g.branch.clone()),
        SegmentType::GitStatus => {
            let g = git.as_ref()?;
            if g.clean {
                Some("[ok]".to_string())
            } else {
                let mut parts = Vec::new();
                if g.ahead > 0 {
                    parts.push(format!("⇡{}", g.ahead));
                }
                if g.behind > 0 {
                    parts.push(format!("⇣{}", g.behind));
                }
                if g.modified > 0 {
                    parts.push(format!("±{}", g.modified));
                }
                if g.untracked > 0 {
                    parts.push(format!("?{}", g.untracked));
                }
                Some(parts.join(" "))
            }
        }
        SegmentType::ExitCode => {
            if exit_code != 0 {
                Some(format!("[!] {}", exit_code))
            } else {
                Some("[ok] 0".to_string())
            }
        }
        SegmentType::Duration => Some(format_duration(duration)),
        SegmentType::Jobs => {
            if job_count > 0 {
                Some(format!("{} jobs", job_count))
            } else {
                None
            }
        }
        SegmentType::CargoRun => {
            let is_cargo = std::env::var("CARGO_MANIFEST_DIR").is_ok()
                || std::env::var("CARGO_PKG_NAME").is_ok()
                || std::env::var("CARGO").is_ok();
            if is_cargo {
                Some("(fsh_crun)".to_string())
            } else {
                None
            }
        }
        SegmentType::Char => {
            let user = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
            let symbol = if user == "root" { "# " } else { "> " };
            Some(symbol.to_string())
        }
        SegmentType::Time => Some(chrono::Local::now().format("%H:%M:%S").to_string()),
        SegmentType::Date => Some(chrono::Local::now().format("%Y-%m-%d").to_string()),
        SegmentType::Timestamp => Some(chrono::Local::now().format("%H:%M").to_string()),
        SegmentType::Shlvl => {
            let lvl = std::env::var("SHLVL")
                .unwrap_or_else(|_| "1".to_string())
                .parse::<usize>()
                .unwrap_or(1);
            if lvl > 1 {
                Some(format!("sh{}", lvl))
            } else {
                None
            }
        }
        SegmentType::Shell => Some("fsh".to_string()),
        SegmentType::Line => {
            let shlvl = std::env::var("SHLVL").unwrap_or_else(|_| "1".to_string());
            Some(shlvl)
        }
        SegmentType::Aws => std::env::var("AWS_PROFILE")
            .ok()
            .or_else(|| std::env::var("AWS_DEFAULT_PROFILE").ok()),
        SegmentType::Kube => {
            let mut needs_update = false;
            let mut cached_val = None;

            if let Ok(cache) = SEGMENT_CACHE.lock() {
                if let Some(ref entry) = *cache {
                    if entry.cached_at.elapsed() < KUBE_CACHE_TTL {
                        return Some(entry.value.clone());
                    } else {
                        cached_val = Some(entry.value.clone());
                        needs_update = true;
                    }
                } else {
                    needs_update = true;
                }
            }

            if needs_update
                && KUBE_UPDATING
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                std::thread::spawn(move || {
                    let value = std::process::Command::new("kubectl")
                        .args(["config", "current-context"])
                        .output()
                        .ok()
                        .and_then(|o| String::from_utf8(o.stdout).ok())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                    if let Ok(mut cache) = SEGMENT_CACHE.lock() {
                        *cache = value.map(|v| CachedSegment {
                            value: v,
                            cached_at: Instant::now(),
                        });
                    }
                    KUBE_UPDATING.store(false, Ordering::SeqCst);
                });
            }

            cached_val
        }
        SegmentType::Venv => {
            let venv = std::env::var("VIRTUAL_ENV")
                .or_else(|_| std::env::var("CONDA_DEFAULT_ENV"))
                .ok();
            venv.as_ref().and_then(|p| {
                std::path::Path::new(p)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.to_string())
            })
        }
        SegmentType::Ssh => {
            if std::env::var("SSH_TTY").is_ok() {
                Some("[ssh]".to_string())
            } else {
                None
            }
        }
        SegmentType::Text => seg_cfg.text.clone(),
        SegmentType::Separator => Some(seg_cfg.text.clone().unwrap_or_else(|| "│".to_string())),
        SegmentType::Newline => Some("\n".to_string()),
        SegmentType::Custom => {
            let cmd = seg_cfg.command.as_ref()?;
            let mut needs_update = false;
            let mut cached_val = None;

            if let Ok(mut cache) = CUSTOM_SEGMENT_CACHE.lock() {
                if cache.is_none() {
                    *cache = Some(HashMap::new());
                }
                if let Some(map) = cache.as_mut() {
                    if let Some(entry) = map.get(cmd) {
                        if entry.loaded_at.elapsed() < CUSTOM_CACHE_TTL {
                            return entry.value.clone();
                        } else {
                            cached_val = entry.value.clone();
                            needs_update = true;
                        }
                    } else {
                        needs_update = true;
                    }
                }
            }

            if needs_update {
                let mut already_updating = false;
                if let Ok(mut updating) = CUSTOM_SEGMENT_UPDATING.lock() {
                    if updating.is_none() {
                        *updating = Some(HashSet::new());
                    }
                    if let Some(set) = updating.as_mut() {
                        if set.contains(cmd) {
                            already_updating = true;
                        } else {
                            set.insert(cmd.clone());
                        }
                    }
                }

                if !already_updating {
                    let cmd_clone = cmd.clone();
                    std::thread::spawn(move || {
                        let output = std::process::Command::new("sh")
                            .arg("-c")
                            .arg(&cmd_clone)
                            .output()
                            .ok();
                        let value = output.and_then(|o| {
                            String::from_utf8(o.stdout)
                                .ok()
                                .map(|s| s.trim().to_string())
                        });

                        if let Ok(mut cache) = CUSTOM_SEGMENT_CACHE.lock()
                            && let Some(map) = cache.as_mut()
                        {
                            map.insert(
                                cmd_clone.clone(),
                                CustomSegmentCacheEntry {
                                    value,
                                    loaded_at: Instant::now(),
                                },
                            );
                        }

                        if let Ok(mut updating) = CUSTOM_SEGMENT_UPDATING.lock()
                            && let Some(set) = updating.as_mut()
                        {
                            set.remove(&cmd_clone);
                        }
                    });
                }
            }

            cached_val
        }
    }
}

pub struct ResolvedSegment {
    pub content: String,
    pub fg: Option<nu_ansi_term::Color>,
    pub bg: Option<nu_ansi_term::Color>,
    pub bold: bool,
    pub italic: bool,
    pub separator_style: SeparatorStyle,
}

pub fn nu_color_to_ratatui(c: &nu_ansi_term::Color) -> ratatui::style::Color {
    use nu_ansi_term::Color;
    match *c {
        Color::Rgb(r, g, b) => ratatui::style::Color::Rgb(r, g, b),
        Color::Black => ratatui::style::Color::Rgb(0, 0, 0),
        Color::Red => ratatui::style::Color::Rgb(255, 0, 0),
        Color::Green => ratatui::style::Color::Rgb(0, 255, 0),
        Color::Yellow => ratatui::style::Color::Rgb(255, 255, 0),
        Color::Blue => ratatui::style::Color::Rgb(0, 0, 255),
        Color::Magenta => ratatui::style::Color::Rgb(255, 0, 255),
        Color::Cyan => ratatui::style::Color::Rgb(0, 255, 255),
        Color::White => ratatui::style::Color::Rgb(255, 255, 255),
        Color::DarkGray => ratatui::style::Color::Rgb(80, 80, 80),
        Color::LightRed => ratatui::style::Color::Rgb(255, 100, 100),
        Color::LightGreen => ratatui::style::Color::Rgb(100, 255, 100),
        Color::LightYellow => ratatui::style::Color::Rgb(255, 255, 100),
        Color::LightBlue => ratatui::style::Color::Rgb(100, 100, 255),
        Color::LightMagenta => ratatui::style::Color::Rgb(255, 100, 255),
        Color::LightCyan => ratatui::style::Color::Rgb(100, 255, 255),
        _ => ratatui::style::Color::Reset,
    }
}

pub fn resolve_segments_for_preview(
    segment_configs: &[SegmentConfig],
    config: &PromptConfig,
    pwd: &str,
    git_status: &Option<RichGitStatus>,
    theme: &fshell_core::theme::Theme,
) -> Vec<ResolvedSegment> {
    segment_configs
        .iter()
        .filter_map(|seg_cfg| {
            resolve_one(
                seg_cfg,
                config,
                pwd,
                git_status,
                0,
                Duration::ZERO,
                0,
                theme,
            )
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn resolve_one(
    seg_cfg: &SegmentConfig,
    config: &PromptConfig,
    pwd: &str,
    git: &Option<RichGitStatus>,
    exit_code: i64,
    duration: Duration,
    job_count: usize,
    theme: &fshell_core::theme::Theme,
) -> Option<ResolvedSegment> {
    if seg_cfg.hide_on_zero && exit_code == 0 {
        return None;
    }
    if seg_cfg.hide_when_clean {
        match git {
            Some(g) if g.clean => return None,
            None => return None,
            _ => {}
        }
    }
    if seg_cfg.hide_under_ms > 0 && duration.as_millis() < seg_cfg.hide_under_ms as u128 {
        return None;
    }
    if seg_cfg.show_only_in_repo && git.is_none() {
        return None;
    }

    let value = compute_segment_value(seg_cfg, config, pwd, git, exit_code, duration, job_count)?;
    let exit_ok = exit_code == 0;

    let mut fg = resolve_color(&seg_cfg.fg, exit_ok, theme);
    if matches!(seg_cfg.r#type, SegmentType::Duration)
        && let Ok(color_str) = std::env::var("FSH_DURATION_COLOR")
        && let Some(c) = resolve_color_from_str(&color_str, theme)
    {
        fg = Some(c);
    }

    Some(ResolvedSegment {
        content: format!("{}{}{}", seg_cfg.prefix, value, seg_cfg.suffix),
        fg,
        bg: resolve_color(&seg_cfg.bg, exit_ok, theme),
        bold: seg_cfg.bold,
        italic: seg_cfg.italic,
        separator_style: seg_cfg
            .separator_style
            .clone()
            .unwrap_or_else(|| config.separator_style.clone()),
    })
}

fn render_line(segments: &[ResolvedSegment]) -> String {
    if segments.is_empty() {
        return String::new();
    }

    let mut result = String::new();

    for (i, seg) in segments.iter().enumerate() {
        let mut style = Style::new();
        if let Some(fg) = seg.fg {
            style = style.fg(fg);
        }
        if let Some(bg) = seg.bg {
            style = style.on(bg);
        }
        if seg.bold {
            style = style.bold();
        }
        if seg.italic {
            style = style.italic();
        }
        result.push_str(&style.paint(&seg.content).to_string());
        result.push_str("\x1b[0m");

        if i + 1 < segments.len() {
            let next = &segments[i + 1];
            let glyph = seg.separator_style.glyph();

            match (&seg.bg, &next.bg) {
                (Some(cur_bg), Some(next_bg)) => {
                    let sep_style = Style::new().fg(*cur_bg).on(*next_bg);
                    result.push_str(&sep_style.paint(glyph).to_string());
                    result.push_str("\x1b[0m");
                }
                (Some(cur_bg), None) => {
                    let sep_style = Style::new().fg(*cur_bg);
                    result.push_str(&sep_style.paint(glyph).to_string());
                    result.push_str("\x1b[0m");
                }
                (None, Some(next_bg)) => {
                    let sep_style = Style::new().on(*next_bg);
                    result.push_str(&sep_style.paint(glyph).to_string());
                    result.push_str("\x1b[0m");
                }
                (None, None) => {
                    if matches!(seg.separator_style, SeparatorStyle::None) {
                        result.push(' ');
                    } else {
                        result.push_str(glyph);
                    }
                }
            }
        }
    }

    result
}

#[allow(clippy::too_many_arguments)]
pub fn render_segment_list(
    segment_configs: &[SegmentConfig],
    config: &PromptConfig,
    pwd: &str,
    git_status: &Option<RichGitStatus>,
    exit_code: i64,
    duration: Duration,
    job_count: usize,
    _is_left: bool,
    theme: &fshell_core::theme::Theme,
) -> String {
    let mut resolved: Vec<ResolvedSegment> = Vec::new();
    for seg_cfg in segment_configs {
        if let Some(r) = resolve_one(
            seg_cfg, config, pwd, git_status, exit_code, duration, job_count, theme,
        ) {
            resolved.push(r);
        }
    }

    let mut lines: Vec<Vec<ResolvedSegment>> = vec![Vec::new()];
    for seg in resolved {
        if seg.content == "\n" {
            lines.push(Vec::new());
        } else {
            lines
                .last_mut()
                .expect("segment list is non-empty")
                .push(seg);
        }
    }

    let mut result = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            result.push_str("\x1b[0m\n");
        }
        result.push_str(&render_line(line));
    }

    result
}

pub fn render_line_to_ratatui(segments: &[ResolvedSegment]) -> ratatui::text::Line<'static> {
    if segments.is_empty() {
        return ratatui::text::Line::default();
    }

    let mut spans = Vec::new();

    for (i, seg) in segments.iter().enumerate() {
        let mut style = ratatui::style::Style::default();
        if let Some(fg) = seg.fg {
            style = style.fg(nu_color_to_ratatui(&fg));
        }
        if let Some(bg) = seg.bg {
            style = style.bg(nu_color_to_ratatui(&bg));
        }
        if seg.bold {
            style = style.add_modifier(ratatui::style::Modifier::BOLD);
        }
        if seg.italic {
            style = style.add_modifier(ratatui::style::Modifier::ITALIC);
        }
        spans.push(ratatui::text::Span::styled(seg.content.clone(), style));

        if i + 1 < segments.len() {
            let next = &segments[i + 1];
            let glyph = seg.separator_style.glyph();

            match (&seg.bg, &next.bg) {
                (Some(cur_bg), Some(next_bg)) => {
                    let sep_style = ratatui::style::Style::default()
                        .fg(nu_color_to_ratatui(cur_bg))
                        .bg(nu_color_to_ratatui(next_bg));
                    spans.push(ratatui::text::Span::styled(glyph.to_string(), sep_style));
                }
                (Some(cur_bg), None) => {
                    let sep_style =
                        ratatui::style::Style::default().fg(nu_color_to_ratatui(cur_bg));
                    spans.push(ratatui::text::Span::styled(glyph.to_string(), sep_style));
                }
                (None, Some(next_bg)) => {
                    let sep_style =
                        ratatui::style::Style::default().bg(nu_color_to_ratatui(next_bg));
                    spans.push(ratatui::text::Span::styled(glyph.to_string(), sep_style));
                }
                (None, None) => {
                    if matches!(seg.separator_style, SeparatorStyle::None) {
                        spans.push(ratatui::text::Span::raw(" "));
                    } else {
                        spans.push(ratatui::text::Span::raw(glyph.to_string()));
                    }
                }
            }
        }
    }

    ratatui::text::Line::from(spans)
}

#[allow(clippy::too_many_arguments)]
pub fn render_segment_list_to_ratatui_lines(
    segment_configs: &[SegmentConfig],
    config: &PromptConfig,
    pwd: &str,
    git_status: &Option<RichGitStatus>,
    exit_code: i64,
    duration: Duration,
    job_count: usize,
    _is_left: bool,
    theme: &fshell_core::theme::Theme,
) -> Vec<ratatui::text::Line<'static>> {
    let mut resolved: Vec<ResolvedSegment> = Vec::new();
    for seg_cfg in segment_configs {
        if let Some(r) = resolve_one(
            seg_cfg, config, pwd, git_status, exit_code, duration, job_count, theme,
        ) {
            resolved.push(r);
        }
    }

    let mut lines: Vec<Vec<ResolvedSegment>> = vec![Vec::new()];
    for seg in resolved {
        if seg.content == "\n" {
            lines.push(Vec::new());
        } else {
            lines
                .last_mut()
                .expect("segment list is non-empty")
                .push(seg);
        }
    }

    lines
        .iter()
        .map(|line| render_line_to_ratatui(line))
        .collect()
}

pub fn render_segments(segments: &[PromptSegment]) -> String {
    let mut result = String::new();

    for seg in segments.iter() {
        if seg.content == "\n" {
            result.push_str("\x1b[0m\n");
            continue;
        }

        let mut style = Style::new();
        if let Some(fg) = seg.fg {
            style = style.fg(fg);
        }
        if seg.bold {
            style = style.bold();
        }
        if seg.italic {
            style = style.italic();
        }

        result.push_str(&style.paint(&seg.content).to_string());
        result.push_str("\x1b[0m");
    }

    result
}

#[allow(clippy::too_many_arguments)]
pub fn resolve_segment(
    seg_cfg: &SegmentConfig,
    config: &PromptConfig,
    pwd: &str,
    git: &Option<RichGitStatus>,
    exit_code: i64,
    duration: Duration,
    job_count: usize,
    theme: &fshell_core::theme::Theme,
) -> Option<PromptSegment> {
    if seg_cfg.hide_on_zero && exit_code == 0 {
        return None;
    }
    if seg_cfg.hide_when_clean {
        match git {
            Some(g) if g.clean => return None,
            None => return None,
            _ => {}
        }
    }
    if seg_cfg.hide_under_ms > 0 && duration.as_millis() < seg_cfg.hide_under_ms as u128 {
        return None;
    }
    if seg_cfg.show_only_in_repo && git.is_none() {
        return None;
    }

    let value = compute_segment_value(seg_cfg, config, pwd, git, exit_code, duration, job_count);
    let value = value?;

    let exit_ok = exit_code == 0;
    let fg = resolve_color(&seg_cfg.fg, exit_ok, theme);

    Some(PromptSegment {
        content: format!("{}{}{}", seg_cfg.prefix, value, seg_cfg.suffix),
        fg,
        bold: seg_cfg.bold,
        italic: seg_cfg.italic,
    })
}

fn shorten_pwd(pwd: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && pwd.starts_with(&home) {
        pwd.replacen(&home, "~", 1)
    } else {
        pwd.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_segments_no_backgrounds() {
        let segs = vec![
            PromptSegment {
                content: "ariel".to_string(),
                fg: Some(Color::Cyan),
                bold: true,
                italic: false,
            },
            PromptSegment {
                content: "~/src".to_string(),
                fg: Some(Color::Rgb(255, 204, 0)),
                bold: true,
                italic: false,
            },
            PromptSegment {
                content: ">".to_string(),
                fg: Some(Color::Green),
                bold: true,
                italic: false,
            },
        ];
        let result = render_segments(&segs);
        assert!(result.contains("ariel"));
        assert!(result.contains("~/src"));
        assert!(result.contains(">"));
        assert!(!result.contains("48;"));
    }

    #[test]
    fn test_render_line_powerline() {
        let segs = vec![
            ResolvedSegment {
                content: "user".into(),
                fg: Some(Color::White),
                bg: Some(Color::Rgb(30, 58, 95)),
                bold: true,
                italic: false,
                separator_style: SeparatorStyle::Arrow,
            },
            ResolvedSegment {
                content: "pwd".into(),
                fg: Some(Color::White),
                bg: Some(Color::Rgb(45, 80, 22)),
                bold: true,
                italic: false,
                separator_style: SeparatorStyle::Arrow,
            },
        ];
        let result = render_line(&segs);
        assert!(result.contains("user"));
        assert!(result.contains("pwd"));
        assert!(result.contains("\u{e0b0}"));
        assert!(result.contains("48;"));
    }

    #[test]
    fn test_render_line_no_backgrounds() {
        let segs = vec![
            ResolvedSegment {
                content: "hello".into(),
                fg: Some(Color::Cyan),
                bg: None,
                bold: true,
                italic: false,
                separator_style: SeparatorStyle::None,
            },
            ResolvedSegment {
                content: "world".into(),
                fg: Some(Color::Yellow),
                bg: None,
                bold: true,
                italic: false,
                separator_style: SeparatorStyle::None,
            },
        ];
        let result = render_line(&segs);
        assert!(result.contains("hello"));
        assert!(result.contains("world"));
        assert!(!result.contains("\u{e0b0}"));
    }

    #[test]
    fn test_render_segment_list_multiline() {
        use fshell_core::prompt_config::ColorSpec;
        use fshell_core::prompt_config::{PromptConfig, SegmentConfig, SegmentType};

        let configs = vec![
            SegmentConfig {
                r#type: SegmentType::User,
                fg: Some(ColorSpec::Named("cyan".into())),
                bold: true,
                ..Default::default()
            },
            SegmentConfig {
                r#type: SegmentType::Newline,
                ..Default::default()
            },
            SegmentConfig {
                r#type: SegmentType::Char,
                ..Default::default()
            },
        ];
        let config = PromptConfig::default();
        let theme = fshell_core::theme::Theme::default_theme();
        let result = render_segment_list(
            &configs,
            &config,
            "/home/user",
            &None,
            0,
            Duration::ZERO,
            0,
            true,
            &theme,
        );
        assert!(result.contains('\n'));
        assert!(result.len() > 1);
    }

    #[test]
    fn test_color_inheritance_powerline() {
        let segs = vec![
            ResolvedSegment {
                content: "A".into(),
                fg: Some(Color::White),
                bg: Some(Color::Rgb(255, 0, 0)),
                bold: false,
                italic: false,
                separator_style: SeparatorStyle::Arrow,
            },
            ResolvedSegment {
                content: "B".into(),
                fg: Some(Color::White),
                bg: Some(Color::Rgb(0, 255, 0)),
                bold: false,
                italic: false,
                separator_style: SeparatorStyle::Arrow,
            },
            ResolvedSegment {
                content: "C".into(),
                fg: Some(Color::White),
                bg: Some(Color::Rgb(0, 0, 255)),
                bold: false,
                italic: false,
                separator_style: SeparatorStyle::Arrow,
            },
        ];
        let result = render_line(&segs);
        assert!(result.contains("A"));
        assert!(result.contains("B"));
        assert!(result.contains("C"));
        assert_eq!(result.matches('\u{e0b0}').count(), 2);
    }

    #[test]
    fn test_render_line_mixed_backgrounds() {
        let segs = vec![
            ResolvedSegment {
                content: "bg_seg".into(),
                fg: Some(Color::White),
                bg: Some(Color::Rgb(30, 58, 95)),
                bold: false,
                italic: false,
                separator_style: SeparatorStyle::Arrow,
            },
            ResolvedSegment {
                content: "no_bg".into(),
                fg: Some(Color::Cyan),
                bg: None,
                bold: false,
                italic: false,
                separator_style: SeparatorStyle::None,
            },
        ];
        let result = render_line(&segs);
        assert!(result.contains("bg_seg"));
        assert!(result.contains("no_bg"));
    }

    #[test]
    fn test_cargo_run_segment() {
        let seg = SegmentConfig::new(SegmentType::CargoRun, None, false);
        // Under test harness or cargo test, CARGO_MANIFEST_DIR is set
        let res = compute_segment_value(
            &seg,
            &PromptConfig::default(),
            "/tmp",
            &None,
            0,
            Duration::ZERO,
            0,
        );
        assert_eq!(res, Some("(fsh_crun)".to_string()));
    }

    #[test]
    fn test_render_line_to_ratatui() {
        let segs = vec![
            ResolvedSegment {
                content: "seg1".into(),
                fg: Some(Color::Rgb(255, 255, 255)),
                bg: Some(Color::Rgb(255, 0, 0)),
                bold: true,
                italic: false,
                separator_style: SeparatorStyle::Arrow,
            },
            ResolvedSegment {
                content: "seg2".into(),
                fg: Some(Color::Rgb(0, 0, 0)),
                bg: Some(Color::Rgb(0, 255, 0)),
                bold: false,
                italic: true,
                separator_style: SeparatorStyle::Arrow,
            },
        ];
        let line = render_line_to_ratatui(&segs);
        assert_eq!(line.spans.len(), 3); // seg1, separator, seg2
        assert_eq!(line.spans[0].content, "seg1");
        assert_eq!(line.spans[1].content, "\u{e0b0}");
        assert_eq!(line.spans[2].content, "seg2");
        assert_eq!(
            line.spans[0].style.fg,
            Some(ratatui::style::Color::Rgb(255, 255, 255))
        );
        assert_eq!(
            line.spans[0].style.bg,
            Some(ratatui::style::Color::Rgb(255, 0, 0))
        );
        assert_eq!(
            line.spans[1].style.fg,
            Some(ratatui::style::Color::Rgb(255, 0, 0))
        );
        assert_eq!(
            line.spans[1].style.bg,
            Some(ratatui::style::Color::Rgb(0, 255, 0))
        );
    }

    #[test]
    fn test_render_segment_list_to_ratatui_lines_multiline() {
        use fshell_core::prompt_config::ColorSpec;
        use fshell_core::prompt_config::{PromptConfig, SegmentConfig, SegmentType};

        let configs = vec![
            SegmentConfig {
                r#type: SegmentType::User,
                fg: Some(ColorSpec::Named("cyan".into())),
                bold: true,
                ..Default::default()
            },
            SegmentConfig {
                r#type: SegmentType::Newline,
                ..Default::default()
            },
            SegmentConfig {
                r#type: SegmentType::Char,
                ..Default::default()
            },
        ];
        let config = PromptConfig::default();
        let theme = fshell_core::theme::Theme::default_theme();
        let lines = render_segment_list_to_ratatui_lines(
            &configs,
            &config,
            "/home/user",
            &None,
            0,
            Duration::ZERO,
            0,
            true,
            &theme,
        );
        assert_eq!(lines.len(), 2);
    }
}
