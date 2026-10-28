// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! The `read` utility's line semantics, shared by the native and POSIX
//! engines: one logical line (backslash-newline joins the next unless `-r`,
//! any other backslash quotes the next character), split by `$IFS` across the
//! names with the remainder going to the last, `REPLY` when no name is given,
//! and status 1 at end of input.

use crate::Env;
use crate::descriptors::{Descriptor, read_line};
use fshell_core::Val;
use std::future::Future;
use std::io;

/// Where `read` takes physical lines from: each call yields the next line
/// including its newline, if it has one, or `None` at end of input.
pub trait LineSource {
    fn next_line(&mut self) -> impl Future<Output = io::Result<Option<Vec<u8>>>> + Send;
}

/// Lines from a descriptor, consuming exactly one line per call.
pub struct DescriptorLines<'a>(pub &'a Descriptor);

impl LineSource for DescriptorLines<'_> {
    fn next_line(&mut self) -> impl Future<Output = io::Result<Option<Vec<u8>>>> + Send {
        read_line(self.0)
    }
}

/// One logical line, and whether its last physical line ended in a newline.
pub struct LogicalLine {
    pub text: String,
    pub terminated: bool,
}

/// Read one logical line.
pub async fn read_logical_line(source: &mut impl LineSource, raw: bool) -> io::Result<LogicalLine> {
    let mut line = String::new();
    loop {
        let Some(bytes) = source.next_line().await? else {
            return Ok(LogicalLine {
                text: line,
                terminated: false,
            });
        };
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        let terminated = text.ends_with('\n');
        if terminated {
            text.pop();
        }
        if raw {
            line.push_str(&text);
            return Ok(LogicalLine {
                text: line,
                terminated,
            });
        }
        let continued = terminated && ends_with_unescaped_backslash(&text);
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                line.extend(chars.next());
            } else {
                line.push(c);
            }
        }
        if !continued {
            return Ok(LogicalLine {
                text: line,
                terminated,
            });
        }
    }
}

/// Assign a logical line to `names` (or `REPLY`) and return `read`'s status.
pub fn assign_read_line(env: &Env, names: &[String], line: LogicalLine) -> i32 {
    if !line.terminated && line.text.is_empty() {
        for name in names {
            env.set_shell_var(name, Val::String(String::new()));
        }
        return 1;
    }
    if names.is_empty() {
        // No names: the whole line, unsplit and untrimmed, goes to REPLY.
        env.set_shell_var("REPLY", Val::String(line.text));
    } else {
        let ifs = env
            .vars
            .read()
            .get("IFS")
            .map(Val::to_text)
            .unwrap_or_else(|| " \t\n".to_string());
        for (name, field) in names.iter().zip(read_fields(&line.text, &ifs, names.len())) {
            env.set_shell_var(name, Val::String(field.to_string()));
        }
    }
    if line.terminated { 0 } else { 1 }
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
