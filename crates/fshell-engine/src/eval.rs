// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::profiler::{ProfilerCategory, ProfilerState};
use crate::{
    BuiltinHandler, EngineError, Env, IS_TRUSTED_CONTEXT, PipelinePayload, ReactiveEvent,
    collect_pipeline, dispatch_on_signal, format_pipeline, spawn_pipeline_stream,
};
use fshell_core::diagnostic::FshDiag;
use fshell_core::{
    BinOp, Expr, FxIndexMap, ParamModifier, Pipeline, PipelineStage, ProcessSubstDirection, Stmt,
    TimeUnit, Val,
};

struct ErrexitRestoreGuard<'a> {
    env: &'a Env,
    old_errexit: bool,
}

impl<'a> ErrexitRestoreGuard<'a> {
    fn new(env: &'a Env) -> Result<Self, EngineError> {
        let old_errexit = {
            let opts = env.options.read();
            opts.errexit
        };
        if old_errexit {
            let mut opts = env.options.write();
            opts.errexit = false;
        }
        Ok(Self { env, old_errexit })
    }
}

impl<'a> Drop for ErrexitRestoreGuard<'a> {
    fn drop(&mut self) {
        if self.old_errexit {
            self.env.options.write().errexit = true;
        }
    }
}

fn stmt_label(stmt: &Stmt) -> String {
    match stmt {
        Stmt::Local { name, .. } => format!("local {name}"),
        Stmt::Let { name, .. } => format!("let {name}"),
        Stmt::Assign { name, .. } => format!("assign {name}"),
        Stmt::FnDef { name, .. } => format!("fn {name}"),
        Stmt::Update { name, .. } => format!("update {name}"),
        Stmt::Source { .. } => "source ...".into(),
        Stmt::PosixBlock { .. } => "sh { ... }".into(),
        Stmt::Expr(expr) => expr_label(expr),
        Stmt::Comment(text) => {
            let trimmed = text.trim();
            if trimmed.len() > 40 {
                format!("# {:.40}...", trimmed)
            } else {
                format!("# {trimmed}")
            }
        }
        Stmt::While { .. } => "while ...".into(),
        Stmt::For { var, .. } => format!("for {var}"),
        Stmt::ReactiveCell { name, .. } => format!("cell {name}"),
        Stmt::ReactiveCellEvery { name, .. } => format!("cell {name} every"),
        Stmt::Every { .. } => "every ...".into(),
        Stmt::On { signal, .. } => format!("on {signal}"),
        Stmt::Match { .. } => "match".into(),
        Stmt::TryCatch { .. } => "try".into(),
        Stmt::WithCaps { .. } => "with caps".into(),
        Stmt::Unsafe { .. } => "unsafe".into(),
        Stmt::Return(_) => "return".into(),
        Stmt::Exit(_) => "exit".into(),
        Stmt::Break => "break".into(),
        Stmt::Continue => "continue".into(),
        Stmt::And(_, _) => "&&".into(),
        Stmt::Or(_, _) => "||".into(),
        Stmt::Background(_) => "background".into(),
        Stmt::Spanned { stmt, .. } => stmt_label(stmt),
    }
}

fn expr_label(expr: &Expr) -> String {
    let inner = match expr {
        Expr::Spanned { expr: e, .. } => e.as_ref(),
        other => other,
    };
    match inner {
        Expr::Pipeline(p) => p
            .stages
            .first()
            .map(|s| match s {
                PipelineStage::CommandCall { name, .. } => format!("{name} ..."),
                PipelineStage::Grep { .. } => "grep ...".into(),
                PipelineStage::Count => "count".into(),
                PipelineStage::Filter { .. } => "filter ...".into(),
                PipelineStage::Map { .. } => "map ...".into(),
                PipelineStage::Sort { .. } => "sort ...".into(),
                PipelineStage::Limit { .. } => "limit ...".into(),
                PipelineStage::BoundaryOperator { format } => format!("@{format:?}"),
                PipelineStage::Traverse { .. } => "traverse ...".into(),
                PipelineStage::Write { .. } => "write ...".into(),
                PipelineStage::Read { .. } => "read ...".into(),
                PipelineStage::Heredoc { .. } => "heredoc".into(),
                PipelineStage::HereString { .. } => "herestring".into(),
                PipelineStage::FdRedirect { .. } => "fdredirect".into(),
                PipelineStage::Hash { .. } => "hash".into(),
                PipelineStage::Mark { .. } => "mark ...".into(),
            })
            .unwrap_or_else(|| "pipeline".into()),
        Expr::Variable(name) => format!("${name}"),
        Expr::Ident(name) => name.clone(),
        Expr::BinaryOp { op, .. } => format!("binop {op:?}"),
        Expr::Not(_) => "!expr".into(),
        Expr::MemberAccess { member, .. } => format!(".{member}"),
        Expr::If { .. } => "if".into(),
        Expr::List(_) => "list".into(),
        Expr::Map(_) => "map".into(),
        Expr::String(parts) => {
            let s: String = parts
                .iter()
                .map(|p| match p {
                    fshell_core::StringPart::Lit(s) => s.as_str(),
                    fshell_core::StringPart::Expr(_) => "{...}",
                })
                .collect();
            if s.len() > 40 {
                format!("\"{:.40}...\"", s)
            } else {
                format!("\"{s}\"")
            }
        }
        Expr::Int(n) => format!("{n}"),
        Expr::Float(n) => format!("{n}"),
        Expr::Bool(b) => format!("{b}"),
        Expr::Null => "null".into(),
        Expr::VarWithModifier { name, .. } => format!("${{{name}::}}"),
        Expr::ProcessSubst { direction, .. } => match direction {
            fshell_core::ProcessSubstDirection::Input => "<(...)".into(),
            fshell_core::ProcessSubstDirection::Output => ">(...)".into(),
        },
        Expr::ArithmeticExpansion(_) => "$((...))".into(),
        Expr::AnsiCQuote(s) => {
            if s.len() > 30 {
                format!("$'{:.30}...'", s)
            } else {
                format!("$'{s}'")
            }
        }
        Expr::RawMultiLineString(_) => "'''...'''".into(),
        Expr::MultiLineString { .. } => "\"\"\"...\"\"\"".into(),
        Expr::InlinePipeline(_) => "$| ... |".into(),
        Expr::Spanned { expr: e, .. } => expr_label(e),
    }
}

fn extract_setopt_args(stmt: &Stmt) -> Option<(String, Vec<String>)> {
    let inner_stmt = match stmt {
        Stmt::Spanned { stmt, .. } => stmt.as_ref(),
        other => other,
    };
    let expr = match inner_stmt {
        Stmt::Expr(e) => e,
        _ => return None,
    };
    let inner_expr = match expr {
        Expr::Spanned { expr: e, .. } => e.as_ref(),
        other => other,
    };
    let pipeline = match inner_expr {
        Expr::Pipeline(p) => p,
        _ => return None,
    };
    if pipeline.stages.len() != 1 {
        return None;
    }
    match &pipeline.stages[0] {
        PipelineStage::CommandCall { name, args, .. } => {
            if name != "setopt" && name != "unsetopt" {
                return None;
            }
            let arg_strs: Vec<String> = args
                .iter()
                .filter_map(|a| match a {
                    Expr::String(parts) if parts.len() == 1 => {
                        if let fshell_core::StringPart::Lit(s) = &parts[0] {
                            Some(s.clone())
                        } else {
                            None
                        }
                    }
                    Expr::Ident(s) => Some(s.clone()),
                    _ => None,
                })
                .collect();
            if arg_strs.is_empty() {
                return None;
            }
            Some((name.clone(), arg_strs))
        }
        _ => None,
    }
}

fn setopt_stmt(cmd_name: &str, args: Vec<String>) -> Stmt {
    let expr_args: Vec<Expr> = args.into_iter().map(Expr::Ident).collect();
    let stage = PipelineStage::CommandCall {
        name: cmd_name.to_string(),
        args: expr_args,
        env: Vec::new(),
    };
    let pipeline = Pipeline {
        stages: vec![stage],
    };
    Stmt::Expr(Expr::Pipeline(pipeline))
}

fn merge_adjacent_setopt(stmts: &mut Vec<Stmt>) {
    let mut result: Vec<Stmt> = Vec::with_capacity(stmts.len());
    let mut setopt_args: Vec<String> = Vec::new();
    let mut unsetopt_args: Vec<String> = Vec::new();
    let mut flushing = false;

    let drain =
        |setopt_args: &mut Vec<String>, unsetopt_args: &mut Vec<String>, result: &mut Vec<Stmt>| {
            if !setopt_args.is_empty() {
                result.push(setopt_stmt("setopt", std::mem::take(setopt_args)));
            }
            if !unsetopt_args.is_empty() {
                result.push(setopt_stmt("unsetopt", std::mem::take(unsetopt_args)));
            }
        };

    for stmt in std::mem::take(stmts) {
        if let Some((cmd, args)) = extract_setopt_args(&stmt) {
            if cmd == "setopt" {
                setopt_args.extend(args);
            } else {
                unsetopt_args.extend(args);
            }
            flushing = true;
        } else {
            if flushing {
                drain(&mut setopt_args, &mut unsetopt_args, &mut result);
                flushing = false;
            }
            result.push(stmt);
        }
    }
    if flushing {
        drain(&mut setopt_args, &mut unsetopt_args, &mut result);
    }
    *stmts = result;
}

async fn try_run_startup_builtin(
    pipeline: &Pipeline,
    env: &Env,
) -> Option<Result<(), EngineError>> {
    if !env
        .is_loading_init_script
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return None;
    }

    let stage = pipeline.stages.first()?;
    let PipelineStage::CommandCall {
        name,
        args: raw_args,
        env: inline_env,
    } = stage
    else {
        return None;
    };

    if !inline_env.is_empty() {
        return None;
    }

    let handler = env.get_builtin(name)?;

    let mut evaluated_args = Vec::with_capacity(raw_args.len());
    for arg in raw_args {
        evaluated_args.push(eval_expr(arg, env).await.ok()?);
    }

    let (out_tx, _out_rx) = tokio::sync::mpsc::channel(1);

    match handler(None, evaluated_args, env, out_tx) {
        Ok(()) => {
            let last_ec = *Some(env.prompt.last_exit_code.read())?;
            {
                let mut vars = Some(env.vars.write())?;
                vars.insert("?".to_string(), Val::Int(last_ec));
            }
            let errexit = Some(env.options.read())?.errexit;
            if errexit && last_ec != 0 {
                return Some(Err(EngineError::ExitSignal(last_ec as i32)));
            }
            Some(Ok(()))
        }
        Err(e) => {
            let msg = e.to_string();
            {
                let mut ec = env.prompt.last_exit_code.write();
                *ec = 1;
            }
            {
                let mut vars = env.vars.write();
                vars.insert("?".to_string(), Val::Int(1));
            }
            Some(Err(EngineError::PipelineError {
                message: msg,
                span: None,
            }))
        }
    }
}

use fshell_hash::FxHashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::watch;
use ustr::ustr;

pub(crate) fn val_to_bool(val: &Val) -> Result<bool, EngineError> {
    match val {
        Val::Bool(b) => Ok(*b),
        Val::List(list) if list.len() == 1 => val_to_bool(&list[0]),
        other => Err(EngineError::TypeMismatch {
            expected: "Bool".to_string(),
            found: format!("{:?}", other),
            span: None,
        }),
    }
}

macro_rules! eval_sync_val {
    ($expr:expr, $env:expr) => {
        match try_eval_sync($expr, $env)? {
            Ok(v) => v,
            Err(e) => return Some(Err(e)),
        }
    };
}

macro_rules! eval_async_val {
    ($expr:expr, $env:expr) => {
        eval_expr($expr, $env).await?
    };
}

macro_rules! interp_string {
    ($parts:expr, $env:expr, $eval:tt) => {{
        let mut res = String::new();
        for part in $parts {
            match part {
                fshell_core::StringPart::Lit(s) => res.push_str(s),
                fshell_core::StringPart::Expr(e) => {
                    let val = $eval!(e, $env);
                    match val {
                        Val::String(s) => res.push_str(&s),
                        other => res.push_str(&other.to_text()),
                    }
                }
            }
        }
        res
    }};
}

macro_rules! build_list {
    ($exprs:expr, $env:expr, $eval:tt) => {{
        let mut vals = Vec::new();
        for e in $exprs {
            vals.push($eval!(e, $env));
        }
        vals
    }};
}

macro_rules! build_map {
    ($pairs:expr, $env:expr, $eval:tt) => {{
        let mut map = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        for (k, v) in $pairs {
            map.insert(ustr(k), $eval!(v, $env));
        }
        map
    }};
}

