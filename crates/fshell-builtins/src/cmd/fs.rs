// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![allow(clippy::unnecessary_cast)]
use crate::error::BuiltinError;
use crate::utils::{
    change_dir_and_update_caps, check_read_file, expand_tilde, interpret_ansi_escapes,
    val_to_display_string,
};
use fshell_core::ShellError;
use fshell_core::Val;
use fshell_core::diagnostic::ErrorCode;
use fshell_engine::{CapAction, Env, PipeSender, PipeStream, PipelinePayload};
use miette::SourceSpan;
use std::io::{BufRead, Write};
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
    cwd: &std::path::Path,
) -> Result<(fshell_ls::Config, Vec<String>, bool), String> {
    let is_tty = fshell_engine::is_stdout_a_tty();

    let mut ls = LsArgs {
        sort: fshell_ls::SortMode::Name,
        reverse: false,
        color: is_tty,
        icons: false,
        tree: false,
        depth: None,
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
                    "group-directories-first" => ls.group_dirs = true,
                    "icons" => ls.icons = true,
                    "git" => ls.git = true,
                    "dereference" => ls.dereference = true,
                    "recurse" => ls.recursive = true,
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
            for ch in s.chars().skip(1) {
                match ch {
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
        expand_tilde(&path_args[0])
    } else {
        cwd.to_path_buf()
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
    let name_start = info.entry.start();
    let name_end = name_start + info.entry.len();
    let name_bytes = &arena[name_start..name_end];
    let name_str = std::str::from_utf8(name_bytes).unwrap_or("?");

    let is_dir = info.entry.is_dir();
    let mut map = fshell_core::FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    map.insert(ustr::ustr("name"), Val::String(name_str.to_string()));
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
        is_exec = (meta.mode & (libc::S_IXUSR as u32)) != 0;
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
    cache: &mut std::collections::HashMap<std::path::PathBuf, std::path::PathBuf>,
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
    canonical_cache: &mut std::collections::HashMap<std::path::PathBuf, std::path::PathBuf>,
) -> Result<(Vec<fshell_ls::FileInfo>, Vec<u8>), String> {
    let mut visited: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    visited.insert(canonicalize_cached(root, canonical_cache));

    fn is_dir_entry(
        entry: &fshell_ls::FileInfo,
        arena: &[u8],
        base: &std::path::Path,
    ) -> Option<std::path::PathBuf> {
        if entry.entry.is_dir() {
            let start = entry.entry.start();
            let end = start + entry.entry.len();
            let name = std::str::from_utf8(&arena[start..end]).ok()?;
            Some(base.join(name))
        } else {
            None
        }
    }

    let mut dirs_to_visit: Vec<std::path::PathBuf> = initial
        .entries
        .iter()
        .filter_map(|e| is_dir_entry(e, &initial.arena, root))
        .collect();

    let mut entries = initial.entries.clone();
    let mut arena = initial.arena.clone();

    while let Some(dir) = dirs_to_visit.pop() {
        let canonical = canonicalize_cached(&dir, canonical_cache);
        if !visited.insert(canonical) {
            continue;
        }

        let mut sub_config = config.clone();
        sub_config.path = dir.clone();

        let allowed = env.caps.caps.read().check_read_dir(&sub_config.path);
        if allowed && let Ok(sub_result) = fshell_ls::list_dir(&sub_config) {
            for entry in &sub_result.entries {
                if let Some(subdir_path) = is_dir_entry(entry, &sub_result.arena, &dir) {
                    dirs_to_visit.push(subdir_path);
                }
            }
            for entry in sub_result.entries {
                let start = entry.entry.start();
                let end = start + entry.entry.len();
                if let Ok(name_str) = std::str::from_utf8(&sub_result.arena[start..end]) {
                    let full_entry_path = dir.join(name_str);
                    let rel_path = full_entry_path
                        .strip_prefix(root)
                        .unwrap_or(&full_entry_path)
                        .to_string_lossy()
                        .to_string();
                    let offset = arena.len();
                    arena.extend_from_slice(rel_path.as_bytes());
                    let mut entry_clone = entry.clone();
                    entry_clone.entry =
                        fshell_ls::Entry::new(offset, rel_path.len(), entry.entry.is_dir());
                    entries.push(entry_clone);
                }
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
    let (mut config, path_args, verbose) = parse_ls_args_to_rrls_config(&args, &env.cwd())?;

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
            .map(|p| {
                let expanded = expand_tilde(p);
                if expanded.is_relative() {
                    env.cwd().join(expanded)
                } else {
                    expanded
                }
            })
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
                println!("{}:", target.display());
            }

            if config.recursive && !config.tree {
                let mut paths = vec![t_config.path.clone()];
                let mut is_first = true;
                while let Some(current_path) = paths.pop() {
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

                    if let Ok(sub_result) = fshell_ls::list_dir(&sub_config) {
                        if !is_first {
                            println!();
                        }
                        is_first = false;
                        println!("{}:", current_path.display());

                        let _ = fshell_ls::render(&sub_result, &sub_config, |p| {
                            env.caps.caps.read().check_read_dir(p)
                                || p.canonicalize()
                                    .ok()
                                    .as_ref()
                                    .is_some_and(|cp| env.caps.caps.read().check_read_dir(cp))
                        });

                        let mut subdirs = Vec::new();
                        for entry in &sub_result.entries {
                            if entry.entry.is_dir() {
                                let start = entry.entry.start();
                                let end = start + entry.entry.len();
                                if let Ok(name) = std::str::from_utf8(&sub_result.arena[start..end])
                                {
                                    subdirs.push(current_path.join(name));
                                }
                            }
                        }
                        subdirs.reverse();
                        paths.extend(subdirs);
                    }
                }
            } else {
                let all_entries = fshell_ls::list_dir(&t_config)
                    .map_err(|e| format!("{}: {}", t_config.path.display(), e))?;
                fshell_ls::render(&all_entries, &t_config, |p| {
                    env.caps.caps.read().check_read_dir(p)
                        || p.canonicalize()
                            .ok()
                            .as_ref()
                            .is_some_and(|cp| env.caps.caps.read().check_read_dir(cp))
                })
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
                buf.extend_from_slice(format!("{}:\n", target.display()).as_bytes());
            }

            fshell_ls::tree::render_tree(&t_config, &mut buf, |p| {
                !env.is_strict_mode()
                    || env.caps.caps.read().check_read_dir(p)
                    || p.canonicalize()
                        .ok()
                        .as_ref()
                        .is_some_and(|cp| env.caps.caps.read().check_read_dir(cp))
            })
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

        let result = fshell_ls::list_dir(&t_config)
            .map_err(|e| format!("{}: {}", t_config.path.display(), e))?;
        let do_raw = config.raw;

        if config.recursive {
            let mut canonical_cache: std::collections::HashMap<
                std::path::PathBuf,
                std::path::PathBuf,
            > = std::collections::HashMap::new();
            let walk = do_recursive_walk(
                &result,
                &t_config,
                env,
                &t_config.path,
                &mut canonical_cache,
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
                let expanded = expand_tilde(s);
                if !expanded.exists() {
                    let cdable_vars = env.options.read().cdable_vars;
                    if cdable_vars {
                        let vars = env.vars.read();
                        if let Some(Val::String(var_val)) = vars.get(s) {
                            expand_tilde(var_val)
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
        match crate::utils::get_home_dir() {
            Some(home) => home,
            None => return Err("HOME not set".to_string().into()),
        }
    };

    if raw_path.to_str() == Some("-") {
        let oldpwd = match std::env::var("OLDPWD") {
            Ok(o) => PathBuf::from(o),
            Err(_) => return Err("cd: OLDPWD not set".to_string().into()),
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

    let target_path = match std::fs::canonicalize(&resolved_raw) {
        Ok(target_path) => target_path,
        Err(e) => {
            let mut resolved = None;
            if !args.is_empty()
                && let Val::String(_) = args[0]
            {
                let mut fragments = Vec::new();
                let mut subdirectory_only = false;
                for arg in &args {
                    if let Val::String(s) = arg {
                        if s == "/" {
                            subdirectory_only = true;
                        } else {
                            fragments.push(s.to_lowercase());
                        }
                    }
                }
                if let Ok(matched_path) = crate::cmd::frecency::resolve_z_match(
                    &fragments,
                    subdirectory_only,
                    Some(&env.cwd()),
                ) {
                    let _ = tx.try_send(PipelinePayload::Data(Arc::new(Val::String(
                        matched_path.display().to_string(),
                    ))));
                    resolved = Some(matched_path);
                }
            }

            if let Some(r) = resolved {
                r
            } else {
                return Err(BuiltinError::IoError {
                    cmd: "cd".into(),
                    message: format!("invalid path {raw_path:?}: {e}"),
                    span,
                }
                .into());
            }
        }
    };

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
        let target = std::path::PathBuf::from(top_str);

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
        let target = std::fs::canonicalize(expand_tilde(&target_arg))
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
    let target = std::path::PathBuf::from(top_str);

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

pub fn extract_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    if args.is_empty() {
        return Err("extract requires at least one archive file path"
            .to_string()
            .into());
    }

    let archive_path_str = match &args[0] {
        Val::String(s) => s,
        _ => {
            return Err(ShellError::new(
                ErrorCode::InvalidArgument,
                "extract argument must be a string path",
            )
            .maybe_with_span(span));
        }
    };

    let raw_path = expand_tilde(archive_path_str);
    let archive_path = std::fs::canonicalize(&raw_path)
        .map_err(|e| format!("Invalid archive path {:?}: {}", raw_path, e))?;
    env.enforce_capability("extract", CapAction::ReadFile(archive_path.clone()))?;
    let pwd = env.cwd();
    env.enforce_capability("extract", CapAction::WriteDir(pwd.clone()))?;
    env.enforce_capability("extract", CapAction::ProcessSpawn)?;

    env.track_read(archive_path.clone());

    let mut file = std::fs::File::open(&archive_path)
        .map_err(|e| format!("Failed to open file {:?}: {}", archive_path, e))?;

    use std::io::Read;
    let mut magic = [0u8; 6];
    let bytes_read = file.read(&mut magic).unwrap_or(0);

    let format = if bytes_read >= 4 && magic[0..4] == [0x50, 0x4B, 0x03, 0x04] {
        "zip"
    } else if bytes_read >= 2 && magic[0..2] == [0x1F, 0x8B] {
        "gzip"
    } else if bytes_read >= 3 && magic[0..3] == [0x42, 0x5A, 0x68] {
        "bzip2"
    } else if bytes_read >= 6 && magic[0..6] == [0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00] {
        "xz"
    } else {
        let ext = archive_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        if ext == "zip" {
            "zip"
        } else if ext == "tar" {
            "tar"
        } else if ext == "tgz" || archive_path.to_string_lossy().ends_with(".tar.gz") {
            "gzip"
        } else if ext == "tbz2" || archive_path.to_string_lossy().ends_with(".tar.bz2") {
            "bzip2"
        } else if ext == "txz" || archive_path.to_string_lossy().ends_with(".tar.xz") {
            "xz"
        } else {
            return Err(BuiltinError::Unsupported {
                cmd: "extract".into(),
                feature: format!("archive format for {archive_path:?}"),
            }
            .into());
        }
    };

    let mut cmd = match format {
        "zip" => {
            let mut c = tokio::process::Command::new("unzip");
            c.arg("-q").arg(&archive_path).current_dir(&pwd);
            c
        }
        "gzip" => {
            let mut c = tokio::process::Command::new("tar");
            c.arg("-xzf").arg(&archive_path).current_dir(&pwd);
            c
        }
        "bzip2" => {
            let mut c = tokio::process::Command::new("tar");
            c.arg("-xjf").arg(&archive_path).current_dir(&pwd);
            c
        }
        "xz" => {
            let mut c = tokio::process::Command::new("tar");
            c.arg("-xJf").arg(&archive_path).current_dir(&pwd);
            c
        }
        "tar" => {
            let mut c = tokio::process::Command::new("tar");
            c.arg("-xf").arg(&archive_path).current_dir(&pwd);
            c
        }
        _ => {
            return Err(BuiltinError::Unsupported {
                cmd: "extract".into(),
                feature: format!("archive format '{}'", format),
            }
            .into());
        }
    };

    tokio::spawn(async move {
        match cmd.status().await {
            Ok(status) if status.success() => {
                let payload = PipelinePayload::Data(Arc::new(Val::String(format!(
                    "Successfully extracted {:?}",
                    archive_path
                ))));
                let _ = tx.send(payload).await;
            }
            Ok(status) => {
                let _ = tx
                    .send(PipelinePayload::Structured(
                        format!(
                            "Extraction utility failed with exit code: {:?}",
                            status.code()
                        )
                        .into(),
                    ))
                    .await;
            }
            Err(e) => {
                let _ = tx
                    .send(PipelinePayload::Structured(
                        ShellError::new(
                            ErrorCode::IoError,
                            format!("Failed to spawn extraction command: {}", e),
                        )
                        .maybe_with_span(span)
                        .into(),
                    ))
                    .await;
            }
        }
    });

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
    vec![payload.clone()]
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
        let raw = expand_tilde(p);
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

pub fn uniq_builtin(
    in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let mut paths = Vec::new();
    for arg in args {
        match arg {
            Val::String(s) => {
                paths.push(s);
            }
            _ => {
                return Err(ShellError::new(
                    ErrorCode::InvalidArgument,
                    "Unexpected non-string argument to uniq",
                )
                .maybe_with_span(span));
            }
        }
    }

    if paths.is_empty() {
        if let Some(mut rx) = in_rx {
            tokio::spawn(async move {
                let mut last_val: Option<Val> = None;
                while let Some(payload) = rx.recv().await {
                    match payload {
                        PipelinePayload::Data(ref v) => {
                            if last_val.as_ref() != Some(v) {
                                last_val = Some((**v).clone());
                                if tx.send(payload).await.is_err() {
                                    break;
                                }
                            }
                        }
                        other => {
                            if tx.send(other).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
        return Ok(());
    }

    let canonical_paths = resolve_canonical_paths(&paths, env, "uniq")?;

    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        for path in canonical_paths {
            match tokio::fs::File::open(&path).await {
                Ok(file) => {
                    let reader = tokio::io::BufReader::new(file);
                    let mut lines = reader.lines();
                    let mut last_line: Option<String> = None;
                    while let Ok(Some(line)) = lines.next_line().await {
                        if last_line.as_deref() != Some(&line) {
                            last_line = Some(line.clone());
                            let payload = PipelinePayload::Data(Arc::new(Val::String(line)));
                            if tx.send(payload).await.is_err() {
                                return;
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = tx
                        .send(PipelinePayload::Structured(
                            ShellError::new(
                                ErrorCode::IoError,
                                format!("Failed to read file {:?}: {}", path, e),
                            )
                            .maybe_with_span(span)
                            .into(),
                        ))
                        .await;
                    return;
                }
            }
        }
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
    if fshell_engine::is_external_command(name, env_path.as_deref()) {
        let path = find_in_path(name);
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

fn find_in_path(name: &str) -> Option<std::path::PathBuf> {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            let path = std::path::Path::new(dir).join(name);
            if path.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(metadata) = path.metadata()
                        && metadata.permissions().mode() & 0o111 != 0
                    {
                        return Some(path);
                    }
                }
                #[cfg(not(unix))]
                {
                    return Some(path);
                }
            }
        }
    }
    None
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
        expand_tilde(&path_args[0])
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
        let path = expand_tilde(&path_str);
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
        let path = expand_tilde(&s);
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

            let path = expand_tilde(&path_str);
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
