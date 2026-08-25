// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::Val;
use std::time::Instant;

pub struct SpecialVars {
    shell_start: Instant,
}

impl Default for SpecialVars {
    fn default() -> Self {
        Self::new()
    }
}

impl SpecialVars {
    pub fn new() -> Self {
        Self {
            shell_start: Instant::now(),
        }
    }

    pub fn resolve(&self, name: &str) -> Option<Val> {
        match name {
            "RANDOM" => Some(Val::Int(rand::random::<u32>() as i64 % 32768)),
            "SECONDS" => Some(Val::Int(self.shell_start.elapsed().as_secs() as i64)),
            "EPOCHSECONDS" => Some(Val::Int(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
            )),
            "EPOCHREALTIME" => Some(Val::Float(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs_f64(),
            )),
            "FSHPID" => Some(Val::Int(std::process::id() as i64)),
            "COLUMNS" => {
                let (width, _) = terminal_size::terminal_size()?;
                Some(Val::Int(width.0 as i64))
            }
            "LINES" => {
                let (_, height) = terminal_size::terminal_size()?;
                Some(Val::Int(height.0 as i64))
            }
            "HOSTNAME" => hostname::get()
                .ok()
                .map(|h| Val::String(h.to_string_lossy().to_string())),
            "OSTYPE" => {
                #[cfg(target_os = "macos")]
                {
                    Some(Val::String("macos".to_string()))
                }
                #[cfg(target_os = "linux")]
                {
                    Some(Val::String("linux".to_string()))
                }
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                {
                    Some(Val::String(std::env::consts::OS.to_string()))
                }
            }
            "TERM" => std::env::var("TERM").ok().map(Val::String),
            _ => None,
        }
    }
}