pub(crate) fn try_eval_sync(expr: &Expr, env: &Env) -> Option<Result<Val, EngineError>> {
    match expr {
        Expr::Spanned { expr: inner, span } => {
            let res = try_eval_sync(inner, env)?;
            Some(res.map_err(|mut err| {
                if err.span().is_none() {
                    err.set_span(*span);
                }
                err
            }))
        }
        Expr::Null => Some(Ok(Val::Null)),
        Expr::Bool(b) => Some(Ok(Val::Bool(*b))),
        Expr::Int(i) => Some(Ok(Val::Int(*i))),
        Expr::Float(f) => Some(Ok(Val::Float(*f))),
        Expr::Ident(name) => Some(Ok(resolve_ident_value(name, env))),
        Expr::String(parts) => {
            let res = interp_string!(parts, env, eval_sync_val);
            Some(Ok(Val::String(res)))
        }
        Expr::List(exprs) => {
            let mut vals = Vec::new();
            for e in exprs {
                vals.push(eval_sync_val!(e, env));
            }
            Some(Ok(Val::List(vals)))
        }
        Expr::Map(pairs) => {
            let mut map = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            for (k, v) in pairs {
                map.insert(ustr(k), eval_sync_val!(v, env));
            }
            Some(Ok(Val::Map(map)))
        }
        Expr::Variable(name) => {
            if env.reactive.has_cells.load(Ordering::Relaxed)
                && env.reactive.cells.read().contains_key(name)
            {
                return None;
            }
            if let Some(ref locals) = env.local_vars
                && let Some(val) = locals.read().get(name)
            {
                return Some(Ok(val.clone()));
            }
            Some(lookup_variable_fallback(name, env))
        }
        Expr::BinaryOp { op, lhs, rhs } => {
            if *op == BinOp::And || *op == BinOp::Or {
                let l = eval_sync_val!(lhs, env);
                let b = match val_to_bool(&l) {
                    Ok(b) => b,
                    Err(e) => return Some(Err(e)),
                };
                if *op == BinOp::And && !b {
                    return Some(Ok(Val::Bool(false)));
                }
                if *op == BinOp::Or && b {
                    return Some(Ok(Val::Bool(true)));
                }
                let r = eval_sync_val!(rhs, env);
                return Some(eval_binop(*op, l, r));
            }
            let l = eval_sync_val!(lhs, env);
            let r = eval_sync_val!(rhs, env);
            Some(eval_binop(*op, l, r))
        }
        Expr::Not(inner) => {
            let old_errexit = env.options.read().errexit;
            if old_errexit {
                return None;
            }
            let v = eval_sync_val!(inner, env);
            let b = match val_to_bool(&v) {
                Ok(b) => b,
                Err(e) => return Some(Err(e)),
            };
            Some(Ok(Val::Bool(!b)))
        }
        Expr::MemberAccess { expr, member } => {
            let val = eval_sync_val!(expr, env);
            Some(member_access_dispatch(val, member))
        }
        Expr::VarWithModifier { name, modifier } => {
            let val = eval_sync_val!(&Expr::Variable(name.clone()), env);
            try_apply_modifier_sync(name, val, modifier, env)
        }
        Expr::AnsiCQuote(s) => Some(Ok(Val::String(parse_ansi_c_quote(s)))),
        Expr::RawMultiLineString(s) => Some(Ok(Val::String(s.clone()))),
        Expr::MultiLineString { parts, .. } => {
            let res = interp_string!(parts, env, eval_sync_val);
            Some(Ok(Val::String(res)))
        }
        _ => None,
    }
}

/// Apply a parameter expansion modifier (sync version for try_eval_sync).
fn apply_pure_modifier(s: &str, modifier: &ParamModifier) -> Option<Result<Val, EngineError>> {
    let result = match modifier {
        ParamModifier::Tail => std::path::Path::new(s)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default(),
        ParamModifier::Head => std::path::Path::new(s)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string()),
        ParamModifier::Root => std::path::Path::new(s)
            .with_extension("")
            .to_string_lossy()
            .to_string(),
        ParamModifier::Ext => std::path::Path::new(s)
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default(),
        ParamModifier::Upper => s.to_uppercase(),
        ParamModifier::Lower => s.to_lowercase(),
        ParamModifier::StringLength => {
            return Some(Ok(Val::Int(s.chars().count() as i64)));
        }
        ParamModifier::Substring { offset, length } => {
            let chars: Vec<char> = s.chars().collect();
            let start = if *offset < 0 {
                (chars.len() as i64 + offset).max(0) as usize
            } else {
                (*offset).max(0) as usize
            };
            let start = start.min(chars.len());
            match length {
                Some(len) => chars[start..(start + *len as usize).min(chars.len())]
                    .iter()
                    .collect(),
                None => chars[start..].iter().collect(),
            }
        }
        _ => return None,
    };
    Some(Ok(Val::String(result)))
}

macro_rules! eval_sync {
    ($expr:expr, $env:expr) => {
        match try_eval_sync($expr, $env)? {
            Ok(v) => v.to_text(),
            Err(e) => return Some(Err(e)),
        }
    };
}

macro_rules! eval_async {
    ($expr:expr, $env:expr) => {
        eval_expr($expr, $env).await?.to_text()
    };
}

macro_rules! bail_sync {
    ($msg:expr) => {
        return Some(Err(EngineError::from($msg)))
    };
}

macro_rules! bail_async {
    ($msg:expr) => {
        return Err(EngineError::from($msg))
    };
}

macro_rules! modifier_arms {
    ($s:ident, $val:ident, $modifier:ident, $name:ident, $env:ident, $eval:tt, $bail:tt) => {
        match $modifier {
            ParamModifier::Default(default_expr) => {
                if $s.is_empty() || matches!($val, Val::Null) {
                    $eval!(default_expr, $env)
                } else {
                    $s
                }
            }
            ParamModifier::AssignDefault(default_expr) => {
                if $s.is_empty() || matches!($val, Val::Null) {
                    let text = $eval!(default_expr, $env);
                    {
                        let mut vars = $env.vars.write();
                        vars.insert($name.to_string(), Val::String(text.clone()));
                    }
                    text
                } else {
                    $s
                }
            }
            ParamModifier::ErrorIfUnset(msg_expr) => {
                if $s.is_empty() || matches!($val, Val::Null) {
                    let msg = $eval!(msg_expr, $env);
                    $bail!(format!("${{{}}}: {}", $name, msg));
                }
                $s
            }
            ParamModifier::Alternate(alt_expr) => {
                if !$s.is_empty() && !matches!($val, Val::Null) {
                    $eval!(alt_expr, $env)
                } else {
                    String::new()
                }
            }
            ParamModifier::ShortestPrefix(pattern) => {
                let pat = $eval!(pattern, $env);
                trim_shortest_prefix(&$s, &pat)
            }
            ParamModifier::LongestPrefix(pattern) => {
                let pat = $eval!(pattern, $env);
                trim_longest_prefix(&$s, &pat)
            }
            ParamModifier::ShortestSuffix(pattern) => {
                let pat = $eval!(pattern, $env);
                trim_shortest_suffix(&$s, &pat)
            }
            ParamModifier::LongestSuffix(pattern) => {
                let pat = $eval!(pattern, $env);
                trim_longest_suffix(&$s, &pat)
            }
            ParamModifier::Replace {
                pattern,
                replacement,
                global,
            } => {
                let pat = $eval!(pattern, $env);
                let repl = $eval!(replacement, $env);
                if *global {
                    $s.replace(&pat, &repl)
                } else {
                    $s.replacen(&pat, &repl, 1)
                }
            }
            _ => unreachable!("pure modifier should have been handled"),
        }
    };
}

fn try_apply_modifier_sync(
    name: &str,
    val: Val,
    modifier: &ParamModifier,
    env: &Env,
) -> Option<Result<Val, EngineError>> {
    let s = val.to_text();
    if let Some(pure) = apply_pure_modifier(&s, modifier) {
        return Some(pure);
    }
    let result = modifier_arms!(s, val, modifier, name, env, eval_sync, bail_sync);
    Some(Ok(Val::String(result)))
}

/// Apply a parameter expansion modifier (async version for eval_expr).
async fn apply_modifier_async(
    name: &str,
    val: Val,
    modifier: &ParamModifier,
    env: &Env,
) -> Result<Val, EngineError> {
    let s = val.to_text();
    if let Some(pure) = apply_pure_modifier(&s, modifier) {
        return pure;
    }
    let result = modifier_arms!(s, val, modifier, name, env, eval_async, bail_async);
    Ok(Val::String(result))
}

fn trim_shortest_prefix(s: &str, pattern: &str) -> String {
    if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
        for i in 0..=s.len() {
            if globset::Glob::new(pattern)
                .map(|g| g.compile_matcher().is_match(&s[..i]))
                .unwrap_or(false)
            {
                return s[i..].to_string();
            }
        }
        s.to_string()
    } else {
        s.strip_prefix(pattern).unwrap_or(s).to_string()
    }
}

fn trim_longest_prefix(s: &str, pattern: &str) -> String {
    if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
        let mut result = s.to_string();
        for i in (0..=s.len()).rev() {
            if globset::Glob::new(pattern)
                .map(|g| g.compile_matcher().is_match(&s[..i]))
                .unwrap_or(false)
            {
                result = s[i..].to_string();
                break;
            }
        }
        result
    } else {
        s.strip_prefix(pattern).unwrap_or(s).to_string()
    }
}

fn trim_shortest_suffix(s: &str, pattern: &str) -> String {
    if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
        for i in (0..=s.len()).rev() {
            if globset::Glob::new(pattern)
                .map(|g| g.compile_matcher().is_match(&s[i..]))
                .unwrap_or(false)
            {
                return s[..i].to_string();
            }
        }
        s.to_string()
    } else {
        s.strip_suffix(pattern).unwrap_or(s).to_string()
    }
}

fn trim_longest_suffix(s: &str, pattern: &str) -> String {
    if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
        let mut result = s.to_string();
        for i in 0..=s.len() {
            if globset::Glob::new(pattern)
                .map(|g| g.compile_matcher().is_match(&s[i..]))
                .unwrap_or(false)
            {
                result = s[..i].to_string();
                break;
            }
        }
        result
    } else {
        s.strip_suffix(pattern).unwrap_or(s).to_string()
    }
}

fn resolve_ident_value(name: &str, env: &Env) -> Val {
    if let Some(ref locals) = env.local_vars
        && let Some(val) = locals.read().get(name)
    {
        return val.clone();
    }
    let vars = env.vars.read();
    if let Some(val) = vars.get(name) {
        return val.clone();
    }
    if let Some(Val::Map(env_map)) = vars.get("env")
        && let Some(val) = env_map.get(&ustr(name))
    {
        return val.clone();
    }
    Val::String(name.to_string())
}

fn lookup_variable_fallback(name: &str, env: &Env) -> Result<Val, EngineError> {
    if name == "?" || name == "status" {
        let code = env
            .vars
            .read()
            .get("?")
            .cloned()
            .map(|v| match v {
                Val::Int(i) => i,
                other => other.to_text().parse::<i64>().unwrap_or(0),
            })
            .unwrap_or_else(|| *env.prompt.last_exit_code.read());
        return Ok(Val::Int(code));
    }
    if name == "env" {
        env.ensure_env_populated();
    }
    if let Some(val) = env.special_vars.resolve(name) {
        return Ok(val);
    }
    let vars = env.vars.read();
    if let Some(val) = vars.get(name) {
        return Ok(val.clone());
    }
    if let Some(Val::Map(env_map)) = vars.get("env")
        && let Some(val) = env_map.get(&ustr(name))
    {
        return Ok(val.clone());
    }
    let nounset = env.options.read().nounset;
    if nounset {
        Err(EngineError::VariableNotFound {
            name: name.to_string(),
            span: None,
        })
    } else {
        Ok(Val::Null)
    }
}

fn member_access_dispatch(val: Val, member: &str) -> Result<Val, EngineError> {
    let mut base = val;
    if let Val::List(ref mut list) = base
        && list.len() == 1
    {
        base = match list.pop() {
            Some(v) => v,
            None => {
                return Err(EngineError::from(
                    "internal error: expected list with 1 element",
                ));
            }
        };
    }
    match base {
        Val::Map(map) => map
            .get(&ustr(member))
            .cloned()
            .ok_or_else(|| EngineError::from(format!("Map has no field '{member}'"))),
        Val::ObjectGraph { root, graph } => {
            if let Some(node_data) = graph.nodes.get(&root) {
                if let Some(prop_val) = node_data.properties.get(&ustr(member)) {
                    return Ok(prop_val.clone());
                }
                if let Some(edges) = graph.edges.get(&root)
                    && let Some(edge) = edges.iter().find(|e| e.label == ustr(member))
                {
                    return Ok(Val::ObjectGraph {
                        root: edge.target,
                        graph: graph.clone(),
                    });
                }
                Err(EngineError::from(format!(
                    "Node {root} has no property or outgoing edge with label '{member}'"
                )))
            } else {
                Err(EngineError::from(format!("Invalid root node ID: {root}")))
            }
        }
        Val::String(s) if s == "process" && member == "spawn" => {
            Ok(Val::Capability(fshell_core::ResourceHandle::ProcessSpawn))
        }
        Val::String(s) if s == "net" && member == "all" => {
            Ok(Val::Capability(fshell_core::ResourceHandle::NetworkAll))
        }
        _ => Err(EngineError::from(
            "Member access is only supported on Map, ObjectGraph, or capability modules",
        )),
    }
}

