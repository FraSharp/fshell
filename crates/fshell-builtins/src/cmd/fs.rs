// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![allow(clippy::unnecessary_cast)]
use crate::error::BuiltinError;
use crate::utils::{
    change_dir_and_update_caps, check_read_file, expand_tilde_for_env, interpret_ansi_escapes,
    resolve_user_path, val_to_display_string,
};
use fshell_core::ShellError;
use fshell_core::Val;
use fshell_core::diagnostic::ErrorCode;
use fshell_engine::{CapAction, Env, PipeSender, PipeStream, PipelinePayload};
use fshell_hash::{FxHashMap, FxHashSet};
use miette::SourceSpan;
use std::ffi::OsStr;
use std::io::{BufRead, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::Arc;

const PERMS: [(u32, char); 9] = [
    (libc::S_IRUSR as u32, 'r'),
    (libc::S_IWUSR as u32, 'w'),
    (libc::S_IXUSR as u32, 'x'),
    (libc::S_IRGRP as u32, 'r'),
    (libc::S_IWGRP as u32, 'w'),
    (libc::S_IXGRP as u32, 'x'),
    (libc::S_IROTH as u32, 'r'),
    (libc::S_IWOTH as u32, 'w'),
    (libc::S_IXOTH as u32, 'x'),
];

fn format_permissions_from_mode(mode: u32) -> String {
    PERMS
        .iter()
        .map(|(bit, ch)| if mode & bit != 0 { *ch } else { '-' })
        .collect()
}

fn parse_ls_args_to_rrls_config(
    args: &[Val],
    env: &Env,
) -> Result<(fshell_ls::Config, Vec<String>, bool), String> {
    let is_tty = fshell_engine::is_stdout_a_tty();

    let mut ls = LsArgs {
        sort: fshell_ls::SortMode::Name,
        reverse: false,
        color: is_tty,
        icons: false,
        tree: false,
        depth: None,
        exclude: Vec::new(),
        show_hidden: false,
        long: false,
        list_dirs: false,
        one_per_line: !is_tty,
        human: false,
        raw: false,
        inode: false,
        group_dirs: false,
        git: false,
        dereference: false,
        recursive: false,
        verbose: false,
    };
    let mut path_args: Vec<String> = Vec::new();
    let mut end_of_opts = false;

    let mut idx = 0;
    while idx < args.len() {
        let arg = &args[idx];
        let Val::String(s) = arg else {
            return Err("ls argument must be a string path".to_string());
        };
        idx += 1;

        if !end_of_opts && s == "--" {
            end_of_opts = true;
            continue;
        }

        if !end_of_opts && s.starts_with("--") && s.len() > 2 {
            let opt = &s[2..];
            if let Some(eq_pos) = opt.find('=') {
                let key = &opt[..eq_pos];
                let val = &opt[eq_pos + 1..];
                match key {
                    "sort" => match val {
                        "size" => ls.sort = fshell_ls::SortMode::Size,
                        "time" => ls.sort = fshell_ls::SortMode::Time,
                        _ => {
                            return Err(BuiltinError::InvalidArgument {
                                cmd: "ls".into(),
                                arg: format!("invalid sort type '{val}'"),
                                span: None,
                            }
                            .to_string());
                        }
                    },
                    "format" => match val {
                        "single-column" => ls.one_per_line = true,
                        _ => {
                            return Err(BuiltinError::InvalidArgument {
                                cmd: "ls".into(),
                                arg: format!("invalid format '{val}'"),
                                span: None,
                            }
                            .to_string());
                        }
                    },
                    "color" | "colour" => match val {
                        "always" | "yes" | "force" => ls.color = true,
                        "auto" | "tty" | "if-tty" => ls.color = is_tty,
                        "never" | "no" | "none" => ls.color = false,
                        _ => {
                            return Err(BuiltinError::InvalidArgument {
                                cmd: "ls".into(),
                                arg: format!("invalid argument '{val}' for --color"),
                                span: None,
                            }
                            .to_string());
                        }
                    },
                    "icons" => match val {
                        "always" | "yes" | "force" => ls.icons = true,
                        "auto" | "tty" | "if-tty" => ls.icons = is_tty,
                        "never" | "no" | "none" => ls.icons = false,
                        _ => {
                            return Err(BuiltinError::InvalidArgument {
                                cmd: "ls".into(),
                                arg: format!("invalid argument '{val}' for --icons"),
                                span: None,
                            }
                            .to_string());
                        }
                    },
                    "depth" => {
                        if let Ok(d) = val.parse::<usize>() {
                            ls.depth = Some(d);
                        } else {
                            return Err(BuiltinError::InvalidArgument {
                                cmd: "ls".into(),
                                arg: format!("invalid depth '{val}'"),
                                span: None,
                            }
                            .to_string());
                        }
                    }
                    "exclude" | "ignore" | "hide" => ls.exclude.push(val.to_string()),
                    _ => {
                        return Err(BuiltinError::InvalidArgument {
                            cmd: "ls".into(),
                            arg: format!("unknown option --{key}"),
                            span: None,
                        }
                        .to_string());
                    }
                }
            } else {
                match opt {
                    "all" => ls.show_hidden = true,
                    "list-dirs" => ls.list_dirs = true,
                    "long" => {
                        ls.long = true;
                        ls.verbose = true;
                    }
                    "human-readable" => ls.human = true,
                    "raw" => ls.raw = true,
                    "inode" => ls.inode = true,
                    "reverse" => ls.reverse = true,
                    "color" | "colour" => ls.color = true,
                    "tree" => ls.tree = true,
                    "depth" => {
                        if idx < args.len() {
                            if let Val::String(depth_str) = &args[idx] {
                                if let Ok(d) = depth_str.parse::<usize>() {
                                    ls.depth = Some(d);
                                    idx += 1;
                                } else {
                                    return Err(BuiltinError::InvalidArgument {
                                        cmd: "ls".into(),
                                        arg: format!("invalid depth '{depth_str}'"),
                                        span: None,
                                    }
                                    .to_string());
                                }
                            } else {
                                return Err("ls: --depth requires a string value".to_string());
                            }
                        } else {
                            return Err("ls: option '--depth' requires an argument".to_string());
                        }
                    }
                    "exclude" | "ignore" | "hide" => {
                        if idx < args.len() {
                            if let Val::String(pattern) = &args[idx] {
                                ls.exclude.push(pattern.clone());
                                idx += 1;
                            } else {
                                return Err(format!("ls: --{opt} requires a string value"));
                            }
                        } else {
                            return Err(format!("ls: option '--{opt}' requires an argument"));
                        }
                    }
                    "group-directories-first" => ls.group_dirs = true,
                    "icons" => ls.icons = true,
                    "git" => ls.git = true,
                    "dereference" => ls.dereference = true,
                    "recurse" | "recursive" => ls.recursive = true,
                    "verbose" => ls.verbose = true,
                    _ => {
                        return Err(BuiltinError::InvalidArgument {
                            cmd: "ls".into(),
                            arg: format!("unknown option --{opt}"),
                            span: None,
                        }
                        .to_string());
                    }
                }
            }
        } else if !end_of_opts && s.starts_with('-') && s.len() > 1 {
            for (byte_idx, ch) in s.char_indices().skip(1) {
                match ch {
                    'I' => {
                        let next_byte = byte_idx + ch.len_utf8();
                        if next_byte < s.len() {
                            ls.exclude.push(s[next_byte..].to_string());
                            break;
                        } else if idx < args.len() {
                            if let Val::String(pattern) = &args[idx] {
                                ls.exclude.push(pattern.clone());
                                idx += 1;
                                break;
                            } else {
                                return Err("ls: -I requires a string value".to_string());
                            }
                        } else {
                            return Err("ls: option requires an argument -- 'I'".to_string());
                        }
                    }
                    'a' => ls.show_hidden = true,
                    'd' => ls.list_dirs = true,
                    'l' => {
                        ls.long = true;
                        ls.verbose = true;
                    }
                    '1' => ls.one_per_line = true,
                    'h' => ls.human = true,
                    'i' => ls.inode = true,
                    'S' => ls.sort = fshell_ls::SortMode::Size,
                    't' => ls.sort = fshell_ls::SortMode::Time,
                    'r' => ls.reverse = true,
                    'L' => ls.dereference = true,
                    'R' => ls.recursive = true,
                    'v' => ls.verbose = true,
                    _ => {
                        return Err(BuiltinError::InvalidArgument {
                            cmd: "ls".into(),
                            arg: format!("unknown option -{ch}"),
                            span: None,
                        }
                        .to_string());
                    }
                }
            }
        } else {
            path_args.push(s.clone());
        }
    }

    let raw_path = if !path_args.is_empty() {
        resolve_user_path(&path_args[0], env)
    } else {
        env.cwd()
    };

    let config = fshell_ls::Config {
        path: raw_path,
        show_all: ls.show_hidden,
        list_dirs: ls.list_dirs,
        long_listing: ls.long,
        one_per_line: ls.one_per_line,
        human_readable: ls.human,
        raw: ls.raw,
        show_inode: ls.inode,
        sort_mode: ls.sort,
        reverse_sort: ls.reverse,
        use_color: ls.color,
        tree: ls.tree,
        tree_depth: ls.depth,
        tree_exclude: ls.exclude,
        group_directories_first: ls.group_dirs,
        show_icons: ls.icons,
        git: ls.git,
        dereference: ls.dereference,
        recursive: ls.recursive,
        verbose: ls.verbose,
    };

    Ok((config, path_args, ls.verbose))
}

struct LsArgs {
    sort: fshell_ls::SortMode,
    reverse: bool,
    color: bool,
    icons: bool,
    tree: bool,
    depth: Option<usize>,
    exclude: Vec<String>,
    show_hidden: bool,
    long: bool,
    list_dirs: bool,
    one_per_line: bool,
    human: bool,
    raw: bool,
    inode: bool,
    group_dirs: bool,
    git: bool,
    dereference: bool,
    recursive: bool,
    verbose: bool,
}

fn fileinfo_to_val_map(
    info: &fshell_ls::FileInfo,
    arena: &[u8],
    verbose: bool,
    raw: bool,
) -> fshell_core::FxIndexMap<ustr::Ustr, Val> {
    let name_bytes = info
        .entry
        .range(arena.len())
        .and_then(|range| arena.get(range))
        .unwrap_or_default();
    let name_str = String::from_utf8_lossy(name_bytes);

    let is_dir = info.entry.is_dir();
    let mut map = fshell_core::FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    map.insert(ustr::ustr("name"), Val::String(name_str.into_owned()));
    map.insert(
        ustr::ustr("type"),
        Val::String(if is_dir {
            "dir".to_string()
        } else {
            "file".to_string()
        }),
    );

    let mut is_exec = false;
    let mut is_link = false;

    if let Some(ref meta) = info.metadata {
        is_exec = (meta.mode & ((libc::S_IXUSR | libc::S_IXGRP | libc::S_IXOTH) as u32)) != 0;
        is_link = (meta.mode & (libc::S_IFMT as u32)) == (libc::S_IFLNK as u32);

        if raw {
            map.insert(ustr::ustr("size"), Val::String(meta.size.to_string()));
        } else {
            map.insert(ustr::ustr("size"), Val::Int(meta.size as i64));
        }
        if let Some(dt) = chrono::DateTime::from_timestamp(meta.mtime, 0) {
            map.insert(ustr::ustr("last_modified"), Val::DateTime(dt));
        }
        if verbose {
            map.insert(
                ustr::ustr("permissions"),
                Val::String(format_permissions_from_mode(meta.mode)),
            );
        }

        let status_str = match meta.git_status {
            fshell_ls::GitStatus::Clean => "clean",
            fshell_ls::GitStatus::Modified => "modified",
            fshell_ls::GitStatus::New => "new",
            fshell_ls::GitStatus::Deleted => "deleted",
            fshell_ls::GitStatus::Renamed => "renamed",
            fshell_ls::GitStatus::Ignored => "ignored",
            fshell_ls::GitStatus::Untracked => "untracked",
            fshell_ls::GitStatus::Conflicted => "conflicted",
        };
        map.insert(
            ustr::ustr("git_status"),
            Val::String(status_str.to_string()),
        );
    } else {
        map.insert(ustr::ustr("size"), Val::Int(0));
    }

    map.insert(ustr::ustr("is_executable"), Val::Bool(is_exec));
    map.insert(ustr::ustr("is_symlink"), Val::Bool(is_link));

    map
}

fn canonicalize_cached(
    path: &std::path::Path,
    cache: &mut FxHashMap<std::path::PathBuf, std::path::PathBuf>,
) -> std::path::PathBuf {
    cache
        .entry(path.to_path_buf())
        .or_insert_with(|| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
        .clone()
}

fn do_recursive_walk(
    initial: &fshell_ls::ListResult,
    config: &fshell_ls::Config,
    env: &Env,
    root: &std::path::Path,
    canonical_cache: &mut FxHashMap<std::path::PathBuf, std::path::PathBuf>,
    git_status_cache: &mut fshell_ls::scan::GitStatusCache,
) -> Result<(Vec<fshell_ls::FileInfo>, Vec<u8>), String> {
    let mut visited = FxHashSet::default();
    visited.insert(canonicalize_cached(root, canonical_cache));

    fn is_dir_entry(
        entry: &fshell_ls::FileInfo,
        arena: &[u8],
        base: &std::path::Path,
    ) -> Result<Option<std::path::PathBuf>, String> {
        if entry.entry.is_dir() {
            let range = entry.entry.range(arena.len()).ok_or_else(|| {
                "ls: invalid directory entry range while walking recursively".to_owned()
            })?;
            let name = arena.get(range).ok_or_else(|| {
                "ls: invalid directory entry range while walking recursively".to_owned()
            })?;
            Ok(Some(base.join(OsStr::from_bytes(name))))
        } else {
            Ok(None)
        }
    }

    let mut dirs_to_visit = Vec::new();
    for entry in &initial.entries {
        if let Some(path) = is_dir_entry(entry, &initial.arena, root)? {
            dirs_to_visit.push(path);
        }
    }

    let mut entries = initial.entries.clone();
    let mut arena = initial.arena.clone();

    while let Some(dir) = dirs_to_visit.pop() {
        let canonical = canonicalize_cached(&dir, canonical_cache);
        if visited.contains(&canonical) {
            continue;
        }

        let mut sub_config = config.clone();
        sub_config.path = dir.clone();

        let allowed = env.caps.caps.read().check_read_dir(&sub_config.path)
            || env.caps.caps.read().check_read_dir(&canonical);
        if allowed {
            visited.insert(canonical);
            let sub_result =
                fshell_ls::list_dir_with_git_status_cache(&sub_config, git_status_cache)
                    .map_err(|err| format!("{}: {err}", sub_config.path.display()))?;
            for entry in &sub_result.entries {
                if let Some(subdir_path) = is_dir_entry(entry, &sub_result.arena, &dir)? {
                    dirs_to_visit.push(subdir_path);
                }
            }
            for entry in sub_result.entries {
                let Some(range) = entry.entry.range(sub_result.arena.len()) else {
                    return Err("ls: invalid entry range while walking recursively".into());
                };
                let name = sub_result
                    .arena
                    .get(range)
                    .ok_or("ls: invalid entry range while walking recursively")?;
                let full_entry_path = dir.join(OsStr::from_bytes(name));
                let rel_path = full_entry_path
                    .strip_prefix(root)
                    .unwrap_or(&full_entry_path)
                    .as_os_str()
                    .as_bytes();
                let offset = arena.len();
                arena.extend_from_slice(rel_path);
                let mut entry_clone = entry.clone();
                entry_clone.entry =
                    fshell_ls::Entry::new(offset, rel_path.len(), entry.entry.is_dir());
                entries.push(entry_clone);
            }
        }
    }

    Ok((entries, arena))
}

pub fn ls_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    fshell_core::debug_log!(
        "ls_builtin called, is_captured={}, is_last_stage={}",
        env.is_captured,
        env.is_last_stage
    );
    // 1. Parse flags into rrls Config and collect path arguments
    let (mut config, path_args, verbose) = parse_ls_args_to_rrls_config(&args, env)?;

    // Set theme colors for fshell-ls
    let theme = env.active_theme();
    let (d_r, d_g, d_b) = theme.completions.header_directory.to_rgb();
    let (l_r, l_g, l_b) = theme.completions.header_flag.to_rgb();
    let (e_r, e_g, e_b) = theme.completions.header_command.to_rgb();
    fshell_ls::colors::set_colors(
        &format!("\x1b[38;2;{};{};{}m", d_r, d_g, d_b),
        &format!("\x1b[38;2;{};{};{}m", l_r, l_g, l_b),
        &format!("\x1b[38;2;{};{};{}m", e_r, e_g, e_b),
    );

    // When output goes through the pipeline (not direct terminal), force verbose
    // so file metadata (size, mode, mtime) is always collected. Without this,
    // downstream pipeline operators like `filter size > N` see size=0 on every file.
    if env.is_captured || !env.is_last_stage {
        config.verbose = true;
    }

    // Tree mode requires recursive scanning to build the full directory tree
    if config.tree {
        config.recursive = true;
    }

    // Collect all target paths (expand tilde for each and resolve relative against env.cwd())
    let targets: Vec<std::path::PathBuf> = if !path_args.is_empty() {
        path_args
            .iter()
            .map(|p| resolve_user_path(p, env))
            .collect()
    } else {
        vec![env.cwd()]
    };

    // Direct output mode:
    // If we are executing a pipeline and this is the last stage
    // and the output is not captured (e.g. redirected or written to stdout),
    // render directly to avoid pipeline allocation overhead.
    if !env.is_captured && env.is_last_stage {
        fshell_core::debug_log!("ls_builtin: direct output mode");
        for target in &targets {
            let mut t_config = config.clone();
            t_config.path = target.clone();
            env.track_read(t_config.path.clone());
            env.enforce_capability("ls", CapAction::ReadDir(t_config.path.clone()))?;

            if targets.len() > 1 {
                println!(
                    "{}:",
                    fshell_ls::utils::escape_name(target.as_os_str().as_bytes())
                );
            }

            if config.recursive && !config.tree {
                let mut git_status_cache = fshell_ls::scan::GitStatusCache::default();
                let mut paths = vec![t_config.path.clone()];
                let mut is_first = true;
                while let Some(current_path) = paths.pop() {
                    if env.pipeline_cancelled() {
                        break;
                    }
                    let allowed = env.caps.caps.read().check_read_dir(&current_path)
                        || current_path
                            .canonicalize()
                            .ok()
                            .as_ref()
                            .is_some_and(|cp| env.caps.caps.read().check_read_dir(cp));
                    if !allowed {
                        continue;
                    }

                    let mut sub_config = t_config.clone();
                    sub_config.path = current_path.clone();

                    let sub_result = fshell_ls::list_dir_with_git_status_cache(
                        &sub_config,
                        &mut git_status_cache,
                    )
                    .map_err(|e| format!("{}: {e}", sub_config.path.display()))?;
                    if !is_first {
                        println!();
                    }
                    is_first = false;
                    println!(
                        "{}:",
                        fshell_ls::utils::escape_name(current_path.as_os_str().as_bytes())
                    );

                    fshell_ls::render(
                        &sub_result,
                        &sub_config,
                        |p| {
                            env.caps.caps.read().check_read_dir(p)
                                || p.canonicalize()
                                    .ok()
                                    .as_ref()
                                    .is_some_and(|cp| env.caps.caps.read().check_read_dir(cp))
                        },
                        || env.pipeline_cancelled(),
                    )
                    .map_err(|e| format!("{}: {e}", sub_config.path.display()))?;

                    let mut subdirs = Vec::new();
                    for entry in &sub_result.entries {
                        if entry.entry.is_dir() {
                            let range = entry.entry.range(sub_result.arena.len()).ok_or(
                                "ls: invalid directory entry range while walking recursively",
                            )?;
                            let name = sub_result.arena.get(range).ok_or(
                                "ls: invalid directory entry range while walking recursively",
                            )?;
                            subdirs.push(current_path.join(OsStr::from_bytes(name)));
                        }
                    }
                    subdirs.reverse();
                    paths.extend(subdirs);
                }
            } else {
                let all_entries = fshell_ls::list_dir(&t_config)
                    .map_err(|e| format!("{}: {}", t_config.path.display(), e))?;
                fshell_ls::render(
                    &all_entries,
                    &t_config,
                    |p| {
                        env.caps.caps.read().check_read_dir(p)
                            || p.canonicalize()
                                .ok()
                                .as_ref()
                                .is_some_and(|cp| env.caps.caps.read().check_read_dir(cp))
                    },
                    || env.pipeline_cancelled(),
                )
                .map_err(|e| format!("{}: {}", t_config.path.display(), e))?;
            }
        }
        drop(tx);
        return Ok(());
    }

    // Tree pipeline mode: if tree mode is active and the output is captured
    // or not the last stage, render the tree to a buffer and emit its lines.
    if config.tree {
        let mut buf = Vec::new();
        for target in &targets {
            let mut t_config = config.clone();
            t_config.path = target.clone();
            env.track_read(t_config.path.clone());
            env.enforce_capability("ls", CapAction::ReadDir(t_config.path.clone()))?;

            if targets.len() > 1 {
                buf.extend_from_slice(
                    format!(
                        "{}:\n",
                        fshell_ls::utils::escape_name(target.as_os_str().as_bytes())
                    )
                    .as_bytes(),
                );
            }

            fshell_ls::tree::render_tree(
                &t_config,
                &mut buf,
                |p| {
                    !env.is_strict_mode()
                        || env.caps.caps.read().check_read_dir(p)
                        || p.canonicalize()
                            .ok()
                            .as_ref()
                            .is_some_and(|cp| env.caps.caps.read().check_read_dir(cp))
                },
                || env.pipeline_cancelled(),
            )
            .map_err(|e| format!("{}: {}", t_config.path.display(), e))?;
        }

        let lines: Vec<String> = String::from_utf8_lossy(&buf)
            .lines()
            .map(|s| s.to_string())
            .collect();

        let tx_clone = tx.clone();
        tokio::spawn(async move {
            for line in lines {
                let payload = PipelinePayload::Data(Arc::new(Val::String(line)));
                if tx_clone.send(payload).await.is_err() {
                    break;
                }
            }
        });

        return Ok(());
    }

    // Structured output mode: emit Val::Map rows through the pipeline.
    // Each target path is scanned independently; entries from different
    // paths keep their original arena so name offsets remain valid.
    for target in targets {
        let mut t_config = config.clone();
        t_config.path = target.clone();
        env.track_read(t_config.path.clone());
        env.enforce_capability("ls", CapAction::ReadDir(t_config.path.clone()))?;

        let mut git_status_cache = fshell_ls::scan::GitStatusCache::default();
        let result = if config.recursive {
            fshell_ls::list_dir_with_git_status_cache(&t_config, &mut git_status_cache)
        } else {
            fshell_ls::list_dir(&t_config)
        }
        .map_err(|e| format!("{}: {}", t_config.path.display(), e))?;
        let do_raw = config.raw;

        if config.recursive {
            let mut canonical_cache = FxHashMap::default();
            let walk = do_recursive_walk(
                &result,
                &t_config,
                env,
                &t_config.path,
                &mut canonical_cache,
                &mut git_status_cache,
            )?;
            let entries_local = walk.0;
            let arena_local = walk.1;
            let tx_clone = tx.clone();
            tokio::spawn(async move {
                for info in &entries_local {
                    let map = fileinfo_to_val_map(info, &arena_local, verbose, do_raw);
                    let payload = PipelinePayload::Data(std::sync::Arc::new(Val::Map(map)));
                    if tx_clone.send(payload).await.is_err() {
                        break;
                    }
                }
            });
        } else {
            let entries_local = result.entries;
            let arena_local = result.arena;
            let tx_clone = tx.clone();
            tokio::spawn(async move {
                for info in &entries_local {
                    let map = fileinfo_to_val_map(info, &arena_local, verbose, do_raw);
                    let payload = PipelinePayload::Data(Arc::new(Val::Map(map)));
                    if tx_clone.send(payload).await.is_err() {
                        break;
                    }
                }
            });
        }
    }

    Ok(())
}

fn cd_change_dir(target: &std::path::Path, env: &Env) -> Result<(), ShellError> {
    let prev_dir = Some(env.cwd().to_string_lossy().to_string());
    change_dir_and_update_caps(target, env)?;
    let _ = crate::cmd::frecency::log_frecency_visit(target);
    let autopushd = env.options.read().autopushd;
    if autopushd && let Some(prev) = prev_dir {
        let mut vars = env.vars.write();
        let mut stack = match vars.get("DIRSTACK") {
            Some(Val::List(list)) => list.clone(),
            _ => Vec::new(),
        };
        stack.insert(0, Val::String(prev));
        vars.insert("DIRSTACK".to_string(), Val::List(stack));
    }
    Ok(())
}

pub fn cd_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let raw_path = if !args.is_empty() {
        match &args[0] {
            Val::String(s) => {
                let expanded = expand_tilde_for_env(s, env);
                if !expanded.exists() {
                    let cdable_vars = env.options.read().cdable_vars;
                    if cdable_vars {
                        let vars = env.vars.read();
                        if let Some(Val::String(var_val)) = vars.get(s) {
                            expand_tilde_for_env(var_val, env)
                        } else {
                            expanded
                        }
                    } else {
                        expanded
                    }
                } else {
                    expanded
                }
            }
            _ => {
                return Err(ShellError::new(
                    ErrorCode::InvalidArgument,
                    "cd argument must be a string path",
                )
                .maybe_with_span(span));
            }
        }
    } else {
        env.home_dir()
    };

    if raw_path.to_str() == Some("-") {
        let oldpwd = match env.vars.read().get("OLDPWD") {
            Some(Val::String(path)) => PathBuf::from(path),
            Some(value) => PathBuf::from(value.to_text()),
            None => return Err("cd: OLDPWD not set".to_string().into()),
        };
        let _ = tx.try_send(PipelinePayload::Data(Arc::new(Val::String(
            oldpwd.display().to_string(),
        ))));
        cd_change_dir(&oldpwd, env)?;
        drop(tx);
        return Ok(());
    }

    let resolved_raw = if raw_path.is_relative() {
        env.cwd().join(&raw_path)
    } else {
        raw_path.clone()
    };

    let target_path = std::fs::canonicalize(&resolved_raw).map_err(|e| BuiltinError::IoError {
        cmd: "cd".into(),
        message: format!("invalid path {raw_path:?}: {e}"),
        span,
    })?;

    cd_change_dir(&target_path, env)?;
    drop(tx);
    Ok(())
}

