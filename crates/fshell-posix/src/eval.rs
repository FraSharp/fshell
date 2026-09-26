// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use async_recursion::async_recursion;
use brush_parser::ast::*;
use fshell_core::Val;
use fshell_engine::{EngineError, Env, Signal};

use crate::expand::{ExpansionConfig, expand_assignment_word, expand_word, expand_word_as_pattern};
use crate::parser::ParsedScript;

/// How the POSIX evaluator was invoked.
#[derive(Debug, Clone)]
pub struct EvalConfig {
    /// Positional parameters for $1, $2, ... / $@ / $#
    pub positional: Vec<String>,
    /// Whether this evaluation context honours errexit (POSIX: it does not while
    /// evaluating a condition). The *setting* is read live from the env, so
    /// `set -e` inside a script takes effect.
    pub errexit: bool,
    /// The original source text, used to slice the body of a background job so
    /// it can be re-run in a child process. Maybe empty for nested evals.
    pub source: String,
}

impl Default for EvalConfig {
    fn default() -> Self {
        Self {
            positional: Vec::new(),
            // Honoured by default. The *setting* lives in the shell options and is
            // read live, so this flag only ever suppresses errexit in the contexts
            // POSIX exempts (a condition, a `!` pipeline).
            errexit: true,
            source: String::new(),
        }
    }
}

/// Control-flow for POSIX evaluation.
#[derive(Debug)]
pub enum PosixExit {
    Code(i32),
    Return(i32),
    Break,
    Continue,
}

/// Evaluate a parsed POSIX script against env.
pub async fn eval_source(
    parsed: &ParsedScript,
    env: &Env,
    cfg: &EvalConfig,
) -> Result<i32, EngineError> {
    // Positional parameters are only scoped/restored when explicitly passed in cfg
    let has_explicit_positional = !cfg.positional.is_empty();
    let saved_positional = if has_explicit_positional {
        let s = save_positional(env);
        apply_positional(env, &cfg.positional);
        Some(s)
    } else {
        None
    };

    let result = {
        let run_cfg = EvalConfig {
            source: parsed.source.clone(),
            ..cfg.clone()
        };
        eval_program(&parsed.program, env, &run_cfg).await
    };

    if let Some(saved) = saved_positional {
        restore_positional(env, saved);
    }

    let code = match result {
        Ok(code) => {
            env.set_exit_code(code as i64);
            code
        }
        Err(PosixError::Exit(code)) => {
            env.set_exit_code(code as i64);
            code
        }
        Err(PosixError::Return(code)) => {
            env.set_exit_code(code as i64);
            code
        }
        Err(PosixError::Engine(e)) => {
            // A hard error still leaves the shell, so the EXIT handler still runs.
            run_exit_trap(env).await?;
            return Err(e);
        }
        Err(PosixError::Break) | Err(PosixError::Continue) => 0,
        Err(PosixError::Interrupted) => {
            env.set_exit_code(130);
            130
        }
    };

    // Evaluating a whole script is the point at which the shell it ran in is
    // leaving it, so this is where `trap … EXIT` fires — for every way out,
    // including `exit` and a hard error.
    run_exit_trap(env).await?;
    Ok(code)
}

/// Evaluate a parsed POSIX script and optionally capture its stdout bytes.
pub async fn eval_source_stream(
    parsed: &ParsedScript,
    env: &Env,
    cfg: &EvalConfig,
    capture_stdout: bool,
) -> Result<(i32, Option<Vec<u8>>), EngineError> {
    let has_explicit_positional = !cfg.positional.is_empty();
    let saved_positional = if has_explicit_positional {
        let s = save_positional(env);
        apply_positional(env, &cfg.positional);
        Some(s)
    } else {
        None
    };

    let mut captured = if capture_stdout {
        Some(Vec::new())
    } else {
        None
    };
    let mut last_code = 0;

    let run_cfg = EvalConfig {
        source: parsed.source.clone(),
        ..cfg.clone()
    };
    for complete in &parsed.program.complete_commands {
        if check_cancelled(env).is_err() {
            env.set_exit_code(130);
            return Err(EngineError::Interrupted { span: None });
        }
        match eval_compound_list_stream(
            complete,
            env,
            &run_cfg,
            IoStreamConfig {
                capture_stdout,
                ..Default::default()
            },
        )
        .await
        {
            Ok((code, out)) => {
                last_code = code;
                if code == 130 || env.pipeline_cancelled() {
                    let _ = check_cancelled(env);
                    env.set_exit_code(130);
                    return Err(EngineError::Interrupted { span: None });
                }
                if let (Some(acc), Some(bytes)) = (&mut captured, out) {
                    acc.extend_from_slice(&bytes);
                }
            }
            Err(PosixError::Exit(c)) | Err(PosixError::Return(c)) => {
                last_code = c;
                break;
            }
            Err(PosixError::Break) | Err(PosixError::Continue) => {
                break;
            }
            Err(PosixError::Interrupted) => {
                env.set_exit_code(130);
                return Err(EngineError::Interrupted { span: None });
            }
            Err(PosixError::Engine(err)) => return Err(err),
        }
    }

    if let Some(saved) = saved_positional {
        restore_positional(env, saved);
    }

    env.set_exit_code(last_code as i64);
    Ok((last_code, captured))
}

/// Evaluate a parsed POSIX script and capture its stdout bytes (used for command substitution).
pub async fn eval_source_capture(parsed: &ParsedScript, env: &Env) -> Result<Vec<u8>, EngineError> {
    let (_, out) = eval_source_stream(parsed, env, &EvalConfig::default(), true).await?;
    Ok(out.unwrap_or_default())
}

/// Run the `trap … EXIT` handler, if one is set.
///
/// POSIX runs it once, when the shell that set it leaves the script — including
/// when the script leaves through `exit`. It is cleared before it runs, so a
/// handler that itself exits cannot re-enter it.
pub async fn run_exit_trap(env: &Env) -> Result<(), EngineError> {
    let Some(handler) = env.posix_exit_trap.write().take() else {
        return Ok(());
    };
    let parsed = crate::parser::parse_posix_script(&handler)?;
    eval_source_stream(&parsed, env, &EvalConfig::default(), false)
        .await
        .map(|_| ())
}

pub(crate) enum PosixError {
    Engine(EngineError),
    Exit(i32),
    Return(i32),
    Break,
    Continue,
    Interrupted,
}

impl From<EngineError> for PosixError {
    fn from(e: EngineError) -> Self {
        PosixError::Engine(e)
    }
}

impl std::fmt::Display for PosixError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PosixError::Engine(e) => write!(f, "{e}"),
            PosixError::Exit(c) => write!(f, "exit {c}"),
            PosixError::Return(c) => write!(f, "return {c}"),
            PosixError::Break => write!(f, "break"),
            PosixError::Continue => write!(f, "continue"),
            PosixError::Interrupted => write!(f, "interrupted"),
        }
    }
}

pub(crate) fn check_cancelled(env: &Env) -> Result<(), PosixError> {
    if env.pipeline_cancelled() {
        env.job_control
            .sigint_pending
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Err(PosixError::Interrupted)
    } else {
        Ok(())
    }
}

fn save_positional(env: &Env) -> Vec<String> {
    if let Some(Val::List(items)) = env.vars.read().get("@") {
        return items.iter().map(|v| v.to_text()).collect();
    }
    Vec::new()
}

fn apply_positional(env: &Env, positional: &[String]) {
    if positional.is_empty() {
        return;
    }
    {
        let mut vars = env.vars.write();
        let list: Vec<Val> = positional.iter().map(|s| Val::String(s.clone())).collect();
        let n = list.len();
        vars.insert("@".to_string(), Val::List(list.clone()));
        vars.insert("#".to_string(), Val::Int(n as i64));
        for (i, v) in list.into_iter().enumerate() {
            vars.insert((i + 1).to_string(), v);
        }
    }
}

fn restore_positional(env: &Env, saved: Vec<String>) {
    {
        let mut vars = env.vars.write();
        if saved.is_empty() {
            vars.remove("@");
            vars.remove("#");
            for i in 1..=64 {
                if vars.contains_key(&i.to_string()) {
                    vars.remove(&i.to_string());
                } else {
                    break;
                }
            }
        } else {
            let list: Vec<Val> = saved.iter().map(|s| Val::String(s.clone())).collect();
            let n = list.len();
            vars.insert("@".to_string(), Val::List(list.clone()));
            vars.insert("#".to_string(), Val::Int(n as i64));
            for (i, v) in list.into_iter().enumerate() {
                vars.insert((i + 1).to_string(), v);
            }
        }
    }
}

/// Extract the exact source text of an and-or list so a background job can be
/// re-run verbatim in a child process.
fn item_fragment(list: &AndOrList, source: &str) -> String {
    if !source.is_empty()
        && let Some(span) = SourceLocation::location(list)
        && let Some(text) = slice_chars(source, span.start.index, span.end.index)
        && !text.trim().is_empty()
    {
        return text;
    }
    format!("{list}")
}

/// Slice `source` by character indices (brush positions are char-based).
fn slice_chars(source: &str, start: usize, end: usize) -> Option<String> {
    if start > end {
        return None;
    }
    Some(source.chars().skip(start).take(end - start).collect())
}

#[async_recursion]
async fn eval_program(program: &Program, env: &Env, cfg: &EvalConfig) -> Result<i32, PosixError> {
    let mut last_code = 0;
    for complete in &program.complete_commands {
        last_code = eval_compound_list(complete, env, cfg).await?;
    }
    Ok(last_code)
}

#[async_recursion]
async fn eval_compound_list(
    list: &CompoundList,
    env: &Env,
    cfg: &EvalConfig,
) -> Result<i32, PosixError> {
    let (code, _) = eval_compound_list_stream(list, env, cfg, IoStreamConfig::default()).await?;
    Ok(code)
}

#[async_recursion]
async fn eval_compound_list_stream(
    list: &CompoundList,
    env: &Env,
    cfg: &EvalConfig,
    mut io_cfg: IoStreamConfig,
) -> Result<(i32, Option<Vec<u8>>), PosixError> {
    let mut last = 0;
    let mut last_out = None;
    let mut accumulated = if io_cfg.capture_stdout {
        Some(Vec::new())
    } else {
        None
    };
    for (i, item) in list.0.iter().enumerate() {
        check_cancelled(env)?;
        // `cmd &` — run the and-or list in a child process and continue.
        if matches!(item.1, SeparatorOperator::Async) {
            let fragment = item_fragment(&item.0, &cfg.source);
            fshell_engine::background::spawn_background(&fragment, env)?;
            last = 0;
            continue;
        }
        let step_io = if i == 0 {
            IoStreamConfig {
                stdin_bytes: io_cfg.stdin_bytes.clone(),
                stdin_stream: io_cfg.stdin_stream.take(),
                stdout_stream: io_cfg.stdout_stream.clone(),
                capture_stdout: io_cfg.capture_stdout,
            }
        } else {
            IoStreamConfig {
                stdin_bytes: None,
                stdin_stream: None,
                stdout_stream: io_cfg.stdout_stream.clone(),
                capture_stdout: io_cfg.capture_stdout,
            }
        };
        let (code, out) = eval_and_or_list_stream(&item.0, env, cfg, step_io).await?;
        last = code;
        last_out = out.clone();
        if let (Some(acc), Some(bytes)) = (&mut accumulated, out) {
            acc.extend_from_slice(&bytes);
        }
        if code == 130 || env.pipeline_cancelled() {
            check_cancelled(env)?;
            return Err(PosixError::Interrupted);
        }
    }
    let ret_out = if io_cfg.capture_stdout {
        accumulated
    } else {
        last_out
    };
    Ok((last, ret_out))
}

