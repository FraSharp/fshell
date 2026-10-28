// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Redirections, applied to the shell environment's descriptor table.
//!
//! A command's redirections rebind descriptors in source order — that order is
//! the semantics: `> out 2>&1` sends both streams to `out`, `2>&1 > out` only
//! stdout. They are applied to the live table so the command and everything it
//! runs (a function body, a loop, a nested group) sees them, and undone when it
//! finishes, however it finishes. `exec` without a command is the one place
//! they are kept.

use brush_parser::ast::{IoFileRedirectKind, IoFileRedirectTarget, IoRedirect, Word};
use fshell_engine::descriptors::{Descriptor, here_document};
use fshell_engine::{EngineError, Env};
use std::sync::Arc;

use crate::eval::PosixError;
use crate::expand::{ExpansionConfig, expand_word};

/// Bindings displaced by applied redirections, restored on drop.
pub(crate) struct Redirected<'a> {
    env: &'a Env,
    saved: Vec<(i32, Option<Descriptor>)>,
}

impl<'a> Redirected<'a> {
    pub(crate) fn none(env: &'a Env) -> Self {
        Self {
            env,
            saved: Vec::new(),
        }
    }

    /// Apply `redirects` in order. On failure the ones already applied are
    /// undone and the error names the redirection that failed.
    pub(crate) fn apply(
        env: &'a Env,
        redirects: &[&IoRedirect],
        positional: &[String],
    ) -> Result<Self, PosixError> {
        let mut applied = Self::none(env);
        for redirect in redirects {
            applied.apply_one(redirect, positional)?;
        }
        Ok(applied)
    }

    /// Keep the bindings past this command, as `exec` does.
    pub(crate) fn persist(mut self) {
        self.saved.clear();
    }

    fn bind(&mut self, fd: i32, descriptor: Option<Descriptor>) {
        let mut table = self.env.descriptors.write();
        let previous = match descriptor {
            Some(descriptor) => table.set(fd, descriptor),
            None => table.close(fd),
        };
        self.saved.push((fd, previous));
    }

    fn apply_one(
        &mut self,
        redirect: &IoRedirect,
        positional: &[String],
    ) -> Result<(), PosixError> {
        match redirect {
            IoRedirect::File(fd, kind, target) => {
                let fd = fd.unwrap_or(match kind {
                    IoFileRedirectKind::Read
                    | IoFileRedirectKind::ReadAndWrite
                    | IoFileRedirectKind::DuplicateInput => 0,
                    _ => 1,
                });
                match target {
                    IoFileRedirectTarget::Filename(word) => {
                        let file = self.open(word, kind, positional)?;
                        self.bind(fd, Some(Descriptor::File(Arc::new(file))));
                    }
                    IoFileRedirectTarget::Fd(source) => self.duplicate(fd, *source)?,
                    IoFileRedirectTarget::Duplicate(word) => {
                        let text = expand_plain(word, self.env, positional)?;
                        if text == "-" {
                            self.bind(fd, None);
                        } else if let Some(source) = text.strip_suffix('-') {
                            // `n>&m-` moves m to n.
                            let source = parse_fd(source)?;
                            self.duplicate(fd, source)?;
                            self.bind(source, None);
                        } else {
                            self.duplicate(fd, parse_fd(&text)?)?;
                        }
                    }
                    IoFileRedirectTarget::ProcessSubstitution(kind, subshell) => {
                        // `> >(cmd)` sends the shell's output into the child,
                        // exactly as the `>(cmd)` word names that same pipe.
                        let shell_end = crate::eval::spawn_process_substitution(
                            kind, subshell, self.env, positional,
                        )?;
                        self.bind(fd, Some(Descriptor::File(Arc::new(shell_end))));
                    }
                }
            }
            IoRedirect::HereDocument(fd, here_doc) => {
                let mut body = here_doc.doc.value.clone();
                if here_doc.remove_tabs {
                    body = body
                        .split_inclusive('\n')
                        .map(|line| line.trim_start_matches('\t'))
                        .collect();
                }
                if here_doc.requires_expansion {
                    body = expand_here_document(&body, self.env, positional)?;
                }
                self.bind_here_document(fd.unwrap_or(0), body.as_bytes())?;
            }
            IoRedirect::HereString(fd, word) => {
                let mut text = expand_plain(word, self.env, positional)?;
                text.push('\n');
                self.bind_here_document(fd.unwrap_or(0), text.as_bytes())?;
            }
            IoRedirect::OutputAndError(word, append) => {
                let kind = if *append {
                    IoFileRedirectKind::Append
                } else {
                    IoFileRedirectKind::Write
                };
                let file = Arc::new(self.open(word, &kind, positional)?);
                self.bind(1, Some(Descriptor::File(file.clone())));
                self.bind(2, Some(Descriptor::File(file)));
            }
        }
        Ok(())
    }

