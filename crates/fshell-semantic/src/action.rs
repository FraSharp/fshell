// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! The typed semantic action model.
//!
//! This is the shell-independent representation of *what the user wants*, before any
//! decision about how a shell should execute it. It deliberately says nothing about CLI
//! flags, quoting, POSIX vs fsh, or the host operating system — those belong to lowering
//! ([`crate::lower`]).
//!
//! Every parameter is optional so that a *partially understood* request is representable
//! without inventing values ("run nginx on port 8000" carries the port but no image).
//! Requiredness is declared once per action in [`crate::spec`] and consumed by both
//! validation and schema generation.

use crate::error::SemanticError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

/// A single typed operation extracted from a natural-language request.
///
/// Serializes with an internal `kind` tag so the JSON form is flat and easy for a small
/// function-calling model to emit, e.g.
/// `{"kind":"find_files","root":".","extension":"log","min_size":"500MB"}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    ListProcesses(ListProcesses),
    InspectProcess(InspectProcess),
    SignalProcess(SignalProcess),
    MemoryInfo(MemoryInfo),
    SystemInfo(SystemInfo),
    DiskUsage(DiskUsage),
    FindFiles(FindFiles),
    SearchText(SearchText),
    ListDirectory(ListDirectory),
    DeletePath(DeletePath),
    ListeningPorts(ListeningPorts),
    NetworkInterfaces(NetworkInterfaces),
    GitStatus(GitStatus),
    GitBranches(GitBranches),
    RunContainer(RunContainer),
    ListContainers(ListContainers),
    ServiceControl(ServiceControl),
    EnvironmentInfo(EnvironmentInfo),
}

// ---------------------------------------------------------------------------
// Processes
// ---------------------------------------------------------------------------

/// List running processes, optionally filtered, sorted and limited.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ListProcesses {
    /// Criteria a process must match to be included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<ProcessFilter>,
    /// Ordering of the result set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<ProcessSort>,
    /// Keep only the first N processes after sorting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Include processes owned by every user (not just the current user).
    #[serde(default)]
    pub all_users: bool,
}

/// Filters applied to a process listing.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProcessFilter {
    /// Match processes whose command line contains this substring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_contains: Option<String>,
    /// Restrict to processes owned by this user name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Keep processes using at least this much resident memory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_memory: Option<ByteSize>,
    /// Keep processes using at least this percentage of CPU.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_cpu: Option<f32>,
}

/// Sort specification for a process listing.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessSort {
    /// Field to order by.
    #[serde(default)]
    pub by: ProcessSortBy,
    /// Order largest/last-first when true (the usual meaning of "top").
    #[serde(default)]
    pub descending: bool,
}

impl Default for ProcessSort {
    fn default() -> Self {
        Self {
            by: ProcessSortBy::Memory,
            descending: true,
        }
    }
}

/// Field a process listing can be ordered by.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessSortBy {
    Pid,
    Cpu,
    #[default]
    Memory,
    Command,
    User,
}

impl ProcessSortBy {
    /// Human-facing name (also the `ps`/`ff`-style column used by fsh lowering).
    pub const fn column(self) -> &'static str {
        match self {
            ProcessSortBy::Pid => "pid",
            ProcessSortBy::Cpu => "cpu",
            ProcessSortBy::Memory => "rss",
            ProcessSortBy::Command => "command",
            ProcessSortBy::User => "user",
        }
    }
}

/// Inspect a single process by id.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InspectProcess {
    /// Process id to inspect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// Send a signal to a process or to every process matching a name.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SignalProcess {
    /// Which process(es) to signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ProcessTarget>,
    /// Signal to send; defaults to `TERM`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<Signal>,
}

/// How a signal target is identified.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum ProcessTarget {
    /// A single process id.
    Pid { pid: u32 },
    /// Every process whose command line matches this name.
    Name { name: String },
}

/// A portable POSIX signal name.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Signal {
    #[default]
    Term,
    Kill,
    Interrupt,
    Hup,
    Quit,
    Stop,
    Cont,
    Usr1,
    Usr2,
}

