// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::Val;
use fshell_core::diagnostic::StringError;
use fshell_engine::{Env, PipeSender, PipeStream, Signal};

pub fn trap_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    _tx: PipeSender,
) -> Result<(), StringError> {
    // trap [options] [action] [signal...]
    // trap -p           — list traps
    // trap              — list traps (POSIX)
    // trap "cmd" SIG    — set trap
    // trap '' SIG       — ignore signal
    // trap - SIG        — reset signal

    match args.len() {
        0 => {
            // List traps
            let traps = env.posix_traps.read();
            for (signal, cmd) in traps.iter() {
                if cmd.is_empty() {
                    println!("trap -- '' {}", signal.to_str());
                } else {
                    println!("trap -- '{}' {}", cmd, signal.to_str());
                }
            }
            Ok(())
        }
        1 => {
            let arg = args[0].to_text();
            if arg == "-p" {
                // List traps
                let traps = env.posix_traps.read();
                for (signal, cmd) in traps.iter() {
                    if cmd.is_empty() {
                        println!("trap -- '' {}", signal.to_str());
                    } else {
                        println!("trap -- '{}' {}", cmd, signal.to_str());
                    }
                }
                Ok(())
            } else {
                Err("trap: usage: trap [action] [signal...] or trap -p"
                    .to_string()
                    .into())
            }
        }
        _ => {
            // Parse action and signals
            let action = args[0].to_text();

            let signals: Vec<Signal> = args[1..]
                .iter()
                .filter_map(|v| Signal::from_name(&v.to_text()))
                .collect();

            if signals.is_empty() {
                return Err("trap: no valid signals specified".to_string().into());
            }

            let mut traps = env.posix_traps.write();
            for signal in signals {
                if action == "-" {
                    traps.remove(&signal);
                } else {
                    traps.insert(signal, action.clone());
                }
            }
            Ok(())
        }
    }
}