pub fn pushd_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let mut vars = env.vars.write();

    let mut stack = match vars.get("DIRSTACK") {
        Some(Val::List(list)) => list.clone(),
        _ => Vec::new(),
    };

    let current_dir = env.cwd();

    if args.is_empty() {
        if stack.is_empty() {
            return Err("pushd: directory stack empty".to_string().into());
        }
        let top_val = stack.remove(0);
        let top_str = match &top_val {
            Val::String(s) => s.clone(),
            _ => {
                return Err(ShellError::new(
                    ErrorCode::InvalidArgument,
                    "pushd: invalid entry in stack",
                )
                .maybe_with_span(span));
            }
        };
        let target = env.resolve_path(top_str);

        stack.insert(0, Val::String(current_dir.to_string_lossy().to_string()));
        vars.insert("DIRSTACK".to_string(), Val::List(stack));
        drop(vars);

        change_dir_and_update_caps(&target, env)?;
        let _ = crate::cmd::frecency::log_frecency_visit(&target);
    } else {
        let target_arg = match &args[0] {
            Val::String(s) => s.clone(),
            _ => {
                return Err(ShellError::new(
                    ErrorCode::InvalidArgument,
                    "pushd: argument must be a string path",
                )
                .maybe_with_span(span));
            }
        };
        let target = std::fs::canonicalize(resolve_user_path(&target_arg, env))
            .map_err(|e| format!("pushd: {}: {}", target_arg, e))?;

        stack.insert(0, Val::String(current_dir.to_string_lossy().to_string()));
        vars.insert("DIRSTACK".to_string(), Val::List(stack));
        drop(vars);

        change_dir_and_update_caps(&target, env)?;
        let _ = crate::cmd::frecency::log_frecency_visit(&target);
    }

    send_dir_stack(env, &tx)?;
    drop(tx);
    Ok(())
}

