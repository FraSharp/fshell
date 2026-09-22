// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Static metadata for every semantic action.
//!
//! Requiredness, risk and descriptions live here exactly once, and are consumed by
//! validation, the `intent` builtin and FunctionGemma schema generation. Keeping them in
//! one table is what stops the type layer and the schema layer from drifting apart.

use std::path::PathBuf;

use fshell_core::ResourceHandle;

use crate::action::{Action, ServiceAction};
use crate::intent::RiskLevel;

/// Functional grouping of an action, used for documentation and schema presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Process,
    System,
    Files,
    Network,
    Git,
    Containers,
    Services,
    Environment,
}

impl Category {
    /// A short lowercase label.
    pub const fn label(self) -> &'static str {
        match self {
            Category::Process => "process",
            Category::System => "system",
            Category::Files => "files",
            Category::Network => "network",
            Category::Git => "git",
            Category::Containers => "containers",
            Category::Services => "services",
            Category::Environment => "environment",
        }
    }
}

/// Static description of one semantic action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionSpec {
    /// Stable snake_case identifier (also the function-calling tool name).
    pub kind: &'static str,
    /// One-line human/model description.
    pub summary: &'static str,
    /// Functional grouping.
    pub category: Category,
    /// Parameters that must be present before the action can run.
    pub required: &'static [&'static str],
    /// Risk when the action is not further qualified at runtime.
    pub base_risk: RiskLevel,
}

const LIST_PROCESSES: ActionSpec = ActionSpec {
    kind: "list_processes",
    summary: "List running processes, optionally filtered, sorted and limited.",
    category: Category::Process,
    required: &[],
    base_risk: RiskLevel::Safe,
};
const INSPECT_PROCESS: ActionSpec = ActionSpec {
    kind: "inspect_process",
    summary: "Show details of a single process by its PID.",
    category: Category::Process,
    required: &["pid"],
    base_risk: RiskLevel::Safe,
};
const SIGNAL_PROCESS: ActionSpec = ActionSpec {
    kind: "signal_process",
    summary: "Send a signal to a process (by PID) or to every process matching a name.",
    category: Category::Process,
    required: &["target"],
    base_risk: RiskLevel::Destructive,
};
const MEMORY_INFO: ActionSpec = ActionSpec {
    kind: "memory_info",
    summary: "Report system memory usage (total, used, free, available).",
    category: Category::System,
    required: &[],
    base_risk: RiskLevel::Safe,
};
const SYSTEM_INFO: ActionSpec = ActionSpec {
    kind: "system_info",
    summary: "Report general operating-system and kernel information.",
    category: Category::System,
    required: &[],
    base_risk: RiskLevel::Safe,
};
const DISK_USAGE: ActionSpec = ActionSpec {
    kind: "disk_usage",
    summary: "Report disk usage of filesystems or of a directory tree.",
    category: Category::System,
    required: &[],
    base_risk: RiskLevel::Safe,
};
const FIND_FILES: ActionSpec = ActionSpec {
    kind: "find_files",
    summary: "Find files under a directory by name, type, size and modification time.",
    category: Category::Files,
    required: &[],
    base_risk: RiskLevel::Safe,
};
const SEARCH_TEXT: ActionSpec = ActionSpec {
    kind: "search_text",
    summary: "Search file contents recursively for a pattern.",
    category: Category::Files,
    required: &["pattern"],
    base_risk: RiskLevel::Safe,
};
const LIST_DIRECTORY: ActionSpec = ActionSpec {
    kind: "list_directory",
    summary: "List the contents of a directory.",
    category: Category::Files,
    required: &[],
    base_risk: RiskLevel::Safe,
};
const DELETE_PATH: ActionSpec = ActionSpec {
    kind: "delete_path",
    summary: "Delete a file, or a directory tree when recursive is set.",
    category: Category::Files,
    required: &["path"],
    base_risk: RiskLevel::Destructive,
};
const LISTENING_PORTS: ActionSpec = ActionSpec {
    kind: "listening_ports",
    summary: "Show which sockets and processes are listening, optionally on a given port.",
    category: Category::Network,
    required: &[],
    base_risk: RiskLevel::Safe,
};
const NETWORK_INTERFACES: ActionSpec = ActionSpec {
    kind: "network_interfaces",
    summary: "Show the host's network interfaces and their addresses.",
    category: Category::Network,
    required: &[],
    base_risk: RiskLevel::Safe,
};
const GIT_STATUS: ActionSpec = ActionSpec {
    kind: "git_status",
    summary: "Show the working-tree status of a Git repository.",
    category: Category::Git,
    required: &[],
    base_risk: RiskLevel::Safe,
};
const GIT_BRANCHES: ActionSpec = ActionSpec {
    kind: "git_branches",
    summary: "List the branches of a Git repository.",
    category: Category::Git,
    required: &[],
    base_risk: RiskLevel::Safe,
};
const RUN_CONTAINER: ActionSpec = ActionSpec {
    kind: "run_container",
    summary: "Start a container from an image, optionally publishing ports.",
    category: Category::Containers,
    required: &["image"],
    base_risk: RiskLevel::Destructive,
};
const LIST_CONTAINERS: ActionSpec = ActionSpec {
    kind: "list_containers",
    summary: "List containers.",
    category: Category::Containers,
    required: &[],
    base_risk: RiskLevel::Safe,
};
const SERVICE_CONTROL: ActionSpec = ActionSpec {
    kind: "service_control",
    summary: "Query or change the state of a system service.",
    category: Category::Services,
    required: &["action", "name"],
    base_risk: RiskLevel::Destructive,
};
const ENVIRONMENT_INFO: ActionSpec = ActionSpec {
    kind: "environment_info",
    summary: "Report environment variables.",
    category: Category::Environment,
    required: &[],
    base_risk: RiskLevel::Safe,
};