#[async_recursion]
async fn eval_and_or_list_stream(
    list: &AndOrList,
    env: &Env,
    cfg: &EvalConfig,
    mut io_cfg: IoStreamConfig,
) -> Result<(i32, Option<Vec<u8>>), PosixError> {
    let first_io = IoStreamConfig {
        stdin_bytes: io_cfg.stdin_bytes.clone(),
        stdin_stream: io_cfg.stdin_stream.take(),
        stdout_stream: io_cfg.stdout_stream.clone(),
        capture_stdout: io_cfg.capture_stdout,
    };
    let (mut code, mut out) = eval_pipeline_stream(&list.first, env, cfg, first_io).await?;
    if code == 130 || env.pipeline_cancelled() {
        check_cancelled(env)?;
        return Err(PosixError::Interrupted);
    }
    // Which element the final status belongs to, so errexit can tell a failure the
    // shell must act on from one it is allowed to ignore.
    let mut evaluated_index = 0usize;
    let mut last_out = out.clone();
    let mut accumulated = if io_cfg.capture_stdout {
        let mut v = Vec::new();
        if let Some(b) = out.take() {
            v.extend_from_slice(&b);
        }
        Some(v)
    } else {
        None
    };

    for (index, and_or) in list.additional.iter().enumerate() {
        check_cancelled(env)?;
        let step_io = IoStreamConfig {
            stdin_bytes: None,
            stdin_stream: None,
            stdout_stream: io_cfg.stdout_stream.clone(),
            capture_stdout: io_cfg.capture_stdout,
        };
        match and_or {
            AndOr::And(next) => {
                if code == 0 {
                    let (c, next_out) = eval_pipeline_stream(next, env, cfg, step_io).await?;
                    evaluated_index = index + 1;
                    code = c;
                    last_out = next_out.clone();
                    if let (Some(acc), Some(b)) = (&mut accumulated, next_out) {
                        acc.extend_from_slice(&b);
                    }
                    if code == 130 || env.pipeline_cancelled() {
                        check_cancelled(env)?;
                        return Err(PosixError::Interrupted);
                    }
                }
            }
            AndOr::Or(next) => {
                if code != 0 {
                    let (c, next_out) = eval_pipeline_stream(next, env, cfg, step_io).await?;
                    evaluated_index = index + 1;
                    code = c;
                    last_out = next_out.clone();
                    if let (Some(acc), Some(b)) = (&mut accumulated, next_out) {
                        acc.extend_from_slice(&b);
                    }
                    if code == 130 || env.pipeline_cancelled() {
                        check_cancelled(env)?;
                        return Err(PosixError::Interrupted);
                    }
                }
            }
        }
    }
    // POSIX exempts every command of an and-or list except the last one, so the
    // shell leaves only on a failure the *final* element ran into: `false && true`
    // carries on, `true && false` does not. A `!`-negated pipeline is a question
    // rather than a command, so it never triggers errexit either. The setting is
    // read live, because `set -e` inside the script is what turns it on;
    // `cfg.errexit` says whether *this* context honours it at all.
    if cfg.errexit
        && env.options.read().errexit
        && code != 0
        && evaluated_index + 1 == list.additional.len() + 1
    {
        let negated = if evaluated_index == 0 {
            list.first.bang
        } else {
            match &list.additional[evaluated_index - 1] {
                AndOr::And(pipeline) | AndOr::Or(pipeline) => pipeline.bang,
            }
        };
        if !negated {
            env.set_exit_code(code as i64);
            return Err(PosixError::Exit(code));
        }
    }

    let ret_out = if io_cfg.capture_stdout {
        accumulated
    } else {
        last_out
    };
    Ok((code, ret_out))
}

/// Where a standard descriptor points once a command's redirections have been
/// applied in source order.
///
/// The order is the semantics, and it is why this is a table rather than a pair
/// of flags. `2>&1` means "stderr := whatever stdout points at *right now*", so
/// `> out 2>&1` sends both streams to `out` while `2>&1 > out` leaves stderr on
/// the original stdout. A `stderr_to_stdout: bool` cannot express that, because
/// by the time someone read it the ordering information would already be gone.
#[derive(Clone)]
pub(crate) enum FdTarget {
    /// A descriptor of this process (0, 1 or 2), materialised by duplicating it.
    /// Duplication is required rather than inheritance: `2>&1` on an
    /// unredirected stdout has to give the child fd 1, not fd 2.
    Process(i32),
    /// The in-process pipeline channel carrying this stage's output.
    Pipe(tokio::sync::mpsc::Sender<bytes::Bytes>),
    /// An open file, shared by every descriptor duplicated onto it so concurrent
    /// writers do not fight over separate offsets.
    File(std::sync::Arc<std::sync::Mutex<std::fs::File>>),
    /// Explicitly closed (`n>&-`).
    Closed,
}

impl std::fmt::Debug for FdTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FdTarget::Process(fd) => write!(f, "Process({fd})"),
            FdTarget::Pipe(_) => f.write_str("Pipe"),
            FdTarget::File(_) => f.write_str("File"),
            FdTarget::Closed => f.write_str("Closed"),
        }
    }
}

/// The result of applying a command's redirections, in order.
#[derive(Debug, Clone)]
pub struct RedirectionContext {
    pub stdin_bytes: Option<Vec<u8>>,
    pub stdin_file: Option<std::path::PathBuf>,
    /// Destinations of fd 0, 1 and 2 after every redirection was applied.
    targets: [FdTarget; 3],
}

impl Default for RedirectionContext {
    fn default() -> Self {
        Self::new(None)
    }
}

impl RedirectionContext {
    /// Seed the descriptor table for a command, then apply its redirections.
    ///
    /// Seeding matters: a stage whose stdout is the pipeline channel starts with
    /// fd 1 pointing at that channel, so a later `2>&1` correctly duplicates the
    /// channel rather than the process's own stdout.
    pub(crate) fn new(stdout_stream: Option<&tokio::sync::mpsc::Sender<bytes::Bytes>>) -> Self {
        Self {
            stdin_bytes: None,
            stdin_file: None,
            targets: [
                FdTarget::Process(0),
                match stdout_stream {
                    Some(tx) => FdTarget::Pipe(tx.clone()),
                    None => FdTarget::Process(1),
                },
                FdTarget::Process(2),
            ],
        }
    }

    /// Destination of fd 1.
    pub(crate) fn stdout(&self) -> &FdTarget {
        &self.targets[1]
    }

    /// Destination of fd 2.
    pub(crate) fn stderr(&self) -> &FdTarget {
        &self.targets[2]
    }

    /// Destination of fd 0.
    pub(crate) fn stdin(&self) -> &FdTarget {
        &self.targets[0]
    }

    /// The open file fd 1 was redirected to, if any.
    pub(crate) fn stdout_file(&self) -> Option<&std::sync::Arc<std::sync::Mutex<std::fs::File>>> {
        match &self.targets[1] {
            FdTarget::File(handle) => Some(handle),
            _ => None,
        }
    }

    pub(crate) fn apply_item(
        &mut self,
        redir: &IoRedirect,
        env: &Env,
        positional: &[String],
    ) -> Result<(), PosixError> {
        match redir {
            IoRedirect::File(fd_opt, kind, target) => {
                let default_fd = match kind {
                    IoFileRedirectKind::Read | IoFileRedirectKind::ReadAndWrite => 0,
                    _ => 1,
                };
                let fd = fd_opt.unwrap_or(default_fd);
                let append = matches!(kind, IoFileRedirectKind::Append);

                match target {
                    IoFileRedirectTarget::Filename(w) => {
                        let path = self.resolve_path(w, env, positional)?;
                        match fd {
                            0 => {
                                let bytes = std::fs::read(&path).map_err(|e| {
                                    PosixError::Engine(EngineError::IoError {
                                        message: format!("{}: {}", path.display(), e),
                                        span: None,
                                    })
                                })?;
                                self.stdin_bytes = Some(bytes);
                                self.stdin_file = Some(path);
                            }
                            1 | 2 => {
                                // Open the target *now*, before the command runs.
                                // POSIX establishes the redirection whether or not
                                // the command writes, so this is what makes
                                // `: > file` create or truncate it — the idiomatic
                                // way to clear a file or start a log.
                                let handle = open_redirect_file(&path, append)?;
                                self.targets[fd as usize] = FdTarget::File(handle);
                            }
                            _ => return Err(unsupported_descriptor(fd)),
                        }
                    }
                    IoFileRedirectTarget::Duplicate(w) => {
                        let dest =
                            expand_word(&w.value, env, &ExpansionConfig::default(), positional)?
                                .join(" ");
                        self.duplicate(fd, dest.trim())?;
                    }
                    IoFileRedirectTarget::Fd(target_fd) => {
                        self.duplicate(fd, &target_fd.to_string())?;
                    }
                    _ => {}
                }
            }
            IoRedirect::HereDocument(_fd, here_doc) => {
                let raw_body = &here_doc.doc.value;
                let mut out_lines = Vec::new();
                for line in raw_body.lines() {
                    let trimmed = if here_doc.remove_tabs {
                        line.trim_start_matches('\t')
                    } else {
                        line
                    };
                    if here_doc.requires_expansion {
                        let escaped = escape_quotes_for_heredoc(trimmed);
                        let expanded = expand_word(
                            &format!("\"{escaped}\""),
                            env,
                            &ExpansionConfig {
                                do_glob: false,
                                ..Default::default()
                            },
                            positional,
                        )?
                        .join("");
                        out_lines.push(expanded);
                    } else {
                        out_lines.push(trimmed.to_string());
                    }
                }
                let mut content = out_lines.join("\n");
                if raw_body.ends_with('\n') {
                    content.push('\n');
                }
                self.stdin_bytes = Some(content.into_bytes());
            }
            IoRedirect::HereString(_fd, w) => {
                let expanded = expand_word(
                    &w.value,
                    env,
                    &ExpansionConfig {
                        do_glob: false,
                        ..Default::default()
                    },
                    positional,
                )?
                .join(" ");
                let mut content = expanded;
                content.push('\n');
                self.stdin_bytes = Some(content.into_bytes());
            }
            IoRedirect::OutputAndError(w, append) => {
                let path = self.resolve_path(w, env, positional)?;
                let handle = open_redirect_file(&path, *append)?;
                self.targets[1] = FdTarget::File(handle.clone());
                self.targets[2] = FdTarget::File(handle);
            }
        }
        Ok(())
    }

    /// Point `src_fd` at whatever `dest` currently points at.
    fn duplicate(&mut self, src_fd: i32, dest: &str) -> Result<(), PosixError> {
        if dest == "-" {
            self.targets[src_fd as usize] = FdTarget::Closed;
            return Ok(());
        }
        let dest_fd: i32 = dest.parse().map_err(|_| {
            PosixError::Engine(EngineError::Generic {
                message: format!("{src_fd}>&{dest}: bad file descriptor"),
                span: None,
            })
        })?;
        if !(0..=2).contains(&dest_fd) {
            return Err(unsupported_descriptor(dest_fd));
        }
        // Copy the *current* target: this is exactly what makes redirection
        // order observable, and is the whole reason the table is ordered.
        self.targets[src_fd as usize] = self.targets[dest_fd as usize].clone();
        Ok(())
    }

    /// Expand a redirection target word to an absolute path.
    fn resolve_path(
        &self,
        word: &Word,
        env: &Env,
        positional: &[String],
    ) -> Result<std::path::PathBuf, PosixError> {
        let expanded = expand_word(
            &word.value,
            env,
            &ExpansionConfig {
                do_glob: false,
                ..Default::default()
            },
            positional,
        )?;
        let raw_path = std::path::PathBuf::from(expanded.join(" "));
        Ok(if raw_path.is_absolute() {
            raw_path
        } else {
            env.cwd().join(raw_path)
        })
    }
}

/// Open a redirection target for writing, truncating unless appending.
fn open_redirect_file(
    path: &std::path::Path,
    append: bool,
) -> Result<std::sync::Arc<std::sync::Mutex<std::fs::File>>, PosixError> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(!append)
        .append(append)
        .open(path)
        .map_err(|e| {
            PosixError::Engine(EngineError::IoError {
                message: format!("{}: {}", path.display(), e),
                span: None,
            })
        })?;
    Ok(std::sync::Arc::new(std::sync::Mutex::new(file)))
}

/// Only the three standard descriptors are implemented. Anything else is
/// rejected rather than silently dropped, because accepting a redirection and
/// ignoring it is worse than refusing it.
fn unsupported_descriptor(fd: i32) -> PosixError {
    PosixError::Engine(EngineError::Generic {
        message: format!("redirection of file descriptor {fd} is not supported (only 0, 1 and 2)"),
        span: None,
    })
}

/// Escape double quotes for wrapping an expanding heredoc line in `"..."`.
///
/// In POSIX heredoc (IEEE Std 1003.1-2017, section 2.7.4), backslash retains its
/// escape meaning only before `$`, `` ` ``, `\`, and newline. This precisely matches
/// double-quoted string escape rules, except that heredocs do not treat `"` as a
/// delimiter. We therefore only escape `"` (unless already escaped), preserving
/// literal `\$`, `\\`, etc.
fn escape_quotes_for_heredoc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.push('\\');
            if let Some(&next) = chars.peek() {
                out.push(next);
                chars.next();
            }
        } else if c == '"' {
            out.push('\\');
            out.push('"');
        } else {
            out.push(c);
        }
    }
    out
}