pub fn popd_builtin(
    _in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let mut vars = env.vars.write();

    let mut stack = match vars.get("DIRSTACK") {
        Some(Val::List(list)) => list.clone(),
        _ => Vec::new(),
    };

    if stack.is_empty() {
        return Err("popd: directory stack empty".to_string().into());
    }

    let top_val = stack.remove(0);
    let top_str = match &top_val {
        Val::String(s) => s.clone(),
        _ => {
            return Err(ShellError::new(
                ErrorCode::InvalidArgument,
                "popd: invalid entry in stack",
            )
            .maybe_with_span(span));
        }
    };
    let target = env.resolve_path(top_str);

    vars.insert("DIRSTACK".to_string(), Val::List(stack));
    drop(vars);

    change_dir_and_update_caps(&target, env)?;
    let _ = crate::cmd::frecency::log_frecency_visit(&target);

    send_dir_stack(env, &tx)?;
    drop(tx);
    Ok(())
}

pub fn dirs_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let mut verbose = false;
    for arg in args {
        if let Val::String(s) = arg
            && s == "-v"
        {
            verbose = true;
        }
    }

    let vars = env.vars.read();
    let stack = match vars.get("DIRSTACK") {
        Some(Val::List(list)) => list.clone(),
        _ => Vec::new(),
    };

    let current_dir = env.cwd().to_string_lossy().to_string();

    if verbose {
        let _ = tx.try_send(PipelinePayload::Data(Arc::new(Val::String(format!(
            " 0  {}",
            current_dir
        )))));
        for (idx, item) in stack.iter().enumerate() {
            if let Val::String(s) = item {
                let _ = tx.try_send(PipelinePayload::Data(Arc::new(Val::String(format!(
                    " {}  {}",
                    idx + 1,
                    s
                )))));
            }
        }
    } else {
        let mut output = current_dir.clone();
        for item in &stack {
            if let Val::String(s) = item {
                output.push(' ');
                output.push_str(s);
            }
        }
        let _ = tx.try_send(PipelinePayload::Data(Arc::new(Val::String(output))));
    }

    drop(tx);
    Ok(())
}

