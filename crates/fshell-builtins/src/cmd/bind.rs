// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![allow(clippy::result_large_err)]

use fshell_core::Val;
use fshell_core::diagnostic::StringError;
use fshell_engine::keybindings::{KeyAction, KeyMapMode};
use fshell_engine::{Env, PipeSender, PipeStream};

/// `bind` / `bindkey` builtin command.
///
/// Query, configure, and customize editor keybindings and widgets.
///
/// Usage:
///   bind [-M <mode>] [<chord> [<widget>]]
///   bind -e / -v
///   bind -s <chord> <macro_string>
///   bind -f <chord> <function_name>
///   bind -r <chord>
///   bind -l
pub fn builtin_bind(
    _stdin: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    _out: PipeSender,
) -> Result<(), StringError> {
    let arg_strs: Vec<String> = args
        .iter()
        .map(|v| match v {
            Val::String(s) => s.clone(),
            other => other.to_text(),
        })
        .collect();

    let mut target_mode: Option<KeyMapMode> = None;
    let mut macro_mode = false;
    let mut fn_mode = false;
    let mut remove_mode = false;
    let mut list_widgets = false;
    let mut positional = Vec::new();

    let mut idx = 0;
    while idx < arg_strs.len() {
        let arg = &arg_strs[idx];
        match arg.as_str() {
            "-e" | "--emacs" => {
                let mut reg = env.keybindings.write();
                reg.active_mode = KeyMapMode::Emacs;
                println!("Active keymap: emacs");
                return Ok(());
            }
            "-v" | "--vi" => {
                let mut reg = env.keybindings.write();
                reg.active_mode = KeyMapMode::ViInsert;
                println!("Active keymap: vi");
                return Ok(());
            }
            "-M" | "--mode" => {
                idx += 1;
                if idx < arg_strs.len() {
                    let mode_str = &arg_strs[idx];
                    target_mode = KeyMapMode::parse_mode(mode_str);
                    if target_mode.is_none() {
                        return Err(StringError::from(format!(
                            "unknown keymap mode: {}",
                            mode_str
                        )));
                    }
                } else {
                    return Err(StringError::from("missing argument for -M/--mode"));
                }
            }
            "-s" | "--macro" => macro_mode = true,
            "-f" | "--fn" | "--function" => fn_mode = true,
            "-r" | "-d" | "--remove" | "--delete" => remove_mode = true,
            "-w" | "--widgets" | "-l" | "--list" => list_widgets = true,
            "-h" | "--help" => {
                println!("Usage: bind [-M <mode>] [<chord> [<widget>]]");
                println!("       bind -e                  # Switch to Emacs keymap");
                println!("       bind -v                  # Switch to Vi keymap");
                println!("       bind -s <chord> <macro>  # Bind chord to macro string");
                println!("       bind -f <chord> <fn>     # Bind chord to fsh function");
                println!("       bind -r <chord>          # Unbind chord");
                println!("       bind -w / -l             # List all widgets & current bindings");
                return Ok(());
            }
            s if s.starts_with('-') => {
                return Err(StringError::from(format!("unknown option: {}", s)));
            }
            _ => positional.push(arg.clone()),
        }
        idx += 1;
    }

    if list_widgets {
        let reg = env.keybindings.read();
        let widgets = fshell_engine::keybindings::all_widgets();

        let mut current_cat = "";
        println!(
            "+----------------------------------------------------------------------------------------------------+"
        );
        println!(
            "| Canonical Editor Widgets & Keybindings                                                             |"
        );
        println!(
            "+----------------------------------------------------------------------------------------------------+"
        );
        for w in widgets {
            if w.category != current_cat {
                current_cat = w.category;
                println!("\n  [{}]", current_cat);
                println!(
                    "  {:<26} {:<18} {:<14} Description",
                    "Widget Name", "Emacs Binding", "Vi Binding"
                );
                println!(
                    "  {:<26} {:<18} {:<14} --------------------------------",
                    "--------------------------", "------------------", "--------------"
                );
            }
            let emacs_chords = reg.find_chords_for_widget(KeyMapMode::Emacs, w.name);
            let vi_chords = reg.find_chords_for_widget(KeyMapMode::ViNormal, w.name);
            let vi_ins_chords = reg.find_chords_for_widget(KeyMapMode::ViInsert, w.name);

            let emacs_str = if !emacs_chords.is_empty() {
                emacs_chords.join(", ")
            } else {
                w.default_chord_emacs.unwrap_or("-").to_string()
            };

            let vi_str = if !vi_chords.is_empty() {
                vi_chords.join(", ")
            } else if !vi_ins_chords.is_empty() {
                vi_ins_chords.join(", ")
            } else {
                w.default_chord_vi.unwrap_or("-").to_string()
            };

            println!(
                "  {:<26} {:<18} {:<14} {}",
                w.name, emacs_str, vi_str, w.description
            );
        }
        println!(
            "\nTip: Bind chords using: bind <chord> <widget> (e.g. bind \"alt-w\" \"backward-kill-word\")"
        );
        return Ok(());
    }

    let mut reg = env.keybindings.write();
    let mode = target_mode.unwrap_or(reg.active_mode);

    if remove_mode {
        if positional.is_empty() {
            return Err(StringError::from("bind -r requires a key chord to unbind"));
        }
        for chord in positional {
            match reg.unbind(mode, &chord) {
                Ok(true) => println!("Unbound {} in [{}]", chord, mode),
                Ok(false) => println!("No binding found for {} in [{}]", chord, mode),
                Err(e) => return Err(StringError::from(e)),
            }
        }
        return Ok(());
    }

    if positional.is_empty() {
        // List bindings for mode
        let bindings = reg.list_bindings(mode);
        println!("Keybindings for [{}]:", mode);
        for (chord, action) in bindings {
            println!("  {:18} -> {}", chord, action);
        }
        return Ok(());
    }

    if positional.len() == 1 {
        // Lookup specific binding
        let chord_str = &positional[0];
        let chord =
            fshell_engine::keybindings::KeyChord::parse(chord_str).map_err(StringError::from)?;
        if let Some(action) = reg.get_action(mode, &chord) {
            println!("{} -> {} [{}]", chord, action, mode);
        } else {
            println!("{} is not bound in [{}]", chord_str, mode);
        }
        return Ok(());
    }

    let chord_str = &positional[0];
    let target = &positional[1];

    let action = if macro_mode {
        KeyAction::Macro(target.clone())
    } else if fn_mode {
        KeyAction::Function(target.clone())
    } else {
        KeyAction::Widget(target.clone())
    };

    reg.bind(mode, chord_str, action)
        .map_err(StringError::from)?;

    Ok(())
}