/// Write to a shared redirect handle.
fn write_shared_file(
    handle: &std::sync::Arc<std::sync::Mutex<std::fs::File>>,
    bytes: &[u8],
) -> Result<(), PosixError> {
    use std::io::Write;
    let mut file = handle
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    file.write_all(bytes).map_err(|e| {
        PosixError::Engine(EngineError::IoError {
            message: format!("redirect write error: {e}"),
            span: None,
        })
    })
}

/// Duplicate a process descriptor as a child's stdio.
///
/// `dup` rather than `Stdio::inherit()`: inheriting gives the child the
/// descriptor of the *same number*, which is wrong for a duplication such as
/// `2>&1`, where the child's fd 2 must become this process's fd 1.
fn dup_stdio(fd: i32) -> Result<std::process::Stdio, PosixError> {
    use std::os::fd::FromRawFd;
    let duplicated = unsafe { libc::dup(fd) };
    if duplicated < 0 {
        return Err(PosixError::Engine(EngineError::IoError {
            message: format!("dup({fd}) failed: {}", std::io::Error::last_os_error()),
            span: None,
        }));
    }
    // `Stdio::from` takes ownership, so the duplicated descriptor is closed when
    // the command drops it. The original descriptor is untouched.
    Ok(std::process::Stdio::from(unsafe {
        std::fs::File::from_raw_fd(duplicated)
    }))
}

/// Clone a shared redirect handle for handing to a child process. `try_clone`
/// duplicates the descriptor, so both fds share one file offset and interleave
/// in write order.
fn clone_shared_file(
    handle: &std::sync::Arc<std::sync::Mutex<std::fs::File>>,
) -> Result<std::fs::File, PosixError> {
    let file = handle
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    file.try_clone().map_err(|e| {
        PosixError::Engine(EngineError::IoError {
            message: format!("redirect dup error: {e}"),
            span: None,
        })
    })
}

#[derive(Debug, Default)]
pub struct IoStreamConfig {
    pub stdin_bytes: Option<Vec<u8>>,
    pub stdin_stream: Option<tokio::sync::mpsc::Receiver<bytes::Bytes>>,
    pub stdout_stream: Option<tokio::sync::mpsc::Sender<bytes::Bytes>>,
    pub capture_stdout: bool,
}

#[async_recursion]
async fn eval_pipeline_stream(
    pipeline: &Pipeline,
    env: &Env,
    cfg: &EvalConfig,
    mut io_cfg: IoStreamConfig,
) -> Result<(i32, Option<Vec<u8>>), PosixError> {
    let bang = pipeline.bang;

    if pipeline.seq.is_empty() {
        return Ok((0, None));
    }

    let mut last_code = 0;
    let mut final_out = None;

    if pipeline.seq.len() == 1 {
        let (code, out) = eval_command_stream(&pipeline.seq[0], env, cfg, io_cfg).await?;
        last_code = code;
        final_out = out;
    } else {
        let n = pipeline.seq.len();
        let mut senders: Vec<Option<tokio::sync::mpsc::Sender<bytes::Bytes>>> =
            Vec::with_capacity(n);
        let mut receivers: Vec<Option<tokio::sync::mpsc::Receiver<bytes::Bytes>>> =
            Vec::with_capacity(n);

        for _ in 0..n - 1 {
            let (tx, rx) = tokio::sync::mpsc::channel::<bytes::Bytes>(32);
            senders.push(Some(tx));
            receivers.push(Some(rx));
        }

        let mut handles = Vec::with_capacity(n);
        let mut prev_rx = io_cfg.stdin_stream.take();
        for (idx, cmd) in pipeline.seq.iter().enumerate() {
            let is_first = idx == 0;
            let is_last = idx == n - 1;
            let sub_env = crate::bridge::fork_env_for_subshell(env);
            let stage_rx = if is_first {
                prev_rx.take()
            } else {
                receivers[idx - 1].take()
            };
            let stage_tx = if is_last {
                io_cfg.stdout_stream.clone()
            } else {
                senders[idx].take()
            };
            let stage_io = IoStreamConfig {
                stdin_bytes: if is_first {
                    io_cfg.stdin_bytes.clone()
                } else {
                    None
                },
                stdin_stream: stage_rx,
                stdout_stream: stage_tx,
                capture_stdout: is_last && io_cfg.capture_stdout,
            };

            let cmd_clone = cmd.clone();
            let cfg_clone = cfg.clone();
            handles.push(tokio::spawn(async move {
                eval_command_stream(&cmd_clone, &sub_env, &cfg_clone, stage_io).await
            }));
        }

        drop(senders);
        drop(receivers);

        let mut pipefail_code = None;

        for (idx, handle) in handles.into_iter().enumerate() {
            let is_last = idx == n - 1;
            let (code, out) = handle.await.map_err(|e| {
                PosixError::Engine(EngineError::Generic {
                    message: format!("pipeline stage failed: {}", e),
                    span: None,
                })
            })??;
            if code != 0 && pipefail_code.is_none() {
                pipefail_code = Some(code);
            }
            if is_last {
                last_code = code;
                final_out = out;
            }
        }

        let pipefail = env.options.read().pipefail;
        if pipefail && let Some(c) = pipefail_code {
            last_code = c;
        }
    }

    if bang {
        last_code = if last_code == 0 { 1 } else { 0 };
    }

    env.set_exit_code(last_code as i64);
    Ok((last_code, final_out))
}

#[async_recursion]
async fn eval_command_stream(
    cmd: &Command,
    env: &Env,
    cfg: &EvalConfig,
    io_cfg: IoStreamConfig,
) -> Result<(i32, Option<Vec<u8>>), PosixError> {
    match cmd {
        Command::Simple(simple) => eval_simple_command(simple, env, cfg, io_cfg).await,
        Command::Compound(compound, redirects) => {
            let mut redir = RedirectionContext::new(io_cfg.stdout_stream.as_ref());
            if let Some(list) = redirects {
                for r in &list.0 {
                    match redir.apply_item(r, env, &cfg.positional) {
                        Ok(()) => {}
                        Err(error @ PosixError::Engine(EngineError::ParameterExpansion { .. })) => {
                            return Err(error);
                        }
                        Err(e) => {
                            eprintln!("{e}");
                            env.set_exit_code(1);
                            if cfg.errexit {
                                return Err(PosixError::Exit(1));
                            }
                            return Ok((1, None));
                        }
                    }
                }
            }
            // Take the redirect handle before moving the stdin bytes out of
            // `redir`, so the table stays borrowable afterwards.
            let stdout_handle = redir.stdout_file().cloned();
            let capture = io_cfg.capture_stdout || stdout_handle.is_some();
            let sub_io = IoStreamConfig {
                stdin_bytes: redir.stdin_bytes.or(io_cfg.stdin_bytes),
                stdin_stream: io_cfg.stdin_stream,
                stdout_stream: io_cfg.stdout_stream,
                capture_stdout: capture,
            };
            let (code, out) = eval_compound_command_stream(compound, env, cfg, sub_io).await?;
            if let Some(handle) = &stdout_handle
                && let Some(bytes) = &out
            {
                write_shared_file(handle, bytes)?;
            }
            let ret_out = if io_cfg.capture_stdout && stdout_handle.is_none() {
                out
            } else {
                None
            };
            Ok((code, ret_out))
        }
        Command::Function(func_def) => {
            let name = func_def.fname.value.clone();
            let body = func_def.body.0.clone();
            register_posix_function(env, &name, body);
            Ok((0, None))
        }
        Command::ExtendedTest(expr_cmd, _redirects) => {
            let result = eval_extended_test(&expr_cmd.expr, env)?;
            Ok((if result { 0 } else { 1 }, None))
        }
    }
}

fn eval_extended_test(expr: &ExtendedTestExpr, env: &Env) -> Result<bool, PosixError> {
    match expr {
        ExtendedTestExpr::And(a, b) => {
            Ok(eval_extended_test(a, env)? && eval_extended_test(b, env)?)
        }
        ExtendedTestExpr::Or(a, b) => {
            Ok(eval_extended_test(a, env)? || eval_extended_test(b, env)?)
        }
        ExtendedTestExpr::Not(inner) => Ok(!eval_extended_test(inner, env)?),
        ExtendedTestExpr::Parenthesized(inner) => eval_extended_test(inner, env),
        ExtendedTestExpr::UnaryTest(op, word) => {
            let val = expand_word(&word.value, env, &ExpansionConfig::default(), &[])?.join(" ");
            Ok(eval_unary_extended(op, &val, env))
        }
        ExtendedTestExpr::BinaryTest(op, left, right) => {
            let lv = expand_word(&left.value, env, &ExpansionConfig::default(), &[])?.join(" ");
            let rv = expand_word(&right.value, env, &ExpansionConfig::default(), &[])?.join(" ");
            Ok(eval_binary_extended(op, &lv, &rv, env)?)
        }
    }
}

fn eval_unary_extended(op: &brush_parser::ast::UnaryPredicate, val: &str, env: &Env) -> bool {
    let path = || env.resolve_path(val);
    match op {
        brush_parser::ast::UnaryPredicate::StringHasZeroLength => val.is_empty(),
        brush_parser::ast::UnaryPredicate::StringHasNonZeroLength => !val.is_empty(),
        brush_parser::ast::UnaryPredicate::FileExists => path().exists(),
        brush_parser::ast::UnaryPredicate::FileExistsAndIsRegularFile => path().is_file(),
        brush_parser::ast::UnaryPredicate::FileExistsAndIsDir => path().is_dir(),
        brush_parser::ast::UnaryPredicate::FileExistsAndIsReadable => {
            path_access(&path(), libc::R_OK)
        }
        brush_parser::ast::UnaryPredicate::FileExistsAndIsWritable => {
            path_access(&path(), libc::W_OK)
        }
        brush_parser::ast::UnaryPredicate::FileExistsAndIsExecutable => {
            #[cfg(unix)]
            {
                std::fs::metadata(path())
                    .map(|m| {
                        use std::os::unix::fs::PermissionsExt;
                        m.permissions().mode() & 0o111 != 0
                    })
                    .unwrap_or(false)
            }
            #[cfg(not(unix))]
            {
                false
            }
        }
        _ => false,
    }
}

fn eval_binary_extended(
    op: &brush_parser::ast::BinaryPredicate,
    left: &str,
    right: &str,
    env: &Env,
) -> Result<bool, PosixError> {
    match op {
        brush_parser::ast::BinaryPredicate::StringExactlyMatchesString
        | brush_parser::ast::BinaryPredicate::StringExactlyMatchesPattern => Ok(left == right),
        brush_parser::ast::BinaryPredicate::StringDoesNotExactlyMatchString
        | brush_parser::ast::BinaryPredicate::StringDoesNotExactlyMatchPattern => Ok(left != right),
        brush_parser::ast::BinaryPredicate::ArithmeticEqualTo => {
            Ok(parse_test_int_for_extended(left)? == parse_test_int_for_extended(right)?)
        }
        brush_parser::ast::BinaryPredicate::ArithmeticNotEqualTo => {
            Ok(parse_test_int_for_extended(left)? != parse_test_int_for_extended(right)?)
        }
        brush_parser::ast::BinaryPredicate::ArithmeticLessThan => {
            Ok(parse_test_int_for_extended(left)? < parse_test_int_for_extended(right)?)
        }
        brush_parser::ast::BinaryPredicate::ArithmeticLessThanOrEqualTo => {
            Ok(parse_test_int_for_extended(left)? <= parse_test_int_for_extended(right)?)
        }
        brush_parser::ast::BinaryPredicate::ArithmeticGreaterThan => {
            Ok(parse_test_int_for_extended(left)? > parse_test_int_for_extended(right)?)
        }
        brush_parser::ast::BinaryPredicate::ArithmeticGreaterThanOrEqualTo => {
            Ok(parse_test_int_for_extended(left)? >= parse_test_int_for_extended(right)?)
        }
        brush_parser::ast::BinaryPredicate::LeftSortsBeforeRight => Ok(left < right),
        brush_parser::ast::BinaryPredicate::LeftSortsAfterRight => Ok(left > right),
        brush_parser::ast::BinaryPredicate::LeftFileIsNewerOrExistsWhenRightDoesNot => {
            Ok(crate::posix_builtins::test_builtin::eval_file_binary(
                "-nt",
                &env.resolve_path(left),
                &env.resolve_path(right),
            ))
        }
        brush_parser::ast::BinaryPredicate::LeftFileIsOlderOrDoesNotExistWhenRightDoes => {
            Ok(crate::posix_builtins::test_builtin::eval_file_binary(
                "-ot",
                &env.resolve_path(left),
                &env.resolve_path(right),
            ))
        }
        brush_parser::ast::BinaryPredicate::FilesReferToSameDeviceAndInodeNumbers => {
            Ok(crate::posix_builtins::test_builtin::eval_file_binary(
                "-ef",
                &env.resolve_path(left),
                &env.resolve_path(right),
            ))
        }
        _ => Ok(false),
    }
}