fn send_dir_stack(env: &Env, tx: &PipeSender) -> Result<(), ShellError> {
    let vars = env.vars.read();
    let stack = match vars.get("DIRSTACK") {
        Some(Val::List(list)) => list.clone(),
        _ => Vec::new(),
    };
    let current_dir = env.cwd().to_string_lossy().to_string();

    let mut output = current_dir;
    for item in &stack {
        if let Val::String(s) = item {
            output.push(' ');
            output.push_str(s);
        }
    }
    let _ = tx.try_send(PipelinePayload::Data(Arc::new(Val::String(output))));
    Ok(())
}

fn split_multiline_payload(payload: &PipelinePayload) -> Vec<PipelinePayload> {
    if let PipelinePayload::Data(val_arc) = payload
        && let Val::String(s) = val_arc.as_ref()
        && s.contains('\n')
    {
        return s
            .lines()
            .map(|line| PipelinePayload::Data(Arc::new(Val::String(line.to_string()))))
            .collect();
    }
    if matches!(payload, PipelinePayload::Bytes(_)) {
        return split_bytes_lines(payload);
    }
    vec![payload.clone()]
}

/// Splits a raw byte payload into newline-delimited payloads. The terminator is
/// dropped (along with a preceding `\r`), so `head`/`tail`/`uniq` treat a byte
/// stream line-by-line instead of as one opaque item. Payloads without a
/// newline are returned unchanged.
fn split_bytes_lines(payload: &PipelinePayload) -> Vec<PipelinePayload> {
    let PipelinePayload::Bytes(b) = payload else {
        return vec![payload.clone()];
    };
    crate::utils::split_byte_lines(b)
        .into_iter()
        .map(PipelinePayload::Bytes)
        .collect()
}

fn parse_head_tail_args(args: &[Val]) -> Result<(usize, Vec<String>), ShellError> {
    let mut n = 10usize;
    let mut paths = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match &args[i] {
            Val::String(s) if s == "-n" => {
                if i + 1 < args.len() {
                    match &args[i + 1] {
                        Val::Int(val) => n = *val as usize,
                        Val::String(val_str) => {
                            n = val_str
                                .parse::<usize>()
                                .map_err(|_| format!("Invalid number for -n: {val_str}"))?;
                        }
                        _ => {
                            return Err(ShellError::new(
                                ErrorCode::InvalidArgument,
                                "Expected a number after -n",
                            ));
                        }
                    }
                    i += 2;
                } else {
                    return Err(ShellError::new(
                        ErrorCode::InvalidArgument,
                        "Expected a number after -n",
                    ));
                }
            }
            Val::String(s) if s.starts_with("-n") => {
                n = s[2..]
                    .parse::<usize>()
                    .map_err(|_| format!("Invalid option: {s}"))?;
                i += 1;
            }
            Val::Int(count) if *count < 0 => {
                let abs = (*count).unsigned_abs() as usize;
                if abs > 0 {
                    n = abs;
                }
                i += 1;
            }
            Val::String(s) => {
                paths.push(s.clone());
                i += 1;
            }
            _ => {
                return Err(BuiltinError::UnexpectedArgument {
                    cmd: "head/tail".into(),
                    arg: format!("{:?}", args[i]),
                    span: None,
                }
                .into());
            }
        }
    }
    Ok((n, paths))
}

fn resolve_canonical_paths(
    paths: &[String],
    env: &Env,
    cmd: &str,
) -> Result<Vec<PathBuf>, ShellError> {
    let mut canonical = Vec::new();
    for p in paths {
        let raw = resolve_user_path(p, env);
        let path = std::fs::canonicalize(&raw).map_err(|e| format!("Invalid path {raw:?}: {e}"))?;
        check_read_file(env, cmd, path.clone())?;
        canonical.push(path);
    }
    Ok(canonical)
}

