// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Lowering to the POSIX/Bash target.
//!
//! Produces a portable shell script string that is handed to the registered POSIX handler
//! (`fshell_engine::posix_handler`) for execution. Platform differences in `ps`, `find`,
//! `lsof`, `df`, service managers, etc. are resolved here so the semantic model never has
//! to know them.

use crate::action::{
    Action, DirSortBy, DiskMode, FileType, ProcessSortBy, ServiceAction, Transport,
};
use crate::lower::{LowerError, required_str};
use crate::platform::{Os, Platform};

/// Single-quote a value for safe inclusion in a POSIX script.
fn shq(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn join(parts: &[String]) -> String {
    parts.join(" ")
}

/// Lower an action to a POSIX shell script.
pub fn lower(action: &Action, platform: &Platform) -> Result<String, LowerError> {
    match action {
        Action::ListProcesses(a) => Ok(list_processes(a)),
        Action::InspectProcess(a) => inspect_process(a),
        Action::SignalProcess(a) => signal_process(a),
        Action::MemoryInfo(_) => Ok(if platform.os == Os::MacOs {
            "vm_stat".to_string()
        } else {
            "free -h".to_string()
        }),
        Action::SystemInfo(_) => Ok("uname -a".to_string()),
        Action::DiskUsage(a) => Ok(disk_usage(a)),
        Action::FindFiles(a) => find_files(a),
        Action::SearchText(a) => search_text(a, platform),
        Action::ListDirectory(a) => Ok(list_directory(a)),
        Action::DeletePath(a) => delete_path(a),
        Action::ListeningPorts(a) => listening_ports(a, platform),
        Action::NetworkInterfaces(_) => Ok(
            if platform.os == Os::MacOs || !platform.utilities.has("ip") {
                "ifconfig -a".to_string()
            } else {
                "ip addr".to_string()
            },
        ),
        Action::GitStatus(a) => Ok(git_prefix(&a.path, "status --porcelain")),
        Action::GitBranches(a) => {
            let sub = if a.include_remote {
                "branch -r"
            } else {
                "branch"
            };
            Ok(git_prefix(&a.path, sub))
        }
        Action::RunContainer(a) => run_container(a, platform),
        Action::ListContainers(a) => {
            let tool = container_tool(platform);
            Ok(if a.all {
                format!("{tool} ps --all")
            } else {
                format!("{tool} ps")
            })
        }
        Action::ServiceControl(a) => service_control(a, platform),
        Action::EnvironmentInfo(a) => Ok(match &a.variable {
            Some(name) => format!("printenv {}", shq(name)),
            None => "env".to_string(),
        }),
    }
}

fn list_processes(action: &crate::action::ListProcesses) -> String {
    let mut words = vec!["ps aux".to_string()];
    if let Some(filter) = &action.filter {
        if let Some(name) = &filter.name_contains {
            words.push(format!("grep {}", shq(name)));
        }
        if let Some(user) = &filter.user {
            words.push(format!("awk '$1 == {}'", shq(user)));
        }
        if let Some(memory) = filter.min_memory {
            // `ps aux` column 6 is RSS in KiB.
            words.push(format!("awk '$6 + 0 > {}'", memory.kib()));
        }
        if let Some(cpu) = &filter.min_cpu {
            words.push(format!("awk '$3 + 0 > {cpu}'"));
        }
    }
    if let Some(sort) = &action.sort {
        let column = match sort.by {
            ProcessSortBy::User => 1,
            ProcessSortBy::Pid => 2,
            ProcessSortBy::Cpu => 3,
            ProcessSortBy::Memory => 6,
            ProcessSortBy::Command => 11,
        };
        let numeric = matches!(
            sort.by,
            ProcessSortBy::Pid | ProcessSortBy::Cpu | ProcessSortBy::Memory
        );
        let mut sort_words = "sort -k".to_string() + &column.to_string();
        if numeric {
            sort_words.push_str(" -n");
        }
        if sort.descending {
            sort_words.push_str(" -r");
        }
        words.push(sort_words);
    }
    if let Some(limit) = action.limit {
        words.push(format!("head -n {limit}"));
    }
    words.join(" | ")
}

fn inspect_process(action: &crate::action::InspectProcess) -> Result<String, LowerError> {
    let pid = action.pid.ok_or_else(|| {
        LowerError::InvalidValue("missing required parameter 'pid' for inspect_process".to_string())
    })?;
    Ok(format!(
        "ps -p {pid} -o pid,ppid,user,%cpu,%mem,rss,comm,args"
    ))
}

fn signal_process(action: &crate::action::SignalProcess) -> Result<String, LowerError> {
    use crate::action::{ProcessTarget, Signal};
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
                format!("kill {pid}")
            } else {
                format!("kill -{} {pid}", signal.name())
            }
        }
        ProcessTarget::Name { name } => {
            if is_term {
                format!("pkill {}", shq(name))
            } else {
                format!("pkill -{} {}", signal.name(), shq(name))
            }
        }
    })
}