fn parse_test_int_for_extended(s: &str) -> Result<i64, PosixError> {
    parse_test_int(s).map_err(|message| {
        PosixError::Engine(EngineError::Generic {
            message,
            span: None,
        })
    })
}

// Registry for POSIX functions scoped to Env (name -> compound command)
fn register_posix_function(env: &Env, name: &str, body: CompoundCommand) {
    let mut m = env.posix_fns.write();
    m.insert(name.to_string(), std::sync::Arc::new(body));
}

pub fn get_posix_function(env: &Env, name: &str) -> Option<CompoundCommand> {
    let m = env.posix_fns.read();
    m.get(name)
        .and_then(|arc| arc.downcast_ref::<CompoundCommand>().cloned())
}

#[async_recursion]
async fn eval_compound_command_stream(
    compound: &CompoundCommand,
    env: &Env,
    cfg: &EvalConfig,
    io_cfg: IoStreamConfig,
) -> Result<(i32, Option<Vec<u8>>), PosixError> {
    match compound {
        CompoundCommand::BraceGroup(brace) => {
            eval_compound_list_stream(&brace.list, env, cfg, io_cfg).await
        }
        CompoundCommand::Subshell(sub) => {
            // A subshell is a real child process: it gets its own PID and
            // signals, and its `exit`/assignments cannot touch the parent.
            let fragment = compound_source(&sub.list, &cfg.source);
            let (code, out) = run_child_shell(&fragment, io_cfg, env).await?;
            env.set_exit_code(code as i64);
            Ok((code, out))
        }
        CompoundCommand::IfClause(if_clause) => {
            // POSIX ignores `-e` while evaluating a condition: the clause is
            // *asking* whether the command succeeds, so its failure is an answer,
            // not a reason to leave.
            let cond_cfg = EvalConfig {
                errexit: false,
                ..cfg.clone()
            };
            let cond_code = eval_compound_list(&if_clause.condition, env, &cond_cfg).await?;
            if cond_code == 0 {
                eval_compound_list_stream(&if_clause.then, env, cfg, io_cfg).await
            } else if let Some(elses) = &if_clause.elses {
                for else_clause in elses {
                    if let Some(cond) = &else_clause.condition {
                        let c = eval_compound_list(cond, env, &cond_cfg).await?;
                        if c == 0 {
                            return eval_compound_list_stream(&else_clause.body, env, cfg, io_cfg)
                                .await;
                        }
                    } else {
                        return eval_compound_list_stream(&else_clause.body, env, cfg, io_cfg)
                            .await;
                    }
                }
                Ok((0, None))
            } else {
                Ok((0, None))
            }
        }
        CompoundCommand::ForClause(for_clause) => {
            let values: Vec<String> = if let Some(words) = &for_clause.values {
                let mut expanded = Vec::new();
                for w in words {
                    expanded.extend(expand_word(
                        &w.value,
                        env,
                        &ExpansionConfig {
                            do_glob: !env.options.read().noglob,
                            ..Default::default()
                        },
                        &cfg.positional,
                    )?);
                }
                expanded
            } else {
                cfg.positional.clone()
            };
            let mut last = 0;
            let mut accumulated = if io_cfg.capture_stdout {
                Some(Vec::new())
            } else {
                None
            };
            let mut prev_stream = io_cfg.stdin_stream;
            for (i, val) in values.into_iter().enumerate() {
                check_cancelled(env)?;
                {
                    let mut vars = env.vars.write();
                    vars.insert(for_clause.variable_name.clone(), Val::String(val));
                }
                let step_io = IoStreamConfig {
                    stdin_bytes: if i == 0 {
                        io_cfg.stdin_bytes.clone()
                    } else {
                        None
                    },
                    stdin_stream: if i == 0 { prev_stream.take() } else { None },
                    stdout_stream: io_cfg.stdout_stream.clone(),
                    capture_stdout: io_cfg.capture_stdout,
                };
                match eval_compound_list_stream(&for_clause.body.list, env, cfg, step_io).await {
                    Ok((code, out)) => {
                        last = code;
                        if code == 130 || env.pipeline_cancelled() {
                            check_cancelled(env)?;
                            return Err(PosixError::Interrupted);
                        }
                        if let (Some(acc), Some(bytes)) = (&mut accumulated, out) {
                            acc.extend_from_slice(&bytes);
                        }
                    }
                    Err(PosixError::Break) => break,
                    Err(PosixError::Continue) => {
                        check_cancelled(env)?;
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }
            Ok((last, accumulated))
        }
        CompoundCommand::WhileClause(while_clause) | CompoundCommand::UntilClause(while_clause) => {
            let is_until = matches!(compound, CompoundCommand::UntilClause(_));
            let mut last = 0;
            let mut current_stdin_bytes = io_cfg.stdin_bytes;
            let mut accumulated = if io_cfg.capture_stdout {
                Some(Vec::new())
            } else {
                None
            };
            loop {
                check_cancelled(env)?;
                // The condition is a question, so `-e` does not apply to it; the
                // body is a command list like any other.
                let cond_cfg = EvalConfig {
                    errexit: false,
                    ..cfg.clone()
                };
                let cond_io = IoStreamConfig {
                    stdin_bytes: current_stdin_bytes.clone(),
                    stdin_stream: None,
                    stdout_stream: io_cfg.stdout_stream.clone(),
                    capture_stdout: io_cfg.capture_stdout,
                };
                let (cond_code, cond_out) =
                    eval_compound_list_stream(&while_clause.0, env, &cond_cfg, cond_io).await?;
                if cond_code == 130 || env.pipeline_cancelled() {
                    check_cancelled(env)?;
                    return Err(PosixError::Interrupted);
                }
                if let Some(rem) = cond_out {
                    current_stdin_bytes = Some(rem);
                }
                let should_continue = if is_until {
                    cond_code != 0
                } else {
                    cond_code == 0
                };
                if !should_continue {
                    break;
                }
                let body_io = IoStreamConfig {
                    stdin_bytes: current_stdin_bytes.clone(),
                    stdin_stream: None,
                    stdout_stream: io_cfg.stdout_stream.clone(),
                    capture_stdout: io_cfg.capture_stdout,
                };
                match eval_compound_list_stream(&while_clause.1.list, env, cfg, body_io).await {
                    Ok((code, out)) => {
                        last = code;
                        if code == 130 || env.pipeline_cancelled() {
                            check_cancelled(env)?;
                            return Err(PosixError::Interrupted);
                        }
                        if let (Some(acc), Some(bytes)) = (&mut accumulated, out) {
                            acc.extend_from_slice(&bytes);
                        }
                    }
                    Err(PosixError::Break) => break,
                    Err(PosixError::Continue) => {
                        check_cancelled(env)?;
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }
            Ok((last, accumulated))
        }
        CompoundCommand::CaseClause(case_clause) => {
            let value_expanded = expand_word(
                &case_clause.value.value,
                env,
                &ExpansionConfig {
                    do_glob: false,
                    ..Default::default()
                },
                &cfg.positional,
            )?
            .join(" ");
            for item in &case_clause.cases {
                let mut matched = false;
                for pat in &item.patterns {
                    let pat_raw = &pat.value;
                    if pattern_matches(pat_raw, &value_expanded) {
                        matched = true;
                        break;
                    }
                    let expanded_pats = expand_word_as_pattern(
                        &pat.value,
                        env,
                        &ExpansionConfig {
                            do_glob: false,
                            ..Default::default()
                        },
                        &cfg.positional,
                    )?;
                    for pat_str in expanded_pats {
                        if pattern_matches(&pat_str, &value_expanded) {
                            matched = true;
                            break;
                        }
                    }
                    if matched {
                        break;
                    }
                }
                if matched {
                    if let Some(cmd_list) = &item.cmd {
                        return eval_compound_list_stream(cmd_list, env, cfg, io_cfg).await;
                    } else {
                        return Ok((0, None));
                    }
                }
            }
            Ok((0, None))
        }
        CompoundCommand::Arithmetic(arith) => {
            let expr = arith.expr.value.clone();
            let code = eval_arithmetic_command(&expr, env)?;
            Ok((if code != 0 { 0 } else { 1 }, None))
        }
        CompoundCommand::ArithmeticForClause(_arith_for) => Ok((0, None)),
        CompoundCommand::Coprocess(_) => Ok((0, None)),
    }
}

fn pattern_matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" || pattern == value {
        return true;
    }
    if pattern == "\\?" {
        return value == "?";
    }
    if pattern == "\\*" {
        return value == "*";
    }
    if pattern.starts_with('\\') && &pattern[1..] == value {
        return true;
    }
    match glob::Pattern::new(pattern) {
        Ok(p) => p.matches(value),
        Err(_) => pattern == value,
    }
}

fn eval_arithmetic_command(expr: &str, env: &Env) -> Result<i64, PosixError> {
    crate::arithmetic::eval_arithmetic_expr(expr, env)
        .map_err(crate::arithmetic::to_engine_error)
        .map_err(PosixError::Engine)
}

#[async_recursion]
async fn eval_simple_command(
    simple: &SimpleCommand,
    env: &Env,
    cfg: &EvalConfig,
    io_cfg: IoStreamConfig,
) -> Result<(i32, Option<Vec<u8>>), PosixError> {
    let positional = &cfg.positional;

    let mut prefix_assignments: Vec<(String, String)> = Vec::new();
    let mut redirects: Vec<&IoRedirect> = Vec::new();

    if let Some(prefix) = &simple.prefix {
        for item in &prefix.0 {
            match item {
                CommandPrefixOrSuffixItem::AssignmentWord(assign, _) => {
                    let name = match &assign.name {
                        AssignmentName::VariableName(n) => n.clone(),
                        AssignmentName::ArrayElementName(n, idx) => format!("{}[{}]", n, idx),
                    };
                    let value = match &assign.value {
                        AssignmentValue::Scalar(word) => {
                            expand_assignment_word(&word.value, env, positional)?
                        }
                        AssignmentValue::Array(elems) => elems
                            .iter()
                            .map(|(_, w)| w.value.clone())
                            .collect::<Vec<_>>()
                            .join(" "),
                    };
                    prefix_assignments.push((name, value));
                }
                CommandPrefixOrSuffixItem::IoRedirect(r) => {
                    redirects.push(r);
                }
                _ => {}
            }
        }
    }

    let cmd_word = simple.word_or_name.as_ref().map(|w| w.value.clone());

    let mut args: Vec<String> = Vec::new();
    if let Some(suffix) = &simple.suffix {
        for item in &suffix.0 {
            match item {
                CommandPrefixOrSuffixItem::Word(w) => {
                    let expanded = expand_word(
                        &w.value,
                        env,
                        &ExpansionConfig {
                            do_glob: !env.options.read().noglob,
                            ..Default::default()
                        },
                        positional,
                    )?;
                    args.extend(expanded);
                }
                // POSIX: `name=value` is an assignment only *before* the command
                // word. After it the word is an ordinary argument, so rebuild its
                // source text and expand it exactly like any other word — which is
                // what makes `make PREFIX=/usr`, `git -c foo=bar` and `argvdump n=1`
                // behave. Position is structural, not textual: brush records it by
                // placing the item in `prefix` or `suffix`, and this is the suffix
                // loop, so the information is already here and must not be folded
                // into the prefix assignment path.
                CommandPrefixOrSuffixItem::AssignmentWord(assign, _) => {
                    let name = match &assign.name {
                        AssignmentName::VariableName(n) => n.clone(),
                        AssignmentName::ArrayElementName(n, idx) => format!("{}[{}]", n, idx),
                    };
                    // `Word::value` is the raw, unexpanded source text, including
                    // any quoting, so the rebuilt word re-parses with its quotes.
                    let raw = match &assign.value {
                        AssignmentValue::Scalar(word) => format!("{name}={}", word.value),
                        // Arrays are outside the POSIX subset this engine promises;
                        // render the word literally rather than dropping the argument.
                        AssignmentValue::Array(elems) => {
                            let joined = elems
                                .iter()
                                .map(|(_, w)| w.value.as_str())
                                .collect::<Vec<_>>()
                                .join(" ");
                            format!("{name}=({joined})")
                        }
                    };
                    let expanded = expand_word(
                        &raw,
                        env,
                        &ExpansionConfig {
                            do_glob: !env.options.read().noglob,
                            ..Default::default()
                        },
                        positional,
                    )?;
                    args.extend(expanded);
                }
                CommandPrefixOrSuffixItem::IoRedirect(r) => {
                    redirects.push(r);
                }
                _ => {}
            }
        }
    }

    let mut redir = RedirectionContext::new(io_cfg.stdout_stream.as_ref());
    for r in &redirects {
        match redir.apply_item(r, env, positional) {
            Ok(()) => {}
            Err(error @ PosixError::Engine(EngineError::ParameterExpansion { .. })) => {
                return Err(error);
            }
            Err(e) => {
                eprintln!("{e}");
                env.set_exit_code(1);
                if cfg.errexit {
                    return Err(PosixError::Exit(1));
                }
                return Ok((1, None));
            }
        }
    }

    if cmd_word.is_none() {
        for (name, value) in prefix_assignments {
            env.set_shell_var(&name, Val::String(value));
        }
        return Ok((0, None));
    }

    let raw_cmd = match cmd_word {
        Some(w) => w,
        None => return Ok((0, None)),
    };
    let expanded_cmd_words = expand_word(
        &raw_cmd,
        env,
        &ExpansionConfig {
            do_glob: false,
            ..Default::default()
        },
        positional,
    )?;
    let (cmd_name, extra_args) = if !expanded_cmd_words.is_empty() {
        (
            expanded_cmd_words[0].clone(),
            expanded_cmd_words[1..].to_vec(),
        )
    } else {
        (raw_cmd, Vec::new())
    };
    let mut all_args = extra_args;
    all_args.extend(args);
    let args = all_args;

    let capture_stdout = io_cfg.capture_stdout || redir.stdout_file().is_some();

    let is_decl_cmd = matches!(
        cmd_name.as_str(),
        "export" | "readonly" | "declare" | "typeset" | "local"
    );

    let mut saved_prefix_vars: Vec<(String, Option<Val>)> = Vec::new();
    if !prefix_assignments.is_empty() {
        let mut vars = env.vars.write();
        for (name, value) in &prefix_assignments {
            let prev = vars.get(name).cloned();
            saved_prefix_vars.push((name.clone(), prev));
            vars.insert(name.clone(), Val::String(value.clone()));
            // A declaration builtin declares a shell variable; only a plain
            // command's prefix assignment becomes part of that command's
            // environment.
            if !is_decl_cmd && let Some(Val::Map(map)) = vars.get_mut("env") {
                map.insert(ustr::ustr(name), Val::String(value.clone()));
            }
        }
    }

    let res = eval_simple_command_inner(
        &cmd_name,
        &args,
        &prefix_assignments,
        &redir,
        capture_stdout,
        env,
        cfg,
        io_cfg,
    )
    .await;

    if is_decl_cmd {
        for (name, _) in prefix_assignments {
            let val = env.vars.read().get(name.as_str()).cloned();
            if let Some(val) = val {
                if cmd_name == "export" {
                    env.set_exported_var(name.as_str(), val);
                } else {
                    // `local`/`readonly`/`declare`/`typeset` declare a shell
                    // variable; they must not export it to the environment.
                    env.set_shell_var(name.as_str(), val);
                }
            }
        }
    } else if !saved_prefix_vars.is_empty() {
        let mut vars = env.vars.write();
        for (name, prev) in saved_prefix_vars {
            if let Some(p) = prev {
                vars.insert(name, p);
            } else {
                vars.remove(&name);
                if let Some(Val::Map(map)) = vars.get_mut("env") {
                    map.shift_remove(&ustr::ustr(&name));
                }
            }
        }
    }

    res
}

#[allow(clippy::too_many_arguments)]
async fn eval_simple_command_inner(
    cmd_name: &str,
    args: &[String],
    prefix_assignments: &[(String, String)],
    redir: &RedirectionContext,
    capture_stdout: bool,
    env: &Env,
    cfg: &EvalConfig,
    mut io_cfg: IoStreamConfig,
) -> Result<(i32, Option<Vec<u8>>), PosixError> {
    match cmd_name {
        ":" | "true" => return Ok((0, None)),
        "false" => return Ok((1, None)),
        "exit" => {
            if args.len() > 1 {
                eprintln!("exit: too many arguments");
                return Ok((1, None));
            }
            let code = match parse_status_argument(args.first(), env.exit_code()) {
                Ok(code) => code,
                Err(error) => {
                    eprintln!("exit: {error}");
                    return Err(PosixError::Exit(2));
                }
            };
            return Err(PosixError::Exit(code));
        }
        "return" => {
            if args.len() > 1 {
                eprintln!("return: too many arguments");
                return Ok((1, None));
            }
            let code = match parse_status_argument(args.first(), env.exit_code()) {
                Ok(code) => code,
                Err(error) => {
                    eprintln!("return: {error}");
                    return Err(PosixError::Return(2));
                }
            };
            return Err(PosixError::Return(code));
        }
        "local" => {
            // A declaration builtin: its operands name shell variables. A
            // function-local *scope* is not implemented yet, so a name declared
            // this way outlives the call — the `compose/local-does-not-leak` case
            // pins that gap.
            let mut vars = env.vars.write();
            for arg in args {
                match arg.split_once('=') {
                    Some((name, value)) => {
                        vars.insert(name.to_string(), Val::String(value.to_string()));
                    }
                    None => {
                        vars.entry(arg.to_string()).or_insert(Val::Null);
                    }
                }
            }
            return Ok((0, None));
        }
        "break" => return Err(PosixError::Break),
        "continue" => return Err(PosixError::Continue),
        "shift" => {
            if args.len() > 1 {
                eprintln!("shift: too many arguments");
                return Ok((2, None));
            }
            let n = match args.first() {
                None => 1,
                Some(value) => match value.parse::<usize>() {
                    Ok(n) => n,
                    Err(error) => {
                        eprintln!("shift: invalid count {:?}: {}", value, error);
                        return Ok((2, None));
                    }
                },
            };
            match crate::posix_builtins::shift::shift_posix(env, n) {
                Ok(()) => return Ok((0, None)),
                Err(error) => {
                    eprintln!("{error}");
                    return Ok((1, None));
                }
            }
        }
        "set" => {
            let rendered = crate::posix_builtins::shift::set_posix(env, args).map_err(|e| {
                PosixError::Engine(EngineError::Generic {
                    message: e,
                    span: None,
                })
            })?;
            // `set -o` prints the options, and that output is the command's stdout:
            // it goes through the redirection machinery like any other builtin's.
            let out = match &rendered {
                Some(text) => write_builtin_output(text, redir, capture_stdout).await?,
                None => None,
            };
            return Ok((0, out));
        }
        "unset" => {
            let mut unset_fns_only = false;
            let mut unset_vars_only = false;
            let mut names: Vec<String> = Vec::new();
            for a in args {
                match a.as_str() {
                    "-f" => {
                        unset_fns_only = true;
                        unset_vars_only = false;
                    }
                    "-v" => {
                        unset_fns_only = false;
                        unset_vars_only = true;
                    }
                    s if s.starts_with('-') => {
                        // Unknown flag like -n, treat as no-op for compatibility
                    }
                    _ => names.push(a.clone()),
                }
            }
            for name in names {
                if unset_fns_only {
                    env.posix_fns.write().remove(&name);
                    {
                        let mut fns = env.fns.write();
                        fns.remove(&name);
                    }
                } else if unset_vars_only {
                    env.unset_var(&name);
                } else {
                    // Default: unset both var and function (bash parity)
                    env.unset_var(&name);
                    env.posix_fns.write().remove(&name);
                    {
                        let mut fns = env.fns.write();
                        fns.remove(&name);
                    }
                }
            }
            return Ok((0, None));
        }
        "export" => {
            let mut exports = Vec::new();
            for (name, val) in prefix_assignments {
                exports.push(format!("{}={}", name, val));
            }
            exports.extend(args.iter().cloned());
            if exports.is_empty() {
                let mut rendered = String::new();
                {
                    let vars = env.vars.read();
                    for (k, v) in vars.iter() {
                        rendered.push_str(&format!("export {}={:?}\n", k, v.to_text()));
                    }
                }
                let out = write_builtin_output(&rendered, redir, io_cfg.capture_stdout).await?;
                return Ok((0, out));
            }
            for arg in &exports {
                if let Some((name, value)) = arg.split_once('=') {
                    let expanded_val =
                        expand_word(value, env, &ExpansionConfig::default(), &cfg.positional)?
                            .join(" ");
                    env.set_exported_var(name, Val::String(expanded_val));
                } else {
                    // export VAR without value: promote existing shell var
                    env.export_existing_var(arg);
                }
            }
            return Ok((0, None));
        }
        "read" => {
            let stdin_bytes_val = if let Some(b) = redir
                .stdin_bytes
                .as_deref()
                .or(io_cfg.stdin_bytes.as_deref())
            {
                Some(b.to_vec())
            } else if let Some(mut rx) = io_cfg.stdin_stream.take() {
                rx.recv().await.map(|chunk| chunk.to_vec())
            } else {
                None
            };
            let stdin_str = stdin_bytes_val
                .as_deref()
                .map(|b| String::from_utf8_lossy(b).into_owned());
            let (code, remaining_stdin) =
                crate::posix_builtins::read_cmd::read_posix(args, env, stdin_str.as_deref())
                    .map_err(|e| {
                        PosixError::Engine(EngineError::Generic {
                            message: e,
                            span: None,
                        })
                    })?;
            return Ok((code, remaining_stdin));
        }
        "printf" => {
            let result = crate::posix_builtins::printf::format_printf_with_status(
                args.first().map(|s| s.as_str()).unwrap_or(""),
                if args.len() > 1 { &args[1..] } else { &[] },
            )
            .map_err(|e| {
                PosixError::Engine(EngineError::Generic {
                    message: e,
                    span: None,
                })
            })?;
            for diagnostic in &result.diagnostics {
                eprintln!("{diagnostic}");
            }
            let out = write_builtin_output(&result.output, redir, io_cfg.capture_stdout).await?;
            return Ok((result.status, out));
        }
        "getopts" => {
            let code = crate::posix_builtins::getopts::getopts_posix(args, env).map_err(|e| {
                PosixError::Engine(EngineError::Generic {
                    message: e,
                    span: None,
                })
            })?;
            return Ok((code, None));
        }
        "type" => {
            let (code, rendered) =
                crate::posix_builtins::type_cmd::type_posix(args, env).map_err(|e| {
                    PosixError::Engine(EngineError::Generic {
                        message: e,
                        span: None,
                    })
                })?;
            let out = write_builtin_output(&rendered, redir, io_cfg.capture_stdout).await?;
            return Ok((code, out));
        }
        "eval" => {
            let code = crate::posix_builtins::eval_builtin::eval_posix(args, env)
                .await
                .map_err(PosixError::Engine)?;
            return Ok((code, None));
        }
        "test" | "[" => {
            let test_args: &[String] = if cmd_name == "[" {
                if args.last().map(|s| s.as_str()) == Some("]") {
                    &args[..args.len().saturating_sub(1)]
                } else {
                    args
                }
            } else {
                args
            };
            let code = match eval_test_args(test_args, env) {
                Ok(true) => 0,
                Ok(false) => 1,
                Err(error) => {
                    eprintln!("test: {error}");
                    2
                }
            };
            return Ok((code, None));
        }
        "echo" => {
            let mut no_newline = false;
            let mut start = 0;
            if args.first().map(|s| s.as_str()) == Some("-n") {
                no_newline = true;
                start = 1;
            }
            let mut text = args[start..].join(" ");
            if !no_newline {
                text.push('\n');
            }
            let out = write_builtin_output(&text, redir, io_cfg.capture_stdout).await?;
            return Ok((0, out));
        }
        "cd" => {
            let target = args.first().map(|s| s.as_str()).unwrap_or("");
            let path = if target.is_empty() {
                env.home_dir().to_string_lossy().into_owned()
            } else if target == "-" {
                let vars = env.vars.read();
                vars.get("OLDPWD")
                    .map(|v| v.to_text())
                    .unwrap_or_else(|| "/".to_string())
            } else {
                target.to_string()
            };
            let target_path = std::path::PathBuf::from(path);
            let prev_cwd = env.cwd();
            let resolved = if target_path.is_absolute() {
                target_path
            } else {
                prev_cwd.join(&target_path)
            };
            if let Ok(canon) = resolved.canonicalize()
                && canon.is_dir()
            {
                env.set_cwd(canon.clone());
                {
                    let mut vars = env.vars.write();
                    vars.insert(
                        "OLDPWD".to_string(),
                        Val::String(prev_cwd.to_string_lossy().to_string()),
                    );
                    vars.insert(
                        "PWD".to_string(),
                        Val::String(canon.to_string_lossy().to_string()),
                    );
                }
                return Ok((0, None));
            } else {
                eprintln!("{}: cd: {}: No such file or directory", cmd_name, target);
                return Ok((1, None));
            }
        }
        "pwd" => {
            let text = format!("{}\n", env.cwd().display());
            let out = write_builtin_output(&text, redir, io_cfg.capture_stdout).await?;
            return Ok((0, out));
        }
        "exec" => {
            if args.is_empty() {
                return Ok((0, None));
            }
            let mut cmd = std::process::Command::new(&args[0]);
            cmd.current_dir(env.cwd());
            if args.len() > 1 {
                cmd.args(&args[1..]);
            }
            let status = cmd.status().map(|s| s.code().unwrap_or(127)).unwrap_or(127);
            return Err(PosixError::Exit(status));
        }
        "trap" => {
            let rendered = handle_posix_trap(args, env)?;
            let out = write_builtin_output(&rendered, redir, capture_stdout).await?;
            return Ok((0, out));
        }
        "wait" => {
            let code = handle_posix_wait(args, env).await?;
            return Ok((code, None));
        }
        "umask" => {
            match args.first() {
                None => {
                    // Reading the mask requires a set-then-restore round trip.
                    let current = unsafe { libc::umask(0o022 as libc::mode_t) };
                    unsafe { libc::umask(current) };
                    let text = format!("{:04o}\n", current);
                    let out = write_builtin_output(&text, redir, io_cfg.capture_stdout).await?;
                    return Ok((0, out));
                }
                Some(mask_arg) => {
                    let parsed = i32::from_str_radix(mask_arg.trim_start_matches('0'), 8)
                        .or_else(|_| i32::from_str_radix(mask_arg, 8));
                    match parsed {
                        Ok(mask) => {
                            unsafe { libc::umask(mask as libc::mode_t) };
                            return Ok((0, None));
                        }
                        Err(_) => {
                            eprintln!("umask: invalid mask: {mask_arg}");
                            return Ok((1, None));
                        }
                    }
                }
            }
        }
        "alias" => {
            // `alias` / `alias -p` lists all; `alias name` prints one;
            // `alias name=value` defines one. A `name=value` argument is parsed
            // as a prefix assignment, so pull definitions from there too.
            for (name, value) in prefix_assignments {
                env.register_alias(name, value);
            }
            let defines = !prefix_assignments.is_empty() || args.iter().any(|a| a.contains('='));
            let list_all = !defines && (args.is_empty() || args.iter().all(|a| a == "-p"));
            if list_all {
                let mut out = String::new();
                for (name, expansion) in env.get_all_aliases() {
                    out.push_str(&format!("alias {name}='{expansion}'\n"));
                }
                let out = write_builtin_output(&out, redir, io_cfg.capture_stdout).await?;
                return Ok((0, out));
            }
            let mut listed = String::new();
            let mut status = 0;
            for arg in args {
                if let Some((name, value)) = arg.split_once('=') {
                    env.register_alias(name, value);
                } else if let Some(expansion) = env.get_alias(arg) {
                    listed.push_str(&format!("alias {arg}='{expansion}'\n"));
                } else {
                    eprintln!("alias: {arg}: not found");
                    status = 1;
                }
            }
            if listed.is_empty() {
                return Ok((status, None));
            }
            let out = write_builtin_output(&listed, redir, io_cfg.capture_stdout).await?;
            return Ok((status, out));
        }
        "unalias" => {
            if args.iter().any(|a| a == "-a") {
                for (name, _) in env.get_all_aliases() {
                    env.remove_alias(&name);
                }
                return Ok((0, None));
            }
            let mut status = 0;
            for arg in args {
                if env.remove_alias(arg).is_none() {
                    eprintln!("unalias: {arg}: not found");
                    status = 1;
                }
            }
            return Ok((status, None));
        }
        "ulimit" => {
            // Report or set a soft resource limit. -n files, -c core, -s stack,
            // -v address space, -f file size (default).
            let mut resource = libc::RLIMIT_FSIZE;
            let mut idx = 0;
            if let Some(flag) = args.first().and_then(|a| a.strip_prefix('-')) {
                idx = 1;
                match flag.chars().next() {
                    Some('n') => resource = libc::RLIMIT_NOFILE,
                    Some('c') => resource = libc::RLIMIT_CORE,
                    Some('s') => resource = libc::RLIMIT_STACK,
                    Some('v') => resource = libc::RLIMIT_AS,
                    Some('f') | None => {}
                    Some(other) => {
                        eprintln!("ulimit: unsupported option -{other}");
                        return Ok((1, None));
                    }
                }
            }
            let mut rl = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if unsafe { libc::getrlimit(resource, &mut rl) } != 0 {
                eprintln!("ulimit: getrlimit: {}", std::io::Error::last_os_error());
                return Ok((1, None));
            }
            if let Some(value) = args.get(idx) {
                rl.rlim_cur = if value == "unlimited" {
                    libc::RLIM_INFINITY
                } else {
                    match value.parse::<u64>() {
                        Ok(v) => v as libc::rlim_t,
                        Err(_) => {
                            eprintln!("ulimit: invalid value: {value}");
                            return Ok((1, None));
                        }
                    }
                };
                if unsafe { libc::setrlimit(resource, &rl) } != 0 {
                    eprintln!("ulimit: setrlimit: {}", std::io::Error::last_os_error());
                    return Ok((1, None));
                }
                return Ok((0, None));
            }
            let text = if rl.rlim_cur == libc::RLIM_INFINITY {
                "unlimited\n".to_string()
            } else if resource == libc::RLIMIT_FSIZE {
                // POSIX reports the file-size limit in 512-byte blocks.
                format!("{}\n", rl.rlim_cur / 512)
            } else {
                format!("{}\n", rl.rlim_cur)
            };
            let out = write_builtin_output(&text, redir, io_cfg.capture_stdout).await?;
            return Ok((0, out));
        }
        "times" => {
            return Ok((0, None));
        }
        "hash" => {
            // hash -r clears the command hash table (= PATH cache) so mutated
            // PATH is respected. venv activate/deactivate rely on this.
            if args.iter().any(|a| a == "-r") {
                fshell_engine::invalidate_path_cache();
            }
            return Ok((0, None));
        }
        "dot" | "." | "source" => {
            if let Some(path) = args.first() {
                let source_path = env.resolve_path(path);
                let content = std::fs::read_to_string(&source_path).map_err(|e| {
                    PosixError::Engine(EngineError::IoError {
                        message: format!("{}: {}: {}", cmd_name, path, e),
                        span: None,
                    })
                })?;
                let parsed =
                    crate::parser::parse_posix_script(&content).map_err(PosixError::Engine)?;
                let cfg2 = EvalConfig {
                    positional: args[1..].to_vec(),
                    ..Default::default()
                };
                let code = eval_program(&parsed.program, env, &cfg2).await?;
                return Ok((code, None));
            }
            return Ok((0, None));
        }
        _ => {}
    }

    // Check for POSIX shell function
    if let Some(func_body) = get_posix_function(env, cmd_name) {
        let saved = save_positional(env);
        apply_positional(env, args);
        let fn_cfg = EvalConfig {
            positional: args.to_vec(),
            errexit: cfg.errexit,
            source: cfg.source.clone(),
        };
        let result = eval_compound_command_stream(&func_body, env, &fn_cfg, io_cfg).await;
        restore_positional(env, saved);
        // `return` ends the function, not the script that called it: a caller sees
        // the status the function returned with and carries on.
        return match result {
            Ok((code, out)) => Ok((code, out)),
            Err(PosixError::Return(code)) => {
                env.set_exit_code(code as i64);
                Ok((code, None))
            }
            Err(other) => Err(other),
        };
    }

    // Job control is shared with the native engine: run `jobs`, `kill`, `fg`,
    // `bg` and `disown` through the registered builtin so both engines report
    // and signal the same job table.
    if matches!(cmd_name, "jobs" | "kill" | "fg" | "bg" | "disown")
        && let Some(handler) = env.get_builtin(cmd_name)
    {
        let argv: Vec<Val> = args.iter().map(|a| Val::String(a.clone())).collect();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<fshell_engine::PipelinePayload>(64);
        handler(None, argv, env, tx, None).map_err(|e| {
            PosixError::Engine(EngineError::Generic {
                message: e.to_string(),
                span: None,
            })
        })?;
        let mut captured = if io_cfg.capture_stdout {
            Some(Vec::new())
        } else {
            None
        };
        while let Some(payload) = rx.recv().await {
            if let fshell_engine::PipelinePayload::Data(val) = payload {
                let mut text = val.to_text();
                text.push('\n');
                if let Some(acc) = &mut captured {
                    acc.extend_from_slice(text.as_bytes());
                }
                if let Some(tx_out) = io_cfg.stdout_stream.as_ref() {
                    let _ = tx_out.send(bytes::Bytes::from(text)).await;
                } else if captured.is_none() {
                    print!("{text}");
                }
            }
        }
        return Ok((0, captured));
    }

    // Fallback: subprocess execution with full I/O piping and redirections
    run_external_command(cmd_name, args, prefix_assignments, redir, io_cfg, env).await
}

fn parse_status_argument(value: Option<&String>, default: i64) -> Result<i32, String> {
    let number = match value {
        Some(value) => value.parse::<i64>().map_err(|error| {
            format!("numeric argument required: {:?} ({})", value.trim(), error)
        })?,
        None => default,
    };
    Ok(number.rem_euclid(256) as i32)
}

/// Render one trap the way `trap` and `trap -p` print it, so the output can be
/// fed back to the shell.
fn render_trap(name: &str, handler: &str) -> String {
    if handler.is_empty() {
        format!("trap -- '' {name}\n")
    } else {
        format!("trap -- '{}' {name}\n", handler.replace('\'', "'\\''"))
    }
}

fn handle_posix_trap(args: &[String], env: &Env) -> Result<String, PosixError> {
    if args.is_empty() {
        let traps = env.posix_traps.read();
        let mut out = String::new();
        for (sig, handler) in traps.iter() {
            out.push_str(&render_trap(sig.to_str(), handler));
        }
        if let Some(handler) = env.posix_exit_trap.read().as_ref() {
            out.push_str(&render_trap("EXIT", handler));
        }
        return Ok(out);
    }
    if args.len() == 1 && args[0] == "-p" {
        let traps = env.posix_traps.read();
        let mut out = String::new();
        for (sig, handler) in traps.iter() {
            out.push_str(&render_trap(sig.to_str(), handler));
        }
        if let Some(handler) = env.posix_exit_trap.read().as_ref() {
            out.push_str(&render_trap("EXIT", handler));
        }
        return Ok(out);
    }
    // Handle `trap -- action sig...` form
    let (action, sig_args) = if args[0] == "--" {
        if args.len() < 2 {
            return Err(PosixError::Engine(EngineError::Generic {
                message: "trap: missing action after --".to_string(),
                span: None,
            }));
        }
        (&args[1], &args[2..])
    } else {
        (&args[0], &args[1..])
    };
    if sig_args.is_empty() {
        return Err(PosixError::Engine(EngineError::Generic {
            message: "trap: no signals specified".to_string(),
            span: None,
        }));
    }
    let signals: Vec<Signal> = sig_args
        .iter()
        .filter_map(|s| Signal::from_name(s))
        .collect();
    // `EXIT` is not a signal: it is the one handler that runs when the shell
    // leaves the script, so it is stored on its own and may accompany or replace
    // signal handlers.
    let wants_exit = sig_args.iter().any(|s| s == "EXIT" || s == "0");
    if wants_exit {
        let mut exit_trap = env.posix_exit_trap.write();
        *exit_trap = if action == "-" {
            None
        } else {
            Some(action.to_string())
        };
    }
    if signals.is_empty() && !wants_exit {
        return Err(PosixError::Engine(EngineError::Generic {
            message: "trap: no valid signals specified".to_string(),
            span: None,
        }));
    }
    let mut traps = env.posix_traps.write();
    for sig in signals {
        if action == "-" {
            traps.remove(&sig);
        } else {
            traps.insert(sig, action.clone());
        }
    }
    Ok(String::new())
}

async fn handle_posix_wait(args: &[String], env: &Env) -> Result<i32, PosixError> {
    use std::sync::atomic::Ordering;
    if args.is_empty() {
        loop {
            let count = env.background_count.load(Ordering::Relaxed);
            if count == 0 {
                // Also ensure jobs map has no non-disowned background pids
                let has_bg = {
                    let jobs = env.job_control.jobs.read();
                    jobs.values().any(|j| !j.disowned && j.pgid > 0)
                };
                if !has_bg {
                    break;
                }
            }
            env.background_notify.notified().await;
        }
        return Ok(0);
    }
    let mut last_code = 0;
    for arg in args {
        let trimmed = arg.trim_start_matches('%');
        // Try as job id first
        if let Ok(jid) = trimmed.parse::<usize>() {
            let pgid_opt = {
                let jobs = env.job_control.jobs.read();
                jobs.values()
                    .find(|j| j.id == jid && !j.disowned)
                    .map(|j| j.pgid)
            };
            if let Some(pgid) = pgid_opt {
                loop {
                    let still = {
                        let jobs = env.job_control.jobs.read();
                        jobs.get(&pgid).is_some()
                    };
                    if !still {
                        break;
                    }
                    env.background_notify.notified().await;
                }
                last_code = env.exit_code() as i32;
                continue;
            }
        }
        // Try as pid
        if let Ok(pid) = arg.parse::<i32>() {
            // If job exists, wait via jobs map
            let pgid_opt = {
                let jobs = env.job_control.jobs.read();
                if jobs.contains_key(&pid) {
                    Some(pid)
                } else {
                    jobs.values()
                        .find(|j| j.pgid == pid || j.pids.contains(&pid))
                        .map(|j| j.pgid)
                }
            };
            if let Some(pgid) = pgid_opt {
                loop {
                    let still = {
                        let jobs = env.job_control.jobs.read();
                        jobs.get(&pgid).is_some()
                    };
                    if !still {
                        break;
                    }
                    env.background_notify.notified().await;
                }
                last_code = env.exit_code() as i32;
            } else {
                // No job: the process may be an external child (already reaped
                // by the background reaper) or not ours. POSIX treats an
                // unknown pid as status 127 and keeps going — never fatal.
                let mut status = 0;
                let res = unsafe { libc::waitpid(pid, &mut status, 0) };
                if res <= 0 {
                    eprintln!("wait: pid {pid} is not a child of this shell");
                    last_code = 127;
                } else {
                    let code = if (status & 0x7f) == 0 {
                        (status >> 8) & 0xff
                    } else {
                        128 + (status & 0x7f)
                    };
                    last_code = code;
                }
            }
            continue;
        }
        return Err(PosixError::Engine(EngineError::Generic {
            message: format!("wait: {arg}: no such job"),
            span: None,
        }));
    }
    Ok(last_code)
}

async fn write_builtin_output(
    rendered: &str,
    redir: &RedirectionContext,
    capture_stdout: bool,
) -> Result<Option<Vec<u8>>, PosixError> {
    match redir.stdout() {
        // The target was opened when the redirection was applied, so writing must
        // go through that handle: re-opening by path here would truncate a file
        // that `>>` or a second redirect in the same command had already touched.
        FdTarget::File(handle) => {
            write_shared_file(handle, rendered.as_bytes())?;
            Ok(if capture_stdout {
                Some(Vec::new())
            } else {
                None
            })
        }
        FdTarget::Pipe(tx) => {
            let chunk = bytes::Bytes::copy_from_slice(rendered.as_bytes());
            let _ = tx.send(chunk).await;
            Ok(if capture_stdout {
                Some(rendered.as_bytes().to_vec())
            } else {
                None
            })
        }
        FdTarget::Closed => Ok(if capture_stdout {
            Some(Vec::new())
        } else {
            None
        }),
        FdTarget::Process(_) => {
            if capture_stdout {
                Ok(Some(rendered.as_bytes().to_vec()))
            } else {
                print!("{}", rendered);
                use std::io::Write;
                let _ = std::io::stdout().flush();
                Ok(None)
            }
        }
    }
}

/// Failure from a child process's stdout pump task.
enum StdoutPumpError {
    DownstreamClosed,
    Io(std::io::Error),
}

struct PosixForegroundGuard<'a> {
    env: &'a Env,
    job_id: Option<usize>,
    pid: Option<i32>,
    is_interactive: bool,
}

impl<'a> PosixForegroundGuard<'a> {
    fn new(env: &'a Env, pid: Option<i32>, cmd_name: &str, is_interactive: bool) -> Self {
        let job_id = if let Some(p) = pid {
            let mut jobs = env.job_control.jobs.write();
            let next_id = jobs.values().map(|j| j.id).max().unwrap_or(0) + 1;
            jobs.insert(
                p,
                fshell_engine::Job {
                    id: next_id,
                    pgid: p,
                    pids: vec![p],
                    last_stage_pid: Some(p),
                    last_stage_exit_code: None,
                    cmd: cmd_name.to_string(),
                    status: fshell_engine::JobStatus::Running,
                    disowned: false,
                    started_at: Some(std::time::Instant::now()),
                },
            );
            if is_interactive {
                let _ = env.set_foreground_job(Some(next_id));
            }
            Some(next_id)
        } else {
            None
        };
        Self {
            env,
            job_id,
            pid,
            is_interactive,
        }
    }
}

impl<'a> Drop for PosixForegroundGuard<'a> {
    fn drop(&mut self) {
        if let Some(jid) = self.job_id {
            let _ = self.env.clear_foreground(jid);
        }
        if let Some(p) = self.pid {
            let mut jobs = self.env.job_control.jobs.write();
            jobs.remove(&p);
        }
        if self.is_interactive {
            #[cfg(unix)]
            unsafe {
                libc::signal(libc::SIGTTOU, libc::SIG_IGN);
                libc::tcsetpgrp(libc::STDIN_FILENO, libc::getpgrp());
                libc::signal(libc::SIGTTOU, libc::SIG_DFL);
            }
        }
    }
}

async fn run_external_command(
    cmd_name: &str,
    args: &[String],
    prefix_assignments: &[(String, String)],
    redir: &RedirectionContext,
    mut io_cfg: IoStreamConfig,
    env: &Env,
) -> Result<(i32, Option<Vec<u8>>), PosixError> {
    use std::process::Stdio;

    let effective_stdin = redir
        .stdin_bytes
        .as_deref()
        .or(io_cfg.stdin_bytes.as_deref());

    let mut cmd = tokio::process::Command::new(cmd_name);
    cmd.args(args);
    cmd.current_dir(env.cwd());

    {
        let vars = env.vars.read();
        for (k, v) in vars.iter() {
            if k == "env" {
                if let Val::Map(m) = v {
                    for (ek, ev) in m.iter() {
                        cmd.env(ek.as_str(), ev.to_text());
                    }
                }
            } else if !k.starts_with(|c: char| c.is_ascii_digit())
                && k != "@"
                && k != "#"
                && k != "?"
            {
                cmd.env(k, v.to_text());
            }
        }
    }
    for (k, v) in prefix_assignments {
        cmd.env(k, v);
    }

    if let Some(stdin_file) = &redir.stdin_file {
        let f = std::fs::File::open(stdin_file).map_err(|e| {
            PosixError::Engine(EngineError::IoError {
                message: format!("{}: {}", stdin_file.display(), e),
                span: None,
            })
        })?;
        cmd.stdin(Stdio::from(f));
    } else if effective_stdin.is_some() || io_cfg.stdin_stream.is_some() {
        cmd.stdin(Stdio::piped());
    } else if matches!(redir.stdin(), FdTarget::Closed) {
        // `0<&-` (or `<&-`) closes stdin rather than leaving it inherited.
        cmd.stdin(Stdio::null());
    }

    // fd 1, taken from the ordered descriptor table. The table was built by
    // applying the redirections in source order, so whatever it says here already
    // accounts for things like `2>&1 > out` versus `> out 2>&1`.
    let capture_requested = io_cfg.capture_stdout && matches!(redir.stdout(), FdTarget::Process(1));
    match redir.stdout() {
        FdTarget::File(handle) => {
            cmd.stdout(Stdio::from(clone_shared_file(handle)?));
        }
        FdTarget::Pipe(_) => {
            cmd.stdout(Stdio::piped());
        }
        FdTarget::Closed => {
            cmd.stdout(Stdio::null());
        }
        FdTarget::Process(fd) => {
            if capture_requested {
                cmd.stdout(Stdio::piped());
            } else {
                cmd.stdout(dup_stdio(*fd)?);
            }
        }
    }

    // fd 2, likewise: if it was duplicated onto fd 1 then it must end up exactly
    // where fd 1 goes — the same file, or the same pipeline channel.
    let mut stderr_into_pipe: Option<tokio::sync::mpsc::Sender<bytes::Bytes>> = None;
    match redir.stderr() {
        FdTarget::File(handle) => {
            cmd.stderr(Stdio::from(clone_shared_file(handle)?));
        }
        FdTarget::Pipe(tx) => {
            cmd.stderr(Stdio::piped());
            stderr_into_pipe = Some(tx.clone());
        }
        FdTarget::Closed => {
            cmd.stderr(Stdio::null());
        }
        FdTarget::Process(fd) => {
            cmd.stderr(dup_stdio(*fd)?);
        }
    }

    let has_controlling_terminal = !fshell_engine::is_test_mode()
        && unsafe {
            !env.is_captured
                && libc::isatty(libc::STDIN_FILENO) == 1
                && fshell_engine::is_stdout_a_tty()
        };
    let is_interactive = has_controlling_terminal
        && !capture_requested
        && io_cfg.stdout_stream.is_none()
        && io_cfg.stdin_stream.is_none()
        && redir.stdin_file.is_none();

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.as_std_mut().process_group(0);
        unsafe {
            cmd.as_std_mut().pre_exec(move || {
                let mut set = std::mem::zeroed::<libc::sigset_t>();
                libc::sigemptyset(&mut set);
                libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
                libc::signal(libc::SIGCHLD, libc::SIG_DFL);
                libc::signal(libc::SIGPIPE, libc::SIG_DFL);
                if is_interactive {
                    libc::signal(libc::SIGTTOU, libc::SIG_IGN);
                    libc::tcsetpgrp(libc::STDIN_FILENO, libc::getpid());
                    libc::signal(libc::SIGTTOU, libc::SIG_DFL);
                }
                Ok(())
            });
        }
    }

    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // POSIX: a missing command is status 127, not a fatal error — the
            // surrounding script (e.g. a loop) must keep running.
            eprintln!("{cmd_name}: command not found");
            return Ok((127, None));
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!("{cmd_name}: permission denied");
            return Ok((126, None));
        }
        Err(e) => {
            return Err(PosixError::Engine(EngineError::IoError {
                message: format!("{}: {}", cmd_name, e),
                span: None,
            }));
        }
    };

    let child_pid = child.id().map(|id| id as i32);
    let _fg_guard = PosixForegroundGuard::new(env, child_pid, cmd_name, is_interactive);

    let stdin_bytes = effective_stdin.map(<[u8]>::to_vec);
    let (status, captured_stdout) = finish_child(
        child,
        cmd_name,
        &mut io_cfg,
        stdin_bytes,
        stderr_into_pipe,
        env,
        child_pid,
    )
    .await?;

    let code = match status.code() {
        Some(c) => c,
        None => {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                status.signal().map(|s| 128 + s).unwrap_or(127)
            }
            #[cfg(not(unix))]
            127
        }
    };
    env.set_exit_code(code as i64);

    if code == 130 || env.pipeline_cancelled() {
        env.job_control
            .sigint_pending
            .store(false, std::sync::atomic::Ordering::SeqCst);
        return Err(PosixError::Interrupted);
    }

    if capture_requested {
        Ok((code, captured_stdout))
    } else {
        Ok((code, None))
    }
}