    fn bind_here_document(&mut self, fd: i32, body: &[u8]) -> Result<(), PosixError> {
        let file = here_document(body).map_err(|error| {
            failure(format!(
                "cannot create here-document: {}",
                os_message(&error)
            ))
        })?;
        self.bind(fd, Some(Descriptor::File(Arc::new(file))));
        Ok(())
    }

    fn duplicate(&mut self, fd: i32, source: i32) -> Result<(), PosixError> {
        let descriptor = self
            .env
            .descriptor(source)
            .ok_or_else(|| failure(format!("{source}: Bad file descriptor")))?;
        self.bind(fd, Some(descriptor));
        Ok(())
    }

    fn open(
        &self,
        word: &Word,
        kind: &IoFileRedirectKind,
        positional: &[String],
    ) -> Result<std::fs::File, PosixError> {
        let name = expand_plain(word, self.env, positional)?;
        let path = self.env.resolve_path(&name);
        let mut options = std::fs::OpenOptions::new();
        match kind {
            IoFileRedirectKind::Read | IoFileRedirectKind::DuplicateInput => {
                options.read(true);
            }
            IoFileRedirectKind::ReadAndWrite => {
                options.read(true).write(true).create(true);
            }
            IoFileRedirectKind::Append => {
                options.append(true).create(true);
            }
            IoFileRedirectKind::Write
                if self.env.options.read().noclobber
                    && std::fs::metadata(&path).is_ok_and(|m| m.is_file()) =>
            {
                return Err(failure(format!("{name}: cannot overwrite existing file")));
            }
            IoFileRedirectKind::Write
            | IoFileRedirectKind::Clobber
            | IoFileRedirectKind::DuplicateOutput => {
                options.write(true).create(true).truncate(true);
            }
        }
        options
            .open(&path)
            .map_err(|error| failure(format!("{name}: {}", os_message(&error))))
    }
}

impl Drop for Redirected<'_> {
    fn drop(&mut self) {
        if self.saved.is_empty() {
            return;
        }
        let mut table = self.env.descriptors.write();
        for (fd, previous) in self.saved.drain(..).rev() {
            table.restore(fd, previous);
        }
    }
}

/// A redirection that could not be established: the command does not run and
/// its status is 1, as POSIX requires for a redirection error.
fn failure(message: impl Into<String>) -> PosixError {
    PosixError::Redirection(message.into())
}

fn parse_fd(text: &str) -> Result<i32, PosixError> {
    text.parse::<i32>()
        .ok()
        .filter(|fd| *fd >= 0)
        .ok_or_else(|| failure(format!("{text}: ambiguous redirect")))
}

/// The system's description of an I/O error, without Rust's "(os error N)".
pub(crate) fn os_message(error: &std::io::Error) -> String {
    let text = error.to_string();
    match text.rfind(" (os error ") {
        Some(index) => text[..index].to_string(),
        None => text,
    }
}

/// Expand a redirection word to exactly one field: no splitting, no globbing.
fn expand_plain(word: &Word, env: &Env, positional: &[String]) -> Result<String, PosixError> {
    Ok(expand_word(
        &word.value,
        env,
        &ExpansionConfig {
            do_glob: false,
            ..Default::default()
        },
        positional,
    )?
    .join(" "))
}

/// Expand a here-document body: parameters, command substitutions and
/// arithmetic, where a backslash quotes only `$`, `` ` ``, `\` and newline and a
/// double quote is an ordinary character.
fn expand_here_document(
    body: &str,
    env: &Env,
    positional: &[String],
) -> Result<String, EngineError> {
    let mut quoted = String::with_capacity(body.len() + 2);
    quoted.push('"');
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.peek() {
                Some('$' | '`' | '\\' | '\n') => {
                    quoted.push('\\');
                    quoted.extend(chars.next());
                }
                // Inside double quotes `\"` would drop the backslash; here the
                // backslash is literal, so it is kept as an escaped backslash.
                Some('"') => {
                    quoted.push_str("\\\\");
                }
                _ => quoted.push('\\'),
            },
            '"' => quoted.push_str("\\\""),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    Ok(expand_word(
        &quoted,
        env,
        &ExpansionConfig {
            do_glob: false,
            ..Default::default()
        },
        positional,
    )?
    .join(""))
}
