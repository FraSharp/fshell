// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Lowering to the native fsh target.
//!
//! Prefers fshell's own builtins (`ps`, `ls`, `ff`, `env`, `kill`) and pipeline operators
//! (`filter`, `sort`, `limit`, `grep`) so that the fsh backend is genuinely native rather
//! than a shell-out. External programs are used only where fshell has no native primitive,
//! and then only as a `CommandCall` — the engine's ordinary fallback path, which keeps all
//! capability and safety checks in force.

use fshell_core::{BinOp, Expr, Pipeline, PipelineStage, StringPart};
use miette::SourceSpan;

use crate::action::{
    Action, DirSortBy, DiskMode, FileType, ListProcesses, ServiceAction, Signal, Transport,
};
use crate::lower::{LowerError, required_str};
use crate::platform::{Os, Platform};

fn span() -> SourceSpan {
    SourceSpan::new(0.into(), 0)
}

fn lit(value: impl Into<String>) -> Expr {
    Expr::String(vec![StringPart::Lit(value.into())])
}

fn whole(value: i64) -> Expr {
    Expr::Int(value)
}

fn col(name: &str) -> Expr {
    Expr::Ident(name.to_string())
}

fn compare(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
    Expr::BinaryOp {
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    }
}

fn combine(left: Option<Expr>, right: Expr) -> Expr {
    match left {
        Some(previous) => compare(BinOp::And, previous, right),
        None => right,
    }
}

fn call(name: &str, args: Vec<Expr>) -> PipelineStage {
    PipelineStage::CommandCall {
        name: name.to_string(),
        args,
        env: Vec::new(),
        span: span(),
    }
}

/// Lower an action to a native fsh pipeline.
pub fn lower(action: &Action, platform: &Platform) -> Result<Pipeline, LowerError> {
    let stages = match action {
        Action::ListProcesses(a) => list_processes(a),
        Action::InspectProcess(a) => vec![inspect_process(a)?],
        Action::SignalProcess(a) => vec![signal_process(a)?],
        Action::MemoryInfo(_) => vec![memory_info(platform)],
        Action::SystemInfo(_) => vec![call("uname", vec![lit("-a")])],
        Action::DiskUsage(a) => vec![disk_usage(a)],
        Action::FindFiles(a) => find_files(a)?,
        Action::SearchText(a) => search_text(a, platform)?,
        Action::ListDirectory(a) => list_directory(a),
        Action::DeletePath(a) => vec![delete_path(a)?],
        Action::ListeningPorts(a) => listening_ports(a, platform)?,
        Action::NetworkInterfaces(_) => vec![network_interfaces(platform)],
        Action::GitStatus(a) => vec![git(a, "status", &[lit("--porcelain")])],
        Action::GitBranches(a) => {
            let flag = if a.include_remote {
                vec![lit("-r")]
            } else {
                Vec::new()
            };
            vec![git(a, "branch", &flag)]
        }
        Action::RunContainer(a) => vec![run_container(a, platform)?],
        Action::ListContainers(a) => {
            let flag = if a.all {
                vec![lit("--all")]
            } else {
                Vec::new()
            };
            vec![call(container_tool(platform), prefixed("ps", flag))]
        }
        Action::ServiceControl(a) => vec![service_control(a, platform)?],
        Action::EnvironmentInfo(a) => environment_info(a),
    };
    Ok(Pipeline::new(stages))
}

fn list_processes(query: &ListProcesses) -> Vec<PipelineStage> {
    let needs_all_users =
        query.all_users || query.filter.as_ref().is_some_and(|f| f.user.is_some());
    let args = if needs_all_users {
        vec![lit("-a")]
    } else {
        Vec::new()
    };
    let mut stages = vec![call("ps", args)];

    if let Some(filter) = &query.filter {
        let mut condition: Option<Expr> = None;
        if let Some(name) = &filter.name_contains {
            condition = Some(combine(
                condition,
                compare(BinOp::ReMatch, col("command"), lit(name.clone())),
            ));
        }
        if let Some(user) = &filter.user {
            condition = Some(combine(
                condition,
                compare(BinOp::Eq, col("user"), lit(user.clone())),
            ));
        }
        if let Some(memory) = filter.min_memory {
            // `ps` reports RSS in KiB, so compare in KiB.
            condition = Some(combine(
                condition,
                compare(BinOp::Gt, col("rss"), whole(memory.kib() as i64)),
            ));
        }
        if let Some(cpu) = filter.min_cpu {
            condition = Some(combine(
                condition,
                compare(BinOp::Gt, col("cpu"), Expr::Float(cpu as f64)),
            ));
        }
        if let Some(condition) = condition {
            stages.push(PipelineStage::Filter { condition });
        }
    }

    if let Some(sort) = &query.sort {
        stages.push(PipelineStage::Sort {
            column: sort.by.column().to_string(),
            descending: sort.descending,
        });
    }
    if let Some(limit) = query.limit {
        stages.push(PipelineStage::Limit {
            amount: whole(limit as i64),
        });
    }
    stages
}