impl Signal {
    /// The signal name without the leading `SIG`, e.g. `"TERM"`.
    pub const fn name(self) -> &'static str {
        match self {
            Signal::Term => "TERM",
            Signal::Kill => "KILL",
            Signal::Interrupt => "INT",
            Signal::Hup => "HUP",
            Signal::Quit => "QUIT",
            Signal::Stop => "STOP",
            Signal::Cont => "CONT",
            Signal::Usr1 => "USR1",
            Signal::Usr2 => "USR2",
        }
    }
}

// ---------------------------------------------------------------------------
// System
// ---------------------------------------------------------------------------

/// Report system memory usage.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryInfo {}

/// Report general system information.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SystemInfo {}

/// Report disk usage.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiskUsage {
    /// Path to report on; defaults to the current directory (or all filesystems).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Whether to summarise filesystems or break usage down per directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<DiskMode>,
}

/// Granularity of a disk-usage report.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskMode {
    /// Summarise mounted filesystems (`df`).
    #[default]
    Summary,
    /// Break usage down per directory (`du`).
    PerDirectory,
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

/// Find files under a root directory using typed filters.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FindFiles {
    /// Directory to search; defaults to the current directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// Glob matched against the file *name* (e.g. `"*.log"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Extension to match when `name` is not given (e.g. `"log"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension: Option<String>,
    /// Restrict results to this kind of filesystem entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_type: Option<FileType>,
    /// Minimum size (inclusive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_size: Option<ByteSize>,
    /// Maximum size (inclusive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size: Option<ByteSize>,
    /// Only files modified within this span of the present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_within: Option<TimeSpan>,
    /// Only files last modified longer ago than this span.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_before: Option<TimeSpan>,
    /// Include dotfiles and hidden directories.
    #[serde(default)]
    pub include_hidden: bool,
    /// Maximum recursion depth below `root`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
    /// Maximum number of results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// The kind of filesystem entry a query targets.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileType {
    File,
    Dir,
    Symlink,
}

/// Search file contents for a pattern.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchText {
    /// Pattern to search for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Files or directories to search; defaults to the current directory.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// Match case-insensitively.
    #[serde(default)]
    pub ignore_case: bool,
    /// Treat the pattern as a literal string rather than a regular expression.
    #[serde(default)]
    pub fixed_strings: bool,
    /// Only search files matching this glob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_glob: Option<String>,
    /// Maximum number of matching lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u32>,
}

/// List a directory, optionally with sorting and recursion.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ListDirectory {
    /// Directory to list; defaults to the current directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Include dotfiles.
    #[serde(default)]
    pub include_hidden: bool,
    /// Produce a long/detailed listing.
    #[serde(default)]
    pub long: bool,
    /// Recurse into subdirectories.
    #[serde(default)]
    pub recursive: bool,
    /// Ordering of the listing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<DirSort>,
    /// Maximum number of entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Sort specification for a directory listing.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirSort {
    /// Field to order by.
    #[serde(default)]
    pub by: DirSortBy,
    /// Reverse the order when true.
    #[serde(default)]
    pub descending: bool,
}

impl Default for DirSort {
    fn default() -> Self {
        Self {
            by: DirSortBy::Name,
            descending: false,
        }
    }
}

/// Field a directory listing can be ordered by.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirSortBy {
    #[default]
    Name,
    Size,
    Time,
}

/// Delete a file or directory tree.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeletePath {
    /// Path to delete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Delete directories and their contents recursively.
    #[serde(default)]
    pub recursive: bool,
    /// Ignore non-existent paths and never prompt.
    #[serde(default)]
    pub force: bool,
}

// ---------------------------------------------------------------------------
// Network
// ---------------------------------------------------------------------------

/// Show sockets that are listening, optionally on a specific port.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ListeningPorts {
    /// Restrict to this local port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Restrict to this transport protocol.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<Transport>,
}

/// Transport protocol selector.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    #[default]
    Any,
    Tcp,
    Udp,
}

/// Show the host's network interfaces and addresses.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NetworkInterfaces {}

// ---------------------------------------------------------------------------
// Git
// ---------------------------------------------------------------------------

/// Show the working-tree status of a Git repository.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GitStatus {
    /// Repository path; defaults to the current directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// List branches of a Git repository.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GitBranches {
    /// Repository path; defaults to the current directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Include remote-tracking branches.
    #[serde(default)]
    pub include_remote: bool,
}

// ---------------------------------------------------------------------------
// Containers
// ---------------------------------------------------------------------------

