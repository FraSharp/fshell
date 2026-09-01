// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Custom command-specific and shell-registry completion resolvers.

use super::types::{CompletionCandidate, CompletionKind, TextSpan};
use fshell_core::{Stmt, Val};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

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

pub fn git_branches() -> Vec<String> {
    let pwd = std::env::current_dir().unwrap_or_default();
    git_branches_for_path(&pwd)
}

pub fn git_branches_cached(env: &fshell_engine::Env) -> Vec<String> {
    let cwd = env.cwd();
    let head_path = cwd.join(".git/HEAD");
    {
        let cache = env.prompt.git_branch_cache.read();
        if let Some((time, branches, _head_mtime)) = cache.as_ref()
            && time.elapsed() < fshell_engine::GIT_CACHE_TTL
        {
            return branches.clone();
        }
        if let Some((_, branches, head_mtime)) = cache.as_ref()
            && let Ok(current_mtime) = std::fs::metadata(&head_path).and_then(|m| m.modified())
            && current_mtime == *head_mtime
        {
            let branches = branches.clone();
            let mtime = *head_mtime;
            drop(cache);
            let mut cache = env.prompt.git_branch_cache.write();
            *cache = Some((Instant::now(), branches.clone(), mtime));
            return branches;
        }
    }
    let branches = git_branches_for_path(&cwd);
    let head_mtime = std::fs::metadata(&head_path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    {
        let mut cache = env.prompt.git_branch_cache.write();
        *cache = Some((Instant::now(), branches.clone(), head_mtime));
    }
    branches
}

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
) -> Option<Vec<CompletionCandidate>> {
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

    let mut typed_subcmds = Vec::new();
    let words_after_cmd = &words[1..];
    if starting_new_arg {
        for w in words_after_cmd {
            if !w.starts_with('-') {
                typed_subcmds.push(w.to_string());
            }
        }
    } else if words_after_cmd.len() > 1 {
        for w in &words_after_cmd[..words_after_cmd.len() - 1] {
            if !w.starts_with('-') {
                typed_subcmds.push(w.to_string());
            }
        }
    }

    let mut suggestions = Vec::new();

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
                        suggestions.push(CompletionCandidate::new(
                            val,
                            CompletionKind::ExternalCommand,
                            TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                        ));
                    }
                }
                return Some(suggestions);
            }
        }
    }

    for sub in &comp.subcommands {
        if sub.parent_subcmds == typed_subcmds && sub.name.to_lowercase().starts_with(&filter) {
            let mut cand = CompletionCandidate::new(
                sub.name.clone(),
                CompletionKind::ExternalCommand,
                TextSpan::new(pos.saturating_sub(last_word.len()), pos),
            );
            if let Some(d) = &sub.desc {
                cand = cand.with_description(format!("[{}] {}", cmd, d));
            }
            suggestions.push(cand);
        }
    }

    for flag in &comp.flags {
        if flag.parent_subcmds == typed_subcmds {
            if let Some(ref s) = flag.short {
                let s_with_dash = format!("-{}", s);
                if s_with_dash.to_lowercase().starts_with(&filter) {
                    let mut cand = CompletionCandidate::new(
                        s_with_dash,
                        CompletionKind::Flag,
                        TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                    );
                    if let Some(d) = &flag.desc {
                        cand = cand.with_description(d.clone());
                    }
                    suggestions.push(cand);
                }
            }
            if let Some(ref l) = flag.long {
                let l_with_dash = format!("--{}", l);
                if l_with_dash.to_lowercase().starts_with(&filter) {
                    let mut cand = CompletionCandidate::new(
                        l_with_dash,
                        CompletionKind::Flag,
                        TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                    );
                    if let Some(d) = &flag.desc {
                        cand = cand.with_description(d.clone());
                    }
                    suggestions.push(cand);
                }
            }
        }
    }

    for prov in &comp.dynamic_providers {
        if prov.parent_subcmds == typed_subcmds {
            let choices = resolve_choices(&prov.command, env);
            for val in choices {
                if val.to_lowercase().starts_with(&filter) {
                    suggestions.push(
                        CompletionCandidate::new(
                            val,
                            CompletionKind::ExternalCommand,
                            TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                        )
                        .with_description(format!("[{}] dynamic", cmd)),
                    );
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

pub fn get_custom_completions(
    line: &str,
    pos: usize,
    env: &fshell_engine::Env,
) -> Option<Vec<CompletionCandidate>> {
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

    let add_suggestions = |items: Vec<(&str, &str)>, suggestions: &mut Vec<CompletionCandidate>| {
        for (val, desc) in items {
            if val.to_lowercase().starts_with(&filter) {
                let kind = if val.starts_with('-') {
                    CompletionKind::Flag
                } else {
                    CompletionKind::ExternalCommand
                };
                suggestions.push(
                    CompletionCandidate::new(
                        val,
                        kind,
                        TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                    )
                    .with_description(format!("[{}] {}", cmd, desc)),
                );
            }
        }
    };

    let add_dynamic_suggestions =
        |items: Vec<String>,
         kind: CompletionKind,
         desc: &str,
         suggestions: &mut Vec<CompletionCandidate>| {
            for val in items {
                if val.to_lowercase().starts_with(&filter) {
                    suggestions.push(
                        CompletionCandidate::new(
                            val,
                            kind,
                            TextSpan::new(pos.saturating_sub(last_word.len()), pos),
                        )
                        .with_description(format!("[{}] {}", cmd, desc)),
                    );
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
                    add_dynamic_suggestions(
                        branches,
                        CompletionKind::GitBranch,
                        "git branch",
                        &mut suggestions,
                    );
                    add_dynamic_suggestions(
                        tags,
                        CompletionKind::GitBranch,
                        "git tag",
                        &mut suggestions,
                    );
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
                    return None;
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
                    add_dynamic_suggestions(
                        scripts,
                        CompletionKind::ExternalCommand,
                        "npm script",
                        &mut suggestions,
                    );
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
                    add_dynamic_suggestions(
                        containers,
                        CompletionKind::ExternalCommand,
                        "docker container",
                        &mut suggestions,
                    );
                } else if sub == "rmi" {
                    let images = docker_images();
                    add_dynamic_suggestions(
                        images,
                        CompletionKind::ExternalCommand,
                        "docker image",
                        &mut suggestions,
                    );
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
                    add_dynamic_suggestions(
                        pods,
                        CompletionKind::ExternalCommand,
                        "k8s pod",
                        &mut suggestions,
                    );
                }
            }
        }
        "ssh" => {
            let hosts = ssh_hosts();
            add_dynamic_suggestions(
                hosts,
                CompletionKind::ExternalCommand,
                "ssh host",
                &mut suggestions,
            );
        }
        "make" => {
            let targets = make_targets();
            add_dynamic_suggestions(
                targets,
                CompletionKind::ExternalCommand,
                "makefile target",
                &mut suggestions,
            );
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
                    add_dynamic_suggestions(
                        formulas,
                        CompletionKind::ExternalCommand,
                        "installed package",
                        &mut suggestions,
                    );
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
        let _ = branches;
    }

    #[tokio::test]
    async fn test_custom_completions() {
        let env = fshell_engine::Env::new();

        let suggs = get_custom_completions("git ch", 6, &env).unwrap();
        assert!(!suggs.is_empty());
        assert_eq!(suggs[0].value, "checkout");

        let suggs = get_custom_completions("cargo bu", 8, &env).unwrap();
        assert!(!suggs.is_empty());
        assert_eq!(suggs[0].value, "build");

        let suggs = get_custom_completions("git checkout ", 13, &env).unwrap();
        let _ = suggs;

        let suggs = get_custom_completions("ls -l", 5, &env);
        assert!(suggs.is_none());
    }
}