fn inspect_process(action: &crate::action::InspectProcess) -> Result<PipelineStage, LowerError> {
    let pid = action.pid.ok_or_else(|| {
        LowerError::InvalidValue("missing required parameter 'pid' for inspect_process".to_string())
    })?;
    Ok(call("ps", vec![lit(format!("-p{pid}"))]))
}

fn signal_process(action: &crate::action::SignalProcess) -> Result<PipelineStage, LowerError> {
    use crate::action::ProcessTarget;
    let target = action.target.as_ref().ok_or_else(|| {
        LowerError::InvalidValue(
            "missing required parameter 'target' for signal_process".to_string(),
        )
    })?;
    let signal = action.signal.unwrap_or_default();
    let is_term = matches!(signal, Signal::Term);
    Ok(match target {
        ProcessTarget::Pid { pid } => {
            if is_term {
                // The native `kill` builtin sends SIGTERM.
                call("kill", vec![whole(*pid as i64)])
            } else {
                // Non-TERM signals go through the external `kill` (via `exec`, which
                // invokes the fallback handler directly, bypassing the builtin).
                call(
                    "exec",
                    vec![
                        lit("kill"),
                        lit(format!("-{}", signal.name())),
                        whole(*pid as i64),
                    ],
                )
            }
        }
        ProcessTarget::Name { name } => {
            let mut args = Vec::new();
            if !is_term {
                args.push(lit(format!("-{}", signal.name())));
            }
            args.push(lit(name.clone()));
            call("pkill", args)
        }
    })
}

fn memory_info(platform: &Platform) -> PipelineStage {
    if platform.os == Os::MacOs {
        call("vm_stat", Vec::new())
    } else {
        call("free", vec![lit("-h")])
    }
}

fn disk_usage(action: &crate::action::DiskUsage) -> PipelineStage {
    let mut args = vec![lit("-h")];
    if let Some(path) = &action.path {
        args.push(lit(path.clone()));
    }
    match action.mode.unwrap_or_default() {
        DiskMode::Summary => call("df", args),
        DiskMode::PerDirectory => call("du", args),
    }
}

fn find_files(action: &crate::action::FindFiles) -> Result<Vec<PipelineStage>, LowerError> {
    let root = action
        .root
        .clone()
        .filter(|r| !r.trim().is_empty())
        .unwrap_or_else(|| ".".to_string());
    let mut args = vec![lit(root)];

    if let Some(depth) = action.max_depth {
        args.push(lit("--depth"));
        args.push(whole(depth as i64));
    }
    if let Some(min) = action.min_size {
        args.push(lit("size"));
        args.push(lit(">="));
        args.push(whole(min.bytes() as i64));
    }
    if let Some(max) = action.max_size {
        args.push(lit("size"));
        args.push(lit("<="));
        args.push(whole(max.bytes() as i64));
    }
    if let Some(within) = action.modified_within {
        args.push(lit("modified"));
        args.push(lit("<"));
        args.push(lit(format!("{}s", within.seconds())));
    }
    if let Some(before) = action.modified_before {
        args.push(lit("modified"));
        args.push(lit(">"));
        args.push(lit(format!("{}s", before.seconds())));
    }
    if let Some(kind) = action.file_type {
        let label = match kind {
            FileType::File => "file",
            FileType::Dir => "dir",
            // `ff` does not classify symlinks separately.
            FileType::Symlink => {
                return Err(LowerError::Unsupported {
                    kind: "find_files".to_string(),
                    target: "fsh".to_string(),
                    reason: "the native `ff` finder cannot select symlinks".to_string(),
                });
            }
        };
        args.push(lit("type"));
        args.push(lit("="));
        args.push(lit(label));
    }
    if action.include_hidden {
        args.push(lit("hidden"));
    }

    let mut stages = vec![call("ff", args)];

    // Name/extension matching is done with a native `filter` stage rather than `ff`'s
    // `name = <glob>` argument: fsh expands globs (even quoted ones) before dispatch, so a
    // glob passed as an argument would be rewritten against the working directory. A regex
    // over the `name` column is exact and expansion-proof.
    if let Some(pattern) = name_regex(action) {
        stages.push(PipelineStage::Filter {
            condition: compare(BinOp::ReMatch, col("name"), lit(pattern)),
        });
    }
    if let Some(limit) = action.limit {
        stages.push(PipelineStage::Limit {
            amount: whole(limit as i64),
        });
    }
    Ok(stages)
}

