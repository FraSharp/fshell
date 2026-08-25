// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::panic))]
pub mod app;
pub mod theme_ext;
pub mod widgets;

use fshell_engine::Env;

pub fn run_config_tui(env: &Env) -> Result<(), String> {
    app::run(env)
}
