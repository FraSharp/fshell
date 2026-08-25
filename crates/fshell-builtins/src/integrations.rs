// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![allow(clippy::result_large_err)]

//! Native integrations for modern CLI ecosystem tools (Starship, Direnv, Zoxide, FZF).
//!
//! Replaces fragile shell eval string scripts (`eval "$(starship init bash)"`) with
//! native, typed in-process engine hooks and fast execution pipelines.

use fshell_core::Val;
use fshell_core::diagnostic::StringError;
use fshell_engine::{Env, PipeSender, PipeStream, register_hook};

/// `direnv_init` builtin: registers native chpwd and precmd hooks for direnv.
pub fn direnv_init_builtin(
    _in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    env: &Env,
    _out: PipeSender,
) -> Result<(), StringError> {
    register_hook("chpwd", "eval_direnv", env).map_err(StringError::from)?;
    register_hook("precmd", "eval_direnv", env).map_err(StringError::from)?;
    Ok(())
}

/// `zoxide_init` builtin: registers native chpwd hook to track directories with zoxide.
pub fn zoxide_init_builtin(
    _in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    env: &Env,
    _out: PipeSender,
) -> Result<(), StringError> {
    register_hook("chpwd", "zoxide_hook", env).map_err(StringError::from)?;
    Ok(())
}

/// `starship_init` builtin: configures native prompt integration with Starship.
pub fn starship_init_builtin(
    _in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    env: &Env,
    _out: PipeSender,
) -> Result<(), StringError> {
    register_hook("precmd", "starship_precmd", env).map_err(StringError::from)?;
    register_hook("preexec", "starship_preexec", env).map_err(StringError::from)?;
    Ok(())
}

/// `fzf_init` builtin: binds default FZF shortcuts into active keymaps.
pub fn fzf_init_builtin(
    _in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    env: &Env,
    _out: PipeSender,
) -> Result<(), StringError> {
    let mut reg = env.keybindings.write();
    reg.bind(
        fshell_engine::keybindings::KeyMapMode::Emacs,
        "ctrl-t",
        fshell_engine::keybindings::KeyAction::Widget("fzf-file-widget".to_string()),
    )
    .map_err(StringError::from)?;
    reg.bind(
        fshell_engine::keybindings::KeyMapMode::Emacs,
        "alt-c",
        fshell_engine::keybindings::KeyAction::Widget("fzf-cd-widget".to_string()),
    )
    .map_err(StringError::from)?;
    reg.bind(
        fshell_engine::keybindings::KeyMapMode::Emacs,
        "ctrl-r",
        fshell_engine::keybindings::KeyAction::Widget("interactive-history-search".to_string()),
    )
    .map_err(StringError::from)?;
    Ok(())
}