/// Start a container from an image.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunContainer {
    /// Image to run (e.g. `"nginx"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Name to assign to the container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Published port mappings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<PortMapping>,
    /// Environment variables to set inside the container.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<EnvVar>,
    /// Run in the background.
    #[serde(default)]
    pub detached: bool,
    /// Command to run inside the container (overrides the image default).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
}

/// A single host→container port publication.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PortMapping {
    /// Port on the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_port: Option<u16>,
    /// Port inside the container; defaults to `host_port` when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_port: Option<u16>,
}

/// An environment variable passed to a container.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
}

/// List containers.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ListContainers {
    /// Include stopped containers.
    #[serde(default)]
    pub all: bool,
}

// ---------------------------------------------------------------------------
// Services
// ---------------------------------------------------------------------------

/// Query or change the state of a system service.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServiceControl {
    /// Operation to perform.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<ServiceAction>,
    /// Service name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Operation performed on a service.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceAction {
    Status,
    Start,
    Stop,
    Restart,
}

impl ServiceAction {
    /// Whether the action mutates system state (and is therefore destructive).
    pub const fn is_mutating(self) -> bool {
        !matches!(self, ServiceAction::Status)
    }
}

// ---------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------

/// Report environment variables.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentInfo {
    /// Report only this variable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variable: Option<String>,
}

// ---------------------------------------------------------------------------
// Value types
// ---------------------------------------------------------------------------

/// A byte quantity.
///
/// Deserializes from either an integer number of bytes or a human string such as
/// `"500MB"` (decimal, powers of 1000) or `"1GiB"` (binary, powers of 1024). Serializes
/// back to the exact integer byte count.
#[cfg_attr(
    feature = "schema",
    derive(JsonSchema),
    schemars(
        description = "Byte quantity: an integer number of bytes, or a string like \"500MB\" / \"1GiB\"."
    )
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ByteSize(u64);

impl ByteSize {
    /// Construct from an exact number of bytes.
    pub const fn from_bytes(bytes: u64) -> Self {
        Self(bytes)
    }

    /// The exact number of bytes.
    pub const fn bytes(self) -> u64 {
        self.0
    }

    /// The quantity in kibibytes (rounded down).
    ///
    /// Used when comparing against tools that report memory in KiB (e.g. `ps` RSS).
    pub const fn kib(self) -> u64 {
        self.0 / 1024
    }

    /// Parse a human byte-size string.
    pub fn parse(input: &str) -> Result<Self, SemanticError> {
        let text = input.trim().to_ascii_lowercase();
        if text.is_empty() {
            return Err(SemanticError::InvalidValue(
                "empty byte-size value".to_string(),
            ));
        }
        let split = text
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(text.len());
        let (number, unit) = text.split_at(split);
        let magnitude: f64 = number.parse().map_err(|_| {
            SemanticError::InvalidValue(format!("invalid byte-size value: {input:?}"))
        })?;
        if magnitude < 0.0 {
            return Err(SemanticError::InvalidValue(format!(
                "negative byte-size value: {input:?}"
            )));
        }
        let scale = match unit.trim() {
            "" | "b" | "byte" | "bytes" => 1.0,
            "k" | "kb" => 1000.0,
            "m" | "mb" => 1000f64.powi(2),
            "g" | "gb" => 1000f64.powi(3),
            "t" | "tb" => 1000f64.powi(4),
            "p" | "pb" => 1000f64.powi(5),
            "e" | "eb" => 1000f64.powi(6),
            "kib" => 1024.0,
            "mib" => 1024f64.powi(2),
            "gib" => 1024f64.powi(3),
            "tib" => 1024f64.powi(4),
            "pib" => 1024f64.powi(5),
            "eib" => 1024f64.powi(6),
            other => {
                return Err(SemanticError::InvalidValue(format!(
                    "unknown byte-size unit {other:?} in {input:?}"
                )));
            }
        };
        Ok(Self((magnitude * scale).round() as u64))
    }

    /// A compact human label (used in clarification text).
    pub fn human(self) -> String {
        const UNITS: [(u64, &str); 5] = [
            (1 << 40, "TiB"),
            (1 << 30, "GiB"),
            (1 << 20, "MiB"),
            (1 << 10, "KiB"),
            (1, "B"),
        ];
        for (factor, suffix) in UNITS {
            if self.0 >= factor && self.0.is_multiple_of(factor) {
                return format!("{}{}", self.0 / factor, suffix);
            }
        }
        format!("{}B", self.0)
    }
}