pub fn head_builtin(
    in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let (n, paths) = parse_head_tail_args(&args)?;

    if paths.is_empty() {
        if let Some(mut rx) = in_rx {
            tokio::spawn(async move {
                let mut count = 0;
                'outer: while count < n {
                    if let Some(payload) = rx.recv().await {
                        let items = split_multiline_payload(&payload);
                        for item in items {
                            if count >= n {
                                break 'outer;
                            }
                            if tx.send(item).await.is_err() {
                                return;
                            }
                            count += 1;
                        }
                    } else {
                        break;
                    }
                }
            });
        }
        return Ok(());
    }

    let canonical_paths = resolve_canonical_paths(&paths, env, "head")?;

    tokio::spawn(async move {
        for path in canonical_paths {
            let file = match std::fs::File::open(&path) {
                Ok(f) => f,
                Err(e) => {
                    let _ = tx
                        .send(PipelinePayload::Structured(
                            ShellError::new(
                                ErrorCode::IoError,
                                format!("Failed to open file {path:?}: {e}"),
                            )
                            .maybe_with_span(span)
                            .into(),
                        ))
                        .await;
                    return;
                }
            };
            let reader = std::io::BufReader::new(file);
            for line in reader.lines().take(n) {
                match line {
                    Ok(l) => {
                        let payload = PipelinePayload::Data(Arc::new(Val::String(l)));
                        if tx.send(payload).await.is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        let _ = tx
                            .send(PipelinePayload::Structured(
                                ShellError::new(
                                    ErrorCode::IoError,
                                    format!("Error reading {path:?}: {e}"),
                                )
                                .maybe_with_span(span)
                                .into(),
                            ))
                            .await;
                        return;
                    }
                }
            }
        }
    });

    Ok(())
}

pub fn tail_builtin(
    in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let (n, paths) = parse_head_tail_args(&args)?;

    if paths.is_empty() {
        if let Some(mut rx) = in_rx {
            tokio::spawn(async move {
                let mut buffer: std::collections::VecDeque<PipelinePayload> =
                    std::collections::VecDeque::new();
                while let Some(payload) = rx.recv().await {
                    let items = split_multiline_payload(&payload);
                    for item in items {
                        if buffer.len() >= n {
                            buffer.pop_front();
                        }
                        buffer.push_back(item);
                    }
                }
                for payload in buffer {
                    if tx.send(payload).await.is_err() {
                        break;
                    }
                }
            });
        }
        return Ok(());
    }

    let canonical_paths = resolve_canonical_paths(&paths, env, "tail")?;

    tokio::spawn(async move {
        for path in canonical_paths {
            let file = match std::fs::File::open(&path) {
                Ok(f) => f,
                Err(e) => {
                    let _ = tx
                        .send(PipelinePayload::Structured(
                            ShellError::new(
                                ErrorCode::IoError,
                                format!("Failed to open file {path:?}: {e}"),
                            )
                            .maybe_with_span(span)
                            .into(),
                        ))
                        .await;
                    return;
                }
            };
            let reader = std::io::BufReader::new(file);
            let mut buffer: std::collections::VecDeque<String> =
                std::collections::VecDeque::with_capacity(n + 1);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        if buffer.len() >= n {
                            buffer.pop_front();
                        }
                        buffer.push_back(l);
                    }
                    Err(e) => {
                        let _ = tx
                            .send(PipelinePayload::Structured(
                                ShellError::new(
                                    ErrorCode::IoError,
                                    format!("Error reading {path:?}: {e}"),
                                )
                                .maybe_with_span(span)
                                .into(),
                            ))
                            .await;
                        return;
                    }
                }
            }
            for line in buffer {
                let payload = PipelinePayload::Data(Arc::new(Val::String(line)));
                if tx.send(payload).await.is_err() {
                    return;
                }
            }
        }
    });

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DuplicateFilter {
    All,
    RepeatedOnly,
    AllRepeated,
    UniqueOnly,
}

#[derive(Debug, Clone)]
struct UniqArgs {
    filter: DuplicateFilter,
    count: bool,
    ignore_case: bool,
    skip_fields: usize,
    skip_chars: usize,
    zero_terminated: bool,
    input_file: Option<String>,
    output_file: Option<String>,
}

fn parse_uniq_args(args: &[Val], span: Option<SourceSpan>) -> Result<UniqArgs, ShellError> {
    let mut config = UniqArgs {
        filter: DuplicateFilter::All,
        count: false,
        ignore_case: false,
        skip_fields: 0,
        skip_chars: 0,
        zero_terminated: false,
        input_file: None,
        output_file: None,
    };
    let mut end_of_opts = false;
    let mut idx = 0;

    while idx < args.len() {
        let arg = &args[idx];
        idx += 1;

        let s = match arg {
            Val::String(s) => s.clone(),
            Val::Int(n) if !end_of_opts && *n < 0 => n.to_string(),
            other => {
                if end_of_opts || config.input_file.is_none() || config.output_file.is_none() {
                    val_to_display_string(other)
                } else {
                    return Err(ShellError::new(
                        ErrorCode::InvalidArgument,
                        "uniq: unexpected non-string argument",
                    )
                    .maybe_with_span(span));
                }
            }
        };

        if !end_of_opts && s == "--" {
            end_of_opts = true;
            continue;
        }

        if !end_of_opts && s.starts_with("--") && s.len() > 2 {
            let opt = &s[2..];
            if let Some(eq_pos) = opt.find('=') {
                let key = &opt[..eq_pos];
                let val = &opt[eq_pos + 1..];
                match key {
                    "count" => config.count = true,
                    "repeated" => config.filter = DuplicateFilter::RepeatedOnly,
                    "all-repeated" => config.filter = DuplicateFilter::AllRepeated,
                    "unique" => config.filter = DuplicateFilter::UniqueOnly,
                    "ignore-case" => config.ignore_case = true,
                    "zero-terminated" => config.zero_terminated = true,
                    "skip-fields" => {
                        config.skip_fields = val.parse::<usize>().map_err(|_| {
                            ShellError::new(
                                ErrorCode::InvalidArgument,
                                format!("uniq: invalid number of fields to skip: '{val}'"),
                            )
                            .maybe_with_span(span)
                        })?;
                    }
                    "skip-chars" => {
                        config.skip_chars = val.parse::<usize>().map_err(|_| {
                            ShellError::new(
                                ErrorCode::InvalidArgument,
                                format!("uniq: invalid number of bytes to skip: '{val}'"),
                            )
                            .maybe_with_span(span)
                        })?;
                    }
                    _ => {
                        return Err(ShellError::new(
                            ErrorCode::InvalidArgument,
                            format!("uniq: unrecognized option '--{key}'"),
                        )
                        .maybe_with_span(span));
                    }
                }
            } else {
                match opt {
                    "count" => config.count = true,
                    "repeated" => config.filter = DuplicateFilter::RepeatedOnly,
                    "all-repeated" => config.filter = DuplicateFilter::AllRepeated,
                    "unique" => config.filter = DuplicateFilter::UniqueOnly,
                    "ignore-case" => config.ignore_case = true,
                    "zero-terminated" => config.zero_terminated = true,
                    "skip-fields" => {
                        if idx < args.len() {
                            let val_str = val_to_display_string(&args[idx]);
                            idx += 1;
                            config.skip_fields = val_str.parse::<usize>().map_err(|_| {
                                ShellError::new(
                                    ErrorCode::InvalidArgument,
                                    format!("uniq: invalid number of fields to skip: '{val_str}'"),
                                )
                                .maybe_with_span(span)
                            })?;
                        } else {
                            return Err(ShellError::new(
                                ErrorCode::InvalidArgument,
                                "uniq: option '--skip-fields' requires an argument",
                            )
                            .maybe_with_span(span));
                        }
                    }
                    "skip-chars" => {
                        if idx < args.len() {
                            let val_str = val_to_display_string(&args[idx]);
                            idx += 1;
                            config.skip_chars = val_str.parse::<usize>().map_err(|_| {
                                ShellError::new(
                                    ErrorCode::InvalidArgument,
                                    format!("uniq: invalid number of bytes to skip: '{val_str}'"),
                                )
                                .maybe_with_span(span)
                            })?;
                        } else {
                            return Err(ShellError::new(
                                ErrorCode::InvalidArgument,
                                "uniq: option '--skip-chars' requires an argument",
                            )
                            .maybe_with_span(span));
                        }
                    }
                    _ => {
                        return Err(ShellError::new(
                            ErrorCode::InvalidArgument,
                            format!("uniq: unrecognized option '--{opt}'"),
                        )
                        .maybe_with_span(span));
                    }
                }
            }
        } else if !end_of_opts && s.starts_with('-') && s.len() > 1 && s != "-" {
            let chars: Vec<char> = s[1..].chars().collect();
            let mut c_idx = 0;
            while c_idx < chars.len() {
                let ch = chars[c_idx];
                match ch {
                    'c' => config.count = true,
                    'd' => config.filter = DuplicateFilter::RepeatedOnly,
                    'D' => config.filter = DuplicateFilter::AllRepeated,
                    'u' => config.filter = DuplicateFilter::UniqueOnly,
                    'i' => config.ignore_case = true,
                    'z' => config.zero_terminated = true,
                    'f' => {
                        let val_str = if c_idx + 1 < chars.len() {
                            chars[c_idx + 1..].iter().collect()
                        } else if idx < args.len() {
                            let val = val_to_display_string(&args[idx]);
                            idx += 1;
                            val
                        } else {
                            return Err(ShellError::new(
                                ErrorCode::InvalidArgument,
                                "uniq: option requires an argument -- 'f'",
                            )
                            .maybe_with_span(span));
                        };
                        config.skip_fields = val_str.parse::<usize>().map_err(|_| {
                            ShellError::new(
                                ErrorCode::InvalidArgument,
                                format!("uniq: invalid number of fields to skip: '{val_str}'"),
                            )
                            .maybe_with_span(span)
                        })?;
                        break;
                    }
                    's' => {
                        let val_str = if c_idx + 1 < chars.len() {
                            chars[c_idx + 1..].iter().collect()
                        } else if idx < args.len() {
                            let val = val_to_display_string(&args[idx]);
                            idx += 1;
                            val
                        } else {
                            return Err(ShellError::new(
                                ErrorCode::InvalidArgument,
                                "uniq: option requires an argument -- 's'",
                            )
                            .maybe_with_span(span));
                        };
                        config.skip_chars = val_str.parse::<usize>().map_err(|_| {
                            ShellError::new(
                                ErrorCode::InvalidArgument,
                                format!("uniq: invalid number of bytes to skip: '{val_str}'"),
                            )
                            .maybe_with_span(span)
                        })?;
                        break;
                    }
                    _ => {
                        return Err(ShellError::new(
                            ErrorCode::InvalidArgument,
                            format!("uniq: invalid option -- '{ch}'"),
                        )
                        .maybe_with_span(span));
                    }
                }
                c_idx += 1;
            }
        } else if config.input_file.is_none() {
            if s != "-" {
                config.input_file = Some(s);
            }
        } else if config.output_file.is_none() {
            if s != "-" {
                config.output_file = Some(s);
            }
        } else {
            return Err(ShellError::new(
                ErrorCode::InvalidArgument,
                format!("uniq: extra operand '{s}'"),
            )
            .maybe_with_span(span));
        }
    }

    Ok(config)
}

