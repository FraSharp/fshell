// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::path::PathBuf;

/// High-level sandbox confinement mode.
#[derive(Clone, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum SandboxMode {
    /// ReadOnlySystem: Protects the system and sensitive files (/etc, /usr, /bin, /System, ~/.ssh, ~/.aws, etc.)
    /// by blocking write mutations at the OS kernel level, while allowing writes to the working directory ($PWD),
    /// temporary paths (/tmp, /var/tmp), and explicitly permitted paths. (Default)
    #[default]
    ReadOnlySystem,
    /// DenyAll: Equivalent to ReadOnlySystem for backwards compatibility.
    DenyAll,
    /// Isolated: System write protection PLUS network isolation (blocks network socket creation / outbound / inbound).
    Isolated,
    /// Monitor: Logs the sandboxing profile before executing.
    Monitor,
    /// Prompt: Interactive mode fallback (now maps to standard ReadOnlySystem kernel isolation).
    Prompt,
    /// Off: Runs without any OS sandboxing restrictions.
    Off,
}

/// Confinement profile describing kernel sandbox parameters.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SandboxProfile {
    pub mode: SandboxMode,
    /// Paths where file creation / modification is explicitly permitted.
    pub allow_write_paths: Vec<PathBuf>,
    /// Paths where file creation / modification is explicitly blocked.
    pub deny_write_paths: Vec<PathBuf>,
    /// Whether network connections are permitted.
    pub allow_network: bool,
}

impl SandboxProfile {
    pub fn new(mode: SandboxMode) -> Self {
        let allow_network = !matches!(mode, SandboxMode::Isolated);
        Self {
            mode,
            allow_write_paths: Vec::new(),
            deny_write_paths: Vec::new(),
            allow_network,
        }
    }

    pub fn allow_write(mut self, path: PathBuf) -> Self {
        self.allow_write_paths.push(path);
        self
    }

    pub fn deny_write(mut self, path: PathBuf) -> Self {
        self.deny_write_paths.push(path);
        self
    }

    pub fn with_network(mut self, allow: bool) -> Self {
        self.allow_network = allow;
        self
    }
}