/// Every action spec, in a stable order.
pub fn all_specs() -> Vec<ActionSpec> {
    vec![
        LIST_PROCESSES,
        INSPECT_PROCESS,
        SIGNAL_PROCESS,
        MEMORY_INFO,
        SYSTEM_INFO,
        DISK_USAGE,
        FIND_FILES,
        SEARCH_TEXT,
        LIST_DIRECTORY,
        DELETE_PATH,
        LISTENING_PORTS,
        NETWORK_INTERFACES,
        GIT_STATUS,
        GIT_BRANCHES,
        RUN_CONTAINER,
        LIST_CONTAINERS,
        SERVICE_CONTROL,
        ENVIRONMENT_INFO,
    ]
}

/// Look up the spec for a kind identifier.
pub fn spec_for(kind: &str) -> Option<ActionSpec> {
    all_specs().into_iter().find(|spec| spec.kind == kind)
}

/// A human description of a required parameter.
pub fn param_description(kind: &str, param: &str) -> String {
    match (kind, param) {
        (_, "pid") => "the process id".to_string(),
        (_, "target") => "the process to signal (a PID or a process name)".to_string(),
        (_, "image") => "the container image to run".to_string(),
        (_, "pattern") => "the text pattern to search for".to_string(),
        (_, "path") => "the path to operate on".to_string(),
        ("service_control", "action") => {
            "the operation to perform (status, start, stop or restart)".to_string()
        }
        ("service_control", "name") => "the service name".to_string(),
        _ => format!("the {param} parameter"),
    }
}

impl Action {
    /// Static metadata for this action.
    pub fn spec(&self) -> ActionSpec {
        match self {
            Action::ListProcesses(_) => LIST_PROCESSES,
            Action::InspectProcess(_) => INSPECT_PROCESS,
            Action::SignalProcess(_) => SIGNAL_PROCESS,
            Action::MemoryInfo(_) => MEMORY_INFO,
            Action::SystemInfo(_) => SYSTEM_INFO,
            Action::DiskUsage(_) => DISK_USAGE,
            Action::FindFiles(_) => FIND_FILES,
            Action::SearchText(_) => SEARCH_TEXT,
            Action::ListDirectory(_) => LIST_DIRECTORY,
            Action::DeletePath(_) => DELETE_PATH,
            Action::ListeningPorts(_) => LISTENING_PORTS,
            Action::NetworkInterfaces(_) => NETWORK_INTERFACES,
            Action::GitStatus(_) => GIT_STATUS,
            Action::GitBranches(_) => GIT_BRANCHES,
            Action::RunContainer(_) => RUN_CONTAINER,
            Action::ListContainers(_) => LIST_CONTAINERS,
            Action::ServiceControl(_) => SERVICE_CONTROL,
            Action::EnvironmentInfo(_) => ENVIRONMENT_INFO,
        }
    }

