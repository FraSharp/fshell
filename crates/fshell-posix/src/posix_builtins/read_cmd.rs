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

use fshell_core::Val;
use fshell_engine::Env;
use std::io::BufRead;

pub fn read_posix(
    args: &[String],
    env: &Env,
    stdin_data: Option<&str>,
) -> Result<(i32, Option<Vec<u8>>), String> {
    let mut raw_mode = false;
    let mut var_names = Vec::new();

    let mut arg_iter = args.iter().peekable();
    while let Some(arg) = arg_iter.next() {
        if arg == "-r" {
            raw_mode = true;
        } else if arg == "--" {
            for rest in arg_iter {
                var_names.push(rest.clone());
            }
            break;
        } else if arg.starts_with('-') {
            // Other flags like -p prompt can be ignored or handled
        } else {
            var_names.push(arg.clone());
        }
    }

    if var_names.is_empty() {
        var_names.push("REPLY".to_string());
    }

    // Read one line from stdin_data if provided, else from std::io::stdin
    let (line, remaining_bytes, is_eof) = if let Some(data) = stdin_data {
        if let Some(first_nl) = data.find('\n') {
            (
                data[..first_nl].to_string(),
                Some(data.as_bytes()[first_nl + 1..].to_vec()),
                false,
            )
        } else {
            (data.to_string(), Some(Vec::new()), data.is_empty())
        }
    } else {
        let stdin = std::io::stdin();
        let mut handle = stdin.lock();
        let mut buf = String::new();
        match handle.read_line(&mut buf) {
            Ok(0) => (String::new(), None, true),
            Ok(_) => {
                if buf.ends_with('\n') {
                    buf.pop();
                    if buf.ends_with('\r') {
                        buf.pop();
                    }
                }
                (buf, None, false)
            }
            Err(e) => return Err(format!("read error: {}", e)),
        }
    };

    if is_eof && line.is_empty() {
        return Ok((1, remaining_bytes));
    }

    // Handle line processing (backslash escapes if not -r)
    let processed_line = if raw_mode {
        line
    } else {
        let mut res = String::new();
        let mut escaped = false;
        for c in line.chars() {
            if escaped {
                res.push(c);
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else {
                res.push(c);
            }
        }
        res
    };

    // Split processed_line into fields using $IFS
    let ifs = Some(env.vars.read())
        .and_then(|vars| vars.get("IFS").map(|v| v.to_text()))
        .unwrap_or_else(|| " \t\n".to_string());

    let fields = crate::expand::split_ifs(&processed_line, &ifs);

    {
        let mut vars = env.vars.write();
        if var_names.len() == 1 && var_names[0] == "REPLY" {
            // POSIX says if no names, line is assigned without IFS splitting to REPLY (leading/trailing IFS ws trimmed)
            let trimmed = processed_line.trim_matches(|c| ifs.contains(c));
            vars.insert("REPLY".to_string(), Val::String(trimmed.to_string()));
        } else {
            let num_vars = var_names.len();
            for (idx, name) in var_names.iter().enumerate() {
                if idx < num_vars - 1 {
                    let val = fields.get(idx).cloned().unwrap_or_default();
                    vars.insert(name.clone(), Val::String(val));
                } else {
                    // Last variable gets all remaining fields
                    if idx < fields.len() {
                        let remainder = fields[idx..].join(" ");
                        vars.insert(name.clone(), Val::String(remainder));
                    } else {
                        vars.insert(name.clone(), Val::String(String::new()));
                    }
                }
            }
        }
    }

    if is_eof {
        Ok((1, remaining_bytes))
    } else {
        Ok((0, remaining_bytes))
    }
}