/// Pump a spawned child's stdin/stdout according to the POSIX stream
/// configuration and wait for it to exit.
///
/// Shared by external commands and by `( … )` subshells so both honour stdin
/// bytes/streams and stdout streams/capture identically.
async fn finish_child(
    mut child: tokio::process::Child,
    label: &str,
    io_cfg: &mut IoStreamConfig,
    stdin_bytes: Option<Vec<u8>>,
    stderr_into_pipe: Option<tokio::sync::mpsc::Sender<bytes::Bytes>>,
    env: &Env,
    child_pgid: Option<i32>,
) -> Result<(std::process::ExitStatus, Option<Vec<u8>>), PosixError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let stdin_task = if let Some(bytes) = stdin_bytes
        && let Some(mut stdin) = child.stdin.take()
    {
        Some(tokio::spawn(async move { stdin.write_all(&bytes).await }))
    } else if let Some(mut rx) = io_cfg.stdin_stream.take()
        && let Some(mut stdin) = child.stdin.take()
    {
        Some(tokio::spawn(async move {
            while let Some(chunk) = rx.recv().await {
                stdin.write_all(&chunk).await?;
            }
            Ok(())
        }))
    } else {
        None
    };

    let mut stdout_pump = if let Some(tx) = io_cfg.stdout_stream.clone()
        && let Some(mut stdout) = child.stdout.take()
    {
        Some(tokio::spawn(async move {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = stdout.read(&mut buf).await.map_err(StdoutPumpError::Io)?;
                if n == 0 {
                    break;
                }
                let chunk = bytes::Bytes::copy_from_slice(&buf[..n]);
                if tx.send(chunk).await.is_err() {
                    return Err(StdoutPumpError::DownstreamClosed);
                }
            }
            Ok(Vec::new())
        }))
    } else {
        child.stdout.take().map(|mut stdout| {
            tokio::spawn(async move {
                let mut output = Vec::new();
                stdout
                    .read_to_end(&mut output)
                    .await
                    .map_err(StdoutPumpError::Io)?;
                Ok(output)
            })
        })
    };

    // When fd 2 was duplicated onto fd 1 this stage's stderr belongs in the same
    // pipeline channel as its stdout, so it is pumped into that channel too. The
    // pump has to be running before we wait, or a chatty child could block on a
    // full stderr pipe.
    let stderr_pump: Option<tokio::task::JoinHandle<Result<Vec<u8>, StdoutPumpError>>> =
        if let Some(tx) = stderr_into_pipe
            && let Some(mut stderr) = child.stderr.take()
        {
            Some(tokio::spawn(async move {
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    let n = stderr.read(&mut buf).await.map_err(StdoutPumpError::Io)?;
                    if n == 0 {
                        break;
                    }
                    let chunk = bytes::Bytes::copy_from_slice(&buf[..n]);
                    if tx.send(chunk).await.is_err() {
                        return Err(StdoutPumpError::DownstreamClosed);
                    }
                }
                Ok(Vec::new())
            }))
        } else {
            None
        };

    let cancel_poll = async {
        loop {
            if env.pipeline_cancelled() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    };

    let mut completed_stdout_pump = None;
    let status = if let Some(stdout_pump) = stdout_pump.as_mut() {
        tokio::select! {
            _ = cancel_poll => {
                if let Some(pgid) = child_pgid {
                    #[cfg(unix)]
                    unsafe { libc::kill(-pgid, libc::SIGINT); }
                } else {
                    let _ = child.start_kill();
                }
                let _ = child.wait().await;
                return Err(PosixError::Interrupted);
            }
            result = child.wait() => result,
            pump = stdout_pump => {
                let should_kill = !matches!(&pump, Ok(Ok(_)));
                if should_kill {
                    let _ = child.start_kill();
                }
                completed_stdout_pump = Some(pump);
                child.wait().await
            }
        }
    } else {
        tokio::select! {
            _ = cancel_poll => {
                if let Some(pgid) = child_pgid {
                    #[cfg(unix)]
                    unsafe { libc::kill(-pgid, libc::SIGINT); }
                } else {
                    let _ = child.start_kill();
                }
                let _ = child.wait().await;
                return Err(PosixError::Interrupted);
            }
            result = child.wait() => result,
        }
    }
    .map_err(|e| {
        PosixError::Engine(EngineError::IoError {
            message: format!("{}: {}", label, e),
            span: None,
        })
    })?;

    // The child has exited, so its stderr pipe is closed and this cannot block.
    // A closed downstream channel is not an error worth propagating.
    if let Some(pump) = stderr_pump {
        let _ = pump.await;
    }

    let captured_stdout = if let Some(pump) = completed_stdout_pump {
        match pump {
            Ok(Ok(output)) => Some(output),
            Ok(Err(StdoutPumpError::DownstreamClosed)) => None,
            Ok(Err(StdoutPumpError::Io(e))) => {
                return Err(PosixError::Engine(EngineError::IoError {
                    message: format!("{}: {}", label, e),
                    span: None,
                }));
            }
            Err(e) => {
                return Err(PosixError::Engine(EngineError::IoError {
                    message: format!("{} output task failed: {}", label, e),
                    span: None,
                }));
            }
        }
    } else if let Some(stdout_pump) = stdout_pump {
        match stdout_pump.await {
            Ok(Ok(output)) => Some(output),
            Ok(Err(StdoutPumpError::DownstreamClosed)) => None,
            Ok(Err(StdoutPumpError::Io(e))) => {
                return Err(PosixError::Engine(EngineError::IoError {
                    message: format!("{}: {}", label, e),
                    span: None,
                }));
            }
            Err(e) => {
                return Err(PosixError::Engine(EngineError::IoError {
                    message: format!("{} output task failed: {}", label, e),
                    span: None,
                }));
            }
        }
    } else {
        None
    };

    if let Some(stdin_task) = stdin_task {
        match stdin_task.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
            Ok(Err(e)) => {
                return Err(PosixError::Engine(EngineError::IoError {
                    message: format!("{} stdin: {}", label, e),
                    span: None,
                }));
            }
            Err(e) => {
                return Err(PosixError::Engine(EngineError::IoError {
                    message: format!("{} stdin task failed: {}", label, e),
                    span: None,
                }));
            }
        }
    }

    Ok((status, captured_stdout))
}