fn disk_usage(action: &crate::action::DiskUsage) -> String {
    let tool = match action.mode.unwrap_or_default() {
        DiskMode::Summary => "df -h",
        DiskMode::PerDirectory => "du -h",
    };
    match &action.path {
        Some(path) => format!("{tool} {}", shq(path)),
        None => tool.to_string(),
    }
}

fn find_files(action: &crate::action::FindFiles) -> Result<String, LowerError> {
    let root = action
        .root
        .clone()
        .filter(|r| !r.trim().is_empty())
        .unwrap_or_else(|| ".".to_string());
    let mut words = vec!["find".to_string(), shq(&root)];

    if let Some(depth) = action.max_depth {
        words.push("-maxdepth".to_string());
        words.push(depth.to_string());
    }
    let glob = action
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .or_else(|| {
            action
                .extension
                .clone()
                .filter(|e| !e.trim().is_empty())
                .map(|ext| format!("*.{ext}"))
        });
    if let Some(glob) = glob {
        words.push("-name".to_string());
        words.push(shq(&glob));
    }
    if let Some(min) = action.min_size {
        // Byte-precise: `+<n>c` avoids the GNU `M` = MiB vs decimal-MB ambiguity.
        words.push("-size".to_string());
        words.push(format!("+{}c", min.bytes()));
    }
    if let Some(max) = action.max_size {
        words.push("-size".to_string());
        words.push(format!("-{}c", max.bytes()));
    }
    if let Some(within) = action.modified_within {
        words.push("-mmin".to_string());
        words.push(format!("-{}", within.minutes_ceil()));
    }
    if let Some(before) = action.modified_before {
        words.push("-mmin".to_string());
        words.push(format!("+{}", before.minutes_ceil()));
    }
    if let Some(kind) = action.file_type {
        let label = match kind {
            FileType::File => "f",
            FileType::Dir => "d",
            FileType::Symlink => "l",
        };
        words.push("-type".to_string());
        words.push(label.to_string());
    }

    let mut script = join(&words);
    if let Some(limit) = action.limit {
        script.push_str(&format!(" | head -n {limit}"));
    }
    Ok(script)
}

fn search_text(
    action: &crate::action::SearchText,
    platform: &Platform,
) -> Result<String, LowerError> {
    let pattern = required_str(&action.pattern, "search_text", "pattern")?;
    let tool = platform
        .utilities
        .first_of(&["rg", "grep"])
        .unwrap_or("grep");
    let mut words = vec![tool.to_string()];
    if tool == "rg" {
        words.push("-n".to_string());
    } else {
        words.push("-rn".to_string());
    }
    if action.ignore_case {
        words.push("-i".to_string());
    }
    if action.fixed_strings {
        words.push("-F".to_string());
    }
    if let Some(glob) = &action.file_glob {
        if tool == "rg" {
            words.push("-g".to_string());
            words.push(shq(glob));
        } else {
            words.push(format!("--include={}", shq(glob)));
        }
    }
    words.push(shq(&pattern));
    if action.paths.is_empty() {
        words.push(".".to_string());
    } else {
        for path in &action.paths {
            words.push(shq(path));
        }
    }

    let mut script = join(&words);
    if let Some(max) = action.max_results {
        script.push_str(&format!(" | head -n {max}"));
    }
    Ok(script)
}

