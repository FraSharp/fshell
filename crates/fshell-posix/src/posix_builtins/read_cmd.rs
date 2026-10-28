// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! POSIX `read` builtin implementation.
//!
//! Conforms to IEEE Std 1003.1 `read` specification:
//! - `read [-r] [name ...]`
//! - Reads one line from stdin (or input stream).
//! - Splits line by `$IFS` across given variable names.
//! - Remainder is assigned to the last variable.
//! - Default variable is `REPLY`.

use fshell_engine::Env;
use fshell_engine::read_input::{DescriptorLines, assign_read_line, read_logical_line};

/// Read one logical line from fd 0 and assign its fields. Status 1 at end of
/// input, even when a final unterminated line was assigned.
///
/// Only the line is consumed: the descriptor's shared offset (a file) or the
/// pipe leaves the rest for the next command, which is what makes
/// `while read line; do …; done < file` iterate.
/// Errors carry the status to return: 2 for usage, 1 for an unreadable input.
pub async fn read_posix(args: &[String], env: &Env) -> Result<i32, (i32, String)> {
    let mut raw_mode = false;
    let mut var_names = Vec::new();

    let mut arg_iter = args.iter();
    while let Some(arg) = arg_iter.next() {
        if arg == "-r" {
            raw_mode = true;
        } else if arg == "--" {
            var_names.extend(arg_iter.by_ref().cloned());
            break;
        } else if arg.starts_with('-') && arg.len() > 1 {
            return Err((2, format!("read: {arg}: invalid option")));
        } else {
            var_names.push(arg.clone());
        }
    }

    let input = env
        .descriptor(0)
        .ok_or_else(|| (1, "read: standard input: Bad file descriptor".to_string()))?;
    let line = read_logical_line(&mut DescriptorLines(&input), raw_mode)
        .await
        .map_err(|e| (1, format!("read: read error: {e}")))?;
    Ok(assign_read_line(env, &var_names, line))
}