/// The source text of a compound list, so a subshell can be re-run as a script.
fn compound_source(list: &CompoundList, source: &str) -> String {
    if !source.is_empty()
        && let Some(span) = SourceLocation::location(list)
        && let Some(text) = slice_chars(source, span.start.index, span.end.index)
        && !text.trim().is_empty()
    {
        return text;
    }
    format!("{list}")
}

/// Evaluate `source` in a child `fsh` process (POSIX mode), as a subshell.
///
/// A real process gives the subshell its own PID, signals and `exit` without
/// disturbing the parent, matching POSIX.
async fn run_child_shell(
    source: &str,
    mut io_cfg: IoStreamConfig,
    env: &Env,
) -> Result<(i32, Option<Vec<u8>>), PosixError> {
    use std::process::Stdio;

    let launch = fshell_engine::background::prepare_child(source, env)?;

    let mut cmd = tokio::process::Command::new(&launch.program);
    cmd.args(&launch.args);
    cmd.current_dir(env.cwd());
    {
        let vars = env.vars.read();
        for (k, v) in vars.iter() {
            if k == "env" {
                if let Val::Map(m) = v {
                    for (ek, ev) in m.iter() {
                        cmd.env(ek.as_str(), ev.to_text());
                    }
                }
            } else if !k.starts_with(|c: char| c.is_ascii_digit())
                && k != "@"
                && k != "#"
                && k != "?"
            {
                cmd.env(k, v.to_text());
            }
        }
    }

    let want_stdin = io_cfg.stdin_bytes.is_some() || io_cfg.stdin_stream.is_some();
    if want_stdin {
        cmd.stdin(Stdio::piped());
    }
    if io_cfg.stdout_stream.is_some() || io_cfg.capture_stdout {
        cmd.stdout(Stdio::piped());
    }

    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            for file in &launch.temp_files {
                let _ = std::fs::remove_file(file);
            }
            return Err(PosixError::Engine(EngineError::IoError {
                message: format!("subshell: {e}"),
                span: None,
            }));
        }
    };

    let stdin_bytes = io_cfg.stdin_bytes.take();
    let child_pid = child.id().map(|id| id as i32);
    let result = finish_child(
        child,
        "subshell",
        &mut io_cfg,
        stdin_bytes,
        None,
        env,
        child_pid,
    )
    .await;
    for file in &launch.temp_files {
        let _ = std::fs::remove_file(file);
    }
    let (status, captured) = result?;
    let code = status.code().unwrap_or(127);
    if io_cfg.capture_stdout {
        Ok((code, captured))
    } else {
        Ok((code, None))
    }
}

