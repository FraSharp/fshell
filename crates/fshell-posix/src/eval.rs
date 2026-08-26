// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use async_recursion::async_recursion;
use brush_parser::ast::*;
use fshell_core::Val;
use fshell_engine::{EngineError, Env, Signal};

use crate::expand::{ExpansionConfig, expand_word};
use crate::parser::ParsedScript;

/// How the POSIX evaluator was invoked.
#[derive(Debug, Clone, Default)]
pub struct EvalConfig {
    /// Positional parameters for $1, $2, ... / $@ / $#
    pub positional: Vec<String>,
    /// If true, errexit (-e) handling is active inside this eval.
    pub errexit: bool,
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

    let result = eval_program(&parsed.program, env, cfg).await;

    if let Some(saved) = saved_positional {
        restore_positional(env, saved);
    }

    match result {
        Ok(code) => {
            env.set_exit_code(code as i64);
            Ok(code)
        }
        Err(PosixError::Exit(code)) => {
            env.set_exit_code(code as i64);
            Ok(code)
        }
        Err(PosixError::Return(code)) => {
            env.set_exit_code(code as i64);
            Ok(code)
        }
        Err(PosixError::Engine(e)) => Err(e),
        Err(PosixError::Break) | Err(PosixError::Continue) => Ok(0),
    }
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