fn extract_comparison_key<'a>(
    s: &'a str,
    skip_fields: usize,
    skip_chars: usize,
    ignore_case: bool,
) -> std::borrow::Cow<'a, str> {
    let mut remainder = s;
    if skip_fields > 0 {
        let mut fields_skipped = 0;
        let mut chars = remainder.char_indices().peekable();
        let mut end_offset = remainder.len();
        while fields_skipped < skip_fields {
            while let Some(&(_, ch)) = chars.peek() {
                if ch.is_whitespace() {
                    chars.next();
                } else {
                    break;
                }
            }
            if chars.peek().is_none() {
                end_offset = remainder.len();
                break;
            }
            while let Some(&(_, ch)) = chars.peek() {
                if !ch.is_whitespace() {
                    chars.next();
                } else {
                    break;
                }
            }
            fields_skipped += 1;
            if fields_skipped == skip_fields {
                end_offset = chars.peek().map(|&(idx, _)| idx).unwrap_or(remainder.len());
                break;
            }
        }
        remainder = &remainder[end_offset..];
    }

    if skip_chars > 0 {
        let skip_idx = remainder
            .char_indices()
            .nth(skip_chars)
            .map(|(idx, _)| idx)
            .unwrap_or(remainder.len());
        remainder = &remainder[skip_idx..];
    }

    if ignore_case {
        std::borrow::Cow::Owned(remainder.to_lowercase())
    } else {
        std::borrow::Cow::Borrowed(remainder)
    }
}

enum UniqOutput {
    Channel(PipeSender),
    File(tokio::io::BufWriter<tokio::fs::File>),
}

impl UniqOutput {
    async fn emit_payload(
        &mut self,
        payload: PipelinePayload,
        zero_terminated: bool,
    ) -> Result<(), ()> {
        match self {
            Self::Channel(tx) => tx.send(payload).await.map_err(|_| ()),
            Self::File(writer) => {
                use tokio::io::AsyncWriteExt;
                let text = match &payload {
                    PipelinePayload::Data(val) => val_to_display_string(val),
                    PipelinePayload::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                    PipelinePayload::Structured(_) => return Ok(()),
                };
                let sep = if zero_terminated { b"\0" } else { b"\n" };
                writer.write_all(text.as_bytes()).await.map_err(|_| ())?;
                writer.write_all(sep).await.map_err(|_| ())?;
                Ok(())
            }
        }
    }

    async fn flush(&mut self) -> Result<(), ()> {
        if let Self::File(writer) = self {
            use tokio::io::AsyncWriteExt;
            writer.flush().await.map_err(|_| ())?;
        }
        Ok(())
    }
}

fn format_count_payload(count: usize, payload: &PipelinePayload) -> PipelinePayload {
    match payload {
        PipelinePayload::Data(val) => {
            let s = val_to_display_string(val);
            PipelinePayload::Data(Arc::new(Val::String(format!("{:7} {}", count, s))))
        }
        PipelinePayload::Bytes(b) => {
            let s = String::from_utf8_lossy(b);
            PipelinePayload::Data(Arc::new(Val::String(format!("{:7} {}", count, s))))
        }
        other => other.clone(),
    }
}

struct UniqEngine {
    config: UniqArgs,
    output: UniqOutput,
    current_key: Option<String>,
    current_item: Option<PipelinePayload>,
    count: usize,
    emitted_count: usize,
}

impl UniqEngine {
    fn new(config: UniqArgs, output: UniqOutput) -> Self {
        Self {
            config,
            output,
            current_key: None,
            current_item: None,
            count: 0,
            emitted_count: 0,
        }
    }

    fn key_for_payload(&self, payload: &PipelinePayload) -> String {
        match payload {
            PipelinePayload::Data(val) => match val.as_ref() {
                Val::String(s) => extract_comparison_key(
                    s,
                    self.config.skip_fields,
                    self.config.skip_chars,
                    self.config.ignore_case,
                )
                .into_owned(),
                other => {
                    if self.config.skip_fields == 0
                        && self.config.skip_chars == 0
                        && !self.config.ignore_case
                    {
                        format!("{other:?}")
                    } else {
                        let s = val_to_display_string(other);
                        extract_comparison_key(
                            &s,
                            self.config.skip_fields,
                            self.config.skip_chars,
                            self.config.ignore_case,
                        )
                        .into_owned()
                    }
                }
            },
            PipelinePayload::Bytes(b) => {
                let s = String::from_utf8_lossy(b);
                extract_comparison_key(
                    &s,
                    self.config.skip_fields,
                    self.config.skip_chars,
                    self.config.ignore_case,
                )
                .into_owned()
            }
            PipelinePayload::Structured(_) => String::new(),
        }
    }

    async fn feed(&mut self, item: PipelinePayload) -> Result<(), ()> {
        let key = self.key_for_payload(&item);
        let is_same = self.current_key.as_ref() == Some(&key);

        if is_same {
            self.count += 1;
            match self.config.filter {
                DuplicateFilter::AllRepeated => {
                    if self.count == 2 {
                        if let Some(first) = self.current_item.take() {
                            self.output
                                .emit_payload(first, self.config.zero_terminated)
                                .await?;
                        }
                        self.output
                            .emit_payload(item, self.config.zero_terminated)
                            .await?;
                    } else if self.count > 2 {
                        self.output
                            .emit_payload(item, self.config.zero_terminated)
                            .await?;
                    }
                }
                DuplicateFilter::RepeatedOnly if !self.config.count && self.emitted_count == 0 => {
                    self.output
                        .emit_payload(item, self.config.zero_terminated)
                        .await?;
                    self.emitted_count = 1;
                }
                _ => {}
            }
        } else {
            self.flush_current().await?;
            self.current_key = Some(key);
            self.count = 1;
            self.emitted_count = 0;

            if self.config.filter == DuplicateFilter::All && !self.config.count {
                self.output
                    .emit_payload(item.clone(), self.config.zero_terminated)
                    .await?;
                self.emitted_count = 1;
                self.current_item = None;
            } else {
                self.current_item = Some(item);
            }
        }
        Ok(())
    }

