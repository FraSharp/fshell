// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SubcmdCompletion {
    pub parent_subcmds: Vec<String>,
    pub name: String,
    pub desc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct FlagCompletion {
    pub parent_subcmds: Vec<String>,
    pub short: Option<String>,
    pub long: Option<String>,
    pub desc: Option<String>,
    pub choices: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct DynamicProvider {
    pub parent_subcmds: Vec<String>,
    pub command: String,
    pub cache_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct CommandCompletion {
    #[serde(default)]
    pub subcommands: Vec<SubcmdCompletion>,
    #[serde(default)]
    pub flags: Vec<FlagCompletion>,
    #[serde(default)]
    pub dynamic_providers: Vec<DynamicProvider>,
}