fn list_directory(action: &crate::action::ListDirectory) -> String {
    let mut words = vec!["ls".to_string()];
    if action.include_hidden {
        words.push("-a".to_string());
    }
    if action.long {
        words.push("-l".to_string());
    }
    if action.recursive {
        words.push("-R".to_string());
    }
    if let Some(sort) = &action.sort {
        match sort.by {
            DirSortBy::Name => {}
            DirSortBy::Size => words.push("-S".to_string()),
            DirSortBy::Time => words.push("-t".to_string()),
        }
        if sort.descending {
            words.push("-r".to_string());
        }
    }
    words.push(shq(action.path.as_deref().unwrap_or(".")));

    let mut script = join(&words);
    if let Some(limit) = action.limit {
        script.push_str(&format!(" | head -n {limit}"));
    }
    script
}

fn delete_path(action: &crate::action::DeletePath) -> Result<String, LowerError> {
    let path = required_str(&action.path, "delete_path", "path")?;
    let mut words = vec!["rm".to_string()];
    if action.recursive {
        words.push("-r".to_string());
    }
    if action.force {
        words.push("-f".to_string());
    }
    words.push(shq(&path));
    Ok(join(&words))
}

fn listening_ports(
    action: &crate::action::ListeningPorts,
    platform: &Platform,
) -> Result<String, LowerError> {
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
        return Ok(format!("lsof -nP {iface}"));
    }
    if platform.os == Os::Linux && platform.utilities.has("ss") {
        let mut script = "ss -lntup".to_string();
        if let Some(port) = action.port {
            script.push_str(&format!(" | grep {}", shq(&format!(":{port}"))));
        }
        return Ok(script);
    }
    Err(LowerError::Unsupported {
        kind: "listening_ports".to_string(),
        target: "posix".to_string(),
        reason: "neither lsof nor ss is available".to_string(),
    })
}

fn git_prefix(path: &Option<String>, subcommand: &str) -> String {
    match path.as_deref() {
        Some(p) if !p.trim().is_empty() => format!("git -C {} {subcommand}", shq(p)),
        _ => format!("git {subcommand}"),
    }
}

fn run_container(
    action: &crate::action::RunContainer,
    platform: &Platform,
) -> Result<String, LowerError> {
    let image = required_str(&action.image, "run_container", "image")?;
    let mut words = vec![container_tool(platform).to_string(), "run".to_string()];
    if action.detached {
        words.push("-d".to_string());
    }
    if let Some(name) = &action.name {
        words.push("--name".to_string());
        words.push(shq(name));
    }
    for mapping in &action.ports {
        let host = mapping.host_port.ok_or_else(|| {
            LowerError::InvalidValue(
                "a run_container port mapping is missing 'host_port'".to_string(),
            )
        })?;
        let container = mapping.container_port.unwrap_or(host);
        words.push("-p".to_string());
        words.push(format!("{host}:{container}"));
    }
    for env in &action.env {
        words.push("-e".to_string());
        words.push(shq(&format!("{}={}", env.name, env.value)));
    }
    words.push(shq(&image));
    for part in &action.command {
        words.push(shq(part));
    }
    Ok(join(&words))
}

fn service_control(
    action: &crate::action::ServiceControl,
    platform: &Platform,
) -> Result<String, LowerError> {
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
        Os::Linux => Ok(format!("systemctl {word} {}", shq(&name))),
        Os::MacOs if matches!(service_action, ServiceAction::Status) => {
            Ok(format!("launchctl list {}", shq(&name)))
        }
        Os::MacOs => Err(LowerError::Unsupported {
            kind: "service_control".to_string(),
            target: "posix".to_string(),
            reason: format!("launchctl cannot safely '{word}' a service by name on macOS"),
        }),
        Os::Other => Err(LowerError::Unsupported {
            kind: "service_control".to_string(),
            target: "posix".to_string(),
            reason: "no supported service manager on this platform".to_string(),
        }),
    }
}

fn container_tool(platform: &Platform) -> &'static str {
    platform
        .utilities
        .first_of(&["docker", "podman"])
        .unwrap_or("docker")
}
