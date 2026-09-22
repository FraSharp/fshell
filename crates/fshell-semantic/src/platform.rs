// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Platform awareness used by lowering.
//!
//! Lowering must not assume GNU/Linux. It consults the host OS and which external tools
//! are actually installed. [`UtilitySet`] can be constructed explicitly so tests stay
//! hermetic and independent of the machine running them.

use std::collections::HashSet;
use std::path::Path;

/// The host operating-system family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Os {
    Linux,
    MacOs,
    Other,
}

impl Os {
    /// The OS this binary was compiled for.
    pub const fn current() -> Self {
        if cfg!(target_os = "linux") {
            Os::Linux
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Other
        }
    }

    /// A short lowercase label.
    pub const fn label(self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::MacOs => "macos",
            Os::Other => "other",
        }
    }
}

/// External tools fshell lowers to probe for on the host.
const PROBED: &[&str] = &[
    "rg",
    "grep",
    "find",
    "lsof",
    "ss",
    "netstat",
    "df",
    "du",
    "free",
    "vm_stat",
    "sysctl",
    "ifconfig",
    "ip",
    "docker",
    "podman",
    "systemctl",
    "launchctl",
    "sw_vers",
    "uname",
    "pkill",
    "kill",
];

/// A set of external tools known to be available.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UtilitySet {
    available: HashSet<String>,
}

impl UtilitySet {
    /// An empty set.
    pub fn empty() -> Self {
        Self::default()
    }

    /// A set containing exactly the named tools (used by tests).
    pub fn of<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            available: names.into_iter().map(Into::into).collect(),
        }
    }

    /// Probe `PATH` for the tools fshell can lower to.
    pub fn probe() -> Self {
        let mut available = HashSet::new();
        if let Some(path) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path) {
                for tool in PROBED {
                    if available.contains(*tool) {
                        continue;
                    }
                    if is_executable(&dir.join(tool)) {
                        available.insert((*tool).to_string());
                    }
                }
            }
        }
        Self { available }
    }

    /// Whether a tool is available.
    pub fn has(&self, tool: &str) -> bool {
        self.available.contains(tool)
    }

    /// The first available tool from a preference-ordered list.
    pub fn first_of(&self, tools: &[&'static str]) -> Option<&'static str> {
        tools.iter().copied().find(|tool| self.has(tool))
    }
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// The lowering context: host OS plus the external tools available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Platform {
    /// Host operating-system family.
    pub os: Os,
    /// External tools available on the host.
    pub utilities: UtilitySet,
}

impl Platform {
    /// Detect the platform for the current machine.
    pub fn detect() -> Self {
        Self {
            os: Os::current(),
            utilities: UtilitySet::probe(),
        }
    }

    /// Build a platform explicitly (used by tests).
    pub fn new(os: Os, utilities: UtilitySet) -> Self {
        Self { os, utilities }
    }
}

impl Default for Platform {
    fn default() -> Self {
        Self::detect()
    }
}
