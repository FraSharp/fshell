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

    let fields = read_fields(&processed_line, &ifs, var_names.len());

    {
        let mut vars = env.vars.write();
        if var_names.len() == 1 && var_names[0] == "REPLY" {
            // POSIX says if no names, line is assigned without IFS splitting to REPLY (leading/trailing IFS ws trimmed)
            let trimmed = processed_line.trim_matches(|c| ifs.contains(c));
            vars.insert("REPLY".to_string(), Val::String(trimmed.to_string()));
        } else {
            for (name, field) in var_names.iter().zip(fields) {
                vars.insert(name.clone(), Val::String(field.to_string()));
            }
        }
    }

    if is_eof {
        Ok((1, remaining_bytes))
    } else {
        Ok((0, remaining_bytes))
    }
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
    fn raw_read_assigns_the_original_remainder_and_returns_unconsumed_input() {
        let env = Env::for_command();
        let result = read_posix(
            &["-r".into(), "first".into(), "rest".into()],
            &env,
            Some("alpha beta  gamma\nnext\n"),
        )
        .expect("raw read");
        assert_eq!(result, (0, Some(b"next\n".to_vec())));
        assert_eq!(
            env.vars.read().get("first"),
            Some(&Val::String("alpha".into()))
        );
        assert_eq!(
            env.vars.read().get("rest"),
            Some(&Val::String("beta  gamma".into()))
        );
    }
}
