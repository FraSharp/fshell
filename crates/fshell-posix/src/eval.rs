// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

// `async_recursion` marks each boxed-future function `#[must_use]`, and the
// returned future already is; the duplicate is generated, not written here.
#![allow(clippy::double_must_use)]

use async_recursion::async_recursion;
use brush_parser::ast::*;
use fshell_core::Val;
use fshell_engine::descriptors::{self, ChildDescriptors, Descriptor};
use fshell_engine::{EngineError, Env, Signal};

use crate::expand::{ExpansionConfig, expand_assignment_word, expand_word, expand_word_as_pattern};
use crate::parser::ParsedScript;
use crate::redirect::Redirected;

/// How the POSIX evaluator was invoked.
#[derive(Debug, Clone)]
pub struct EvalConfig {
    /// Positional parameters for $1, $2, ... / $@ / $#
    pub positional: Vec<String>,
    /// Whether this evaluation context honours errexit (POSIX: it does not while
    /// evaluating a condition). The *setting* is read live from the env, so
    /// `set -e` inside a script takes effect.
    pub errexit: bool,
}

impl Default for EvalConfig {
    fn default() -> Self {
        Self {
            positional: Vec::new(),
            // Honoured by default. The *setting* lives in the shell options and is
            // read live, so this flag only ever suppresses errexit in the contexts
            // POSIX exempts (a condition, a `!` pipeline).
            errexit: true,
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
    // An empty config inherits the caller's live arguments; a function call,
    // unlike this invocation API, always installs a scope even with no args.
    let _positional_scope =
        (!cfg.positional.is_empty()).then(|| crate::positional::Scope::new(env, &cfg.positional));

    let result = eval_program(&parsed.program, env, cfg).await;

    // EXIT handlers are still part of this script's argument scope. The guard
    // restores the caller only after termination handling finishes.
    match result {
        Ok(code) | Err(PosixError::Exit(code)) | Err(PosixError::Return(code)) => {
            env.set_exit_code(code as i64);
        }
        Err(PosixError::Engine(e)) => {
            // A hard error still leaves the shell, so the EXIT handler still runs
            // and observes failure rather than the last successful command.
            env.set_exit_code(1);
            run_exit_trap(env).await?;
            return Err(e);
        }
        Err(PosixError::Break) | Err(PosixError::Continue) => env.set_exit_code(0),
        Err(PosixError::Interrupted) => env.set_exit_code(130),
        Err(PosixError::Redirection(message)) => {
            report(env, &message).await;
            env.set_exit_code(1);
        }
    };

    // Evaluating a whole script is the point at which the shell it ran in is
    // leaving it, so this is where `trap … EXIT` fires — for every way out,
    // including `exit` and a hard error.
    run_exit_trap(env).await?;
    Ok(env.exit_code() as i32)
}

/// Evaluate a parsed POSIX script and optionally capture its stdout bytes.
///
/// Without capture the script runs against the caller's descriptor table, so
/// `exec 3>log` outlives it. With capture it runs as a substitution does: on a
/// private copy of the table whose fd 1 feeds the capture, drained concurrently
/// so output larger than the channel cannot deadlock, and kept even when the
/// script leaves through exit/return/break/continue.
pub async fn eval_source_stream(
    parsed: &ParsedScript,
    env: &Env,
    cfg: &EvalConfig,
    capture_stdout: bool,
) -> Result<(i32, Option<Vec<u8>>), EngineError> {
    if !capture_stdout {
        return eval_source_to_stream(parsed, env, cfg)
            .await
            .map(|code| (code, None));
    }
    let (read_end, write_end) = descriptors::pipe().map_err(|error| EngineError::IoError {
        message: format!("cannot capture output: {error}"),
        span: None,
    })?;
    let mut table = env.descriptors.read().clone();
    table.set(1, Descriptor::File(std::sync::Arc::new(write_end)));
    let capture_env = env.with_descriptors(table);
    let run = async move {
        // Dropping the environment closes the shell's write end; the drain
        // ends once every child holding a copy has closed it too.
        let result = eval_source_to_stream(parsed, &capture_env, cfg).await;
        drop(capture_env);
        result
    };
    let drain = descriptors::read_to_end(read_end);
    let (result, captured) = tokio::join!(run, drain);
    let captured = captured.map_err(|error| EngineError::IoError {
        message: format!("cannot capture output: {error}"),
        span: None,
    })?;
    result.map(|code| (code, Some(captured)))
}

async fn eval_source_to_stream(
    parsed: &ParsedScript,
    env: &Env,
    cfg: &EvalConfig,
) -> Result<i32, EngineError> {
    let _positional_scope =
        (!cfg.positional.is_empty()).then(|| crate::positional::Scope::new(env, &cfg.positional));
    let mut last_code = 0;

    for complete in &parsed.program.complete_commands {
        if check_cancelled(env).is_err() {
            env.set_exit_code(130);
            return Err(EngineError::Interrupted { span: None });
        }
        match eval_compound_list(complete, env, cfg).await {
            Ok(code) => {
                last_code = code;
                if code == 130 || env.pipeline_cancelled() {
                    let _ = check_cancelled(env);
                    env.set_exit_code(130);
                    return Err(EngineError::Interrupted { span: None });
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
            Err(PosixError::Redirection(message)) => {
                report(env, &message).await;
                last_code = 1;
                break;
            }
            Err(PosixError::Engine(err)) => return Err(err),
        }
    }

    env.set_exit_code(last_code as i64);
    Ok(last_code)
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
    let mut status = ExitTrapStatus {
        env,
        code: env.exit_code(),
    };
    let parsed = crate::parser::parse_posix_script(&handler).inspect_err(|_| {
        status.code = 1;
    })?;
    // Keep control transfers visible: a normally finishing handler preserves
    // the incoming status, but exit (including errexit) chooses a new one.
    match eval_program(&parsed.program, env, &EvalConfig::default()).await {
        Ok(_) | Err(PosixError::Return(_)) | Err(PosixError::Break) | Err(PosixError::Continue) => {
            Ok(())
        }
        Err(PosixError::Exit(code)) => {
            status.code = code as i64;
            Ok(())
        }
        Err(PosixError::Engine(error)) => {
            status.code = 1;
            Err(error)
        }
        Err(PosixError::Interrupted) => {
            status.code = 130;
            Err(EngineError::Interrupted { span: None })
        }
        Err(PosixError::Redirection(message)) => {
            report(env, &message).await;
            status.code = 1;
            Ok(())
        }
    }
}

struct ExitTrapStatus<'a> {
    env: &'a Env,
    code: i64,
}

impl Drop for ExitTrapStatus<'_> {
    fn drop(&mut self) {
        self.env.set_exit_code(self.code);
    }
}

#[derive(Debug)]
pub(crate) enum PosixError {
    Engine(EngineError),
    Exit(i32),
    Return(i32),
    Break,
    Continue,
    Interrupted,
    /// A redirection could not be established; the command's status is 1.
    Redirection(String),
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
            PosixError::Redirection(message) => f.write_str(message),
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
    let mut last = 0;
    for item in &list.0 {
        check_cancelled(env)?;
        // `cmd &` — run the and-or list in a child process and continue.
        if matches!(item.1, SeparatorOperator::Async) {
            let payload = crate::child::and_or_payload(&item.0, env, cfg)?;
            fshell_engine::background::spawn_background(&payload, &item.0.to_string(), env)?;
            last = 0;
            continue;
        }
        last = eval_and_or_list(&item.0, env, cfg).await?;
        if last == 130 || env.pipeline_cancelled() {
            check_cancelled(env)?;
            return Err(PosixError::Interrupted);
        }
    }
    Ok(last)
}

#[async_recursion]
async fn eval_and_or_list(
    list: &AndOrList,
    env: &Env,
    cfg: &EvalConfig,
) -> Result<i32, PosixError> {
    let mut code = eval_pipeline(&list.first, env, cfg).await?;
    if code == 130 || env.pipeline_cancelled() {
        check_cancelled(env)?;
        return Err(PosixError::Interrupted);
    }
    // Which element the final status belongs to, so errexit can tell a failure the
    // shell must act on from one it is allowed to ignore.
    let mut evaluated_index = 0usize;
    for (index, and_or) in list.additional.iter().enumerate() {
        check_cancelled(env)?;
        let (run, next) = match and_or {
            AndOr::And(next) => (code == 0, next),
            AndOr::Or(next) => (code != 0, next),
        };
        if run {
            code = eval_pipeline(next, env, cfg).await?;
            evaluated_index = index + 1;
            if code == 130 || env.pipeline_cancelled() {
                check_cancelled(env)?;
                return Err(PosixError::Interrupted);
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
        && evaluated_index == list.additional.len()
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
    Ok(code)
}

/// Write a diagnostic line to the environment's standard error, wherever it
/// currently points. A closed or broken stderr loses the message, as it would
/// in any shell.
pub(crate) async fn report(env: &Env, message: &str) {
    if let Some(stderr) = env.descriptor(2) {
        let mut line = message.to_string();
        line.push('\n');
        let _ = descriptors::write_all(&stderr, line.as_bytes()).await;
    }
}

/// Write a builtin's output to standard output. Failing to write is the
/// builtin's failure (status 1), reported on stderr like any shell does —
/// except a reader that went away: a process would die of SIGPIPE, so the
/// shell (or the subshell this runs in) leaves with that status, silently.
async fn emit(env: &Env, builtin: &str, text: &str) -> Result<i32, PosixError> {
    if text.is_empty() {
        return Ok(0);
    }
    let result = match env.descriptor(1) {
        Some(stdout) => descriptors::write_all(&stdout, text.as_bytes()).await,
        None => Err(std::io::Error::from_raw_os_error(libc::EBADF)),
    };
    match result {
        Ok(()) => Ok(0),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
            Err(PosixError::Exit(128 + libc::SIGPIPE))
        }
        Err(error) => {
            report(
                env,
                &format!(
                    "{builtin}: write error: {}",
                    crate::redirect::os_message(&error)
                ),
            )
            .await;
            Ok(1)
        }
    }
}

/// A redirection failed: the command does not run and its status is 1.
///
/// A redirection error on a special built-in is fatal to a non-interactive
/// shell (POSIX 2.8.1): `exec > /dev/tty` that cannot open the terminal must
/// not leave the script running with the redirection silently missing.
async fn redirection_failed(
    env: &Env,
    cfg: &EvalConfig,
    command: Option<&str>,
    error: PosixError,
) -> Result<i32, PosixError> {
    let PosixError::Redirection(message) = error else {
        return Err(error);
    };
    report(env, &message).await;
    env.set_exit_code(1);
    if !env.options.read().interactive
        && command.is_some_and(|name| {
            crate::posix_builtins::type_cmd::is_special_builtin(name)
                && get_posix_function(env, name).is_none()
        })
    {
        return Err(PosixError::Exit(1));
    }
    if cfg.errexit && env.options.read().errexit {
        return Err(PosixError::Exit(1));
    }
    Ok(1)
}

#[async_recursion]
async fn eval_pipeline(
    pipeline: &Pipeline,
    env: &Env,
    cfg: &EvalConfig,
) -> Result<i32, PosixError> {
    let mut code = match pipeline.seq.as_slice() {
        [] => 0,
        [single] => eval_command(single, env, cfg).await?,
        stages => {
            // Every stage is a subshell running concurrently; adjacent stages are
            // joined by a pipe, exactly as in any shell, so external stages
            // exchange bytes directly and builtins share the same ends.
            let mut handles = Vec::with_capacity(stages.len());
            let mut upstream = None;
            for (index, stage) in stages.iter().enumerate() {
                let stage_env = crate::bridge::fork_env_for_subshell(env);
                {
                    let mut table = stage_env.descriptors.write();
                    if let Some(read_end) = upstream.take() {
                        table.set(0, Descriptor::File(std::sync::Arc::new(read_end)));
                    }
                    if index + 1 < stages.len() {
                        let (read_end, write_end) = descriptors::pipe().map_err(|error| {
                            PosixError::Engine(EngineError::IoError {
                                message: format!("cannot create pipe: {error}"),
                                span: None,
                            })
                        })?;
                        table.set(1, Descriptor::File(std::sync::Arc::new(write_end)));
                        upstream = Some(read_end);
                    }
                }
                let stage = stage.clone();
                let stage_cfg = cfg.clone();
                handles.push(tokio::spawn(async move {
                    let result = eval_command(&stage, &stage_env, &stage_cfg).await;
                    subshell_status(&stage_env, result).await
                }));
            }
            let mut last = 0;
            let mut rightmost_failure = None;
            for handle in handles {
                let stage_code = handle.await.map_err(|e| {
                    PosixError::Engine(EngineError::Generic {
                        message: format!("pipeline stage failed: {e}"),
                        span: None,
                    })
                })??;
                if stage_code != 0 {
                    rightmost_failure = Some(stage_code);
                }
                last = stage_code;
            }
            match rightmost_failure {
                Some(failure) if env.options.read().pipefail => failure,
                _ => last,
            }
        }
    };
    if pipeline.bang {
        code = if code == 0 { 1 } else { 0 };
    }
    env.set_exit_code(code as i64);
    Ok(code)
}

/// The status a subshell leaves with. Its `exit`, `return`, `break` and
/// `continue` end only the subshell, and a hard error is reported and ends it
/// with status 1; an interrupt still propagates.
async fn subshell_status(env: &Env, result: Result<i32, PosixError>) -> Result<i32, PosixError> {
    match result {
        Ok(code) | Err(PosixError::Exit(code)) | Err(PosixError::Return(code)) => Ok(code),
        Err(PosixError::Break) | Err(PosixError::Continue) => Ok(0),
        Err(PosixError::Interrupted) => Err(PosixError::Interrupted),
        Err(PosixError::Redirection(message)) => {
            report(env, &message).await;
            Ok(1)
        }
        Err(PosixError::Engine(error)) => {
            report(env, &error.to_string()).await;
            Ok(1)
        }
    }
}

fn redirect_items(list: &Option<RedirectList>) -> Vec<&IoRedirect> {
    list.iter().flat_map(|l| l.0.iter()).collect()
}

#[async_recursion]
async fn eval_command(cmd: &Command, env: &Env, cfg: &EvalConfig) -> Result<i32, PosixError> {
    match cmd {
        Command::Simple(simple) => eval_simple_command(simple, env, cfg).await,
        Command::Compound(compound, redirects) => {
            let _redirected =
                match Redirected::apply(env, &redirect_items(redirects), &cfg.positional) {
                    Ok(applied) => applied,
                    Err(error) => return redirection_failed(env, cfg, None, error).await,
                };
            eval_compound_command(compound, env, cfg).await
        }
        Command::Function(func_def) => {
            register_posix_function(env, &func_def.fname.value, func_def.body.clone());
            Ok(0)
        }
        Command::ExtendedTest(expr_cmd, redirects) => {
            let _redirected =
                match Redirected::apply(env, &redirect_items(redirects), &cfg.positional) {
                    Ok(applied) => applied,
                    Err(error) => return redirection_failed(env, cfg, None, error).await,
                };
            let result = eval_extended_test(&expr_cmd.expr, env)?;
            Ok(if result { 0 } else { 1 })
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
fn register_posix_function(env: &Env, name: &str, body: FunctionBody) {
    let mut m = env.posix_fns.write();
    m.insert(name.to_string(), std::sync::Arc::new(body));
}

/// A function's body with the redirections written after it in the
/// definition, which apply on every call.
pub fn get_posix_function(env: &Env, name: &str) -> Option<FunctionBody> {
    let m = env.posix_fns.read();
    m.get(name)
        .and_then(|arc| arc.downcast_ref::<FunctionBody>().cloned())
}

#[async_recursion]
async fn eval_compound_command(
    compound: &CompoundCommand,
    env: &Env,
    cfg: &EvalConfig,
) -> Result<i32, PosixError> {
    // POSIX ignores `-e` while evaluating a condition: the clause is *asking*
    // whether the command succeeds, so its failure is an answer, not a reason
    // to leave.
    let cond_cfg = EvalConfig {
        errexit: false,
        ..cfg.clone()
    };
    match compound {
        CompoundCommand::BraceGroup(brace) => eval_compound_list(&brace.list, env, cfg).await,
        CompoundCommand::Subshell(sub) => {
            // A subshell is a real child process: it gets its own PID and
            // signals, and its `exit`/assignments cannot touch the parent.
            let payload = crate::child::list_payload(&sub.list, env, cfg)?;
            let code = run_child_shell(&payload, env).await?;
            env.set_exit_code(code as i64);
            Ok(code)
        }
        CompoundCommand::IfClause(if_clause) => {
            if eval_compound_list(&if_clause.condition, env, &cond_cfg).await? == 0 {
                return eval_compound_list(&if_clause.then, env, cfg).await;
            }
            for else_clause in if_clause.elses.iter().flatten() {
                match &else_clause.condition {
                    Some(cond) if eval_compound_list(cond, env, &cond_cfg).await? != 0 => {}
                    _ => return eval_compound_list(&else_clause.body, env, cfg).await,
                }
            }
            Ok(0)
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
                crate::expand::get_effective_positional(env, &cfg.positional)
            };
            let mut last = 0;
            for val in values {
                check_cancelled(env)?;
                env.set_shell_var(&for_clause.variable_name, Val::String(val));
                match eval_compound_list(&for_clause.body.list, env, cfg).await {
                    Ok(code) => {
                        last = code;
                        if code == 130 || env.pipeline_cancelled() {
                            check_cancelled(env)?;
                            return Err(PosixError::Interrupted);
                        }
                    }
                    Err(PosixError::Break) => break,
                    Err(PosixError::Continue) => continue,
                    Err(e) => return Err(e),
                }
            }
            Ok(last)
        }
        CompoundCommand::WhileClause(while_clause) | CompoundCommand::UntilClause(while_clause) => {
            let is_until = matches!(compound, CompoundCommand::UntilClause(_));
            let mut last = 0;
            loop {
                check_cancelled(env)?;
                let cond_code = eval_compound_list(&while_clause.0, env, &cond_cfg).await?;
                if cond_code == 130 || env.pipeline_cancelled() {
                    check_cancelled(env)?;
                    return Err(PosixError::Interrupted);
                }
                if (cond_code == 0) == is_until {
                    break;
                }
                match eval_compound_list(&while_clause.1.list, env, cfg).await {
                    Ok(code) => {
                        last = code;
                        if code == 130 || env.pipeline_cancelled() {
                            check_cancelled(env)?;
                            return Err(PosixError::Interrupted);
                        }
                    }
                    Err(PosixError::Break) => break,
                    Err(PosixError::Continue) => continue,
                    Err(e) => return Err(e),
                }
            }
            Ok(last)
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
                        return eval_compound_list(cmd_list, env, cfg).await;
                    } else {
                        return Ok(0);
                    }
                }
            }
            Ok(0)
        }
        CompoundCommand::Arithmetic(arith) => {
            let code = eval_arithmetic_command(&arith.expr.value, env)?;
            Ok(if code != 0 { 0 } else { 1 })
        }
        CompoundCommand::ArithmeticForClause(_) | CompoundCommand::Coprocess(_) => {
            report(
                env,
                "this bash construct is not supported by the POSIX engine",
            )
            .await;
            Ok(2)
        }
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

fn declaration_suffix_start(
    command_word: Option<&str>,
    suffix: Option<&CommandSuffix>,
    env: &Env,
) -> Option<usize> {
    let mut command_name = static_command_word(command_word?)?;
    let mut cursor = 0;
    let mut suppress_function_lookup = false;
    loop {
        if matches!(
            command_name,
            "export" | "readonly" | "declare" | "typeset" | "local"
        ) {
            if !suppress_function_lookup
                && !crate::posix_builtins::type_cmd::is_special_builtin(command_name)
                && get_posix_function(env, command_name).is_some()
            {
                return None;
            }
            return Some(cursor);
        }
        if command_name != "command" {
            return None;
        }
        if !suppress_function_lookup && get_posix_function(env, "command").is_some() {
            return None;
        }

        let items = &suffix?.0;
        let mut index = cursor;
        let mut options = true;
        let mut found_target = false;
        while let Some(item) = items.get(index) {
            match item {
                CommandPrefixOrSuffixItem::IoRedirect(_) => index += 1,
                CommandPrefixOrSuffixItem::Word(word) => {
                    let value = static_command_word(&word.value)?;
                    if options && value == "--" {
                        options = false;
                        index += 1;
                        continue;
                    }
                    if options && value.starts_with('-') && value.len() > 1 {
                        if value[1..].chars().all(|flag| flag == 'p') {
                            index += 1;
                            continue;
                        }
                        return None;
                    }
                    command_name = value;
                    cursor = index + 1;
                    suppress_function_lookup = true;
                    found_target = true;
                    break;
                }
                CommandPrefixOrSuffixItem::AssignmentWord(..)
                | CommandPrefixOrSuffixItem::ProcessSubstitution(..) => return None,
            }
        }
        if !found_target {
            return None;
        }
    }
}

fn static_command_word(word: &str) -> Option<&str> {
    if let Some(literal) = word.strip_prefix('\'').and_then(|s| s.strip_suffix('\''))
        && !literal.contains('\'')
    {
        return Some(literal);
    }
    if let Some(literal) = word.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
        && !literal
            .chars()
            .any(|character| matches!(character, '"' | '$' | '`' | '\\'))
    {
        return Some(literal);
    }
    if word.chars().any(|character| {
        matches!(
            character,
            '\'' | '"' | '$' | '`' | '\\' | '*' | '?' | '[' | ']'
        )
    }) {
        return None;
    }
    Some(word)
}

#[async_recursion]
async fn eval_simple_command(
    simple: &SimpleCommand,
    env: &Env,
    cfg: &EvalConfig,
) -> Result<i32, PosixError> {
    let positional = &cfg.positional;

    // POSIX 2.9.1: collect assignments and redirections, expand the command
    // word and arguments, perform the redirections, then expand and perform
    // the assignments left to right.
    let mut assignments: Vec<&Assignment> = Vec::new();
    let mut redirects: Vec<&IoRedirect> = Vec::new();
    for item in simple.prefix.iter().flat_map(|prefix| prefix.0.iter()) {
        match item {
            CommandPrefixOrSuffixItem::AssignmentWord(assign, _) => assignments.push(assign),
            CommandPrefixOrSuffixItem::IoRedirect(r) => redirects.push(r),
            _ => {}
        }
    }

    let cmd_word = simple.word_or_name.as_ref().map(|w| w.value.clone());
    let assignment_context_start =
        declaration_suffix_start(cmd_word.as_deref(), simple.suffix.as_ref(), env);

    let mut args: Vec<String> = match &cmd_word {
        Some(word) => expand_word(
            word,
            env,
            &ExpansionConfig {
                do_glob: !env.options.read().noglob,
                ..Default::default()
            },
            positional,
        )?,
        None => Vec::new(),
    };
    if let Some(suffix) = &simple.suffix {
        for (item_index, item) in suffix.0.iter().enumerate() {
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
                    if assignment_context_start.is_some_and(|start| item_index >= start) {
                        let value = match &assign.value {
                            AssignmentValue::Scalar(word) => {
                                expand_assignment_word(&word.value, env, positional)?
                            }
                            AssignmentValue::Array(elems) => elems
                                .iter()
                                .map(|(_, word)| word.value.as_str())
                                .collect::<Vec<_>>()
                                .join(" "),
                        };
                        args.push(format!("{name}={value}"));
                        continue;
                    }
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

    let redirected = match Redirected::apply(env, &redirects, positional) {
        Ok(applied) => applied,
        Err(error) => {
            return redirection_failed(env, cfg, args.first().map(String::as_str), error).await;
        }
    };

    // Each name's shell value and command-environment value before this
    // command assigned it, so temporary assignments can be undone.
    let mut prior: Vec<(String, Option<Val>, Option<Val>)> = Vec::new();
    let mut prefix_assignments: Vec<(String, String)> = Vec::new();
    let (assigned, substitution_status) = crate::expand::tracking_substitutions(|| {
        for assign in &assignments {
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
            // A later value sees an earlier one: `a=1 b=$a`.
            let mut vars = env.vars.write();
            if !prior.iter().any(|(n, _, _)| *n == name) {
                let in_env = match vars.get("env") {
                    Some(Val::Map(map)) => map.get(&ustr::ustr(&name)).cloned(),
                    _ => None,
                };
                prior.push((name.clone(), vars.get(&name).cloned(), in_env));
            }
            vars.insert(name.clone(), Val::String(value.clone()));
            drop(vars);
            prefix_assignments.push((name, value));
        }
        Ok::<(), PosixError>(())
    });
    assigned?;

    // A command word that expands to nothing hands the role to the next field;
    // with no fields at all the assignments stay in this shell.
    if args.is_empty() {
        for (name, value) in prefix_assignments {
            env.set_shell_var(&name, Val::String(value));
        }
        // The status is that of the last command substitution performed, or
        // zero: `false; x=1` succeeds, `x=$(exit 3)` fails with 3.
        let status = substitution_status.unwrap_or(0);
        env.set_exit_code(status as i64);
        return Ok(status);
    }
    let mut cmd_name = args.remove(0);

    // Resolve executable `command` wrappers before prefix-assignment handling.
    // Lookup forms stay as the `command` builtin for execution.
    let mut suppress_function_lookup = false;
    let mut search_path_override = None;
    let mut command_action = None;
    loop {
        if cmd_name != "command"
            || (!suppress_function_lookup && get_posix_function(env, "command").is_some())
        {
            break;
        }
        match crate::posix_builtins::command::parse_command_args(&args) {
            crate::posix_builtins::command::CommandAction::Execute {
                command_name,
                args: command_args,
                use_default_path,
            } => {
                cmd_name = command_name;
                args = command_args;
                suppress_function_lookup = true;
                search_path_override = use_default_path
                    .then_some(crate::posix_builtins::command::default_path().to_string());
            }
            action => {
                command_action = Some(action);
                break;
            }
        }
    }

    // `exec` with only redirections applies them to the shell itself.
    if cmd_name == "exec" && args.is_empty() && get_posix_function(env, "exec").is_none() {
        redirected.persist();
        return Ok(0);
    }

    // `command` suppresses the special properties of special builtins. In
    // particular, a prefix assignment before `command export` is temporary.
    let is_special_decl_cmd =
        !suppress_function_lookup && matches!(cmd_name.as_str(), "export" | "readonly");
    let is_regular_decl_cmd = matches!(cmd_name.as_str(), "declare" | "typeset" | "local")
        && (suppress_function_lookup || get_posix_function(env, &cmd_name).is_none());
    let is_decl_cmd = is_special_decl_cmd || is_regular_decl_cmd;

    // A declaration builtin declares a shell variable; only a plain command's
    // prefix assignment becomes part of that command's environment.
    if !is_decl_cmd && let Some(Val::Map(map)) = env.vars.write().get_mut("env") {
        for (name, value) in &prefix_assignments {
            map.insert(ustr::ustr(name), Val::String(value.clone()));
        }
    }

    let res = eval_simple_command_inner(
        &cmd_name,
        &args,
        &prefix_assignments,
        env,
        cfg,
        suppress_function_lookup,
        search_path_override.as_deref(),
        command_action,
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
    } else if !prior.is_empty() {
        let mut vars = env.vars.write();
        for (name, shell_value, env_value) in prior.into_iter().rev() {
            match shell_value {
                Some(value) => vars.insert(name.clone(), value),
                None => vars.remove(&name),
            };
            if let Some(Val::Map(map)) = vars.get_mut("env") {
                match env_value {
                    Some(value) => map.insert(ustr::ustr(&name), value),
                    None => map.shift_remove(&ustr::ustr(&name)),
                };
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
    env: &Env,
    cfg: &EvalConfig,
    suppress_function_lookup: bool,
    search_path_override: Option<&str>,
    command_action: Option<crate::posix_builtins::command::CommandAction>,
) -> Result<i32, PosixError> {
    if !suppress_function_lookup
        && !crate::posix_builtins::type_cmd::is_special_builtin(cmd_name)
        && let Some(func_body) = get_posix_function(env, cmd_name)
    {
        let positional_scope = crate::positional::Scope::new(env, args);
        let fn_cfg = EvalConfig {
            positional: args.to_vec(),
            errexit: cfg.errexit,
        };
        let result = match Redirected::apply(env, &redirect_items(&func_body.1), &fn_cfg.positional)
        {
            Ok(_redirected) => eval_compound_command(&func_body.0, env, &fn_cfg).await,
            Err(error) => redirection_failed(env, cfg, None, error).await,
        };
        drop(positional_scope);
        // `return` ends the function, not the script that called it: a caller sees
        // the status the function returned with and carries on.
        return match result {
            Ok(code) => Ok(code),
            Err(PosixError::Return(code)) => {
                env.set_exit_code(code as i64);
                Ok(code)
            }
            Err(other) => Err(other),
        };
    }

    match cmd_name {
        ":" | "true" => return Ok(0),
        "false" => return Ok(1),
        "exit" => {
            if args.len() > 1 {
                report(env, "exit: too many arguments").await;
                return Ok(1);
            }
            let code = match parse_status_argument(args.first(), env.exit_code()) {
                Ok(code) => code,
                Err(error) => {
                    report(env, &format!("exit: {error}")).await;
                    return Err(PosixError::Exit(2));
                }
            };
            return Err(PosixError::Exit(code));
        }
        "return" => {
            if args.len() > 1 {
                report(env, "return: too many arguments").await;
                return Ok(1);
            }
            let code = match parse_status_argument(args.first(), env.exit_code()) {
                Ok(code) => code,
                Err(error) => {
                    report(env, &format!("return: {error}")).await;
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
            return Ok(0);
        }
        "break" => return Err(PosixError::Break),
        "continue" => return Err(PosixError::Continue),
        "shift" => {
            if args.len() > 1 {
                report(env, "shift: too many arguments").await;
                return Ok(2);
            }
            let n = match args.first() {
                None => 1,
                Some(value) => match value.parse::<usize>() {
                    Ok(n) => n,
                    Err(error) => {
                        report(env, &format!("shift: invalid count {:?}: {}", value, error)).await;
                        return Ok(2);
                    }
                },
            };
            match crate::posix_builtins::shift::shift_posix(env, n) {
                Ok(()) => return Ok(0),
                Err(error) => {
                    report(env, &error.to_string()).await;
                    return Ok(1);
                }
            }
        }
        "set" => {
            return Ok(match crate::posix_builtins::shift::set_posix(env, args) {
                // `set -o` prints the options as the command's own stdout.
                Ok(Some(text)) => emit(env, cmd_name, &text).await?,
                Ok(None) => 0,
                Err(message) => {
                    report(env, &message).await;
                    2
                }
            });
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
            return Ok(0);
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
                let written = emit(env, cmd_name, &rendered).await?;
                return Ok(if written != 0 { written } else { 0 });
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
            return Ok(0);
        }
        "read" => {
            return Ok(
                match crate::posix_builtins::read_cmd::read_posix(args, env).await {
                    Ok(code) => code,
                    Err((code, message)) => {
                        report(env, &message).await;
                        code
                    }
                },
            );
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
                report(env, diagnostic).await;
            }
            let written = emit(env, cmd_name, &result.output).await?;
            return Ok(if written != 0 { written } else { result.status });
        }
        "getopts" => {
            let mut diagnostics = Vec::new();
            let result = crate::posix_builtins::getopts::getopts_posix(args, env, &mut diagnostics);
            for diagnostic in &diagnostics {
                report(env, diagnostic).await;
            }
            return Ok(match result {
                Ok(code) => code,
                Err(message) => {
                    report(env, &message).await;
                    2
                }
            });
        }
        "type" => {
            let mut diagnostics = Vec::new();
            let (code, rendered) =
                match crate::posix_builtins::type_cmd::type_posix(args, env, &mut diagnostics) {
                    Ok(result) => result,
                    Err(message) => {
                        diagnostics.push(message);
                        (2, String::new())
                    }
                };
            let written = emit(env, cmd_name, &rendered).await?;
            for diagnostic in &diagnostics {
                report(env, diagnostic).await;
            }
            return Ok(if written != 0 { written } else { code });
        }
        "command" => {
            let action = command_action
                .unwrap_or_else(|| crate::posix_builtins::command::parse_command_args(args));
            match action {
                crate::posix_builtins::command::CommandAction::Noop => {
                    return Ok(0);
                }
                crate::posix_builtins::command::CommandAction::Error(error) => {
                    report(env, &format!("command: {error}")).await;
                    return Ok(2);
                }
                crate::posix_builtins::command::CommandAction::Lookup {
                    names,
                    verbose,
                    use_default_path,
                } => {
                    let path_override =
                        use_default_path.then_some(crate::posix_builtins::command::default_path());
                    let (code, rendered) = crate::posix_builtins::command::lookup_commands(
                        &names,
                        verbose,
                        env,
                        path_override,
                    );
                    let written = emit(env, cmd_name, &rendered).await?;
                    return Ok(if written != 0 { written } else { code });
                }
                crate::posix_builtins::command::CommandAction::Execute { .. } => {
                    return Err(PosixError::Engine(EngineError::Generic {
                        message: "command execution was not unwrapped before POSIX dispatch"
                            .to_string(),
                        span: None,
                    }));
                }
            }
        }
        "eval" => {
            let code = crate::posix_builtins::eval_builtin::eval_posix(args, env)
                .await
                .map_err(PosixError::Engine)?;
            return Ok(code);
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
                    report(env, &format!("test: {error}")).await;
                    2
                }
            };
            return Ok(code);
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
            let written = emit(env, cmd_name, &text).await?;
            return Ok(if written != 0 { written } else { 0 });
        }
        "cd" => {
            return Ok(match crate::posix_builtins::cd::cd_posix(args, env) {
                Ok(Some(line)) => emit(env, cmd_name, &line).await?,
                Ok(None) => 0,
                Err((code, message)) => {
                    report(env, &message).await;
                    code
                }
            });
        }
        "pwd" => {
            return Ok(match crate::posix_builtins::cd::pwd_posix(args, env) {
                Ok(line) => emit(env, cmd_name, &line).await?,
                Err((code, message)) => {
                    report(env, &message).await;
                    code
                }
            });
        }
        "exec" => {
            if args.is_empty() {
                return Ok(0);
            }
            // The utility runs with this shell's descriptors, environment and
            // PATH, and the shell then leaves with its status. Evaluation may
            // be an in-process subshell, so replacing the whole process image
            // is not an option.
            let code = run_external_command(
                &args[0],
                &args[1..],
                prefix_assignments,
                env,
                search_path_override,
            )
            .await?;
            return Err(PosixError::Exit(code));
        }
        "trap" => {
            let rendered = handle_posix_trap(args, env)?;
            let written = emit(env, cmd_name, &rendered).await?;
            return Ok(if written != 0 { written } else { 0 });
        }
        "wait" => {
            let code = handle_posix_wait(args, env).await?;
            return Ok(code);
        }
        "umask" => {
            match args.first() {
                None => {
                    // Reading the mask requires a set-then-restore round trip.
                    let current = unsafe { libc::umask(0o022 as libc::mode_t) };
                    unsafe { libc::umask(current) };
                    let text = format!("{:04o}\n", current);
                    let written = emit(env, cmd_name, &text).await?;
                    return Ok(if written != 0 { written } else { 0 });
                }
                Some(mask_arg) => {
                    let parsed = i32::from_str_radix(mask_arg.trim_start_matches('0'), 8)
                        .or_else(|_| i32::from_str_radix(mask_arg, 8));
                    match parsed {
                        Ok(mask) => {
                            unsafe { libc::umask(mask as libc::mode_t) };
                            return Ok(0);
                        }
                        Err(_) => {
                            report(env, &format!("umask: invalid mask: {mask_arg}")).await;
                            return Ok(1);
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
                let written = emit(env, cmd_name, &out).await?;
                return Ok(if written != 0 { written } else { 0 });
            }
            let mut listed = String::new();
            let mut status = 0;
            for arg in args {
                if let Some((name, value)) = arg.split_once('=') {
                    env.register_alias(name, value);
                } else if let Some(expansion) = env.get_alias(arg) {
                    listed.push_str(&format!("alias {arg}='{expansion}'\n"));
                } else {
                    report(env, &format!("alias: {arg}: not found")).await;
                    status = 1;
                }
            }
            if listed.is_empty() {
                return Ok(status);
            }
            let written = emit(env, cmd_name, &listed).await?;
            return Ok(if written != 0 { written } else { status });
        }
        "unalias" => {
            if args.iter().any(|a| a == "-a") {
                for (name, _) in env.get_all_aliases() {
                    env.remove_alias(&name);
                }
                return Ok(0);
            }
            let mut status = 0;
            for arg in args {
                if env.remove_alias(arg).is_none() {
                    report(env, &format!("unalias: {arg}: not found")).await;
                    status = 1;
                }
            }
            return Ok(status);
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
                        report(env, &format!("ulimit: unsupported option -{other}")).await;
                        return Ok(1);
                    }
                }
            }
            let mut rl = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if unsafe { libc::getrlimit(resource, &mut rl) } != 0 {
                report(
                    env,
                    &format!("ulimit: getrlimit: {}", std::io::Error::last_os_error()),
                )
                .await;
                return Ok(1);
            }
            if let Some(value) = args.get(idx) {
                rl.rlim_cur = if value == "unlimited" {
                    libc::RLIM_INFINITY
                } else {
                    match value.parse::<u64>() {
                        Ok(v) => v as libc::rlim_t,
                        Err(_) => {
                            report(env, &format!("ulimit: invalid value: {value}")).await;
                            return Ok(1);
                        }
                    }
                };
                if unsafe { libc::setrlimit(resource, &rl) } != 0 {
                    report(
                        env,
                        &format!("ulimit: setrlimit: {}", std::io::Error::last_os_error()),
                    )
                    .await;
                    return Ok(1);
                }
                return Ok(0);
            }
            let text = if rl.rlim_cur == libc::RLIM_INFINITY {
                "unlimited\n".to_string()
            } else if resource == libc::RLIMIT_FSIZE {
                // POSIX reports the file-size limit in 512-byte blocks.
                format!("{}\n", rl.rlim_cur / 512)
            } else {
                format!("{}\n", rl.rlim_cur)
            };
            let written = emit(env, cmd_name, &text).await?;
            return Ok(if written != 0 { written } else { 0 });
        }
        "times" => {
            return Ok(0);
        }
        "hash" => {
            // hash -r clears the command hash table (= PATH cache) so mutated
            // PATH is respected. venv activate/deactivate rely on this.
            if args.iter().any(|a| a == "-r") {
                fshell_engine::invalidate_path_cache();
            }
            return Ok(0);
        }
        "dot" | "." | "source" => {
            let Some(path) = args.first() else {
                report(env, &format!("{cmd_name}: filename argument required")).await;
                return Ok(2);
            };
            let Some(source_path) = find_dot_script(path, env) else {
                report(env, &format!("{cmd_name}: {path}: not found")).await;
                return Ok(1);
            };
            let content = match std::fs::read_to_string(&source_path) {
                Ok(content) => content,
                Err(error) => {
                    report(
                        env,
                        &format!(
                            "{cmd_name}: {path}: {}",
                            crate::redirect::os_message(&error)
                        ),
                    )
                    .await;
                    return Ok(1);
                }
            };
            let parsed = crate::parser::parse_posix_script(&content).map_err(PosixError::Engine)?;
            // Operands become the positional parameters for the duration of the
            // file; without operands it sees the caller's.
            let operands = &args[1..];
            let _positional =
                (!operands.is_empty()).then(|| crate::positional::Scope::new(env, operands));
            let dot_cfg = EvalConfig {
                positional: if operands.is_empty() {
                    cfg.positional.clone()
                } else {
                    operands.to_vec()
                },
                errexit: cfg.errexit,
            };
            return match eval_program(&parsed.program, env, &dot_cfg).await {
                Ok(code) | Err(PosixError::Return(code)) => Ok(code),
                Err(other) => Err(other),
            };
        }
        _ => {}
    }

    // Job control is shared with the native engine: run `jobs`, `kill`, `fg`,
    // `bg` and `disown` through the registered builtin so both engines report
    // and signal the same job table.
    if matches!(cmd_name, "jobs" | "kill" | "fg" | "bg" | "disown")
        && let Some(handler) = env.get_builtin(cmd_name)
    {
        let argv: Vec<Val> = args.iter().map(|a| Val::String(a.clone())).collect();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<fshell_engine::PipelinePayload>(64);
        if let Err(error) = handler(None, argv, env, tx, None) {
            report(env, &error.to_string()).await;
            return Ok(1);
        }
        let mut status = 0;
        while let Some(payload) = rx.recv().await {
            if let fshell_engine::PipelinePayload::Data(val) = payload {
                let mut text = val.to_text();
                text.push('\n');
                status = status.max(emit(env, cmd_name, &text).await?);
            }
        }
        return Ok(status);
    }

    // Fallback: subprocess execution with full I/O piping and redirections
    run_external_command(
        cmd_name,
        args,
        prefix_assignments,
        env,
        search_path_override,
    )
    .await
}

/// Locate a `.` operand: a name with a slash is a path, anything else is
/// searched for along PATH (readable, not necessarily executable) and then,
/// as bash does, in the current directory.
fn find_dot_script(name: &str, env: &Env) -> Option<std::path::PathBuf> {
    if name.contains('/') {
        let path = env.resolve_path(name);
        return path.is_file().then_some(path);
    }
    let search = env
        .vars
        .read()
        .get("PATH")
        .map(Val::to_text)
        .unwrap_or_default();
    search
        .split(':')
        .filter(|dir| !dir.is_empty())
        .map(|dir| env.resolve_path(dir).join(name))
        .chain(std::iter::once(env.resolve_path(name)))
        .find(|candidate| candidate.is_file())
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

/// POSIX `wait [pid|%job ...]`: with no operands wait for every background
/// job and succeed; otherwise the status of the last operand's job, or 127
/// when it is not a job of this shell.
async fn handle_posix_wait(args: &[String], env: &Env) -> Result<i32, PosixError> {
    if args.is_empty() {
        fshell_engine::background::wait_all_jobs(env).await;
        return Ok(0);
    }
    let mut last_code = 0;
    for arg in args {
        let job = fshell_engine::background::wait_operand_pid(env, arg);
        last_code = match job {
            Some(pid) => match fshell_engine::background::wait_job(env, pid).await {
                Some(code) => code,
                None => {
                    report(
                        env,
                        &format!("wait: pid {pid} is not a child of this shell"),
                    )
                    .await;
                    127
                }
            },
            None => {
                report(env, &format!("wait: {arg}: no such job")).await;
                127
            }
        };
    }
    Ok(last_code)
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
    env: &Env,
    search_path_override: Option<&str>,
) -> Result<i32, PosixError> {
    // Resolve against the shell's PATH here rather than letting `execvp` search:
    // the shell variable, not the process environment, is authoritative, and a
    // resolved program lets std launch with `posix_spawn` instead of `fork`.
    let program = if cmd_name.contains('/') {
        cmd_name.to_string()
    } else {
        let search_path = search_path_override.map(str::to_string).or_else(|| {
            prefix_assignments
                .iter()
                .rev()
                .find(|(k, _)| k == "PATH")
                .map(|(_, v)| v.clone())
                .or_else(|| env.vars.read().get("PATH").map(Val::to_text))
        });
        match fshell_engine::resolve_cached_command_path_at(
            cmd_name,
            search_path.as_deref(),
            &env.cwd(),
        ) {
            Some(path) => path,
            None => {
                report(env, &format!("{cmd_name}: command not found")).await;
                return Ok(127);
            }
        }
    };
    let mut cmd = std::process::Command::new(program);
    {
        use std::os::unix::process::CommandExt;
        cmd.arg0(cmd_name);
    }
    cmd.args(args);
    cmd.current_dir(env.cwd());
    configure_child_environment(&mut cmd, env, prefix_assignments);
    if let Some(path) = search_path_override {
        // `command -p` controls the utility search as well as the environment
        // the utility itself sees.
        cmd.env("PATH", path);
    }

    let is_interactive = owns_terminal(env);
    match spawn_supervised(cmd, env, cmd_name, is_interactive).await {
        Ok(code) => Ok(code),
        Err(SpawnError::Launch(error)) => {
            let (code, reason) = match error.kind() {
                std::io::ErrorKind::NotFound => (127, "command not found".to_string()),
                std::io::ErrorKind::PermissionDenied => (126, "Permission denied".to_string()),
                _ => (126, crate::redirect::os_message(&error)),
            };
            report(env, &format!("{cmd_name}: {reason}")).await;
            Ok(code)
        }
        Err(SpawnError::Shell(error)) => Err(error),
    }
}

/// Whether a child would run in the foreground of a controlling terminal: the
/// shell has one, and the child's stdin and stdout are the shell's own.
fn owns_terminal(env: &Env) -> bool {
    !fshell_engine::is_test_mode()
        && !env.is_captured
        && unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
        && fshell_engine::is_stdout_a_tty()
        && matches!(env.descriptor(0), Some(Descriptor::Process(0)))
        && matches!(env.descriptor(1), Some(Descriptor::Process(1)))
}

/// The environment a child starts with: every exported variable with its
/// current shell value, then this command's prefix assignments.
///
/// Membership of the `env` map is the export attribute; the value is the
/// shell variable's, since an assignment to an exported variable updates only
/// the shell value. Unexported variables never reach a child.
fn configure_child_environment(
    cmd: &mut std::process::Command,
    env: &Env,
    prefix_assignments: &[(String, String)],
) {
    let vars = env.vars.read();
    if let Some(Val::Map(exported)) = vars.get("env") {
        cmd.env_clear();
        for (name, inherited) in exported.iter() {
            let value = vars.get(name.as_str()).unwrap_or(inherited);
            cmd.env(name.as_str(), value.to_text());
        }
    }
    drop(vars);
    for (k, v) in prefix_assignments {
        cmd.env(k, v);
    }
}

enum SpawnError {
    /// The child could not be started; the caller reports it as a status.
    Launch(std::io::Error),
    /// The shell itself failed (interrupt, descriptor plumbing).
    Shell(PosixError),
}

/// Start `cmd` with this environment's descriptors, register it as the
/// foreground job, and wait for it while serving its in-process descriptors.
async fn spawn_supervised(
    mut cmd: std::process::Command,
    env: &Env,
    label: &str,
    is_interactive: bool,
) -> Result<i32, SpawnError> {
    let io_error = |error: std::io::Error| {
        SpawnError::Shell(PosixError::Engine(EngineError::IoError {
            message: format!("{label}: {}", crate::redirect::os_message(&error)),
            span: None,
        }))
    };
    let child_fds = ChildDescriptors::prepare(&env.descriptors.read().clone()).map_err(io_error)?;
    child_fds.attach(&mut cmd).map_err(io_error)?;
    fshell_engine::job_control::configure_child_launch(&mut cmd, 0, is_interactive);

    let mut cmd = tokio::process::Command::from(cmd);
    let spawned = {
        let _launch = fshell_engine::job_control::launch_guard();
        cmd.spawn()
    };
    // The command owns the child's copies of the descriptors; keeping them
    // would hold pipe write ends open past the child.
    drop(cmd);
    let mut child = spawned.map_err(SpawnError::Launch)?;

    let child_pid = child.id().map(|id| id as i32);
    let _fg_guard = PosixForegroundGuard::new(env, child_pid, label, is_interactive);

    let cancelled = async {
        while !env.pipeline_cancelled() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    };
    let status = tokio::select! {
        status = child.wait() => status,
        _ = cancelled => {
            match child_pid {
                Some(pgid) => unsafe {
                    libc::kill(-pgid, libc::SIGINT);
                },
                None => {
                    let _ = child.start_kill();
                }
            }
            let _ = child.wait().await;
            return Err(SpawnError::Shell(PosixError::Interrupted));
        }
    }
    .map_err(io_error)?;

    let code = {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .or_else(|| status.signal().map(|signal| 128 + signal))
            .unwrap_or(127)
    };
    env.set_exit_code(code as i64);
    if code == 130 || env.pipeline_cancelled() {
        env.job_control
            .sigint_pending
            .store(false, std::sync::atomic::Ordering::SeqCst);
        return Err(SpawnError::Shell(PosixError::Interrupted));
    }
    Ok(code)
}

/// Evaluate `payload` in a child `fsh` process (POSIX mode), as a subshell.
///
/// A real process gives the subshell its own PID, signals, umask and `exit`
/// without disturbing the parent, matching POSIX. It starts with this
/// environment's descriptors, exactly as an external command does.
async fn run_child_shell(
    payload: &fshell_engine::background::ChildPayload,
    env: &Env,
) -> Result<i32, PosixError> {
    let launch = fshell_engine::background::prepare_child(payload, env)?;
    let mut cmd = std::process::Command::new(&launch.program);
    cmd.args(&launch.args);
    cmd.current_dir(env.cwd());
    configure_child_environment(&mut cmd, env, &[]);
    let result = spawn_supervised(cmd, env, "subshell", owns_terminal(env)).await;
    for file in &launch.temp_files {
        let _ = std::fs::remove_file(file);
    }
    match result {
        Ok(code) => Ok(code),
        Err(SpawnError::Launch(error)) => Err(PosixError::Engine(EngineError::IoError {
            message: format!("subshell: {}", crate::redirect::os_message(&error)),
            span: None,
        })),
        Err(SpawnError::Shell(error)) => Err(error),
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
    async fn builtin_output_goes_to_the_current_stdout_binding() {
        use std::io::Read;
        use std::os::fd::{AsRawFd, FromRawFd};
        let (read_fd, write_fd) =
            fshell_engine::create_cloexec_pipe().expect("private output pipe");
        // SAFETY: each fresh pipe descriptor is transferred to exactly one File.
        let mut reader = unsafe { std::fs::File::from_raw_fd(read_fd) };
        let writer = unsafe { std::fs::File::from_raw_fd(write_fd) };
        let env = Env::for_command();
        env.descriptors
            .write()
            .set(1, Descriptor::Process(writer.as_raw_fd()));
        assert_eq!(emit(&env, "echo", "redirected").await.expect("write"), 0);
        assert!(
            writer.metadata().is_ok(),
            "the caller still owns its original descriptor"
        );
        drop(env);
        drop(writer);
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .expect("read redirected bytes");
        assert_eq!(bytes, b"redirected");
    }

    #[tokio::test]
    async fn writing_to_a_closed_stdout_is_the_builtins_failure() {
        let env = Env::for_command();
        env.descriptors.write().close(1);
        let (read_end, write_end) = descriptors::pipe().expect("pipe");
        env.descriptors
            .write()
            .set(2, Descriptor::File(std::sync::Arc::new(write_end)));
        assert_eq!(emit(&env, "echo", "lost").await.expect("status"), 1);
        drop(env);
        assert_eq!(
            descriptors::read_to_end(read_end)
                .await
                .expect("diagnostic"),
            b"echo: write error: Bad file descriptor\n"
        );
    }

    #[tokio::test]
    async fn writing_to_a_pipe_without_readers_ends_like_sigpipe() {
        let env = Env::for_command();
        let (read_end, write_end) = descriptors::pipe().expect("pipe");
        drop(read_end);
        env.descriptors
            .write()
            .set(1, Descriptor::File(std::sync::Arc::new(write_end)));
        assert!(matches!(
            emit(&env, "echo", "lost").await,
            Err(PosixError::Exit(code)) if code == 128 + libc::SIGPIPE
        ));
    }

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

#[cfg(test)]
mod exit_trap_status {
    use super::*;

    async fn whole(source: &str, env: &Env) -> Result<i32, EngineError> {
        let parsed = crate::parse_posix_script(source).expect("script");
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            eval_source(&parsed, env, &EvalConfig::default()),
        )
        .await
        .expect("trap deadline")
    }

    #[tokio::test]
    async fn normal_handler_preserves_the_exiting_status_and_is_consumed() {
        let env = Env::for_command();
        env.options.write().errexit = false;
        assert_eq!(
            whole("trap 'seen=$?; false' EXIT; exit 23", &env)
                .await
                .expect("exit"),
            23
        );
        assert_eq!(env.vars.read().get("seen"), Some(&Val::String("23".into())));
        assert_eq!(env.exit_code(), 23);
        assert!(env.posix_exit_trap.read().is_none());
    }

    #[tokio::test]
    async fn handler_exit_overrides_the_original_exit_without_reentry() {
        let env = Env::for_command();
        assert_eq!(
            whole("trap 'seen=$?; exit 17; wrong=yes' EXIT; exit 23", &env)
                .await
                .expect("exit"),
            17
        );
        assert_eq!(env.vars.read().get("seen"), Some(&Val::String("23".into())));
        assert_eq!(env.exit_code(), 17);
        assert!(!env.vars.read().contains_key("wrong"));
        assert!(env.posix_exit_trap.read().is_none());
    }

    #[tokio::test]
    async fn hard_error_handler_observes_failure_and_keeps_the_original_error() {
        let env = Env::for_command();
        env.vars.write().remove("env");
        let error = whole("trap 'seen=$?' EXIT; : \"${required:?supply value}\"", &env)
            .await
            .expect_err("hard error");
        assert!(
            matches!(error, EngineError::ParameterExpansion { ref parameter, ref message, span: None } if parameter == "${required}" && message == "supply value")
        );
        assert_eq!(env.vars.read().get("seen"), Some(&Val::String("1".into())));
        assert_eq!(env.exit_code(), 1);
        assert!(env.posix_exit_trap.read().is_none());
    }

    #[tokio::test]
    async fn handler_uses_script_arguments_before_the_callers_bindings_are_restored() {
        let env = Env::for_command();
        crate::positional::replace(
            &mut env.vars.write(),
            vec![Val::Int(7), Val::String("parent".into())],
        );
        let before = env.vars.read().clone();
        let parsed =
            crate::parse_posix_script("trap 'seen=$1; count=$#' EXIT; exit 23").expect("script");
        let cfg = EvalConfig {
            positional: vec!["script argument".into()],
            ..Default::default()
        };
        let code = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            eval_source(&parsed, &env, &cfg),
        )
        .await
        .expect("deadline")
        .expect("exit");
        assert_eq!(code, 23);
        assert_eq!(
            env.vars.read().get("seen"),
            Some(&Val::String("script argument".into()))
        );
        assert_eq!(env.vars.read().get("count"), Some(&Val::String("1".into())));
        for name in ["@", "#", "1", "2"] {
            assert_eq!(env.vars.read().get(name), before.get(name));
        }
    }

    #[tokio::test]
    async fn dropped_awaiting_handler_restores_the_incoming_status() {
        use std::future::Future;
        use std::task::{Context, Waker};
        let env = Env::for_command();
        env.options.write().errexit = false;
        env.set_exit_code(23);
        *env.posix_exit_trap.write() = Some("false; jobs".into());
        let lease = std::sync::Arc::new(std::sync::Mutex::new(None));
        let handler_lease = lease.clone();
        #[expect(
            clippy::result_large_err,
            reason = "production BuiltinHandler error type"
        )]
        let handler: fshell_engine::BuiltinHandler = std::sync::Arc::new(move |_, _, _, tx, _| {
            *handler_lease.lock().expect("lease") = Some(tx);
            Ok(())
        });
        env.register_builtin("jobs", handler);
        let mut call = Box::pin(run_exit_trap(&env));
        assert!(
            call.as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        assert!(
            lease.lock().expect("lease").is_some(),
            "handler is actually awaiting EOF"
        );
        assert_eq!(env.exit_code(), 1, "handler has changed the live status");
        drop(call);
        assert_eq!(env.exit_code(), 23);
        assert!(env.posix_exit_trap.read().is_none());
        let sender = lease.lock().expect("lease").take().expect("held sender");
        assert!(
            sender.is_closed(),
            "dropping the handler also drops its receiver"
        );
    }
}

#[cfg(test)]
mod redirection_ownership {
    use super::*;

    #[tokio::test]
    async fn function_and_group_output_reaches_inherited_files_before_return() {
        let directory = tempfile::tempdir().expect("directory");
        let env = Env::for_command();
        env.set_cwd(directory.path().to_path_buf());
        env.options.write().errexit = false;
        let parsed = crate::parse_posix_script(
            "f() { printf outer; { printf inner; return 19; } > inner; }; f > outer; printf after",
        )
        .expect("script");
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            eval_source_stream(&parsed, &env, &EvalConfig::default(), true),
        )
        .await
        .expect("deadline")
        .expect("evaluation");
        assert_eq!(result, (0, Some(b"after".to_vec())));
        assert_eq!(
            std::fs::read(directory.path().join("outer")).expect("outer"),
            b"outer"
        );
        assert_eq!(
            std::fs::read(directory.path().join("inner")).expect("inner"),
            b"inner"
        );
    }

    #[tokio::test]
    async fn a_compound_redirect_is_undone_after_a_hard_error() {
        let directory = tempfile::tempdir().expect("directory");
        let env = Env::for_command();
        env.set_cwd(directory.path().to_path_buf());
        env.vars.write().remove("env");
        let parsed =
            crate::parse_posix_script("{ printf before; : \"${required:?supply value}\"; } > out")
                .expect("script");
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            eval_compound_list(
                &parsed.program.complete_commands[0],
                &env,
                &EvalConfig::default(),
            ),
        )
        .await
        .expect("deadline")
        .expect_err("hard error");
        assert!(matches!(
            error,
            PosixError::Engine(EngineError::ParameterExpansion { .. })
        ));
        assert!(matches!(env.descriptor(1), Some(Descriptor::Process(1))));
        assert_eq!(
            std::fs::read(directory.path().join("out")).expect("written prefix"),
            b"before"
        );
    }

    #[tokio::test]
    async fn dropping_a_command_awaiting_eof_restores_the_callers_descriptors() {
        use std::future::Future;
        use std::task::{Context, Waker};
        let directory = tempfile::tempdir().expect("directory");
        let env = Env::for_command();
        env.set_cwd(directory.path().to_path_buf());
        env.options.write().errexit = false;
        let lease = std::sync::Arc::new(std::sync::Mutex::new(None));
        let handler_lease = lease.clone();
        #[expect(
            clippy::result_large_err,
            reason = "production BuiltinHandler error type"
        )]
        let handler: fshell_engine::BuiltinHandler = std::sync::Arc::new(move |_, _, _, tx, _| {
            *handler_lease.lock().expect("lease") = Some(tx);
            Ok(())
        });
        env.register_builtin("jobs", handler);
        let parsed = crate::parse_posix_script("{ printf before; jobs; } > out").expect("script");
        let cfg = EvalConfig::default();
        let mut call = Box::pin(eval_compound_list(
            &parsed.program.complete_commands[0],
            &env,
            &cfg,
        ));
        assert!(
            call.as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        assert!(
            lease.lock().expect("lease").is_some(),
            "actually awaiting EOF"
        );
        assert!(matches!(env.descriptor(1), Some(Descriptor::File(_))));
        drop(call);
        assert!(matches!(env.descriptor(1), Some(Descriptor::Process(1))));
        assert_eq!(
            std::fs::read(directory.path().join("out")).expect("prefix"),
            b"before"
        );
        let sender = lease.lock().expect("lease").take().expect("held sender");
        assert!(sender.is_closed(), "the awaiting receiver is dropped");
    }

    #[tokio::test]
    async fn duplicating_a_closed_descriptor_fails_the_command_only() {
        let env = Env::for_command();
        env.options.write().errexit = false;
        let parsed =
            crate::parse_posix_script("{ echo x >&9; echo status=$?; } 2>&1").expect("script");
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            eval_source_stream(&parsed, &env, &EvalConfig::default(), true),
        )
        .await
        .expect("deadline")
        .expect("evaluation");
        assert_eq!(
            result,
            (0, Some(b"9: Bad file descriptor\nstatus=1\n".to_vec()))
        );
    }
}

#[cfg(test)]
mod capture_transport {
    use super::*;

    async fn capture(source: &str) -> (i32, Option<Vec<u8>>) {
        let env = Env::for_command();
        env.options.write().errexit = false;
        let parsed = crate::parse_posix_script(source).expect("script");
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            eval_source_stream(&parsed, &env, &EvalConfig::default(), true),
        )
        .await
        .expect("capture deadline")
        .expect("capture")
    }

    #[tokio::test]
    async fn exit_and_function_return_preserve_preceding_bytes() {
        assert_eq!(
            capture("printf before; { printf nested; exit 23; }").await,
            (23, Some(b"beforenested".to_vec()))
        );
        assert_eq!(
            capture("f() { printf body; return 19; }; f; printf after").await,
            (0, Some(b"bodyafter".to_vec()))
        );
    }

    #[tokio::test]
    async fn capture_drains_more_chunks_than_the_channel_can_hold() {
        assert_eq!(
            capture("i=0; while [ \"$i\" -lt 100 ]; do printf x; i=$((i+1)); done; exit 7").await,
            (7, Some(vec![b'x'; 100]))
        );
    }

    #[tokio::test]
    async fn conditional_and_loop_condition_output_uses_the_capture_route() {
        assert_eq!(
            capture("if printf condition; true; then printf body; fi").await,
            (0, Some(b"conditionbody".to_vec()))
        );
        assert_eq!(
            capture("i=0; while printf c; [ \"$i\" -lt 2 ]; do printf b; i=$((i+1)); done").await,
            (0, Some(b"cbcbc".to_vec()))
        );
    }
}

#[cfg(test)]
mod argument_iteration {
    use super::*;

    #[tokio::test]
    async fn implicit_for_preserves_live_empty_and_spaced_arguments() {
        let env = Env::for_command();
        let parsed = crate::parse_posix_script(
            "set -- 'a b' '' c; for item; do printf '<%s>' \"$item\"; done",
        )
        .expect("script");
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            eval_source_stream(&parsed, &env, &EvalConfig::default(), true),
        )
        .await
        .expect("loop deadline")
        .expect("loop");
        assert_eq!(result, (0, Some(b"<a b><><c>".to_vec())));
    }

    #[tokio::test]
    async fn implicit_for_snapshots_arguments_before_running_its_body() {
        let env = Env::for_command();
        let parsed = crate::parse_posix_script("set -- a b; for item; do set -- changed; printf '%s' \"$item\"; done; printf '<%s>' \"$1\"").expect("script");
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            eval_source_stream(&parsed, &env, &EvalConfig::default(), true),
        )
        .await
        .expect("loop deadline")
        .expect("loop");
        assert_eq!(result, (0, Some(b"ab<changed>".to_vec())));
    }
}