    async fn flush_current(&mut self) -> Result<(), ()> {
        if self.count == 0 {
            return Ok(());
        }

        let count = self.count;
        let item = self.current_item.take();

        match self.config.filter {
            DuplicateFilter::All => {
                if self.config.count
                    && let Some(it) = item
                {
                    let formatted = format_count_payload(count, &it);
                    self.output
                        .emit_payload(formatted, self.config.zero_terminated)
                        .await?;
                }
            }
            DuplicateFilter::UniqueOnly => {
                if count == 1
                    && let Some(it) = item
                {
                    let payload = if self.config.count {
                        format_count_payload(count, &it)
                    } else {
                        it
                    };
                    self.output
                        .emit_payload(payload, self.config.zero_terminated)
                        .await?;
                }
            }
            DuplicateFilter::RepeatedOnly => {
                if count >= 2
                    && self.config.count
                    && let Some(it) = item
                {
                    let formatted = format_count_payload(count, &it);
                    self.output
                        .emit_payload(formatted, self.config.zero_terminated)
                        .await?;
                }
            }
            DuplicateFilter::AllRepeated => {}
        }

        self.count = 0;
        self.emitted_count = 0;
        self.current_key = None;
        Ok(())
    }

    async fn finish(&mut self) -> Result<(), ()> {
        self.flush_current().await?;
        self.output.flush().await
    }
}

pub fn uniq_builtin(
    in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let config = parse_uniq_args(&args, span)?;

    let canonical_input = if let Some(ref p) = config.input_file {
        let raw = resolve_user_path(p, env);
        let path = std::fs::canonicalize(&raw).map_err(|e| format!("Invalid path {raw:?}: {e}"))?;
        check_read_file(env, "uniq", path.clone())?;
        Some(path)
    } else {
        None
    };

    let canonical_output = if let Some(ref p) = config.output_file {
        let raw = resolve_user_path(p, env);
        let path = if raw.exists() {
            std::fs::canonicalize(&raw).map_err(|e| format!("Invalid path {raw:?}: {e}"))?
        } else if let Some(parent) = raw.parent() {
            let canon_parent =
                std::fs::canonicalize(parent).map_err(|e| format!("Invalid path {raw:?}: {e}"))?;
            canon_parent.join(raw.file_name().unwrap_or_default())
        } else {
            raw
        };
        env.enforce_capability("uniq", CapAction::WriteFile(path.clone()))?;
        Some(path)
    } else {
        None
    };

    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;

        let output = if let Some(out_path) = canonical_output {
            match tokio::fs::File::create(&out_path).await {
                Ok(f) => UniqOutput::File(tokio::io::BufWriter::new(f)),
                Err(e) => {
                    let _ = tx
                        .send(PipelinePayload::Structured(
                            ShellError::new(
                                ErrorCode::IoError,
                                format!("Failed to create output file {:?}: {}", out_path, e),
                            )
                            .maybe_with_span(span)
                            .into(),
                        ))
                        .await;
                    return;
                }
            }
        } else {
            UniqOutput::Channel(tx.clone())
        };

        let mut engine = UniqEngine::new(config, output);

        if let Some(in_path) = canonical_input {
            match tokio::fs::File::open(&in_path).await {
                Ok(file) => {
                    let reader = tokio::io::BufReader::new(file);
                    let mut lines = reader.lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let payload = PipelinePayload::Data(Arc::new(Val::String(line)));
                        if engine.feed(payload).await.is_err() {
                            return;
                        }
                    }
                }
                Err(e) => {
                    let _ = tx
                        .send(PipelinePayload::Structured(
                            ShellError::new(
                                ErrorCode::IoError,
                                format!("Failed to read file {:?}: {}", in_path, e),
                            )
                            .maybe_with_span(span)
                            .into(),
                        ))
                        .await;
                    return;
                }
            }
        } else if let Some(mut rx) = in_rx {
            while let Some(payload) = rx.recv().await {
                if let PipelinePayload::Structured(s) = payload {
                    if tx.send(PipelinePayload::Structured(s)).await.is_err() {
                        return;
                    }
                    continue;
                }
                for item in split_multiline_payload(&payload) {
                    if engine.feed(item).await.is_err() {
                        return;
                    }
                }
            }
        } else {
            let stdin = tokio::io::stdin();
            let reader = tokio::io::BufReader::new(stdin);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let payload = PipelinePayload::Data(Arc::new(Val::String(line)));
                if engine.feed(payload).await.is_err() {
                    return;
                }
            }
        }

        let _ = engine.finish().await;
    });

    Ok(())
}

pub fn echo_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    _env: &Env,
    tx: PipeSender,
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let mut no_newline = false;
    let mut interpret_escapes = false;
    let mut idx = 0;

    while idx < args.len() {
        let s = val_to_display_string(&args[idx]);
        if s.starts_with('-')
            && s.len() > 1
            && s.chars().skip(1).all(|c| c == 'n' || c == 'e' || c == 'E')
        {
            for c in s.chars().skip(1) {
                match c {
                    'n' => no_newline = true,
                    'e' => interpret_escapes = true,
                    'E' => interpret_escapes = false,
                    _ => {}
                }
            }
            idx += 1;
        } else {
            break;
        }
    }

    let mut parts = Vec::new();
    for arg in &args[idx..] {
        parts.push(val_to_display_string(arg));
    }
    let echo_str = parts.join(" ");

    let (mut result_str, stop_output) = if interpret_escapes {
        interpret_ansi_escapes(&echo_str)
    } else {
        (echo_str, false)
    };

    if no_newline || stop_output {
        result_str.push('\0');
    }

    tokio::spawn(async move {
        let _ = tx
            .send(PipelinePayload::Data(Arc::new(Val::String(result_str))))
            .await;
    });

    Ok(())
}

pub fn clear_builtin(
    _in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    _env: &Env,
    _tx: PipeSender,
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    print!("\x1B[2J\x1B[3J\x1B[1;1H");
    let _ = std::io::stdout().flush();
    Ok(())
}

pub fn wrap_builtin(
    _in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    _env: &Env,
    _tx: PipeSender,
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let (_, h) = crossterm::terminal::size().unwrap_or((80, 24));
    print!("{}", "\n".repeat(h as usize));
    print!("\x1B[1;1H");
    let _ = std::io::stdout().flush();
    Ok(())
}

pub fn type_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let names: Vec<String> = args
        .into_iter()
        .map(|arg| match arg {
            Val::String(s) => Ok(s),
            other => Err(format!("type: arguments must be strings, got {:?}", other)),
        })
        .collect::<Result<Vec<_>, _>>()?;

    if names.is_empty() {
        return Err(
            ShellError::new(ErrorCode::MissingArgument, "type: missing operand")
                .maybe_with_span(span),
        );
    }

    for name in names {
        let result = type_one(&name, env);
        let tx_clone = tx.clone();
        tokio::spawn(async move {
            let _ = tx_clone.send(PipelinePayload::Data(Arc::new(result))).await;
        });
    }

    Ok(())
}

fn type_one(name: &str, env: &Env) -> Val {
    if env.get_builtin(name).is_some() {
        return Val::Map({
            let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            m.insert(ustr::ustr("name"), Val::String(name.to_string()));
            m.insert(ustr::ustr("type"), Val::String("builtin".to_string()));
            m
        });
    }

    if env.fns.read().contains_key(name) {
        return Val::Map({
            let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            m.insert(ustr::ustr("name"), Val::String(name.to_string()));
            m.insert(ustr::ustr("type"), Val::String("user-function".to_string()));
            m
        });
    }

    let env_path = Some(env.vars.read()).and_then(|vars| {
        vars.get("env").and_then(|v| {
            if let fshell_core::Val::Map(map) = v {
                map.get(&ustr::ustr("PATH")).and_then(|pv| {
                    if let fshell_core::Val::String(s) = pv {
                        Some(s.clone())
                    } else {
                        None
                    }
                })
            } else {
                None
            }
        })
    });
    let normalized_path =
        fshell_engine::normalize_path_for_cwd(env_path.as_deref().unwrap_or_default(), &env.cwd());
    if fshell_engine::is_external_command_at(name, Some(&normalized_path), &env.cwd()) {
        let path =
            fshell_engine::resolve_cached_command_path_at(name, Some(&normalized_path), &env.cwd())
                .map(std::path::PathBuf::from);
        let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        m.insert(ustr::ustr("name"), Val::String(name.to_string()));
        m.insert(ustr::ustr("type"), Val::String("external".to_string()));
        if let Some(p) = path {
            m.insert(
                ustr::ustr("path"),
                Val::String(p.to_string_lossy().to_string()),
            );
        }
        return Val::Map(m);
    }

    Val::Map({
        let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        m.insert(ustr::ustr("name"), Val::String(name.to_string()));
        m.insert(ustr::ustr("type"), Val::String("not-found".to_string()));
        m
    })
}