/// Core expression evaluator.
pub fn eval_expr<'a>(
    expr: &'a Expr,
    env: &'a Env,
) -> Pin<Box<dyn Future<Output = Result<Val, EngineError>> + Send + 'a>> {
    if let Some(res) = try_eval_sync(expr, env) {
        return Box::pin(async move { res });
    }
    Box::pin(async move {
        match expr {
            Expr::Spanned { expr: inner, span } => {
                let res = eval_expr(inner, env).await;
                res.map_err(|mut err| {
                    if err.span().is_none() {
                        err.set_span(*span);
                    }
                    err
                })
            }
            Expr::Null => Ok(Val::Null),
            Expr::Bool(b) => Ok(Val::Bool(*b)),
            Expr::Int(i) => Ok(Val::Int(*i)),
            Expr::Float(f) => Ok(Val::Float(*f)),
            Expr::Ident(name) => Ok(resolve_ident_value(name, env)),
            Expr::String(parts) => {
                let res = interp_string!(parts, env, eval_async_val);
                Ok(Val::String(res))
            }
            Expr::List(exprs) => {
                let vals = build_list!(exprs, env, eval_async_val);
                Ok(Val::List(vals))
            }
            Expr::Map(pairs) => {
                let map = build_map!(pairs, env, eval_async_val);
                Ok(Val::Map(map))
            }
            Expr::Variable(name) => {
                if let Some(ref locals) = env.local_vars
                    && let Some(val) = locals.read().get(name)
                {
                    return Ok(val.clone());
                }
                let mut found_cell = None;
                if env
                    .reactive
                    .has_cells
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    let cells = lock_reactive!(env.reactive.cells.read());
                    if let Some(rx) = cells.get(name) {
                        env.track_cell(name.clone());
                        found_cell = Some(Val::List((**rx.borrow()).clone()));
                    }
                }
                if let Some(val) = found_cell {
                    Ok(val)
                } else {
                    lookup_variable_fallback(name, env)
                }
            }
            // Unary boolean negation `!expr`
            Expr::Not(inner) => {
                let _guard = ErrexitRestoreGuard::new(env)?;
                let v = eval_async_val!(inner, env);
                let b = val_to_bool(&v)?;
                Ok(Val::Bool(!b))
            }
            Expr::BinaryOp { op, lhs, rhs } => {
                if *op == BinOp::And || *op == BinOp::Or {
                    let l = eval_async_val!(lhs, env);
                    let b = val_to_bool(&l)?;
                    if *op == BinOp::And && !b {
                        return Ok(Val::Bool(false));
                    }
                    if *op == BinOp::Or && b {
                        return Ok(Val::Bool(true));
                    }
                    let r = eval_async_val!(rhs, env);
                    return eval_binop(*op, l, r);
                }
                let l = eval_async_val!(lhs, env);
                let r = eval_async_val!(rhs, env);
                eval_binop(*op, l, r)
            }
            Expr::MemberAccess { expr, member } => {
                let val = eval_async_val!(expr, env);
                member_access_dispatch(val, member)
            }
            Expr::Pipeline(pipeline) => {
                let results = collect_pipeline(pipeline, env).await?;
                Ok(Val::List(results))
            }
            Expr::InlinePipeline(pipeline) => {
                let results = collect_pipeline(pipeline, env).await?;
                Ok(Val::List(results))
            }
            Expr::VarWithModifier { name, modifier } => {
                let val = eval_expr(&Expr::Variable(name.clone()), env).await?;
                apply_modifier_async(name, val, modifier, env).await
            }
            Expr::ProcessSubst {
                direction,
                pipeline,
            } => match direction {
                ProcessSubstDirection::Input => {
                    let tmp = tempfile::NamedTempFile::new()
                        .map_err(|e| EngineError::from(format!("process substitution: {}", e)))?;
                    let path = tmp.path().to_path_buf();
                    let results = collect_pipeline(pipeline, env).await?;
                    let mut out = String::new();
                    for r in results {
                        out.push_str(&r.to_text());
                        out.push('\n');
                    }
                    std::fs::write(&path, &out)
                        .map_err(|e| EngineError::from(format!("process substitution: {}", e)))?;
                    let temp_path = tmp.into_temp_path();
                    if let Ok(mut tf) = env.temp_files.lock() {
                        tf.push(temp_path);
                    }
                    Ok(Val::String(path.to_string_lossy().to_string()))
                }
                ProcessSubstDirection::Output => {
                    let tmp = tempfile::NamedTempFile::new()
                        .map_err(|e| EngineError::from(format!("process substitution: {}", e)))?;
                    let path = tmp.path().to_path_buf();
                    let temp_path = tmp.into_temp_path();
                    let path_clone = path.clone();
                    let env_clone = env.clone();
                    let mut pipeline_clone = pipeline.clone();
                    pipeline_clone.stages.insert(
                        0,
                        PipelineStage::Read {
                            path: Expr::String(vec![fshell_core::StringPart::Lit(
                                path_clone.to_string_lossy().to_string(),
                            )]),
                        },
                    );

                    tokio::task::spawn(async move {
                        let (out_tx, _out_rx) = tokio::sync::mpsc::channel(100);
                        let _ = crate::execute_pipeline(&pipeline_clone, &env_clone, out_tx).await;
                    });

                    if let Ok(mut tf) = env.temp_files.lock() {
                        tf.push(temp_path);
                    }
                    Ok(Val::String(path.to_string_lossy().to_string()))
                }
            },
            Expr::If {
                condition,
                then_body,
                else_body,
            } => {
                let old_errexit = {
                    let opts = env.options.read();
                    opts.errexit
                };
                if old_errexit {
                    let mut opts = env.options.write();
                    opts.errexit = false;
                }
                let cond_res = eval_expr(condition, env).await;
                if old_errexit {
                    let mut opts = env.options.write();
                    opts.errexit = true;
                }
                let cond_val = cond_res?;
                let is_truthy = val_to_bool(&cond_val)?;
                let body = if is_truthy {
                    then_body
                } else if let Some(else_body) = else_body {
                    else_body
                } else {
                    return Ok(Val::Null);
                };
                let mut result = Val::Null;
                for stmt in body {
                    match stmt.unpack() {
                        Stmt::Expr(e) => {
                            result = eval_expr(e, env).await?;
                        }
                        other => {
                            eval_stmt(other, env, false).await?;
                        }
                    }
                }
                Ok(result)
            }

            Expr::AnsiCQuote(s) => Ok(Val::String(parse_ansi_c_quote(s))),
            Expr::RawMultiLineString(s) => Ok(Val::String(s.clone())),
            Expr::MultiLineString { parts, .. } => {
                let res = interp_string!(parts, env, eval_async_val);
                Ok(Val::String(res))
            }
            Expr::ArithmeticExpansion(inner) => {
                let val = eval_async_val!(inner, env);
                match &val {
                    Val::Int(_) | Val::Float(_) => Ok(val),
                    Val::String(s) => {
                        if let Ok(i) = s.trim().parse::<i64>() {
                            Ok(Val::Int(i))
                        } else if let Ok(f) = s.trim().parse::<f64>() {
                            Ok(Val::Float(f))
                        } else {
                            Err(EngineError::from(format!(
                                "arithmetic expansion: cannot coerce '{}' to number",
                                s
                            )))
                        }
                    }
                    _ => Err(EngineError::from(format!(
                        "arithmetic expansion: expected number, got {:?}",
                        val
                    ))),
                }
            }
        }
    })
}

pub(crate) fn parse_ansi_c_quote(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some('\'') => out.push('\''),
                Some('\"') => out.push('\"'),
                Some('0') => out.push('\0'),
                Some('x') => {
                    let mut hex = String::new();
                    for _ in 0..2 {
                        if let Some(h) = chars.next() {
                            hex.push(h);
                        }
                    }
                    if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                        out.push(byte as char);
                    }
                }
                Some(c @ '0'..='7') => {
                    let mut oct = String::new();
                    oct.push(c);
                    for _ in 0..2 {
                        if let Some(o) = chars.next() {
                            oct.push(o);
                        }
                    }
                    if let Ok(byte) = u8::from_str_radix(&oct, 8) {
                        out.push(byte as char);
                    }
                }
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub(crate) fn matches_pattern(val: &Val, pattern: &fshell_core::MatchPattern) -> bool {
    match pattern {
        fshell_core::MatchPattern::Wildcard => true,
        fshell_core::MatchPattern::Literal(lit) => match lit {
            fshell_core::LiteralPattern::Null => matches!(val, Val::Null),
            fshell_core::LiteralPattern::Bool(b) => matches!(val, Val::Bool(v) if v == b),
            fshell_core::LiteralPattern::Int(i) => matches!(val, Val::Int(v) if v == i),
            fshell_core::LiteralPattern::Float(f) => matches!(val, Val::Float(v) if v == f),
            fshell_core::LiteralPattern::String(s) => matches!(val, Val::String(v) if v == s),
        },
        fshell_core::MatchPattern::Map { fields, rest } => match val {
            Val::Map(map) => {
                for (field_name, field_pat) in fields {
                    match map.get(&ustr::ustr(field_name)) {
                        Some(field_val) => {
                            if !matches_pattern(field_val, field_pat) {
                                return false;
                            }
                        }
                        None => return false,
                    }
                }
                if !rest && fields.len() != map.len() {
                    return false;
                }
                true
            }
            _ => false,
        },
    }
}