fn eval_test_args(args: &[String], env: &Env) -> Result<bool, String> {
    let clean_args: &[String] = if let Some(last) = args.last()
        && last == "]"
    {
        &args[..args.len() - 1]
    } else {
        args
    };

    match clean_args.len() {
        0 => Ok(false),
        1 => Ok(!clean_args[0].is_empty()),
        2 => {
            if clean_args[0] == "!" {
                Ok(clean_args[1].is_empty())
            } else {
                Ok(eval_unary_primary(&clean_args[0], &clean_args[1], env))
            }
        }
        3 => {
            if is_binary_primary(&clean_args[1]) {
                eval_binary_primary(&clean_args[0], &clean_args[1], &clean_args[2], env)
            } else if clean_args[0] == "!" {
                Ok(!eval_test_args(&clean_args[1..], env)?)
            } else if clean_args[0] == "(" && clean_args[2] == ")" {
                Ok(!clean_args[1].is_empty())
            } else {
                Ok(false)
            }
        }
        4 => {
            if clean_args[0] == "!" {
                Ok(!eval_test_args(&clean_args[1..], env)?)
            } else if clean_args[0] == "(" && clean_args[3] == ")" {
                eval_test_args(&clean_args[1..3], env)
            } else {
                match brush_parser::test_command::parse(clean_args) {
                    Ok(expr) => crate::posix_builtins::test_builtin::eval_test_expr(&expr, env),
                    Err(error) => Err(format!("invalid test expression: {error}")),
                }
            }
        }
        _ => match brush_parser::test_command::parse(clean_args) {
            Ok(expr) => crate::posix_builtins::test_builtin::eval_test_expr(&expr, env),
            Err(error) => Err(format!("invalid test expression: {error}")),
        },
    }
}