/// A regex matching the requested name/extension against the `name` column.
fn name_regex(action: &crate::action::FindFiles) -> Option<String> {
    if let Some(name) = action.name.as_deref().filter(|n| !n.trim().is_empty()) {
        return Some(glob_to_regex(name));
    }
    action
        .extension
        .as_deref()
        .filter(|e| !e.trim().is_empty())
        .map(|ext| format!(r"\.{}$", regex_escape(ext)))
}

fn glob_to_regex(glob: &str) -> String {
    let mut pattern = String::from("^");
    for ch in glob.chars() {
        match ch {
            '*' => pattern.push_str(".*"),
            '?' => pattern.push('.'),
            other => pattern.push_str(&regex_escape(&other.to_string())),
        }
    }
    pattern.push('$');
    pattern
}

fn regex_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(
            ch,
            '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$' | '\\'
        ) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

fn search_text(
    action: &crate::action::SearchText,
    platform: &Platform,
) -> Result<Vec<PipelineStage>, LowerError> {
    let pattern = required_str(&action.pattern, "search_text", "pattern")?;
    let tool = platform
        .utilities
        .first_of(&["rg", "grep"])
        .unwrap_or("grep");
    let mut args = Vec::new();
    if tool == "rg" {
        args.push(lit("-n"));
        if action.ignore_case {
            args.push(lit("-i"));
        }
        if action.fixed_strings {
            args.push(lit("-F"));
        }
        if let Some(glob) = &action.file_glob {
            args.push(lit("-g"));
            args.push(lit(glob.clone()));
        }
        args.push(lit("--"));
        args.push(lit(pattern));
    } else {
        args.push(lit("-rn"));
        if action.ignore_case {
            args.push(lit("-i"));
        }
        if action.fixed_strings {
            args.push(lit("-F"));
        }
        if let Some(glob) = &action.file_glob {
            args.push(lit(format!("--include={glob}")));
        }
        args.push(lit(pattern));
    }
    if action.paths.is_empty() {
        args.push(lit("."));
    } else {
        for path in &action.paths {
            args.push(lit(path.clone()));
        }
    }

    let mut stages = vec![call(tool, args)];
    if let Some(max) = action.max_results {
        stages.push(PipelineStage::Limit {
            amount: whole(max as i64),
        });
    }
    Ok(stages)
}

fn list_directory(action: &crate::action::ListDirectory) -> Vec<PipelineStage> {
    let mut args = Vec::new();
    if action.include_hidden {
        args.push(lit("-a"));
    }
    if action.long {
        args.push(lit("-l"));
    }
    if action.recursive {
        args.push(lit("-R"));
    }
    if let Some(sort) = &action.sort {
        match sort.by {
            DirSortBy::Name => {}
            DirSortBy::Size => args.push(lit("-S")),
            DirSortBy::Time => args.push(lit("-t")),
        }
        if sort.descending {
            args.push(lit("-r"));
        }
    }
    args.push(lit(action.path.clone().unwrap_or_else(|| ".".to_string())));

    let mut stages = vec![call("ls", args)];
    if let Some(limit) = action.limit {
        stages.push(PipelineStage::Limit {
            amount: whole(limit as i64),
        });
    }
    stages
}

fn delete_path(action: &crate::action::DeletePath) -> Result<PipelineStage, LowerError> {
    let path = required_str(&action.path, "delete_path", "path")?;
    let mut args = Vec::new();
    if action.recursive {
        args.push(lit("-r"));
    }
    if action.force {
        args.push(lit("-f"));
    }
    args.push(lit(path));
    Ok(call("rm", args))
}

fn listening_ports(
    action: &crate::action::ListeningPorts,
    platform: &Platform,
) -> Result<Vec<PipelineStage>, LowerError> {
    if platform.utilities.has("lsof") {
        let mut iface = String::from("-i");
        match action.protocol.unwrap_or_default() {
            Transport::Tcp => iface.push_str("TCP"),
            Transport::Udp => iface.push_str("UDP"),
            Transport::Any => {}
        }
        if let Some(port) = action.port {
            iface.push_str(&format!(":{port}"));
        }
        return Ok(vec![call("lsof", vec![lit("-nP"), lit(iface)])]);
    }
    if platform.os == Os::Linux && platform.utilities.has("ss") {
        let mut stages = vec![call("ss", vec![lit("-lntup")])];
        if let Some(port) = action.port {
            stages.push(PipelineStage::Grep {
                pattern: lit(format!(":{port}")),
            });
        }
        return Ok(stages);
    }
    Err(LowerError::Unsupported {
        kind: "listening_ports".to_string(),
        target: "fsh".to_string(),
        reason: "neither lsof nor ss is available".to_string(),
    })
}