pub(crate) fn eval_binop(op: BinOp, l: Val, r: Val) -> Result<Val, EngineError> {
    // Helper: coerce Int to Float when one side is Float.
    fn to_float(v: &Val) -> Option<f64> {
        match v {
            Val::Float(f) => Some(*f),
            Val::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    match op {
        BinOp::Add => match (l, r) {
            (Val::Int(a), Val::Int(b)) => a
                .checked_add(b)
                .map(Val::Int)
                .ok_or_else(|| EngineError::from("Integer overflow in addition")),
            (Val::Float(a), Val::Float(b)) => Ok(Val::Float(a + b)),
            // Mixed Int+Float: promote Int to Float
            (Val::Int(a), Val::Float(b)) => Ok(Val::Float(a as f64 + b)),
            (Val::Float(a), Val::Int(b)) => Ok(Val::Float(a + b as f64)),
            (Val::String(a), Val::String(b)) => Ok(Val::String(a + &b)),
            other => Err(EngineError::TypeMismatch {
                expected: "Int, Float, or String".to_string(),
                found: format!("{:?}", other),
                span: None,
            }),
        },
        BinOp::Sub => match (l, r) {
            (Val::Int(a), Val::Int(b)) => a
                .checked_sub(b)
                .map(Val::Int)
                .ok_or_else(|| EngineError::from("Integer overflow in subtraction")),
            (Val::Float(a), Val::Float(b)) => Ok(Val::Float(a - b)),
            (Val::Int(a), Val::Float(b)) => Ok(Val::Float(a as f64 - b)),
            (Val::Float(a), Val::Int(b)) => Ok(Val::Float(a - b as f64)),
            other => Err(EngineError::TypeMismatch {
                expected: "Int or Float".to_string(),
                found: format!("{:?}", other),
                span: None,
            }),
        },
        BinOp::Mul => match (l, r) {
            (Val::Int(a), Val::Int(b)) => a
                .checked_mul(b)
                .map(Val::Int)
                .ok_or_else(|| EngineError::from("Integer overflow in multiplication")),
            (Val::Float(a), Val::Float(b)) => Ok(Val::Float(a * b)),
            (Val::Int(a), Val::Float(b)) => Ok(Val::Float(a as f64 * b)),
            (Val::Float(a), Val::Int(b)) => Ok(Val::Float(a * b as f64)),
            other => Err(EngineError::TypeMismatch {
                expected: "Int or Float".to_string(),
                found: format!("{:?}", other),
                span: None,
            }),
        },
        BinOp::Div => match (l, r) {
            (Val::Int(a), Val::Int(b)) => {
                if b == 0 {
                    Err(EngineError::DivisionByZero { span: None })
                } else {
                    a.checked_div(b)
                        .map(Val::Int)
                        .ok_or_else(|| EngineError::from("Integer overflow in division"))
                }
            }
            (Val::Float(a), Val::Float(b)) => {
                if b == 0.0 {
                    Err(EngineError::DivisionByZero { span: None })
                } else {
                    Ok(Val::Float(a / b))
                }
            }
            (Val::Int(a), Val::Float(b)) => {
                if b == 0.0 {
                    Err(EngineError::DivisionByZero { span: None })
                } else {
                    Ok(Val::Float(a as f64 / b))
                }
            }
            (Val::Float(a), Val::Int(b)) => {
                if b == 0 {
                    Err(EngineError::DivisionByZero { span: None })
                } else {
                    Ok(Val::Float(a / b as f64))
                }
            }
            other => Err(EngineError::TypeMismatch {
                expected: "Int or Float".to_string(),
                found: format!("{:?}", other),
                span: None,
            }),
        },
        BinOp::Eq => Ok(Val::Bool(l == r)),
        BinOp::Neq => Ok(Val::Bool(l != r)),
        BinOp::Lt => {
            if let (Some(a), Some(b)) = (to_float(&l), to_float(&r)) {
                return Ok(Val::Bool(a < b));
            }
            match (&l, &r) {
                (Val::String(a), Val::String(b)) => Ok(Val::Bool(a < b)),
                _ => Err(EngineError::TypeMismatch {
                    expected: "Comparable types (Int, Float, or String)".to_string(),
                    found: format!("{:?}, {:?}", l, r),
                    span: None,
                }),
            }
        }
        BinOp::Lte => {
            if let (Some(a), Some(b)) = (to_float(&l), to_float(&r)) {
                return Ok(Val::Bool(a <= b));
            }
            match (&l, &r) {
                (Val::String(a), Val::String(b)) => Ok(Val::Bool(a <= b)),
                _ => Err(EngineError::TypeMismatch {
                    expected: "Comparable types (Int, Float, or String)".to_string(),
                    found: format!("{:?}, {:?}", l, r),
                    span: None,
                }),
            }
        }
        BinOp::Gt => {
            if let (Some(a), Some(b)) = (to_float(&l), to_float(&r)) {
                return Ok(Val::Bool(a > b));
            }
            match (&l, &r) {
                (Val::String(a), Val::String(b)) => Ok(Val::Bool(a > b)),
                _ => Err(EngineError::TypeMismatch {
                    expected: "Comparable types (Int, Float, or String)".to_string(),
                    found: format!("{:?}, {:?}", l, r),
                    span: None,
                }),
            }
        }
        BinOp::Gte => {
            if let (Some(a), Some(b)) = (to_float(&l), to_float(&r)) {
                return Ok(Val::Bool(a >= b));
            }
            match (&l, &r) {
                (Val::String(a), Val::String(b)) => Ok(Val::Bool(a >= b)),
                _ => Err(EngineError::TypeMismatch {
                    expected: "Comparable types (Int, Float, or String)".to_string(),
                    found: format!("{:?}, {:?}", l, r),
                    span: None,
                }),
            }
        }
        BinOp::ReMatch => {
            let lhs_str = l.to_text();
            let pattern_str = r.to_text();
            match regex::Regex::new(&pattern_str) {
                Ok(re) => Ok(Val::Bool(re.is_match(&lhs_str))),
                Err(e) => Err(EngineError::from(format!("Invalid regex pattern: {}", e))),
            }
        }
        BinOp::And => {
            let a = val_to_bool(&l)?;
            let b = val_to_bool(&r)?;
            Ok(Val::Bool(a && b))
        }
        BinOp::Or => {
            let a = val_to_bool(&l)?;
            let b = val_to_bool(&r)?;
            Ok(Val::Bool(a || b))
        }
    }
}

/// Check for SIGINT at loop boundaries.
pub(crate) fn check_sigint(env: &Env) -> Result<(), EngineError> {
    if env.job_control.sigint_pending.load(Ordering::Acquire) {
        env.job_control
            .sigint_pending
            .store(false, Ordering::SeqCst);
        Err(EngineError::from("Interrupted by Ctrl+C"))
    } else {
        Ok(())
    }
}

/// Raw-mode aware Ctrl-C check — handles kitty keyboard protocol (`CSI 99;5u` / `CSI 99:5u`)
/// and plain `0x03` when the terminal is in raw mode with `DISAMBIGUATE_ESCAPE_CODES`.
/// Falls back to `sigint_pending` when not in raw mode.
/// Also handles direct `0x03` and the raw escape sequence via `libc::read` as a
/// fallback when `crossterm` is not decoding the kitty sequence (seen as
/// `^[[99;5u` being echoed).
fn poll_crossterm_ctrl_c() -> bool {
    // First, try the normal SIGINT flag (cooked mode).
    // This is checked in `check_sigint_or_ctrl_c`, but we keep a fast path here
    // for when crossterm poll is not needed.
    // Then try crossterm's decoded events.
    if crossterm::terminal::is_raw_mode_enabled().unwrap_or(false)
        && let Ok(true) = crossterm::event::poll(std::time::Duration::from_millis(0))
        && let Ok(crossterm::event::Event::Key(key)) = crossterm::event::read()
        && key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL)
        && matches!(
            key.code,
            crossterm::event::KeyCode::Char('c') | crossterm::event::KeyCode::Char('C')
        )
    {
        return true;
    }
    if crossterm::terminal::is_raw_mode_enabled().unwrap_or(false) {
        // Fallback: raw bytes on stdin (handles `0x03` and kitty `ESC[99;5u` not decoded).
        // Use `poll` + `read` on fd 0 with O_NONBLOCK to avoid hanging.
        unsafe {
            let mut pfd = libc::pollfd {
                fd: 0,
                events: libc::POLLIN,
                revents: 0,
            };
            if libc::poll(&mut pfd, 1, 0) > 0 && (pfd.revents & libc::POLLIN) != 0 {
                let mut buf = [0u8; 32];
                let n = libc::read(0, buf.as_mut_ptr() as *mut libc::c_void, buf.len());
                if n > 0 {
                    let slice = &buf[..n as usize];
                    // Plain Ctrl-C
                    if slice.contains(&3) {
                        return true;
                    }
                    // Kitty: ESC [ 9 9 ; 5 u  or ESC [ 9 9 : 5 u  (and variants)
                    // Look for the byte sequence that `^[[99;5u` represents.
                    // `^[[` is ESC '['
                    if slice.windows(2).any(|w| w == [27, b'[']) {
                        // If we see CSI and 'u' terminator, and it contains "99" and "5", treat as Ctrl-C.
                        // This is broad but safe for the `every` interrupt path.
                        let s = String::from_utf8_lossy(slice);
                        if s.contains("99") && s.contains('5') && s.contains('u') {
                            return true;
                        }
                    }
                }
            }
        }
        return false;
    }
    false
}

async fn check_sigint_or_ctrl_c(env: &Env) -> bool {
    if env.job_control.sigint_pending.load(Ordering::Acquire) {
        env.job_control
            .sigint_pending
            .store(false, Ordering::SeqCst);
        return true;
    }
    // Check raw-mode Ctrl-C via crossterm without blocking the async runtime.
    let is_ctrl_c = tokio::task::spawn_blocking(poll_crossterm_ctrl_c)
        .await
        .unwrap_or(false);
    if is_ctrl_c {
        // Mirror the SIGINT path for other waiters.
        env.job_control.sigint_pending.store(true, Ordering::SeqCst);
        env.job_control
            .sigint_pending
            .store(false, Ordering::SeqCst);
        return true;
    }
    false
}

/// Evaluate a list of statements as a loop body iteration.
/// Returns `Ok(true)` to continue the outer loop, `Ok(false)` to break,
/// or `Err(e)` to propagate.
pub(crate) async fn eval_loop_body(body: &[Stmt], env: &Env) -> Result<bool, EngineError> {
    check_sigint(env)?;
    for stmt in body {
        let res = if let Some(r) = try_eval_stmt_sync(stmt, env, false) {
            r
        } else {
            eval_stmt(stmt, env, false).await
        };
        match res {
            Ok(()) => {}
            Err(EngineError::BreakSignal) => {
                check_sigint(env)?;
                return Ok(false);
            }
            Err(EngineError::ContinueSignal) => return Ok(true),
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

pub(crate) fn try_eval_stmt_sync(
    stmt: &Stmt,
    env: &Env,
    _unsafe_context: bool,
) -> Option<Result<(), EngineError>> {
    match stmt {
        Stmt::Spanned { stmt: inner, span } => {
            let res = try_eval_stmt_sync(inner, env, _unsafe_context)?;
            Some(res.map_err(|mut err| {
                if err.span().is_none() {
                    err.set_span(*span);
                }
                err
            }))
        }
        Stmt::Local { name, expr } => {
            let val = if let Some(expr) = expr {
                let val_res = try_eval_sync(expr, env)?;
                match val_res {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                }
            } else {
                Val::Null
            };
            if let Some(ref locals) = env.local_vars {
                locals.write().insert(name.clone(), val);
            } else {
                env.vars.write().insert(name.clone(), val);
            }
            Some(Ok(()))
        }
        Stmt::Let { name, expr } => {
            let val_res = try_eval_sync(expr, env)?;
            let val = match val_res {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            if let Some(ref locals) = env.local_vars {
                let mut map = locals.write();
                if map.contains_key(name) {
                    map.insert(name.clone(), val);
                    return Some(Ok(()));
                }
            }
            env.vars.write().insert(name.clone(), val);
            Some(Ok(()))
        }
        Stmt::Assign { name, expr } => {
            let val_res = try_eval_sync(expr, env)?;
            let val = match val_res {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            if let Some(ref locals) = env.local_vars {
                let mut map = locals.write();
                if let Some(entry) = map.get_mut(name) {
                    *entry = val;
                    return Some(Ok(()));
                }
            }
            let mut vars = env.vars.write();
            if let Some(entry) = vars.get_mut(name) {
                *entry = val;
                Some(Ok(()))
            } else {
                Some(Err(EngineError::VariableNotFound {
                    name: name.clone(),
                    span: None,
                }))
            }
        }
        Stmt::Update { name, op, expr } => {
            let val_res = try_eval_sync(expr, env)?;
            let val = match val_res {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            if let Some(ref locals) = env.local_vars {
                let mut map = locals.write();
                if let Some(entry) = map.get_mut(name) {
                    let new_val = match eval_binop(*op, entry.clone(), val) {
                        Ok(nv) => nv,
                        Err(e) => return Some(Err(e)),
                    };
                    *entry = new_val;
                    return Some(Ok(()));
                }
            }
            let mut vars = env.vars.write();
            if let Some(entry) = vars.get_mut(name) {
                let new_val = match eval_binop(*op, entry.clone(), val) {
                    Ok(nv) => nv,
                    Err(e) => return Some(Err(e)),
                };
                *entry = new_val;
                Some(Ok(()))
            } else {
                Some(Err(EngineError::VariableNotFound {
                    name: name.clone(),
                    span: None,
                }))
            }
        }
        Stmt::Break => Some(Err(EngineError::BreakSignal)),
        Stmt::Continue => Some(Err(EngineError::ContinueSignal)),
        Stmt::Return(expr) => {
            let val_res = try_eval_sync(expr, env)?;
            let val = match val_res {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            Some(Err(EngineError::ReturnSignal(val)))
        }
        Stmt::Exit(expr_opt) => {
            let code = if let Some(expr) = expr_opt {
                let val_res = try_eval_sync(expr, env)?;
                let val = match val_res {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                };
                match val {
                    Val::Int(i) => i as i32,
                    Val::Float(f) => f as i32,
                    other => {
                        return Some(Err(EngineError::TypeMismatch {
                            expected: "Int or Float".to_string(),
                            found: format!("{:?}", other),
                            span: None,
                        }));
                    }
                }
            } else {
                0
            };
            {
                let mut ec = env.prompt.last_exit_code.write();
                *ec = code as i64;
            }
            Some(Err(EngineError::ExitSignal(code)))
        }
        Stmt::Expr(expr) => {
            if matches!(expr.unpack(), Expr::Pipeline(_) | Expr::InlinePipeline(_)) {
                None
            } else {
                let val_res = try_eval_sync(expr, env)?;
                let val = match val_res {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                };
                let exit_code = match &val {
                    Val::Bool(false) => 1,
                    _ => 0,
                };
                env.vars
                    .write()
                    .insert("?".to_string(), Val::Int(exit_code));
                *env.prompt.last_exit_code.write() = exit_code;
                let errexit_enabled = env.options.read().errexit;
                if errexit_enabled && exit_code != 0 {
                    return Some(Err(EngineError::ExitSignal(exit_code as i32)));
                }
                Some(Ok(()))
            }
        }
        Stmt::Comment(_) => Some(Ok(())),
        _ => None,
    }
}

/// Write a streaming pipeline value to the real stdout, preserving binary
/// Blob bytes verbatim (no coercion, no added newline) and honoring the
/// `echo -n` NUL sentinel (strip it and skip the trailing newline).
pub(crate) fn write_val_stdout(v: &Val) {
    use std::io::Write;
    let out = std::io::stdout();
    let mut h = out.lock();
    match v {
        Val::Blob(b) => {
            let _ = h.write_all(b);
        }
        Val::String(s) => {
            if s.starts_with('\0') {
                return;
            }
            if s.ends_with('\0') {
                let _ = write!(h, "{}", &s[..s.len() - 1]);
            } else {
                let _ = writeln!(h, "{s}");
            }
        }
        other => {
            let _ = writeln!(h, "{}", other.to_text());
        }
    }
    let _ = h.flush();
}

/// Core statement evaluator.
pub fn eval_stmt<'a>(
    stmt: &'a Stmt,
    env: &'a Env,
    unsafe_context: bool,
) -> Pin<Box<dyn Future<Output = Result<(), EngineError>> + Send + 'a>> {
    if let Some(res) = try_eval_stmt_sync(stmt, env, unsafe_context) {
        return Box::pin(async move { res });
    }
    Box::pin(async move {
        match stmt {
            Stmt::Spanned { stmt: inner, span } => {
                let res = eval_stmt(inner, env, unsafe_context).await;
                res.map_err(|mut err| {
                    if err.span().is_none() {
                        err.set_span(*span);
                    }
                    err
                })
            }
            Stmt::Local { name, expr } => {
                let val = if let Some(expr) = expr {
                    eval_expr(expr, env).await?
                } else {
                    Val::Null
                };
                if let Some(ref locals) = env.local_vars {
                    let mut map = locals.write();
                    map.insert(name.clone(), val);
                } else {
                    let mut vars = env.vars.write();
                    vars.insert(name.clone(), val);
                }
                Ok(())
            }
            Stmt::Let { name, expr } => {
                let val = eval_expr(expr, env).await?;
                if let Some(ref locals) = env.local_vars {
                    let mut map = locals.write();
                    if map.contains_key(name) {
                        map.insert(name.clone(), val);
                        return Ok(());
                    }
                }
                env.vars.write().insert(name.clone(), val);
                Ok(())
            }
            Stmt::Assign { name, expr } => {
                let val = eval_expr(expr, env).await?;
                if let Some(ref locals) = env.local_vars {
                    let mut map = locals.write();
                    if let Some(entry) = map.get_mut(name) {
                        *entry = val;
                        return Ok(());
                    }
                }
                let mut vars = env.vars.write();
                if let Some(entry) = vars.get_mut(name) {
                    *entry = val;
                    Ok(())
                } else {
                    Err(EngineError::VariableNotFound {
                        name: name.clone(),
                        span: None,
                    })
                }
            }
            Stmt::Update { name, op, expr } => {
                let val = eval_expr(expr, env).await?;
                if let Some(ref locals) = env.local_vars {
                    let mut map = locals.write();
                    if let Some(entry) = map.get_mut(name) {
                        let new_val = eval_binop(*op, entry.clone(), val)?;
                        *entry = new_val;
                        return Ok(());
                    }
                }
                let mut vars = env.vars.write();
                if let Some(entry) = vars.get_mut(name) {
                    let new_val = eval_binop(*op, entry.clone(), val)?;
                    *entry = new_val;
                    Ok(())
                } else {
                    Err(EngineError::VariableNotFound {
                        name: name.clone(),
                        span: None,
                    })
                }
            }
            Stmt::FnDef {
                name,
                params,
                ret_type,
                body,
            } => {
                let mut fns = env.fns.write();
                fns.insert(
                    name.clone(),
                    (params.clone(), ret_type.clone(), body.clone()),
                );
                Ok(())
            }
            Stmt::And(a, b) => {
                let _guard = ErrexitRestoreGuard::new(env)?;
                let res = eval_stmt(a, env, unsafe_context).await;
                res?;
                let last_ec = *env.prompt.last_exit_code.read();
                if last_ec == 0 {
                    eval_stmt(b, env, unsafe_context).await
                } else {
                    Ok(())
                }
            }
            Stmt::Or(a, b) => {
                let _guard = ErrexitRestoreGuard::new(env)?;
                let res = eval_stmt(a, env, unsafe_context).await;
                match res {
                    Ok(()) => {
                        let last_ec = *env.prompt.last_exit_code.read();
                        if last_ec != 0 {
                            eval_stmt(b, env, unsafe_context).await
                        } else {
                            Ok(())
                        }
                    }
                    Err(e) => {
                        if matches!(
                            e,
                            EngineError::ExitSignal(_)
                                | EngineError::BreakSignal
                                | EngineError::ContinueSignal
                                | EngineError::ReturnSignal(_)
                        ) {
                            Err(e)
                        } else {
                            eval_stmt(b, env, unsafe_context).await
                        }
                    }
                }
            }
            Stmt::Comment(_) => Ok(()),
            Stmt::Expr(expr) => {
                if let Expr::Pipeline(pipeline) | Expr::InlinePipeline(pipeline) = expr.unpack() {
                    if let Some(result) = try_run_startup_builtin(pipeline, env).await {
                        return result;
                    }

                    // Reset the status register so the finalizer below reads this
                    // pipeline's own exit status, not a stale value left by a
                    // prior (possibly errexit-aborted) statement.
                    {
                        let mut ec = env.prompt.last_exit_code.write();
                        *ec = 0;
                    }

                    // Pipeline expressions must print output (like run_script_stmt does),
                    // not capture it. collect_pipeline would set is_captured=true and
                    // silently discard output.
                    let mut rx = spawn_pipeline_stream(pipeline, env);
                    let mut errors: Vec<String> = Vec::new();
                    while let Some(payload) = rx.recv().await {
                        match payload {
                            PipelinePayload::Data(v) => {
                                crate::eval::write_val_stdout(&v);
                            }
                            PipelinePayload::Bytes(b) => {
                                use std::io::Write;
                                let _ = std::io::stdout().write_all(&b);
                            }
                            PipelinePayload::Structured(d) => {
                                if crate::is_condition_false_diag(&d) {
                                    // Logical false: exit 1 but don't print an error line.
                                    // Track separately so finalizer can distinguish logical
                                    // failure from hard errors without string-matching.
                                    errors.push("__condition_false__".to_string());
                                } else {
                                    errors.push(d.report.to_string());
                                }
                            }
                        }
                    }
                    let pipefail = env.options.read().pipefail;
                    let last_ec = *env.prompt.last_exit_code.read();
                    let (exit_code, maybe_err) =
                        crate::pipeline_finalize(errors, last_ec, pipefail);
                    env.set_exit_code(exit_code);
                    if env.options.read().errexit && exit_code != 0 {
                        return Err(EngineError::ExitSignal(exit_code as i32));
                    }
                    if let Some(e) = maybe_err {
                        return Err(e);
                    }
                } else {
                    let val = eval_expr(expr, env).await?;
                    let exit_code = match &val {
                        Val::Bool(false) => 1,
                        _ => 0,
                    };
                    env.set_exit_code(exit_code);
                    let errexit_enabled = env.options.read().errexit;
                    if errexit_enabled {
                        let last_ec = *env.prompt.last_exit_code.read();
                        if last_ec != 0 {
                            return Err(EngineError::ExitSignal(last_ec as i32));
                        }
                    }
                }
                Ok(())
            }
            Stmt::TryCatch {
                try_body,
                catch_var,
                catch_body,
            } => {
                let mut caught_err: Option<EngineError> = None;
                for s in try_body {
                    if let Err(e) = eval_stmt(s, env, false).await {
                        // Control-flow signals propagate through try/catch
                        match &e {
                            EngineError::BreakSignal
                            | EngineError::ContinueSignal
                            | EngineError::ReturnSignal(_)
                            | EngineError::ExitSignal(_) => return Err(e),
                            _ => {}
                        }
                        caught_err = Some(e);
                        break;
                    }
                }
                if let Some(e) = caught_err {
                    let diag = FshDiag::new(e);
                    env.set_last_error(diag.clone());
                    let err_val = diag.to_val();
                    {
                        let mut vars = env.vars.write();
                        vars.insert(catch_var.clone(), err_val);
                    }
                    for s in catch_body {
                        eval_stmt(s, env, false).await?;
                    }
                }
                Ok(())
            }
            Stmt::WithCaps { caps, body } => {
                // Temporarily grant capabilities for this execution scope
                let old_caps = env.caps.caps.read().clone();
                for cap_expr in caps {
                    let val = eval_expr(cap_expr, env).await?;
                    // Map Val to ResourceHandle and grant it
                    match val {
                        Val::Capability(handle) => {
                            let mut caps = lock_caps!(env.caps.caps.write());
                            caps.grant(handle);
                        }
                        Val::List(list) => {
                            for item in list {
                                if let Val::Capability(handle) = item {
                                    let mut caps = lock_caps!(env.caps.caps.write());
                                    caps.grant(handle);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                let mut res = Ok(());
                for s in body {
                    if let Err(e) = eval_stmt(s, env, false).await {
                        res = Err(e);
                        break;
                    }
                }
                // Restore previous capabilities registry
                let mut caps_w = env.caps.caps.write();
                *caps_w = old_caps;
                res
            }
            Stmt::ReactiveCell { name, pipeline } => {
                // Reject mutations unless in unsafe context
                if !unsafe_context {
                    let reg = env.builtins.read();
                    for stage in &pipeline.stages {
                        if !is_query_stage(stage, &reg) {
                            let name = match stage {
                                PipelineStage::CommandCall { name, .. } => name.clone(),
                                _ => "unknown".to_string(),
                            };
                            return Err(EngineError::MutationNotAllowed {
                                message: format!(
                                    "Mutation '{}' not allowed in reactive cell. Wrap in 'unsafe {{ ... }}' to allow.",
                                    name
                                ),
                                span: None,
                            });
                        }
                    }
                }

                // Set up reactive watch channel
                let (tx, rx) = watch::channel(Arc::new(Vec::new()));
                {
                    let mut cells = lock_reactive!(env.reactive.cells.write());
                    cells.insert(name.clone(), rx);
                    env.reactive
                        .has_cells
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }

                // Populate reactive pipelines
                {
                    let mut pipes = env.reactive.pipelines.write();
                    pipes.insert(name.clone(), format_pipeline(pipeline));
                }

                // Send registration event to global scheduler
                let _ = env
                    .reactive
                    .tx
                    .send(ReactiveEvent::RegisterCell {
                        name: name.clone(),
                        pipeline: pipeline.clone(),
                        tx,
                    })
                    .await;

                Ok(())
            }
            Stmt::Match { expr, arms } => {
                let val = eval_expr(expr, env).await?;
                let mut matched = false;
                for arm in arms {
                    if matches_pattern(&val, &arm.pattern) {
                        for s in &arm.body {
                            eval_stmt(s, env, false).await?;
                        }
                        matched = true;
                        break;
                    }
                }
                if !matched {
                    return Err(EngineError::MatchNonExhaustive { span: None });
                }
                Ok(())
            }
            Stmt::Source { path, bash } => {
                let path_val = eval_expr(path, env).await?;
                let path_str = match path_val {
                    Val::String(s) => s,
                    other => {
                        return Err(EngineError::TypeMismatch {
                            expected: "String".to_string(),
                            found: format!("{:?}", other),
                            span: None,
                        });
                    }
                };
                // Expand tilde in path
                let path_str = if path_str == "~" {
                    std::env::var("HOME").unwrap_or(path_str)
                } else if let Some(rest) = path_str.strip_prefix("~/") {
                    let home = std::env::var("HOME").unwrap_or_default();
                    if home.is_empty() {
                        path_str
                    } else {
                        format!("{}/{}", home, rest)
                    }
                } else {
                    path_str
                };
                // POSIX fast-path: if the file looks like a POSIX script (shebang or bash constructs),
                // try fshell-posix engine first when available (registered via posix handler).
                let use_posix_engine = !*bash
                    && std::fs::read_to_string(&path_str)
                        .map(|c| crate::login::looks_like_posix(&c))
                        .unwrap_or(false);
                if use_posix_engine && let Some(handler) = crate::posix_handler() {
                    let content =
                        std::fs::read_to_string(&path_str).map_err(|e| EngineError::IoError {
                            message: format!("Failed to read {:?}: {}", path_str, e),
                            span: None,
                        })?;
                    match handler(content.clone(), Vec::new(), env.clone(), false).await {
                        Ok((code, _)) => {
                            env.set_exit_code(code as i64);
                            return Ok(());
                        }
                        Err(e) => return Err(e),
                    }
                }
                if path_str.starts_with("http://") || path_str.starts_with("https://") {
                    let url = path_str.clone();
                    let content = tokio::task::spawn_blocking(move || {
                        let resp = ureq::get(&url)
                            .call()
                            .map_err(|e| format!("source: failed to fetch {}: {}", url, e))?;
                        let mut body = resp.into_body();
                        body.read_to_string()
                            .map_err(|e| format!("source: failed to read response: {}", e))
                    })
                    .await
                    .map_err(|e| EngineError::from(format!("source: task failed: {}", e)))?
                    .map_err(EngineError::from)?;
                    let stmts = fshell_core::Parser::new(&content)
                        .parse_statements()
                        .map_err(|e| {
                            EngineError::from(format!("source: parse error in {}: {}", path_str, e))
                        })?;
                    let noexec = env.options.read().noexec;
                    if noexec {
                        return Ok(());
                    }
                    for stmt in stmts {
                        eval_stmt(&stmt, env, false).await?;
                    }
                    return Ok(());
                }
                if *bash {
                    let content = tokio::fs::read_to_string(&path_str).await.map_err(|e| {
                        EngineError::IoError {
                            message: format!("Failed to read {:?}: {}", path_str, e),
                            span: None,
                        }
                    })?;
                    if let Some(handler) = crate::posix_handler() {
                        let (code, _) = handler(content, Vec::new(), env.clone(), false).await?;
                        env.set_exit_code(code as i64);
                        return Ok(());
                    } else {
                        return Err(EngineError::from(
                            "source --bash: POSIX handler not registered",
                        ));
                    }
                } else {
                    let path_buf = std::path::PathBuf::from(&path_str);
                    let metadata =
                        tokio::fs::metadata(&path_buf)
                            .await
                            .map_err(|e| EngineError::IoError {
                                message: format!("Failed to stat {:?}: {}", path_str, e),
                                span: None,
                            })?;
                    let mtime = metadata
                        .modified()
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

                    let cached_stmts = {
                        let mut cache = env.ast_cache.write();
                        cache.get_by_path(&path_buf, mtime)
                    };

                    let mut stmts = match cached_stmts {
                        Some(stmts) => stmts,
                        None => {
                            let content =
                                tokio::fs::read_to_string(&path_buf).await.map_err(|e| {
                                    EngineError::IoError {
                                        message: format!("Failed to read {:?}: {}", path_str, e),
                                        span: None,
                                    }
                                })?;
                            let hash = fshell_hash::fhash256(content.as_bytes());
                            let mut parser = fshell_core::Parser::new(&content);
                            let stmts = match parser.parse_statements() {
                                Ok(stmts) => stmts,
                                Err(e) => {
                                    // Auto-detect POSIX scripts (e.g. venv
                                    // `activate` files) and delegate to the
                                    // posix handler instead of surfacing a parse
                                    // error for what is really a POSIX script.
                                    if crate::login::looks_like_posix(&content)
                                        && let Some(handler) = crate::posix_handler()
                                    {
                                        let (code, _) =
                                            handler(content, Vec::new(), env.clone(), false)
                                                .await?;
                                        env.set_exit_code(code as i64);
                                        return Ok(());
                                    }
                                    return Err(EngineError::Parse(e));
                                }
                            };
                            {
                                let mut cache = env.ast_cache.write();
                                cache.insert(path_buf.clone(), mtime, hash, stmts.clone());
                            }
                            stmts
                        }
                    };

                    merge_adjacent_setopt(&mut stmts);

                    let noexec = env.options.read().noexec;
                    if noexec {
                        return Ok(());
                    }

                    let trusted = if crate::suggestions::has_trust_profile(&path_str) {
                        let content = tokio::fs::read_to_string(&path_buf).await.map_err(|e| {
                            EngineError::IoError {
                                message: format!("Failed to read {:?}: {}", path_str, e),
                                span: None,
                            }
                        })?;
                        crate::suggestions::is_script_trusted(&path_str, &content)
                    } else {
                        false
                    };

                    let _source_guard = ProfilerState::guard(
                        &env.profiler,
                        &format!("source {}", path_str),
                        ProfilerCategory::Source,
                    );
                    if trusted {
                        IS_TRUSTED_CONTEXT
                            .scope(true, async move {
                                for stmt in stmts {
                                    let _stmt_guard = ProfilerState::guard(
                                        &env.profiler,
                                        &stmt_label(&stmt),
                                        ProfilerCategory::Source,
                                    );
                                    Box::pin(eval_stmt(&stmt, env, false)).await?;
                                }
                                Ok::<(), EngineError>(())
                            })
                            .await?;
                    } else {
                        for stmt in stmts {
                            let _stmt_guard = ProfilerState::guard(
                                &env.profiler,
                                &stmt_label(&stmt),
                                ProfilerCategory::Source,
                            );
                            Box::pin(eval_stmt(&stmt, env, false)).await?;
                        }
                    }
                }
                Ok(())
            }
            Stmt::While { condition, body } => {
                let old_errexit = {
                    let opts = env.options.read();
                    opts.errexit
                };
                loop {
                    let cond_val = if let Some(res) = try_eval_sync(condition, env) {
                        res?
                    } else {
                        if old_errexit {
                            let mut opts = env.options.write();
                            opts.errexit = false;
                        }
                        let cond_res = eval_expr(condition, env).await;
                        if old_errexit {
                            let mut opts = env.options.write();
                            opts.errexit = true;
                        }
                        cond_res?
                    };
                    if !val_to_bool(&cond_val)? {
                        break;
                    }
                    if !eval_loop_body(body, env).await? {
                        return Ok(());
                    }
                }
                Ok(())
            }
            // `for <var> in <iterable> { body }`
            Stmt::For { var, iter, body } => {
                let iterable = eval_expr(iter, env).await?;
                let items = match iterable {
                    Val::List(items) => items,
                    Val::String(s) => {
                        if s.contains('{') && (s.contains(',') || s.contains("..")) {
                            let expanded = fshell_core::expand_braces(&s);
                            expanded
                                .into_iter()
                                .map(|item| {
                                    if let Ok(i) = item.parse::<i64>() {
                                        Val::Int(i)
                                    } else {
                                        Val::String(item)
                                    }
                                })
                                .collect()
                        } else {
                            s.chars().map(|c| Val::String(c.to_string())).collect()
                        }
                    }
                    other => {
                        return Err(EngineError::TypeMismatch {
                            expected: "List or String (iterable)".to_string(),
                            found: format!("{:?}", other),
                            span: None,
                        });
                    }
                };
                for item in items {
                    let mut local_map = FxHashMap::default();
                    local_map.insert(var.clone(), item);
                    let loop_env = env.push_scope(Arc::new(fshell_core::RwLock::new(local_map)));
                    if !eval_loop_body(body, &loop_env).await? {
                        break;
                    }
                }
                Ok(())
            }
            // Loop control-flow signals
            Stmt::Break => Err(EngineError::BreakSignal),
            Stmt::Continue => Err(EngineError::ContinueSignal),
            // Return signal
            Stmt::Return(expr) => {
                let val = eval_expr(expr, env).await?;
                Err(EngineError::ReturnSignal(val))
            }
            // Exit signal — propagates through all frames, only caught by REPL loop
            Stmt::Exit(expr) => {
                let code = if let Some(expr) = expr {
                    let val = eval_expr(expr, env).await?;
                    match val {
                        Val::Int(i) => i as i32,
                        Val::Float(f) => f as i32,
                        _ => {
                            return Err(EngineError::TypeMismatch {
                                expected: "Int or Float".to_string(),
                                found: format!("{:?}", val),
                                span: None,
                            });
                        }
                    }
                } else {
                    0
                };
                {
                    let mut ec = env.prompt.last_exit_code.write();
                    *ec = code as i64;
                }
                Err(EngineError::ExitSignal(code))
            }
            // On signal handler — registered via hook system
            Stmt::On { signal, handler } => {
                dispatch_on_signal(signal, handler, env).await?;
                Ok(())
            }
            Stmt::Unsafe { body } => {
                for s in body {
                    Box::pin(eval_stmt(s, env, true)).await?;
                }
                Ok(())
            }
            Stmt::Every {
                duration,
                unit,
                body,
            } => {
                let millis = match unit {
                    TimeUnit::Second => duration * 1000,
                    TimeUnit::Minute => duration * 60 * 1000,
                    TimeUnit::Hour => duration * 60 * 60 * 1000,
                };

                let interval_duration = std::time::Duration::from_millis(millis);
                let mut interval = tokio::time::interval(interval_duration);
                // The first tick completes immediately; consume it so the first
                // body execution is not followed by an immediate second burst.
                interval.tick().await;
                let unit_label = match unit {
                    TimeUnit::Second => "s",
                    TimeUnit::Minute => "m",
                    TimeUnit::Hour => "h",
                };

                loop {
                    if check_sigint_or_ctrl_c(env).await {
                        return Err(EngineError::from("Interrupted by Ctrl+C"));
                    }

                    // Header for each tick — makes the stream scannable and
                    // distinguishes ticks when the underlying data hasn't changed.
                    // Uses local time for human readability; falls back to UTC.
                    let ts = chrono::Local::now().format("%H:%M:%S").to_string();
                    // Use stderr for the header so that `every 5s { ps | @json }`
                    // remains machine-readable on stdout.
                    eprintln!(
                        "\x1b[2m— every {}{} @ {} —\x1b[0m",
                        duration, unit_label, ts
                    );

                    for stmt in body {
                        if check_sigint_or_ctrl_c(env).await {
                            return Err(EngineError::from("Interrupted by Ctrl+C"));
                        }

                        match stmt.unpack() {
                            Stmt::Expr(expr) => {
                                let val = eval_expr(expr, env).await?;
                                match val {
                                    Val::List(list) => {
                                        if list.is_empty() {
                                            println!("(no results)");
                                            continue;
                                        }
                                        let is_map_list = matches!(list[0], Val::Map(_));
                                        if is_map_list {
                                            let all_maps =
                                                list.iter().all(|v| matches!(v, Val::Map(_)));
                                            if all_maps {
                                                println!("{}", render_table(&list));
                                                continue;
                                            }
                                        }
                                        for item in list {
                                            println!("{}", item.to_text());
                                        }
                                    }
                                    Val::Null => {}
                                    other => {
                                        println!("{}", other.to_text());
                                    }
                                }
                            }
                            other_stmt => {
                                Box::pin(eval_stmt(other_stmt, env, unsafe_context)).await?;
                            }
                        }
                    }

                    // Wait for next tick, polling cancellation every 100ms.
                    // Also polls raw-mode Ctrl-C (kitty `CSI 99;5u` / 0x03) so that
                    // `every` can be interrupted even when the session is in raw
                    // + kitty keyboard protocol where SIGINT is not generated.
                    loop {
                        tokio::select! {
                            _ = interval.tick() => { break; }
                            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                                if check_sigint_or_ctrl_c(env).await {
                                    return Err(EngineError::from("Interrupted by Ctrl+C"));
                                }
                            }
                        }
                    }
                }
            }
            Stmt::ReactiveCellEvery {
                name,
                duration,
                unit,
                body,
            } => {
                let (tx, rx) = tokio::sync::watch::channel(Arc::new(Vec::new()));
                {
                    let mut cells = lock_reactive!(env.reactive.cells.write());
                    cells.insert(name.clone(), rx);
                    env.reactive
                        .has_cells
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }

                {
                    let mut pipes = env.reactive.pipelines.write();
                    pipes.insert(
                        name.clone(),
                        format!(
                            "every {}{} {{ ... }}",
                            duration,
                            match unit {
                                TimeUnit::Second => "s",
                                TimeUnit::Minute => "m",
                                TimeUnit::Hour => "h",
                            }
                        ),
                    );
                }

                let env_clone = env.clone();
                let name_clone = name.clone();
                let body_clone = body.clone();
                let duration_val = *duration;
                let unit_val = *unit;

                tokio::spawn(async move {
                    let millis = match unit_val {
                        TimeUnit::Second => duration_val * 1000,
                        TimeUnit::Minute => duration_val * 60 * 1000,
                        TimeUnit::Hour => duration_val * 60 * 60 * 1000,
                    };

                    let mut interval =
                        tokio::time::interval(std::time::Duration::from_millis(millis));
                    loop {
                        if tx.receiver_count() == 0 {
                            break;
                        }
                        if env_clone.job_control.cancellation.load(Ordering::Acquire) {
                            break;
                        }

                        interval.tick().await;

                        let mut results = Vec::new();
                        for stmt in &body_clone {
                            match stmt.unpack() {
                                Stmt::Expr(expr) => {
                                    if let Ok(val) = eval_expr(expr, &env_clone).await {
                                        match val {
                                            Val::List(list) => {
                                                results.extend(list);
                                            }
                                            Val::Null => {}
                                            other => {
                                                results.push(other);
                                            }
                                        }
                                    }
                                }
                                other_stmt => {
                                    let _ =
                                        Box::pin(eval_stmt(other_stmt, &env_clone, false)).await;
                                }
                            }
                        }

                        let _ = tx.send(Arc::new(results));

                        let _ = env_clone
                            .reactive
                            .tx
                            .send(ReactiveEvent::TriggerCell(name_clone.clone()))
                            .await;
                    }
                });

                Ok(())
            }
            Stmt::PosixBlock { body } => {
                let content = body.clone();
                if let Some(handler) = crate::posix_handler() {
                    let (code, _) = handler(content, Vec::new(), env.clone(), false).await?;
                    env.set_exit_code(code as i64);
                    if code != 0 {
                        return Err(EngineError::PipelineError {
                            message: format!("POSIX block exited with status {}", code),
                            span: None,
                        });
                    }
                } else {
                    // Fallback: execute via bash subprocess for environments without posix handler
                    let output = tokio::task::spawn_blocking(move || {
                        std::process::Command::new("sh")
                            .arg("-c")
                            .arg(&content)
                            .output()
                    })
                    .await
                    .map_err(|e| EngineError::from(format!("POSIX block task failed: {}", e)))?
                    .map_err(|e| EngineError::from(format!("Failed to spawn sh: {}", e)))?;
                    use std::io::Write;
                    if !output.stdout.is_empty() {
                        let _ = std::io::stdout().write_all(&output.stdout);
                    }
                    if !output.stderr.is_empty() {
                        let _ = std::io::stderr().write_all(&output.stderr);
                    }
                    let code = output.status.code().unwrap_or(0);
                    env.set_exit_code(code as i64);
                    if code != 0 {
                        let errexit = env.options.read().errexit;
                        if errexit {
                            return Err(EngineError::ExitSignal(code));
                        }
                    }
                }
                Ok(())
            }
            Stmt::Background(stmt) => {
                let stmt = stmt.clone();
                let env = env.clone();
                let cmd_str = format!("{:?}", stmt);
                let job_id = env.background_count.fetch_add(1, Ordering::Relaxed) as usize + 1;

                // Register a virtual job entry so `jobs` shows it
                let vpid = -(job_id as i32);
                {
                    let mut jobs = env.job_control.jobs.write();
                    jobs.insert(
                        vpid,
                        crate::Job {
                            id: job_id,
                            pgid: vpid,
                            pids: vec![],
                            cmd: cmd_str.clone(),
                            status: crate::JobStatus::Running,
                            disowned: false,
                            started_at: Some(std::time::Instant::now()),
                        },
                    );
                }

                let mut env_for_task = env.clone();
                env_for_task.is_captured = true;
                tokio::spawn(async move {
                    let result = Box::pin(eval_stmt(&stmt, &env_for_task, false)).await;
                    let prev = env_for_task
                        .background_count
                        .fetch_sub(1, Ordering::Relaxed);
                    if prev == 1 {
                        env_for_task.background_notify.notify_waiters();
                    }
                    // Clean up virtual job entry
                    env_for_task.job_control.jobs.write().remove(&vpid);
                    if env_for_task.options.read().notify {
                        let status = if result.is_ok() { "Done" } else { "Exit 1" };
                        eprintln!("[{}]\t{}  {}", job_id, status, cmd_str);
                    }
                });
                Ok(())
            }
        }
    })
}

pub(crate) fn is_query_stage(
    stage: &PipelineStage,
    builtin_registry: &FxHashMap<String, BuiltinHandler>,
) -> bool {
    match stage {
        PipelineStage::CommandCall { name, .. } => {
            if name.starts_with('$') {
                return true;
            }
            // Only CommandCall-dispatched commands go here.
            // filter, map, sort, grep, count are parsed as distinct PipelineStage variants
            // and handled in their own match arm below.
            let query_builtins = ["ls", "pwd", "echo"];
            if query_builtins.contains(&name.as_str()) {
                return true;
            }
            if builtin_registry.contains_key(name) {
                return false;
            }
            false
        }
        PipelineStage::Filter { .. }
        | PipelineStage::Map { .. }
        | PipelineStage::Sort { .. }
        | PipelineStage::Grep { .. }
        | PipelineStage::Mark { .. }
        | PipelineStage::Count
        | PipelineStage::Hash { .. }
        | PipelineStage::Limit { .. }
        | PipelineStage::BoundaryOperator { .. }
        | PipelineStage::Traverse { .. }
        | PipelineStage::Write { .. }
        | PipelineStage::Read { .. }
        | PipelineStage::Heredoc { .. }
        | PipelineStage::HereString { .. }
        | PipelineStage::FdRedirect { .. } => true,
    }
}

/// Convert a serde_json::Value (clean JSON) into a Val.
pub(crate) fn json_value_to_val(value: serde_json::Value) -> Val {
    value.into()
}

/// Convert a Val into its serde_json representation (used by boundary operators).
pub(crate) fn val_to_json_value(val: &Val) -> serde_json::Value {
    val.into()
}

pub(crate) fn run_boundary_operator<F, G, H>(
    current_rx: Option<Receiver<PipelinePayload>>,
    out_tx: Sender<PipelinePayload>,
    env: &Env,
    decode_str: F,
    decode_bytes: G,
    encode_val: H,
) where
    F: Fn(&str) -> Result<Val, String> + Send + 'static,
    G: Fn(&[u8]) -> Result<Val, String> + Send + 'static,
    H: Fn(Val) -> Result<PipelinePayload, String> + Send + 'static,
{
    static BOUNDARY_SEMAPHORE: std::sync::OnceLock<tokio::sync::Semaphore> =
        std::sync::OnceLock::new();
    let sem = BOUNDARY_SEMAPHORE.get_or_init(|| tokio::sync::Semaphore::new(4));
    let env = env.clone();

    tokio::spawn(async move {
        let _permit = match sem.acquire().await {
            Ok(p) => Some(p),
            Err(_) => {
                static BOUNDARY_SEMAPHORE_FALLBACK: std::sync::OnceLock<tokio::sync::Semaphore> =
                    std::sync::OnceLock::new();
                let fb = BOUNDARY_SEMAPHORE_FALLBACK.get_or_init(|| tokio::sync::Semaphore::new(4));
                match fb.acquire().await {
                    Ok(p) => Some(p),
                    Err(e) => {
                        eprintln!("Semaphore unexpectedly closed: {e}");
                        env.report_stage_error();
                        let _ = out_tx
                            .send(PipelinePayload::Structured(
                                format!("Semaphore closed: {e}").into(),
                            ))
                            .await;
                        None
                    }
                }
            }
        };
        if _permit.is_none() {
            return;
        }
        if let Some(mut rx) = current_rx {
            while let Some(payload) = rx.recv().await {
                let result = match payload {
                    PipelinePayload::Data(val_arc) => match (*val_arc).clone() {
                        Val::String(s) => match decode_str(&s) {
                            Ok(parsed) => PipelinePayload::Data(Arc::new(parsed)),
                            Err(e) => {
                                env.report_stage_error();
                                PipelinePayload::Structured(e.into())
                            }
                        },
                        Val::Blob(b) => match decode_bytes(&b) {
                            Ok(parsed) => PipelinePayload::Data(Arc::new(parsed)),
                            Err(e) => {
                                env.report_stage_error();
                                PipelinePayload::Structured(e.into())
                            }
                        },
                        other => match encode_val(other) {
                            Ok(p) => p,
                            Err(e) => {
                                env.report_stage_error();
                                PipelinePayload::Structured(e.into())
                            }
                        },
                    },
                    PipelinePayload::Bytes(b) => match decode_bytes(&b) {
                        Ok(parsed) => PipelinePayload::Data(Arc::new(parsed)),
                        Err(e) => {
                            env.report_stage_error();
                            PipelinePayload::Structured(e.into())
                        }
                    },
                    PipelinePayload::Structured(d) => PipelinePayload::Structured(d),
                };
                let _ = out_tx.send(result).await;
            }
        }
    });
}

/// Parse a complete CSV string into Val::List of Val::Map records.
pub(crate) fn decode_csv_input(input: &str) -> Result<Val, String> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(input.as_bytes());
    let headers = reader
        .headers()
        .map_err(|e| format!("CSV header error: {e}"))?
        .clone();
    let mut records = Vec::new();
    for result in reader.records() {
        let record = result.map_err(|e| format!("CSV row error: {e}"))?;
        let mut map = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        for (i, field) in record.iter().enumerate() {
            let key = headers
                .get(i)
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("_{i}"));
            let val = parse_csv_field(field);
            map.insert(ustr(&key), val);
        }
        records.push(Val::Map(map));
    }
    Ok(Val::List(records))
}

/// Try to parse a CSV field into the most specific Val type.
pub(crate) fn parse_csv_field(field: &str) -> Val {
    let trimmed = field.trim();
    if trimmed.is_empty() {
        return Val::Null;
    }
    if let Ok(i) = trimmed.parse::<i64>() {
        return Val::Int(i);
    }
    if let Ok(f) = trimmed.parse::<f64>() {
        return Val::Float(f);
    }
    Val::String(trimmed.to_string())
}

/// Render a list of Val::Map items as an ASCII table.
pub(crate) fn render_table(items: &[Val]) -> String {
    if items.is_empty() {
        return "(no results)".to_string();
    }

    let mut seen: FxIndexMap<ustr::Ustr, bool> =
        FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    for item in items {
        if let Val::Map(map) = item {
            for key in map.keys() {
                seen.entry(*key).or_insert(true);
            }
        }
    }
    let columns: Vec<String> = seen.into_keys().map(|k| k.to_string()).collect();

    if columns.is_empty() {
        return items
            .iter()
            .map(|v| v.to_text())
            .collect::<Vec<_>>()
            .join("\n");
    }

    // Prefer crossterm's size (works in raw + kitty) with sensible fallbacks.
    // 80 is too narrow for `ps`'s `command` (often 30-50 chars) and makes every
    // table useless. Use 120 as the non-tty default.
    let term_width = crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(0);
    let term_width = if term_width < 20 {
        terminal_size::terminal_size()
            .map(|(w, _)| w.0 as usize)
            .unwrap_or(0)
    } else {
        term_width
    };
    let term_width = if term_width < 20 { 120 } else { term_width };

    let mut widths: Vec<usize> = columns.iter().map(|c| c.len()).collect();
    let mut rows: Vec<Vec<String>> = Vec::new();
    for item in items {
        if let Val::Map(map) = item {
            let mut row = Vec::new();
            for (i, col) in columns.iter().enumerate() {
                let val = map
                    .get(&ustr(col))
                    .map(|v| v.to_text())
                    .unwrap_or_else(|| "—".to_string());
                widths[i] = widths[i].max(val.len());
                row.push(val);
            }
            rows.push(row);
        }
    }

    let padding = 3;
    let separator = 1;
    let total_data_width: usize = widths.iter().sum();
    let total_padding = padding * columns.len().saturating_sub(1) + 2 * separator;
    let available = term_width.saturating_sub(total_padding);
    if total_data_width > available && !columns.is_empty() {
        // Keep at least header width or 8 chars so no column collapses to 0.
        // Truncate the largest column first (fair) instead of rightmost-first
        // which was cutting `cpu` to 0 and then heavily truncating `command`.
        let min_widths: Vec<usize> = columns.iter().map(|c| c.len().max(8)).collect();
        let mut remaining = total_data_width.saturating_sub(available);
        while remaining > 0 {
            let mut max_idx: Option<usize> = None;
            let mut max_w = 0;
            for (i, w) in widths.iter().enumerate() {
                if *w > min_widths[i] && *w > max_w {
                    max_w = *w;
                    max_idx = Some(i);
                }
            }
            let Some(idx) = max_idx else { break };
            widths[idx] -= 1;
            remaining -= 1;
        }
    }

    let mut out = String::new();

    out.push('|');
    for (i, col) in columns.iter().enumerate() {
        out.push(' ');
        out.push_str(&pad_truncate(col, widths[i]));
        out.push_str(" |");
    }
    out.push('\n');

    out.push('|');
    for w in &widths {
        out.push_str(&"-".repeat(w + 2));
        out.push('|');
    }
    out.push('\n');

    for row in &rows {
        out.push('|');
        for (i, val) in row.iter().enumerate() {
            out.push(' ');
            out.push_str(&pad_truncate(val, widths[i]));
            out.push_str(" |");
        }
        out.push('\n');
    }

    out
}

pub(crate) fn pad_truncate(s: &str, width: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= width {
        format!("{:<width$}", s, width = width)
    } else {
        let truncated: String = s.chars().take(width.saturating_sub(1)).collect();
        format!("{}…", truncated)
    }
}

/// Render a list of Val::Map items as a horizontal bar chart.
pub(crate) fn render_bar_chart(items: &[Val]) -> String {
    if items.is_empty() {
        return "(no data)".to_string();
    }

    let mut data: Vec<(String, f64)> = Vec::new();
    for item in items {
        if let Val::Map(map) = item {
            let label = map.values().find_map(|v| match v {
                Val::String(s) => Some(s.clone()),
                _ => None,
            });
            let value = map.values().find_map(|v| match v {
                Val::Int(i) => Some(*i as f64),
                Val::Float(f) => Some(*f),
                _ => None,
            });
            if let (Some(lbl), Some(val)) = (label, value) {
                data.push((lbl, val));
            }
        }
    }

    if data.is_empty() {
        return "(no numeric data found)".to_string();
    }

    data.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let term_width = terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(80);

    let max_label = data.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
    let max_value = data.iter().map(|(_, v)| v).cloned().fold(0_f64, f64::max);

    let value_str_width = 10;
    let bar_area = term_width
        .saturating_sub(max_label + 3 + value_str_width)
        .max(10);

    let mut out = String::new();
    for (label, value) in &data {
        let bar_len = if max_value > 0.0 {
            ((value / max_value) * bar_area as f64).round() as usize
        } else {
            0
        };
        let bar: String = "█".repeat(bar_len);
        let fmt_val = format_value(*value);
        out.push_str(&format!(
            "{:<lbl$}  {}  {}\n",
            label,
            bar,
            fmt_val,
            lbl = max_label
        ));
    }

    out
}

pub(crate) fn format_value(v: f64) -> String {
    let abs_v = v.abs();
    if abs_v >= 1_000_000_000.0 {
        format!("{:>6.1} GB", v / 1_000_000_000.0)
    } else if abs_v >= 1_000_000.0 {
        format!("{:>6.1} MB", v / 1_000_000.0)
    } else if abs_v >= 1_000.0 {
        format!("{:>6.1} KB", v / 1_000.0)
    } else {
        format!("{:>7.0}  ", v)
    }
}

pub(crate) struct PathCache {
    pub(crate) path: String,
    pub(crate) executables: fshell_hash::FxHashMap<String, String>,
    last_updated: std::time::Instant,
}

pub(crate) static PATH_CACHE: std::sync::Mutex<Option<PathCache>> = std::sync::Mutex::new(None);

pub(crate) const PATH_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(300);

/// Invalidate the PATH cache (e.g. when a cached path turns out to be broken)
pub fn invalidate_path_cache() {
    if let Ok(mut cache) = PATH_CACHE.lock() {
        *cache = None;
    }
    if let Ok(mut val_cache) = COMMAND_VALIDATION_CACHE.lock() {
        val_cache.clear();
    }
}

pub(crate) const COMMAND_CACHE_MAX_SIZE: usize = 256;

pub(crate) static COMMAND_VALIDATION_CACHE: std::sync::LazyLock<
    std::sync::Mutex<FxHashMap<(String, String), bool>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(FxHashMap::default()));

/// Like `is_external_command` but backed by a bounded in-memory cache to avoid
/// scanning PATH on every call. Used by the highlighter and completer on the
/// keystroke path.
pub fn is_external_command_cached(name: &str, env_path: Option<&str>) -> bool {
    if name.is_empty() {
        return false;
    }
    let current_path = env_path
        .map(|p| p.to_string())
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();

    let cache_key = (name.to_string(), current_path);
    let cache = COMMAND_VALIDATION_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(cached) = cache.get(&cache_key) {
        return *cached;
    }
    drop(cache);
    let result = is_external_command(name, Some(&cache_key.1));
    let mut cache = COMMAND_VALIDATION_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if cache.len() >= COMMAND_CACHE_MAX_SIZE {
        let keys: Vec<(String, String)> = cache
            .keys()
            .take(COMMAND_CACHE_MAX_SIZE / 2)
            .cloned()
            .collect();
        for k in keys {
            cache.remove(&k);
        }
    }
    cache.insert(cache_key, result);
    result
}

pub(crate) fn rebuild_path_cache(current_path: &str, now: std::time::Instant) -> PathCache {
    if std::env::var("FSH_CNF_DEBUG").as_deref() == Ok("1") {
        eprintln!(
            "[cnf_debug] {}:{}: rebuild_path_cache path={:?} starting",
            file!(),
            line!(),
            current_path
        );
    }
    let mut executables = fshell_hash::FxHashMap::default();
    let mut dir_count = 0;
    let mut total_entries = 0;
    for dir in current_path.split(':') {
        if let Ok(entries) = std::fs::read_dir(dir) {
            dir_count += 1;
            for entry in entries.flatten() {
                total_entries += 1;
                // Use file_type() which can use d_type from readdir (no stat syscall)
                // rather than path.is_file() which always stats.
                // We accept regular files and symlinks, but skip broken symlinks
                // by verifying the symlink target exists via metadata().
                if entry
                    .file_type()
                    .is_ok_and(|t| t.is_file() || t.is_symlink())
                {
                    let file_name = entry.file_name().to_string_lossy().to_string();
                    let is_sym = entry.file_type().map(|t| t.is_symlink()).unwrap_or(false);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let meta_res = if is_sym {
                            std::fs::metadata(entry.path())
                        } else {
                            entry.metadata()
                        };
                        if let Ok(metadata) = meta_res
                            && metadata.permissions().mode() & 0o111 != 0
                        {
                            let full_path = entry.path().to_string_lossy().to_string();
                            executables.entry(file_name).or_insert(full_path);
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        let exists = if is_sym { entry.path().exists() } else { true };
                        if exists {
                            let full_path = entry.path().to_string_lossy().to_string();
                            executables.entry(file_name).or_insert(full_path);
                        }
                    }
                }
            }
        }
    }
    let result = PathCache {
        path: current_path.to_string(),
        executables,
        last_updated: now,
    };
    if std::env::var("FSH_CNF_DEBUG").as_deref() == Ok("1") {
        let elapsed = now.elapsed().as_millis();
        eprintln!(
            "[cnf_debug] {}:{}: rebuild_path_cache done in {}ms, {} dirs, {} entries scanned, {} executables cached",
            file!(),
            line!(),
            elapsed,
            dir_count,
            total_entries,
            result.executables.len()
        );
    }
    result
}

pub(crate) fn start_path_watcher() {
    use notify::{EventKind, RecursiveMode, Watcher};

    static WATCHER_STARTED: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    if WATCHER_STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }

    std::thread::spawn(move || {
        let current_path = std::env::var("PATH").unwrap_or_default();
        if current_path.is_empty() {
            return;
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher =
            match notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    match event.kind {
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                            let _ = tx.send(());
                        }
                        _ => {}
                    }
                }
            }) {
                Ok(w) => w,
                Err(_) => return,
            };

        for dir in current_path.split(':') {
            if !dir.is_empty() {
                let _ = watcher.watch(dir.as_ref(), RecursiveMode::NonRecursive);
            }
        }

        // Rebuild the cache periodically (every 30s) and on filesystem events.
        // This keeps the cache fresh without blocking the command dispatch path.
        loop {
            match rx.recv_timeout(std::time::Duration::from_secs(30)) {
                Ok(_) => {
                    // Filesystem change in a PATH directory — mark cache stale
                    if let Ok(mut cache) = PATH_CACHE.lock()
                        && let Some(ref mut c) = *cache
                    {
                        c.last_updated = std::time::Instant::now()
                            - PATH_CACHE_TTL
                            - std::time::Duration::from_secs(1);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // Periodic refresh — rebuild the cache from scratch
                    let os_path = std::env::var("PATH").unwrap_or_default();
                    if os_path.is_empty() {
                        continue;
                    }
                    let now = std::time::Instant::now();
                    let fresh = rebuild_path_cache(&os_path, now);
                    if let Ok(mut cache) = PATH_CACHE.lock() {
                        *cache = Some(fresh);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
}

/// Pre-warm the PATH cache at startup so the first interactive command
/// is not delayed by scanning every directory on PATH.
///
/// If `env` is provided, reads PATH from fsh's `$PATH` variable (which may
/// have been enriched by login-env loading or init.fsh) rather than from the
/// OS process environment. Rebuilds the cache whenever the PATH changes.
pub fn warmup_path_cache(env: Option<&crate::Env>) {
    start_path_watcher();
    let current_path = env
        .and_then(|e| {
            let vars = Some(e.vars.read())?;
            if let Some(fshell_core::Val::String(s)) = vars.get("PATH") {
                if s.is_empty() { None } else { Some(s.clone()) }
            } else {
                None
            }
        })
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    if current_path.is_empty() {
        return;
    }
    let mut cache_guard = PATH_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let needs_rebuild = match &*cache_guard {
        None => true,
        Some(cache) => cache.path != current_path,
    };
    if needs_rebuild {
        let now = std::time::Instant::now();
        *cache_guard = Some(rebuild_path_cache(&current_path, now));
    }
}

pub fn is_external_command(name: &str, env_path: Option<&str>) -> bool {
    let cnf_debug = std::env::var("FSH_CNF_DEBUG").as_deref() == Ok("1");
    if cnf_debug {
        eprintln!(
            "[cnf_debug] {}:{}: is_external_command name={:?} env_path={:?} at {:?}",
            file!(),
            line!(),
            name,
            env_path,
            std::time::Instant::now()
        );
    }
    let current_path = env_path
        .map(|p| p.to_string())
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    if current_path.is_empty() {
        return false;
    }

    let mut cache_guard = PATH_CACHE.lock().unwrap_or_else(|e| e.into_inner());

    if let Some(ref cache) = *cache_guard
        && cache.path == current_path
        && cache.last_updated.elapsed() < PATH_CACHE_TTL
    {
        return cache.executables.contains_key(name);
    }

    let rebuild_start = std::time::Instant::now();
    *cache_guard = Some(rebuild_path_cache(&current_path, rebuild_start));
    let found = cache_guard
        .as_ref()
        .map(|cache| cache.executables.contains_key(name))
        .unwrap_or(false);
    if cnf_debug {
        eprintln!(
            "[cnf_debug] {}:{}: is_external_command {} found={}, cache built in {}ms",
            file!(),
            line!(),
            name,
            found,
            rebuild_start.elapsed().as_millis()
        );
    }
    found
}

/// Returns the resolved full path for a command name, using the PATH cache.
/// Returns None if the command is not found on PATH.
pub fn resolve_cached_command_path(name: &str, env_path: Option<&str>) -> Option<String> {
    let cnf_debug = std::env::var("FSH_CNF_DEBUG").as_deref() == Ok("1");
    if cnf_debug {
        eprintln!(
            "[cnf_debug] {}:{}: resolve_cached_command_path name={:?} env_path={:?}",
            file!(),
            line!(),
            name,
            env_path
        );
    }
    let current_path = env_path
        .map(|p| p.to_string())
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    if current_path.is_empty() {
        return None;
    }

    let mut cache_guard = PATH_CACHE.lock().unwrap_or_else(|e| e.into_inner());

    // If cache exists and PATH matches and is still fresh, use it.
    if let Some(ref cache) = *cache_guard
        && cache.path == current_path
        && cache.last_updated.elapsed() < PATH_CACHE_TTL
    {
        let result = cache.executables.get(name).cloned();
        if cnf_debug {
            eprintln!(
                "[cnf_debug] {}:{}: resolve cache result={:?}",
                file!(),
                line!(),
                result
            );
        }
        return result;
    }

    let rebuild_start = std::time::Instant::now();
    *cache_guard = Some(rebuild_path_cache(&current_path, rebuild_start));
    let result = cache_guard
        .as_ref()
        .and_then(|cache| cache.executables.get(name).cloned());
    if cnf_debug {
        eprintln!(
            "[cnf_debug] {}:{}: resolve cache built in {}ms, result={:?}",
            file!(),
            line!(),
            rebuild_start.elapsed().as_millis(),
            result
        );
    }
    result
}

pub(crate) fn expand_alias_with_args(expansion: &str, args: &[Val]) -> String {
    if args.is_empty() {
        return expansion.to_string();
    }
    let mut result = expansion.to_string();
    if !result.ends_with(' ') {
        result.push(' ');
    }
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            result.push(' ');
        }
        match arg {
            Val::String(s) => {
                if s.contains(' ') || s.contains('\t') || s.contains('"') || s.contains('\\') {
                    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
                    result.push_str(&format!("\"{}\"", escaped));
                } else {
                    result.push_str(s);
                }
            }
            Val::Int(i) => result.push_str(&i.to_string()),
            Val::Float(f) => result.push_str(&f.to_string()),
            Val::Bool(b) => result.push_str(&b.to_string()),
            Val::Null => result.push_str("null"),
            other => result.push_str(&serde_json::to_string(other).unwrap_or_default()),
        }
    }
    result
}