fn is_binary_primary(op: &str) -> bool {
    matches!(
        op,
        "=" | "=="
            | "!="
            | "<"
            | ">"
            | "-eq"
            | "-ne"
            | "-lt"
            | "-le"
            | "-gt"
            | "-ge"
            | "-nt"
            | "-ot"
            | "-ef"
    )
}

fn path_access(path: &std::path::Path, mode: libc::c_int) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let c_path = match std::ffi::CString::new(path.as_os_str().as_bytes()) {
        Ok(p) => p,
        Err(_) => return false,
    };
    // SAFETY: c_path is a valid null-terminated string; access only queries permissions.
    unsafe { libc::access(c_path.as_ptr(), mode) == 0 }
}

fn eval_unary_primary(op: &str, val: &str, env: &Env) -> bool {
    let path = || env.resolve_path(val);
    match op {
        "-n" => !val.is_empty(),
        "-z" => val.is_empty(),
        "-e" | "-a" => path().exists(),
        "-f" => path().is_file(),
        "-d" => path().is_dir(),
        "-r" => path_access(&path(), libc::R_OK),
        "-w" => path_access(&path(), libc::W_OK),
        "-x" => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::metadata(path())
                    .map(|m| m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
            }
            #[cfg(not(unix))]
            {
                false
            }
        }
        "-s" => std::fs::metadata(path())
            .map(|m| m.len() > 0)
            .unwrap_or(false),
        "-h" | "-L" => path().is_symlink(),
        _ => false,
    }
}

fn eval_binary_primary(left: &str, op: &str, right: &str, env: &Env) -> Result<bool, String> {
    match op {
        "=" | "==" => Ok(left == right),
        "!=" => Ok(left != right),
        "<" => Ok(left < right),
        ">" => Ok(left > right),
        "-eq" => Ok(parse_test_int(left)? == parse_test_int(right)?),
        "-ne" => Ok(parse_test_int(left)? != parse_test_int(right)?),
        "-lt" => Ok(parse_test_int(left)? < parse_test_int(right)?),
        "-le" => Ok(parse_test_int(left)? <= parse_test_int(right)?),
        "-gt" => Ok(parse_test_int(left)? > parse_test_int(right)?),
        "-ge" => Ok(parse_test_int(left)? >= parse_test_int(right)?),
        "-nt" | "-ot" | "-ef" => Ok(crate::posix_builtins::test_builtin::eval_file_binary(
            op,
            &env.resolve_path(left),
            &env.resolve_path(right),
        )),
        _ => Ok(false),
    }
}

fn parse_test_int(s: &str) -> Result<i64, String> {
    s.trim()
        .parse::<i64>()
        .map_err(|error| format!("integer expression expected: {:?} ({})", s.trim(), error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_posix_while_loop_cancellation() {
        let env = Env::new();
        env.job_control
            .sigint_pending
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let parsed =
            crate::parse_posix_script("while true; do :; done").expect("parse should succeed");
        let result = eval_source_stream(&parsed, &env, &EvalConfig::default(), false).await;
        assert!(matches!(result, Err(EngineError::Interrupted { .. })));
        assert_eq!(env.exit_code(), 130);
    }
}