    for complete in &parsed.program.complete_commands {
        match eval_compound_list_stream(
            complete,
            env,
            cfg,
            IoStreamConfig {
                stdin_bytes: None,
                capture_stdout,
            },
        )
        .await
        {
            Ok((code, out)) => {
                last_code = code;
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

enum PosixError {
    Engine(EngineError),
    Exit(i32),
    Return(i32),
    Break,
    Continue,
}

impl From<EngineError> for PosixError {
    fn from(e: EngineError) -> Self {
        PosixError::Engine(e)
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
    io_cfg: IoStreamConfig,
) -> Result<(i32, Option<Vec<u8>>), PosixError> {
    let mut last = 0;
    let mut last_out = None;
    let mut accumulated = if io_cfg.capture_stdout {
        Some(Vec::new())
    } else {
        None
    };
    for item in &list.0 {
        let (code, out) = eval_and_or_list_stream(&item.0, env, cfg, io_cfg.clone()).await?;
        last = code;
        last_out = out.clone();
        if let (Some(acc), Some(bytes)) = (&mut accumulated, out) {
            acc.extend_from_slice(&bytes);
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
    io_cfg: IoStreamConfig,
) -> Result<(i32, Option<Vec<u8>>), PosixError> {
    let (mut code, mut out) = eval_pipeline_stream(&list.first, env, cfg, io_cfg.clone()).await?;
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

    for and_or in &list.additional {
        match and_or {
            AndOr::And(next) => {
                if code == 0 {
                    let (c, next_out) =
                        eval_pipeline_stream(next, env, cfg, io_cfg.clone()).await?;
                    code = c;
                    last_out = next_out.clone();
                    if let (Some(acc), Some(b)) = (&mut accumulated, next_out) {
                        acc.extend_from_slice(&b);
                    }
                }
            }
            AndOr::Or(next) => {
                if code != 0 {
                    let (c, next_out) =
                        eval_pipeline_stream(next, env, cfg, io_cfg.clone()).await?;
                    code = c;
                    last_out = next_out.clone();
                    if let (Some(acc), Some(b)) = (&mut accumulated, next_out) {
                        acc.extend_from_slice(&b);
                    }
                }
            }
        }
    }
    let ret_out = if io_cfg.capture_stdout {
        accumulated
    } else {
        last_out
    };
    Ok((code, ret_out))
}

#[derive(Debug, Clone, Default)]
pub struct RedirectionContext {
    pub stdin_bytes: Option<Vec<u8>>,
    pub stdin_file: Option<std::path::PathBuf>,
    pub stdout_file: Option<(std::path::PathBuf, bool)>, // (path, append)
    pub stderr_file: Option<(std::path::PathBuf, bool)>, // (path, append)
    pub stderr_to_stdout: bool,
    pub stdout_to_stderr: bool,
}

impl RedirectionContext {
    pub fn apply_item(&mut self, redir: &IoRedirect, env: &Env, positional: &[String]) {
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
                        let expanded = expand_word(
                            &w.value,
                            env,
                            &ExpansionConfig {
                                do_glob: false,
                                ..Default::default()
                            },
                            positional,
                        );
                        let filename = expanded.join(" ");
                        let path = std::path::PathBuf::from(filename);
                        match fd {
                            0 => {
                                if let Ok(bytes) = std::fs::read(&path) {
                                    self.stdin_bytes = Some(bytes);
                                }
                                self.stdin_file = Some(path);
                            }
                            1 => self.stdout_file = Some((path, append)),
                            2 => self.stderr_file = Some((path, append)),
                            _ => {}
                        }
                    }
                    IoFileRedirectTarget::Duplicate(w) => {
                        let dest =
                            expand_word(&w.value, env, &ExpansionConfig::default(), positional)
                                .join(" ");
                        if dest == "1" && fd == 2 {
                            self.stderr_to_stdout = true;
                        } else if dest == "2" && fd == 1 {
                            self.stdout_to_stderr = true;
                        }
                    }
                    IoFileRedirectTarget::Fd(target_fd) => {
                        if *target_fd == 1 && fd == 2 {
                            self.stderr_to_stdout = true;
                        } else if *target_fd == 2 && fd == 1 {
                            self.stdout_to_stderr = true;
                        }
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
                        let expanded = expand_word(
                            &format!("\"{}\"", trimmed.replace('\\', "\\\\").replace('"', "\\\"")),
                            env,
                            &ExpansionConfig {
                                do_glob: false,
                                ..Default::default()
                            },
                            positional,
                        )
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
                )
                .join(" ");
                let mut content = expanded;
                content.push('\n');
                self.stdin_bytes = Some(content.into_bytes());
            }
            IoRedirect::OutputAndError(w, append) => {
                let expanded = expand_word(
                    &w.value,
                    env,
                    &ExpansionConfig {
                        do_glob: false,
                        ..Default::default()
                    },
                    positional,
                );
                let path = std::path::PathBuf::from(expanded.join(" "));
                self.stdout_file = Some((path.clone(), *append));
                self.stderr_file = Some((path, *append));
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct IoStreamConfig {
    pub stdin_bytes: Option<Vec<u8>>,
    pub capture_stdout: bool,
}

#[async_recursion]
async fn eval_pipeline_stream(
    pipeline: &Pipeline,
    env: &Env,
    cfg: &EvalConfig,
    io_cfg: IoStreamConfig,
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
        let mut pipe_input: Option<Vec<u8>> = io_cfg.stdin_bytes;
        for (idx, cmd) in pipeline.seq.iter().enumerate() {
            let is_last = idx == pipeline.seq.len() - 1;
            let sub_env = crate::bridge::fork_env_for_subshell(env);
            let stage_io = IoStreamConfig {
                stdin_bytes: pipe_input.take(),
                capture_stdout: !is_last || io_cfg.capture_stdout,
            };
            let (code, out) = eval_command_stream(cmd, &sub_env, cfg, stage_io).await?;
            pipe_input = out.clone();
            last_code = code;
            if is_last {
                final_out = out;
            }
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
            let mut redir = RedirectionContext::default();
            if let Some(list) = redirects {
                for r in &list.0 {
                    redir.apply_item(r, env, &cfg.positional);
                }
            }
            let capture = io_cfg.capture_stdout || redir.stdout_file.is_some();
            let sub_io = IoStreamConfig {
                stdin_bytes: redir.stdin_bytes.or(io_cfg.stdin_bytes),
                capture_stdout: capture,
            };
            let (code, out) = eval_compound_command_stream(compound, env, cfg, sub_io).await?;
            if let Some((path, append)) = &redir.stdout_file
                && let Some(bytes) = &out
            {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(!*append)
                    .append(*append)
                    .open(path)
                    .map_err(|e| {
                        PosixError::Engine(EngineError::IoError {
                            message: format!("{}: {}", path.display(), e),
                            span: None,
                        })
                    })?;
                let _ = file.write_all(bytes);
            }
            let ret_out = if io_cfg.capture_stdout && redir.stdout_file.is_none() {
                out
            } else {
                None
            };
            Ok((code, ret_out))
        }
        Command::Function(func_def) => {
            let name = func_def.fname.value.clone();
            let body = func_def.body.0.clone();
            let func_name = name.clone();
            {
                let mut m = env.fns.write();
                m.insert(
                    func_name,
                    (
                        Vec::new(),
                        None,
                        vec![fshell_core::Stmt::Comment(format!("posix fn {}", name))],
                    ),
                );
            }
            register_posix_function(&name, body);
            Ok((0, None))
        }
        Command::ExtendedTest(expr_cmd, _redirects) => {
            let result = eval_extended_test(&expr_cmd.expr, env);
            Ok((if result { 0 } else { 1 }, None))
        }
    }
}

fn eval_extended_test(expr: &ExtendedTestExpr, env: &Env) -> bool {
    match expr {
        ExtendedTestExpr::And(a, b) => eval_extended_test(a, env) && eval_extended_test(b, env),
        ExtendedTestExpr::Or(a, b) => eval_extended_test(a, env) || eval_extended_test(b, env),
        ExtendedTestExpr::Not(inner) => !eval_extended_test(inner, env),
        ExtendedTestExpr::Parenthesized(inner) => eval_extended_test(inner, env),
        ExtendedTestExpr::UnaryTest(op, word) => {
            let val = expand_word(&word.value, env, &ExpansionConfig::default(), &[]).join(" ");
            eval_unary_extended(op, &val)
        }
        ExtendedTestExpr::BinaryTest(op, left, right) => {
            let lv = expand_word(&left.value, env, &ExpansionConfig::default(), &[]).join(" ");
            let rv = expand_word(&right.value, env, &ExpansionConfig::default(), &[]).join(" ");
            eval_binary_extended(op, &lv, &rv)
        }
    }
}

fn eval_unary_extended(op: &brush_parser::ast::UnaryPredicate, val: &str) -> bool {
    match op {
        brush_parser::ast::UnaryPredicate::StringHasZeroLength => val.is_empty(),
        brush_parser::ast::UnaryPredicate::StringHasNonZeroLength => !val.is_empty(),
        brush_parser::ast::UnaryPredicate::FileExists => std::path::Path::new(val).exists(),
        brush_parser::ast::UnaryPredicate::FileExistsAndIsRegularFile => {
            std::path::Path::new(val).is_file()
        }
        brush_parser::ast::UnaryPredicate::FileExistsAndIsDir => std::path::Path::new(val).is_dir(),
        brush_parser::ast::UnaryPredicate::FileExistsAndIsReadable => path_access(val, libc::R_OK),
        brush_parser::ast::UnaryPredicate::FileExistsAndIsWritable => path_access(val, libc::W_OK),
        brush_parser::ast::UnaryPredicate::FileExistsAndIsExecutable => {
            #[cfg(unix)]
            {
                std::fs::metadata(val)
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

fn eval_binary_extended(op: &brush_parser::ast::BinaryPredicate, left: &str, right: &str) -> bool {
    match op {
        brush_parser::ast::BinaryPredicate::StringExactlyMatchesString
        | brush_parser::ast::BinaryPredicate::StringExactlyMatchesPattern => left == right,
        brush_parser::ast::BinaryPredicate::StringDoesNotExactlyMatchString
        | brush_parser::ast::BinaryPredicate::StringDoesNotExactlyMatchPattern => left != right,
        brush_parser::ast::BinaryPredicate::ArithmeticEqualTo => {
            left.parse::<i64>().unwrap_or(0) == right.parse::<i64>().unwrap_or(0)
        }
        brush_parser::ast::BinaryPredicate::ArithmeticNotEqualTo => {
            left.parse::<i64>().unwrap_or(0) != right.parse::<i64>().unwrap_or(0)
        }
        brush_parser::ast::BinaryPredicate::ArithmeticLessThan => {
            left.parse::<i64>().unwrap_or(0) < right.parse::<i64>().unwrap_or(0)
        }
        brush_parser::ast::BinaryPredicate::ArithmeticLessThanOrEqualTo => {
            left.parse::<i64>().unwrap_or(0) <= right.parse::<i64>().unwrap_or(0)
        }
        brush_parser::ast::BinaryPredicate::ArithmeticGreaterThan => {
            left.parse::<i64>().unwrap_or(0) > right.parse::<i64>().unwrap_or(0)
        }
        brush_parser::ast::BinaryPredicate::ArithmeticGreaterThanOrEqualTo => {
            left.parse::<i64>().unwrap_or(0) >= right.parse::<i64>().unwrap_or(0)
        }
        brush_parser::ast::BinaryPredicate::LeftSortsBeforeRight => left < right,
        brush_parser::ast::BinaryPredicate::LeftSortsAfterRight => left > right,
        brush_parser::ast::BinaryPredicate::LeftFileIsNewerOrExistsWhenRightDoesNot => {
            file_mtime(left) > file_mtime(right)
        }
        brush_parser::ast::BinaryPredicate::LeftFileIsOlderOrDoesNotExistWhenRightDoes => {
            file_mtime(left) < file_mtime(right)
        }
        _ => false,
    }
}

fn file_mtime(path: &str) -> std::time::SystemTime {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::UNIX_EPOCH)
}

// Global registry for POSIX functions (name -> compound command)
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

static POSIX_FNS: OnceLock<Mutex<HashMap<String, CompoundCommand>>> = OnceLock::new();

fn posix_fns() -> &'static Mutex<HashMap<String, CompoundCommand>> {
    POSIX_FNS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_posix_function(name: &str, body: CompoundCommand) {
    if let Ok(mut m) = posix_fns().lock() {
        m.insert(name.to_string(), body);
    }
}

pub fn get_posix_function(name: &str) -> Option<CompoundCommand> {
    posix_fns().lock().ok().and_then(|m| m.get(name).cloned())
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
            let child = crate::bridge::fork_env_for_subshell(env);
            let (code, out) = eval_compound_list_stream(&sub.list, &child, cfg, io_cfg).await?;
            env.set_exit_code(code as i64);
            Ok((code, out))
        }
        CompoundCommand::IfClause(if_clause) => {
            let cond_code = eval_compound_list(&if_clause.condition, env, cfg).await?;
            if cond_code == 0 {
                eval_compound_list_stream(&if_clause.then, env, cfg, io_cfg).await
            } else if let Some(elses) = &if_clause.elses {
                for else_clause in elses {
                    if let Some(cond) = &else_clause.condition {
                        let c = eval_compound_list(cond, env, cfg).await?;
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
                        &ExpansionConfig::default(),
                        &cfg.positional,
                    ));
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
            for val in values {
                {
                    let mut vars = env.vars.write();
                    vars.insert(for_clause.variable_name.clone(), Val::String(val));
                }
                match eval_compound_list_stream(&for_clause.body.list, env, cfg, io_cfg.clone())
                    .await
                {
                    Ok((code, out)) => {
                        last = code;
                        if let (Some(acc), Some(bytes)) = (&mut accumulated, out) {
                            acc.extend_from_slice(&bytes);
                        }
                    }
                    Err(PosixError::Break) => break,
                    Err(PosixError::Continue) => continue,
                    Err(e) => return Err(e),
                }
            }
            Ok((last, accumulated))
        }
        CompoundCommand::WhileClause(while_clause) | CompoundCommand::UntilClause(while_clause) => {
            let is_until = matches!(compound, CompoundCommand::UntilClause(_));
            let mut last = 0;
            let mut current_io = io_cfg.clone();
            let mut accumulated = if io_cfg.capture_stdout {
                Some(Vec::new())
            } else {
                None
            };
            loop {
                let (cond_code, cond_out) =
                    eval_compound_list_stream(&while_clause.0, env, cfg, current_io.clone())
                        .await?;
                if let Some(rem) = cond_out {
                    current_io.stdin_bytes = Some(rem);
                }
                let should_continue = if is_until {
                    cond_code != 0
                } else {
                    cond_code == 0
                };
                if !should_continue {
                    break;
                }
                match eval_compound_list_stream(&while_clause.1.list, env, cfg, current_io.clone())
                    .await
                {
                    Ok((code, out)) => {
                        last = code;
                        if let (Some(acc), Some(bytes)) = (&mut accumulated, out) {
                            acc.extend_from_slice(&bytes);
                        }
                    }
                    Err(PosixError::Break) => break,
                    Err(PosixError::Continue) => continue,
                    Err(e) => return Err(e),
                }
            }
            Ok((last, accumulated))
        }
        CompoundCommand::CaseClause(case_clause) => {
            let value_expanded = expand_word(
                &case_clause.value.value,
                env,
                &ExpansionConfig::default(),
                &cfg.positional,
            )
            .join(" ");
            for item in &case_clause.cases {
                let mut matched = false;
                for pat in &item.patterns {
                    let pat_raw = &pat.value;
                    if pattern_matches(pat_raw, &value_expanded) {
                        matched = true;
                        break;
                    }
                    let expanded_pats = expand_word(
                        &pat.value,
                        env,
                        &ExpansionConfig {
                            do_glob: false,
                            ..Default::default()
                        },
                        &cfg.positional,
                    );
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
            let code = eval_arithmetic_command(&expr, env);
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

fn eval_arithmetic_command(expr: &str, env: &Env) -> i64 {
    crate::arithmetic::eval_arithmetic_expr(expr, env).unwrap_or(0)
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
                            expand_word(&word.value, env, &ExpansionConfig::default(), positional)
                                .join(" ")
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
                            do_glob: true,
                            ..Default::default()
                        },
                        positional,
                    );
                    args.extend(expanded);
                }
                CommandPrefixOrSuffixItem::AssignmentWord(assign, _) => {
                    let name = match &assign.name {
                        AssignmentName::VariableName(n) => n.clone(),
                        AssignmentName::ArrayElementName(n, idx) => format!("{}[{}]", n, idx),
                    };
                    let value = match &assign.value {
                        AssignmentValue::Scalar(word) => {
                            expand_word(&word.value, env, &ExpansionConfig::default(), positional)
                                .join(" ")
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

    let mut redir = RedirectionContext::default();
    for r in &redirects {
        redir.apply_item(r, env, positional);
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
    );
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

    let effective_stdin = redir
        .stdin_bytes
        .as_deref()
        .or(io_cfg.stdin_bytes.as_deref());
    let capture_stdout = io_cfg.capture_stdout || redir.stdout_file.is_some();

    let mut saved_prefix_vars: Vec<(String, Option<Val>)> = Vec::new();
    if !prefix_assignments.is_empty() {
        let mut vars = env.vars.write();
        for (name, value) in &prefix_assignments {
            let prev = vars.get(name).cloned();
            saved_prefix_vars.push((name.clone(), prev));
            vars.insert(name.clone(), Val::String(value.clone()));
            if let Some(Val::Map(map)) = vars.get_mut("env") {
                map.insert(ustr::ustr(name), Val::String(value.clone()));
            }
        }
    }

    let res = eval_simple_command_inner(
        &cmd_name,
        &args,
        &prefix_assignments,
        &redir,
        effective_stdin,
        capture_stdout,
        env,
        cfg,
        io_cfg.clone(),
    )
    .await;

    let is_decl = matches!(
        cmd_name.as_str(),
        "export" | "readonly" | "declare" | "typeset" | "local"
    );

    if !saved_prefix_vars.is_empty() && !is_decl {
        let mut vars = env.vars.write();
        for (name, prev) in saved_prefix_vars {
            if let Some(p) = prev {
                vars.insert(name, p);
            } else {
                vars.remove(&name);
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
    effective_stdin: Option<&[u8]>,
    capture_stdout: bool,
    env: &Env,
    cfg: &EvalConfig,
    io_cfg: IoStreamConfig,
) -> Result<(i32, Option<Vec<u8>>), PosixError> {
    match cmd_name {
        ":" | "true" => return Ok((0, None)),
        "false" => return Ok((1, None)),
        "exit" => {
            let code = args
                .first()
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(0);
            return Err(PosixError::Exit(code));
        }
        "return" => {
            let code = args
                .first()
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(0);
            return Err(PosixError::Return(code));
        }
        "break" => return Err(PosixError::Break),
        "continue" => return Err(PosixError::Continue),
        "shift" => {
            let n = args
                .first()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1);
            crate::posix_builtins::shift::shift_posix(env, n).map_err(|e| {
                PosixError::Engine(EngineError::Generic {
                    message: e,
                    span: None,
                })
            })?;
            return Ok((0, None));
        }
        "set" => {
            crate::posix_builtins::shift::set_posix(env, args).map_err(|e| {
                PosixError::Engine(EngineError::Generic {
                    message: e,
                    span: None,
                })
            })?;
            return Ok((0, None));
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
                    if let Ok(mut m) = posix_fns().lock() {
                        m.remove(&name);
                    }
                    {
                        let mut fns = env.fns.write();
                        fns.remove(&name);
                    }
                } else if unset_vars_only {
                    env.unset_var(&name);
                } else {
                    // Default: unset both var and function (bash parity)
                    env.unset_var(&name);
                    if let Ok(mut m) = posix_fns().lock() {
                        m.remove(&name);
                    }
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
                let out = write_builtin_output(&rendered, redir, io_cfg.capture_stdout)?;
                return Ok((0, out));
            }
            for arg in &exports {
                if let Some((name, value)) = arg.split_once('=') {
                    let expanded_val =
                        expand_word(value, env, &ExpansionConfig::default(), &cfg.positional)
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
            let stdin_str = effective_stdin.map(|b| String::from_utf8_lossy(b).into_owned());
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
            let rendered = crate::posix_builtins::printf::format_printf(
                args.first().map(|s| s.as_str()).unwrap_or(""),
                if args.len() > 1 { &args[1..] } else { &[] },
            )
            .map_err(|e| {
                PosixError::Engine(EngineError::Generic {
                    message: e,
                    span: None,
                })
            })?;
            let out = write_builtin_output(&rendered, redir, io_cfg.capture_stdout)?;
            return Ok((0, out));
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
            let out = write_builtin_output(&rendered, redir, io_cfg.capture_stdout)?;
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
            let code = eval_test_args(test_args, env);
            return Ok((if code { 0 } else { 1 }, None));
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
            let out = write_builtin_output(&text, redir, io_cfg.capture_stdout)?;
            return Ok((0, out));
        }
        "cd" => {
            let target = args.first().map(|s| s.as_str()).unwrap_or("");
            let path = if target.is_empty() {
                std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
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
            let out = write_builtin_output(&text, redir, io_cfg.capture_stdout)?;
            return Ok((0, out));
        }
        "exec" => {
            if args.is_empty() {
                return Ok((0, None));
            }
            let mut cmd = std::process::Command::new(&args[0]);
            if args.len() > 1 {
                cmd.args(&args[1..]);
            }
            let status = cmd.status().map(|s| s.code().unwrap_or(127)).unwrap_or(127);
            return Err(PosixError::Exit(status));
        }
        "trap" => {
            let rendered = handle_posix_trap(args, env)?;
            let out = write_builtin_output(&rendered, redir, capture_stdout)?;
            return Ok((0, out));
        }
        "wait" => {
            let code = handle_posix_wait(args, env).await?;
            return Ok((code, None));
        }
        "umask" | "alias" | "unalias" | "ulimit" | "times" | "jobs" => {
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
        "fg" | "bg" => return Ok((1, None)),
        "dot" | "." | "source" => {
            if let Some(path) = args.first() {
                let content = std::fs::read_to_string(path).map_err(|e| {
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
    if let Some(func_body) = get_posix_function(cmd_name) {
        let saved = save_positional(env);
        apply_positional(env, args);
        let fn_cfg = EvalConfig {
            positional: args.to_vec(),
            errexit: cfg.errexit,
        };
        let (code, out) = eval_compound_command_stream(&func_body, env, &fn_cfg, io_cfg).await?;
        restore_positional(env, saved);
        return Ok((code, out));
    }

    // Fallback: subprocess execution with full I/O piping and redirections
    run_external_command(
        cmd_name,
        args,
        prefix_assignments,
        redir,
        effective_stdin,
        capture_stdout,
        env,
    )
    .await
}

fn handle_posix_trap(args: &[String], env: &Env) -> Result<String, PosixError> {
    if args.is_empty() {
        let traps = env.posix_traps.read();
        let mut out = String::new();
        for (sig, handler) in traps.iter() {
            if handler.is_empty() {
                out.push_str(&format!("trap -- '' {}\n", sig.to_str()));
            } else {
                let esc = handler.replace('\'', "'\\''");
                out.push_str(&format!("trap -- '{esc}' {}\n", sig.to_str()));
            }
        }
        return Ok(out);
    }
    if args.len() == 1 && args[0] == "-p" {
        let traps = env.posix_traps.read();
        let mut out = String::new();
        for (sig, handler) in traps.iter() {
            if handler.is_empty() {
                out.push_str(&format!("trap -- '' {}\n", sig.to_str()));
            } else {
                let esc = handler.replace('\'', "'\\''");
                out.push_str(&format!("trap -- '{esc}' {}\n", sig.to_str()));
            }
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
    if signals.is_empty() {
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
                last_code = *env.prompt.last_exit_code.read() as i32;
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
                last_code = *env.prompt.last_exit_code.read() as i32;
            } else {
                // No job, try direct waitpid (may be external child)
                let mut status = 0;
                let res = unsafe { libc::waitpid(pid, &mut status, 0) };
                if res <= 0 {
                    return Err(PosixError::Engine(EngineError::Generic {
                        message: format!("wait: pid {pid} is not a child of this shell"),
                        span: None,
                    }));
                }
                let code = if (status & 0x7f) == 0 {
                    (status >> 8) & 0xff
                } else {
                    128 + (status & 0x7f)
                };
                last_code = code;
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

fn write_builtin_output(
    rendered: &str,
    redir: &RedirectionContext,
    capture_stdout: bool,
) -> Result<Option<Vec<u8>>, PosixError> {
    if let Some((path, append)) = &redir.stdout_file {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(!*append)
            .append(*append)
            .open(path)
            .map_err(|e| {
                PosixError::Engine(EngineError::IoError {
                    message: format!("redirect error: {}: {}", path.display(), e),
                    span: None,
                })
            })?;
        file.write_all(rendered.as_bytes()).map_err(|e| {
            PosixError::Engine(EngineError::IoError {
                message: format!("redirect write error: {}: {}", path.display(), e),
                span: None,
            })
        })?;
        if capture_stdout {
            Ok(Some(Vec::new()))
        } else {
            Ok(None)
        }
    } else if capture_stdout {
        Ok(Some(rendered.as_bytes().to_vec()))
    } else {
        print!("{}", rendered);
        use std::io::Write;
        let _ = std::io::stdout().flush();
        Ok(None)
    }
}

async fn run_external_command(
    cmd_name: &str,
    args: &[String],
    prefix_assignments: &[(String, String)],
    redir: &RedirectionContext,
    effective_stdin: Option<&[u8]>,
    capture_stdout: bool,
    env: &Env,
) -> Result<(i32, Option<Vec<u8>>), PosixError> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    let mut cmd = tokio::process::Command::new(cmd_name);
    cmd.args(args);

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
    } else if effective_stdin.is_some() {
        cmd.stdin(Stdio::piped());
    }

    if let Some((stdout_file, append)) = &redir.stdout_file {
        let f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(!*append)
            .append(*append)
            .open(stdout_file)
            .map_err(|e| {
                PosixError::Engine(EngineError::IoError {
                    message: format!("{}: {}", stdout_file.display(), e),
                    span: None,
                })
            })?;
        cmd.stdout(Stdio::from(f));
    } else if capture_stdout {
        cmd.stdout(Stdio::piped());
    }

    if let Some((stderr_file, append)) = &redir.stderr_file {
        let f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(!*append)
            .append(*append)
            .open(stderr_file)
            .map_err(|e| {
                PosixError::Engine(EngineError::IoError {
                    message: format!("{}: {}", stderr_file.display(), e),
                    span: None,
                })
            })?;
        cmd.stderr(Stdio::from(f));
    }

    let mut child = cmd.spawn().map_err(|e| {
        PosixError::Engine(EngineError::IoError {
            message: format!("{}: {}", cmd_name, e),
            span: None,
        })
    })?;

    if let Some(bytes) = effective_stdin
        && let Some(mut stdin) = child.stdin.take()
    {
        let b = bytes.to_vec();
        tokio::spawn(async move {
            let _ = stdin.write_all(&b).await;
        });
    }

    let output = child.wait_with_output().await.map_err(|e| {
        PosixError::Engine(EngineError::IoError {
            message: format!("{}: {}", cmd_name, e),
            span: None,
        })
    })?;

    let code = output.status.code().unwrap_or(127);
    env.set_exit_code(code as i64);

    if capture_stdout && redir.stdout_file.is_none() {
        Ok((code, Some(output.stdout)))
    } else {
        Ok((code, None))
    }
}

fn eval_test_args(args: &[String], env: &Env) -> bool {
    let clean_args: &[String] = if let Some(last) = args.last()
        && last == "]"
    {
        &args[..args.len() - 1]
    } else {
        args
    };

    match clean_args.len() {
        0 => false,
        1 => !clean_args[0].is_empty(),
        2 => {
            if clean_args[0] == "!" {
                clean_args[1].is_empty()
            } else {
                eval_unary_primary(&clean_args[0], &clean_args[1])
            }
        }
        3 => {
            if is_binary_primary(&clean_args[1]) {
                eval_binary_primary(&clean_args[0], &clean_args[1], &clean_args[2])
            } else if clean_args[0] == "!" {
                !eval_test_args(&clean_args[1..], env)
            } else if clean_args[0] == "(" && clean_args[2] == ")" {
                !clean_args[1].is_empty()
            } else {
                false
            }
        }
        4 => {
            if clean_args[0] == "!" {
                !eval_test_args(&clean_args[1..], env)
            } else if clean_args[0] == "(" && clean_args[3] == ")" {
                eval_test_args(&clean_args[1..3], env)
            } else {
                match brush_parser::test_command::parse(clean_args) {
                    Ok(expr) => crate::posix_builtins::test_builtin::eval_test_expr(&expr, env),
                    Err(_) => false,
                }
            }
        }
        _ => match brush_parser::test_command::parse(clean_args) {
            Ok(expr) => crate::posix_builtins::test_builtin::eval_test_expr(&expr, env),
            Err(_) => false,
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

fn path_access(path: &str, mode: libc::c_int) -> bool {
    let c_path = match std::ffi::CString::new(path) {
        Ok(p) => p,
        Err(_) => return false,
    };
    // SAFETY: c_path is a valid null-terminated string; access only queries permissions.
    unsafe { libc::access(c_path.as_ptr(), mode) == 0 }
}

fn eval_unary_primary(op: &str, val: &str) -> bool {
    match op {
        "-n" => !val.is_empty(),
        "-z" => val.is_empty(),
        "-e" | "-a" => std::path::Path::new(val).exists(),
        "-f" => std::path::Path::new(val).is_file(),
        "-d" => std::path::Path::new(val).is_dir(),
        "-r" => path_access(val, libc::R_OK),
        "-w" => path_access(val, libc::W_OK),
        "-x" => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::metadata(val)
                    .map(|m| m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
            }
            #[cfg(not(unix))]
            {
                false
            }
        }
        "-s" => std::fs::metadata(val).map(|m| m.len() > 0).unwrap_or(false),
        "-h" | "-L" => std::path::Path::new(val).is_symlink(),
        _ => false,
    }
}

fn eval_binary_primary(left: &str, op: &str, right: &str) -> bool {
    match op {
        "=" | "==" => left == right,
        "!=" => left != right,
        "<" => left < right,
        ">" => left > right,
        "-eq" => parse_test_int(left) == parse_test_int(right),
        "-ne" => parse_test_int(left) != parse_test_int(right),
        "-lt" => parse_test_int(left) < parse_test_int(right),
        "-le" => parse_test_int(left) <= parse_test_int(right),
        "-gt" => parse_test_int(left) > parse_test_int(right),
        "-ge" => parse_test_int(left) >= parse_test_int(right),
        _ => false,
    }
}

fn parse_test_int(s: &str) -> i64 {
    s.trim().parse::<i64>().unwrap_or(0)
}
