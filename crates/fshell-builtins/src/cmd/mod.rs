// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

pub mod bind;
pub mod chart;
pub mod config;
pub mod env;
pub mod explain;
#[cfg(feature = "extract")]
pub mod extract;
pub mod frecency;
pub mod fs;
pub mod hash;
pub mod jobs;
pub mod misc;
pub mod profiler_builtin;
pub mod security;
pub mod self_cmd;
pub mod session;
pub mod sort;
pub mod theme;
pub mod vault;
