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
use fshell_engine::descriptors::{Descriptor, read_line};

/// Read one logical line from fd 0 and assign its fields. Status 1 at end of
/// input, even when a final unterminated line was assigned.
///
/// Only the line is consumed: the descriptor's shared offset (a file) or
/// shared buffer (a pipe) leaves the rest for the next command, which is what
/// makes `while read line; do …; done < file` iterate.
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

    let input: Descriptor = env
        .descriptor(0)
        .ok_or_else(|| (1, "read: standard input: Bad file descriptor".to_string()))?;

    // Without -r a backslash-newline joins the next line and any other
    // backslash quotes the character after it.
    let mut line = String::new();
    let mut terminated;
    loop {
        let chunk = read_line(&input)
            .await
            .map_err(|e| (1, format!("read: read error: {e}")))?;
        let Some(bytes) = chunk else {
            terminated = false;
            break;
        };
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        terminated = text.ends_with('\n');
        if terminated {
            text.pop();
        }
        if raw_mode {
            line.push_str(&text);
            break;
        }
        let continued = terminated && ends_with_unescaped_backslash(&text);
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                if let Some(next) = chars.next() {
                    line.push(next);
                }
            } else {
                line.push(c);
            }
        }
        if !continued {
            break;
        }
    }

    if !terminated && line.is_empty() {
        for name in &var_names {
            env.set_shell_var(name, Val::String(String::new()));
        }
        return Ok(1);
    }

    if var_names.is_empty() {
        // No names: the whole line, unsplit and untrimmed, goes to REPLY.
        env.set_shell_var("REPLY", Val::String(line));
    } else {
        let ifs = env
            .vars
            .read()
            .get("IFS")
            .map(|v| v.to_text())
            .unwrap_or_else(|| " \t\n".to_string());
        let fields = read_fields(&line, &ifs, var_names.len());
        for (name, field) in var_names.iter().zip(fields) {
            env.set_shell_var(name, Val::String(field.to_string()));
        }
    }
    Ok(if terminated { 0 } else { 1 })
}

fn ends_with_unescaped_backslash(text: &str) -> bool {
    text.chars().rev().take_while(|&c| c == '\\').count() % 2 == 1
}

/// Retain field spans so the final name receives the original remainder,
/// including intervening delimiters, rather than reconstructed words.
fn read_fields<'a>(line: &'a str, ifs: &str, names: usize) -> Vec<&'a str> {
    let whitespace = |c: char| matches!(c, ' ' | '\t' | '\n') && ifs.contains(c);
    let line = line.trim_matches(whitespace);
    let mut chars = line.char_indices().peekable();
    let mut fields = Vec::new();
    let mut start = 0;
    while let Some((offset, c)) = chars.next() {
        if !ifs.contains(c) {
            continue;
        }
        fields.push(start..offset);
        if whitespace(c) {
            while chars.peek().is_some_and(|(_, c)| whitespace(*c)) {
                chars.next();
            }
            if chars
                .peek()
                .is_some_and(|(_, c)| ifs.contains(*c) && !whitespace(*c))
            {
                chars.next();
            }
        }
        while chars.peek().is_some_and(|(_, c)| whitespace(*c)) {
            chars.next();
        }
        start = chars.peek().map_or(line.len(), |(offset, _)| *offset);
    }
    if start < line.len() {
        fields.push(start..line.len());
    }
    (0..names)
        .map(|index| match fields.get(index) {
            Some(field) if index + 1 == names && index + 1 < fields.len() => &line[field.start..],
            Some(field) => &line[field.clone()],
            None => "",
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_name_preserves_internal_ifs_whitespace() {
        assert_eq!(read_fields("  alpha  beta  ", " \t\n", 1), ["alpha  beta"]);
        assert_eq!(
            read_fields("  alpha   beta  gamma  ", " \t\n", 2),
            ["alpha", "beta  gamma"]
        );
    }

    #[test]
    fn last_name_preserves_non_whitespace_delimiters_between_remaining_fields() {
        assert_eq!(read_fields("::alpha:beta::", ":", 2), ["", ":alpha:beta::"]);
        assert_eq!(read_fields("alpha:beta::", ":", 2), ["alpha", "beta::"]);
        assert_eq!(
            read_fields(" alpha : beta :: ", ": ", 2),
            ["alpha", "beta ::"]
        );
    }

    #[test]
    fn a_trailing_delimiter_terminates_the_last_field_without_creating_another() {
        assert_eq!(read_fields("alpha:beta:", ":", 2), ["alpha", "beta"]);
        assert_eq!(read_fields("alpha:beta::", ":", 3), ["alpha", "beta", ""]);
        assert_eq!(read_fields("alpha:", ":", 1), ["alpha"]);
    }

    #[test]
    fn empty_ifs_preserves_the_entire_line_and_unfilled_names_are_empty() {
        assert_eq!(
            read_fields("  alpha  beta  ", "", 2),
            ["  alpha  beta  ", ""]
        );
        assert_eq!(read_fields("", " \t\n", 3), ["", "", ""]);
    }

    #[test]
    fn continuation_needs_an_odd_number_of_trailing_backslashes() {
        assert!(ends_with_unescaped_backslash("a\\"));
        assert!(!ends_with_unescaped_backslash("a\\\\"));
        assert!(!ends_with_unescaped_backslash("a"));
    }
}