    /// The stable kind identifier for this action.
    pub fn kind(&self) -> &'static str {
        self.spec().kind
    }

    /// Parameters that must be present before this action can run.
    pub fn required(&self) -> &'static [&'static str] {
        self.spec().required
    }

    /// Whether a required parameter is absent (or empty).
    pub fn param_missing(&self, name: &str) -> bool {
        fn blank(value: &Option<String>) -> bool {
            value.as_deref().map(str::trim).is_none_or(str::is_empty)
        }
        match self {
            Action::InspectProcess(a) => name == "pid" && a.pid.is_none(),
            Action::SignalProcess(a) => name == "target" && a.target.is_none(),
            Action::SearchText(a) => name == "pattern" && blank(&a.pattern),
            Action::DeletePath(a) => name == "path" && blank(&a.path),
            Action::RunContainer(a) => name == "image" && blank(&a.image),
            Action::ServiceControl(a) => match name {
                "action" => a.action.is_none(),
                "name" => blank(&a.name),
                _ => false,
            },
            _ => false,
        }
    }

    /// The risk of this specific action instance.
    ///
    /// Most actions are classified statically; `service_control` is qualified at runtime
    /// because querying a service is safe while starting/stopping it is not.
    pub fn risk(&self) -> RiskLevel {
        match self {
            Action::ServiceControl(control) => match control.action {
                Some(ServiceAction::Status) => RiskLevel::Safe,
                Some(_) => RiskLevel::Destructive,
                None => RiskLevel::Caution,
            },
            _ => self.spec().base_risk,
        }
    }

    /// Capabilities this action will need once lowered.
    ///
    /// Informational: the authoritative enforcement still happens in the engine when the
    /// lowered command actually runs. This exists so the requirement is inspectable before
    /// any shell code is produced.
    pub fn required_caps(&self) -> Vec<ResourceHandle> {
        use Action::*;
        match self {
            ListProcesses(_) | InspectProcess(_) | SignalProcess(_) | MemoryInfo(_)
            | SystemInfo(_) | ListeningPorts(_) | NetworkInterfaces(_) | ListContainers(_)
            | ServiceControl(_) => vec![ResourceHandle::ProcessSpawn],
            RunContainer(_) => vec![ResourceHandle::ProcessSpawn, ResourceHandle::NetworkAll],
            DiskUsage(a) => vec![
                ResourceHandle::ProcessSpawn,
                ResourceHandle::ReadDir(path_or_cwd(a.path.as_deref())),
            ],
            FindFiles(a) => vec![ResourceHandle::ReadDir(path_or_cwd(a.root.as_deref()))],
            SearchText(a) => vec![ResourceHandle::ReadDir(path_or_cwd(
                a.paths.first().map(String::as_str),
            ))],
            ListDirectory(a) => vec![ResourceHandle::ReadDir(path_or_cwd(a.path.as_deref()))],
            GitStatus(a) => vec![
                ResourceHandle::ProcessSpawn,
                ResourceHandle::ReadDir(path_or_cwd(a.path.as_deref())),
            ],
            GitBranches(a) => vec![
                ResourceHandle::ProcessSpawn,
                ResourceHandle::ReadDir(path_or_cwd(a.path.as_deref())),
            ],
            DeletePath(a) => vec![ResourceHandle::WriteDir(path_or_cwd(a.path.as_deref()))],
            EnvironmentInfo(_) => vec![ResourceHandle::ReadEnv("*".to_string())],
        }
    }
}

fn path_or_cwd(path: Option<&str>) -> PathBuf {
    match path {
        Some(p) if !p.trim().is_empty() => PathBuf::from(p),
        _ => PathBuf::from("."),
    }
}