fn network_interfaces(platform: &Platform) -> PipelineStage {
    if platform.os == Os::MacOs {
        call("ifconfig", vec![lit("-a")])
    } else if platform.utilities.has("ip") {
        call("ip", vec![lit("addr")])
    } else {
        call("ifconfig", vec![lit("-a")])
    }
}

fn git(action: &impl GitPath, subcommand: &str, extra: &[Expr]) -> PipelineStage {
    let mut args = Vec::new();
    if let Some(path) = action.repo_path() {
        args.push(lit("-C"));
        args.push(lit(path));
    }
    args.push(lit(subcommand));
    args.extend(extra.iter().cloned());
    call("git", args)
}

fn run_container(
    action: &crate::action::RunContainer,
    platform: &Platform,
) -> Result<PipelineStage, LowerError> {
    let image = required_str(&action.image, "run_container", "image")?;
    let mut args = vec![lit("run")];
    if action.detached {
        args.push(lit("-d"));
    }
    if let Some(name) = &action.name {
        args.push(lit("--name"));
        args.push(lit(name.clone()));
    }
    for mapping in &action.ports {
        let host = mapping.host_port.ok_or_else(|| {
            LowerError::InvalidValue(
                "a run_container port mapping is missing 'host_port'".to_string(),
            )
        })?;
        let container = mapping.container_port.unwrap_or(host);
        args.push(lit("-p"));
        args.push(lit(format!("{host}:{container}")));
    }
    for env in &action.env {
        args.push(lit("-e"));
        args.push(lit(format!("{}={}", env.name, env.value)));
    }
    args.push(lit(image));
    for part in &action.command {
        args.push(lit(part.clone()));
    }
    Ok(call(container_tool(platform), args))
}

fn service_control(
    action: &crate::action::ServiceControl,
    platform: &Platform,
) -> Result<PipelineStage, LowerError> {
    let service_action = action.action.ok_or_else(|| {
        LowerError::InvalidValue(
            "missing required parameter 'action' for service_control".to_string(),
        )
    })?;
    let name = required_str(&action.name, "service_control", "name")?;
    let word = match service_action {
        ServiceAction::Status => "status",
        ServiceAction::Start => "start",
        ServiceAction::Stop => "stop",
        ServiceAction::Restart => "restart",
    };
    match platform.os {
        Os::Linux => Ok(call("systemctl", vec![lit(word), lit(name)])),
        Os::MacOs if matches!(service_action, ServiceAction::Status) => {
            Ok(call("launchctl", vec![lit("list"), lit(name)]))
        }
        Os::MacOs => Err(LowerError::Unsupported {
            kind: "service_control".to_string(),
            target: "fsh".to_string(),
            reason: format!("launchctl cannot safely '{word}' a service by name on macOS"),
        }),
        Os::Other => Err(LowerError::Unsupported {
            kind: "service_control".to_string(),
            target: "fsh".to_string(),
            reason: "no supported service manager on this platform".to_string(),
        }),
    }
}

fn environment_info(action: &crate::action::EnvironmentInfo) -> Vec<PipelineStage> {
    let mut stages = vec![call("env", Vec::new())];
    if let Some(variable) = &action.variable {
        stages.push(PipelineStage::Filter {
            condition: compare(BinOp::Eq, col("key"), lit(variable.clone())),
        });
    }
    stages
}

fn container_tool(platform: &Platform) -> &'static str {
    platform
        .utilities
        .first_of(&["docker", "podman"])
        .unwrap_or("docker")
}

fn prefixed(command: &str, extra: Vec<Expr>) -> Vec<Expr> {
    let mut args = vec![lit(command)];
    args.extend(extra);
    args
}

/// Types that expose an optional repository path (used by the Git lowering).
trait GitPath {
    fn repo_path(&self) -> Option<String>;
}

impl GitPath for crate::action::GitStatus {
    fn repo_path(&self) -> Option<String> {
        self.path.clone().filter(|p| !p.trim().is_empty())
    }
}

impl GitPath for crate::action::GitBranches {
    fn repo_path(&self) -> Option<String> {
        self.path.clone().filter(|p| !p.trim().is_empty())
    }
}