pub fn pwd_builtin(
    _in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let current_dir = env.cwd();
    env.enforce_capability("pwd", CapAction::ReadDir(current_dir.clone()))?;

    let path_str = current_dir.to_string_lossy().to_string();
    tokio::spawn(async move {
        let _ = tx
            .send(PipelinePayload::Data(Arc::new(Val::String(path_str))))
            .await;
    });

    Ok(())
}

pub fn watch_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let mut path_args = Vec::new();
    for arg in &args {
        if let Val::String(s) = arg {
            path_args.push(s.clone());
        } else {
            return Err(ShellError::new(
                ErrorCode::InvalidArgument,
                "watch argument must be a string path",
            )
            .maybe_with_span(span));
        }
    }

    let raw_path = if !path_args.is_empty() {
        resolve_user_path(&path_args[0], env)
    } else {
        env.cwd()
    };

    let target_path = std::fs::canonicalize(&raw_path)
        .map_err(|e| format!("Invalid path {:?}: {}", raw_path, e))?;

    env.enforce_capability("watch", CapAction::ReadDir(target_path.clone()))?;

    env.track_read(target_path.clone());

    if target_path.is_dir() {
        let entries = std::fs::read_dir(&target_path)
            .map_err(|e| format!("Failed to read directory {:?}: {}", target_path, e))?;

        tokio::spawn(async move {
            for entry in entries.flatten() {
                let metadata = entry.metadata().ok();
                let file_name = entry.file_name().to_string_lossy().to_string();

                if file_name.starts_with('.') {
                    continue;
                }

                let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                let size = metadata.as_ref().map(|m| m.len() as i64).unwrap_or(0);

                let mut map =
                    indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
                map.insert(ustr::ustr("name"), Val::String(file_name));
                map.insert(
                    ustr::ustr("type"),
                    Val::String(if is_dir {
                        "dir".to_string()
                    } else {
                        "file".to_string()
                    }),
                );
                map.insert(ustr::ustr("size"), Val::Int(size));

                if let Some(m) = metadata
                    && let Ok(modified) = m.modified()
                {
                    let datetime: chrono::DateTime<chrono::Utc> = modified.into();
                    map.insert(ustr::ustr("last_modified"), Val::DateTime(datetime));
                }

                let payload = PipelinePayload::Data(Arc::new(Val::Map(map)));
                if tx.send(payload).await.is_err() {
                    break;
                }
            }
        });
    } else {
        let metadata = std::fs::metadata(&target_path)
            .map_err(|e| format!("Failed to access file {:?}: {}", target_path, e))?;
        tokio::spawn(async move {
            let file_name = target_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let size = metadata.len() as i64;
            let mut map = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            map.insert(ustr::ustr("name"), Val::String(file_name));
            map.insert(ustr::ustr("type"), Val::String("file".to_string()));
            map.insert(ustr::ustr("size"), Val::Int(size));
            if let Ok(modified) = metadata.modified() {
                let datetime: chrono::DateTime<chrono::Utc> = modified.into();
                map.insert(ustr::ustr("last_modified"), Val::DateTime(datetime));
            }
            let payload = PipelinePayload::Data(Arc::new(Val::Map(map)));
            let _ = tx.send(payload).await;
        });
    }

    Ok(())
}

pub fn mkdir_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    if args.is_empty() {
        return Err(
            ShellError::new(ErrorCode::MissingArgument, "mkdir: missing operand")
                .maybe_with_span(span),
        );
    }

    let mut make_parents = false;
    let mut paths = Vec::new();

    for arg in args {
        if let Val::String(s) = arg {
            if s == "-p" || s == "--parents" {
                make_parents = true;
            } else {
                paths.push(s);
            }
        }
    }

    if paths.is_empty() {
        return Err(
            ShellError::new(ErrorCode::MissingArgument, "mkdir: missing operand")
                .maybe_with_span(span),
        );
    }

    for path_str in paths {
        let path = resolve_user_path(&path_str, env);
        env.enforce_capability("mkdir", CapAction::WriteDir(path.clone()))?;

        if make_parents {
            std::fs::create_dir_all(&path).map_err(|e| {
                format!("mkdir: cannot create directory '{}': {}", path.display(), e)
            })?;
        } else {
            std::fs::create_dir(&path).map_err(|e| {
                format!("mkdir: cannot create directory '{}': {}", path.display(), e)
            })?;
        }
    }

    drop(tx);
    Ok(())
}

#[cfg(unix)]
fn touch_existing_file(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let path_cstr = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let ret = unsafe { libc::utimensat(libc::AT_FDCWD, path_cstr.as_ptr(), std::ptr::null(), 0) };
    if ret == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn touch_existing_file(_path: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

pub fn touch_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    if args.is_empty() {
        return Err(
            ShellError::new(ErrorCode::MissingArgument, "touch: missing file operand")
                .maybe_with_span(span),
        );
    }

    for arg in args {
        let Val::String(s) = arg else {
            return Err(ShellError::new(
                ErrorCode::InvalidArgument,
                "touch: argument must be a string",
            )
            .maybe_with_span(span));
        };
        let path = resolve_user_path(&s, env);
        env.enforce_capability("touch", CapAction::WriteFile(path.clone()))?;

        if path.exists() {
            touch_existing_file(&path)
                .map_err(|e| format!("touch: cannot set times for '{}': {}", path.display(), e))?;
        } else {
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&path)
                .map_err(|e| format!("touch: cannot create file '{}': {}", path.display(), e))?;
        }
    }

    drop(tx);
    Ok(())
}

pub fn cat_builtin(
    in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    if args.is_empty() {
        if let Some(mut stream) = in_rx {
            tokio::spawn(async move {
                while let Some(payload) = stream.recv().await {
                    if tx.send(payload).await.is_err() {
                        break;
                    }
                }
            });
        }
        return Ok(());
    }

    let mut paths = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if let Val::String(ref s) = args[i] {
            paths.push(s.clone());
        }
        i += 1;
    }

    let tx_clone = tx.clone();
    let env_clone = env.clone();

    tokio::spawn(async move {
        let mut in_rx = in_rx;
        for path_str in paths {
            if path_str == "-" {
                if let Some(mut stream) = in_rx.take() {
                    while let Some(payload) = stream.recv().await {
                        if tx_clone.send(payload).await.is_err() {
                            return;
                        }
                    }
                }
                continue;
            }

            let path = resolve_user_path(&path_str, &env_clone);
            if let Err(e) = check_read_file(&env_clone, "cat", path.clone()) {
                let _ = tx_clone
                    .send(PipelinePayload::Data(Arc::new(Val::String(format!(
                        "cat: {}",
                        e
                    )))))
                    .await;
                continue;
            }

            match tokio::fs::File::open(&path).await {
                Ok(file) => {
                    use std::sync::atomic::Ordering;
                    use tokio::io::AsyncBufReadExt;
                    let mut reader = tokio::io::BufReader::new(file);
                    let mut line_buf = Vec::new();
                    loop {
                        if env_clone.job_control.cancellation.load(Ordering::Acquire) {
                            break;
                        }
                        line_buf.clear();
                        match reader.read_until(b'\n', &mut line_buf).await {
                            Ok(0) => break, // EOF
                            Ok(_) => match std::str::from_utf8(&line_buf) {
                                Ok(text) => {
                                    let trimmed = text
                                        .strip_suffix("\r\n")
                                        .unwrap_or_else(|| text.strip_suffix('\n').unwrap_or(text));
                                    if tx_clone
                                        .send(PipelinePayload::Data(Arc::new(Val::String(
                                            trimmed.to_string(),
                                        ))))
                                        .await
                                        .is_err()
                                    {
                                        return;
                                    }
                                }
                                Err(_) => {
                                    if tx_clone
                                        .send(PipelinePayload::Data(Arc::new(Val::Blob(
                                            line_buf.clone(),
                                        ))))
                                        .await
                                        .is_err()
                                    {
                                        return;
                                    }
                                }
                            },
                            Err(e) => {
                                let _ =
                                    tx_clone
                                        .send(PipelinePayload::Data(Arc::new(Val::String(
                                            format!("cat: {}: {}", path.display(), e),
                                        ))))
                                        .await;
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = tx_clone
                        .send(PipelinePayload::Data(Arc::new(Val::String(format!(
                            "cat: {}: {}",
                            path.display(),
                            e
                        )))))
                        .await;
                }
            }
        }
    });

    Ok(())
}