impl std::fmt::Display for ByteSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.human())
    }
}

impl From<u64> for ByteSize {
    fn from(bytes: u64) -> Self {
        Self(bytes)
    }
}

impl Serialize for ByteSize {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(self.0)
    }
}

impl<'de> Deserialize<'de> for ByteSize {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Bytes(u64),
            Human(String),
        }
        match Repr::deserialize(deserializer)? {
            Repr::Bytes(n) => Ok(Self(n)),
            Repr::Human(s) => ByteSize::parse(&s).map_err(serde::de::Error::custom),
        }
    }
}

/// A duration.
///
/// Deserializes from either an integer number of seconds or a human string such as
/// `"7d"`, `"2h"`, `"30m"` or `"45s"`. Serializes back to exact seconds.
#[cfg_attr(
    feature = "schema",
    derive(JsonSchema),
    schemars(
        description = "Duration: an integer number of seconds, or a string like \"7d\" / \"2h\" / \"30m\"."
    )
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimeSpan(i64);

impl TimeSpan {
    /// Construct from an exact number of seconds.
    pub const fn from_seconds(seconds: i64) -> Self {
        Self(seconds)
    }

    /// The exact number of seconds.
    pub const fn seconds(self) -> i64 {
        self.0
    }

    /// The span in whole minutes, rounded up (never zero for a positive span).
    pub const fn minutes_ceil(self) -> i64 {
        if self.0 <= 0 { 0 } else { (self.0 + 59) / 60 }
    }

    /// Parse a human duration string.
    pub fn parse(input: &str) -> Result<Self, SemanticError> {
        let text = input.trim().to_ascii_lowercase();
        if text.is_empty() {
            return Err(SemanticError::InvalidValue(
                "empty time-span value".to_string(),
            ));
        }
        // Longest suffixes first so "mins" is not read as "s".
        const SUFFIXES: &[(&str, i64)] = &[
            ("seconds", 1),
            ("second", 1),
            ("secs", 1),
            ("sec", 1),
            ("minutes", 60),
            ("minute", 60),
            ("mins", 60),
            ("min", 60),
            ("hours", 3600),
            ("hour", 3600),
            ("hrs", 3600),
            ("hr", 3600),
            ("days", 86400),
            ("day", 86400),
            ("weeks", 604800),
            ("week", 604800),
            ("w", 604800),
            ("d", 86400),
            ("h", 3600),
            ("m", 60),
            ("s", 1),
        ];
        let (number, multiplier) = SUFFIXES
            .iter()
            .find_map(|(suffix, mult)| text.strip_suffix(suffix).map(|n| (n, *mult)))
            .unwrap_or((text.as_str(), 1));
        let magnitude: f64 = number.parse().map_err(|_| {
            SemanticError::InvalidValue(format!("invalid time-span value: {input:?}"))
        })?;
        if magnitude < 0.0 {
            return Err(SemanticError::InvalidValue(format!(
                "negative time-span value: {input:?}"
            )));
        }
        Ok(Self((magnitude * multiplier as f64).round() as i64))
    }

    /// A compact human label (used in clarification text).
    pub fn human(self) -> String {
        const UNITS: [(i64, &str); 4] = [(604800, "w"), (86400, "d"), (3600, "h"), (60, "m")];
        for (factor, suffix) in UNITS {
            if self.0 >= factor && (self.0 as u64).is_multiple_of(factor as u64) {
                return format!("{}{}", self.0 / factor, suffix);
            }
        }
        format!("{}s", self.0)
    }
}

impl std::fmt::Display for TimeSpan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.human())
    }
}

impl From<i64> for TimeSpan {
    fn from(seconds: i64) -> Self {
        Self(seconds)
    }
}

impl Serialize for TimeSpan {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(self.0)
    }
}

impl<'de> Deserialize<'de> for TimeSpan {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Seconds(i64),
            Human(String),
        }
        match Repr::deserialize(deserializer)? {
            Repr::Seconds(n) => Ok(Self(n)),
            Repr::Human(s) => TimeSpan::parse(&s).map_err(serde::de::Error::custom),
        }
    }
}
