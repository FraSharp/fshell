// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::{
    CapAction, EngineError, Env, PendingSuggestion, PipeSender, PipeStream, PipelinePayload,
    SuggestionMode, cmp_vals, decode_csv_input, eval_expr, eval_stmt, expand_alias_with_args,
    expand_globs, get_suggested_command, is_external_command, pipeline_channel_size,
    render_bar_chart, render_table, run_boundary_operator,
};
use crate::{Flow, PipelineFailure};
use fshell_core::RwLock;
use fshell_core::ShellError;
use fshell_core::{
    Expr, FshDiag, Parser, Pipeline, PipelineStage, SerializationFormat, Stmt, StringPart,
    TypeConstraint, Val,
};
use fshell_hash::{FxHashMap, FxHashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use ustr::ustr;

/// Boundary conversion used by stage handlers: raw bytes become one String Val
/// per non-empty line (a payload with no newline stays whole) to preserve
/// streaming semantics.
async fn forward_bytes_as_lines(tx: &PipeSender, b: &[u8]) {
    let text = String::from_utf8_lossy(b).into_owned();
    if text.contains('\n') {
        for line in text.lines() {
            if !line.is_empty() {
                let _ = tx
                    .send(PipelinePayload::Data(Arc::new(Val::String(
                        line.to_string(),
                    ))))
                    .await;
            }
        }
    } else {
        let _ = tx
            .send(PipelinePayload::Data(Arc::new(Val::String(text))))
            .await;
    }
}

pub async fn collect_pipeline(pipeline: &Pipeline, env: &Env) -> Result<Vec<Val>, EngineError> {
    let (tx, mut rx) = tokio::sync::mpsc::channel(pipeline_channel_size(env));
    let mut env_clone = env.clone();
    env_clone.is_captured = true;
    let pipeline_clone = pipeline.clone();
    let tx_err = tx.clone();
    tokio::spawn(async move {
        if let Err(e) = execute_pipeline(&pipeline_clone, &env_clone, tx).await {
            let _ = tx_err.send(PipelinePayload::Structured(e.into())).await;
        }
    });
    let mut results = Vec::new();
    let mut has_logical_error = false;
    while let Some(payload) = rx.recv().await {
        if env.job_control.cancellation.load(Ordering::Acquire) {
            break;
        }
        match payload {
            PipelinePayload::Data(v) => results.push(strip_capture_sentinel((*v).clone())),
            PipelinePayload::Bytes(b) => {
                let s = String::from_utf8_lossy(&b).into_owned();
                results.push(strip_capture_sentinel(Val::String(s)));
            }
            PipelinePayload::Structured(d) => {
                if crate::is_condition_false_diag(&d) {
                    env.set_exit_code(1);
                    has_logical_error = true;
                    continue;
                }
                env.set_exit_code(1);
                return Err(EngineError::PipelineError {
                    message: d.to_string(),
                    span: None,
                });
            }
        }
    }
    if !has_logical_error {
        env.set_exit_code(0);
    }
    Ok(results)
}

/// Spawn a pipeline and return a receiver that streams results as they arrive.
pub fn spawn_pipeline_stream(pipeline: &Pipeline, env: &Env) -> PipeStream {
    let (tx, rx) = tokio::sync::mpsc::channel(pipeline_channel_size(env));
    let env_clone = env.clone();
    let pipeline_clone = pipeline.clone();
    let tx_err = tx.clone();
    tokio::spawn(async move {
        if let Err(e) = execute_pipeline(&pipeline_clone, &env_clone, tx).await {
            let _ = tx_err.send(PipelinePayload::Structured(e.into())).await;
        }
    });
    rx
}

/// Spawn a pipeline and collect data payloads into a Vec, silently ignoring diagnostics.
pub(crate) async fn collect_pipeline_silent(pipeline: &Pipeline, env: &Env) -> Vec<Val> {
    let (tx, mut rx) = tokio::sync::mpsc::channel(pipeline_channel_size(env));
    let mut env_clone = env.clone();
    env_clone.is_captured = true;
    let pipeline_clone = pipeline.clone();
    let tx_err = tx.clone();
    tokio::spawn(async move {
        if let Err(e) = execute_pipeline(&pipeline_clone, &env_clone, tx).await {
            let _ = tx_err.send(PipelinePayload::Structured(e.into())).await;
        }
    });
    let mut results = Vec::new();
    while let Some(payload) = rx.recv().await {
        if env.job_control.cancellation.load(Ordering::Acquire) {
            break;
        }
        match payload {
            PipelinePayload::Data(v) => results.push(strip_capture_sentinel((*v).clone())),
            PipelinePayload::Bytes(b) => {
                let s = String::from_utf8_lossy(&b).into_owned();
                results.push(strip_capture_sentinel(Val::String(s)));
            }
            PipelinePayload::Structured(_) => {}
        }
    }
    results
}

/// Remove the trailing NUL that `echo -n`/`echo -e…\c` uses to signal "no
/// trailing newline". It is meaningful only to terminal/refer writers; a
/// captured value must not carry the sentinel (`let x = echo -n hi` ⇒ `$x` is
/// `hi`, not `hi\0`).
fn strip_capture_sentinel(val: Val) -> Val {
    match val {
        Val::String(s) => {
            let s = if s.ends_with('\0') {
                s[..s.len() - 1].to_string()
            } else {
                s
            };
            Val::String(s)
        }
        Val::List(items) => Val::List(items.into_iter().map(strip_capture_sentinel).collect()),
        other => other,
    }
}

/// Check that a Val satisfies the given type constraint.
/// Returns Ok(()) if it does, Err with a message if not.
pub(crate) fn check_type_constraint(
    val: &Val,
    constraint: &TypeConstraint,
) -> Result<(), ShellError> {
    use ustr::ustr;
    match (val, constraint) {
        (_, TypeConstraint::Any) => Ok(()),
        (Val::Null, _) => Ok(()), // null satisfies any constraint
        (val, TypeConstraint::Primitive(name)) => {
            if name.to_lowercase() == val.type_name().to_lowercase() {
                Ok(())
            } else {
                Err(format!(
                    "type constraint error: expected primitive '{}', got '{}'",
                    name,
                    val.type_name()
                )
                .into())
            }
        }
        (Val::Map(m), TypeConstraint::Structural { fields, rest, .. }) => {
            for (key, expected_type) in fields {
                match m.get(&ustr(key)) {
                    Some(v) => check_type_constraint(v, expected_type)?,
                    None => {
                        return Err(
                            format!("type constraint error: missing field '{}'", key).into()
                        );
                    }
                }
            }
            // Check for unexpected fields if `rest` is false
            if !*rest {
                for key in m.keys() {
                    if !fields.iter().any(|(k, _)| k == key.as_str()) {
                        return Err(
                            format!("type constraint error: unexpected field '{}'", key).into()
                        );
                    }
                }
            }
            Ok(())
        }
        _ => Err(format!(
            "type constraint error: expected structural constraint, got '{}'",
            val.type_name()
        )
        .into()),
    }
}

/// Extract referenced identifier names from an expression tree.
/// Used to minimize local variable clones in filter/map pipeline stages.
fn referenced_idents(expr: &Expr) -> FxHashSet<String> {
    let mut idents = FxHashSet::default();
    collect_expr_idents(expr, &mut idents);
    idents
}

fn collect_expr_idents(expr: &Expr, idents: &mut FxHashSet<String>) {
    match expr {
        Expr::Ident(name) | Expr::Variable(name) => {
            idents.insert(name.clone());
        }
        Expr::String(parts) => {
            for part in parts {
                if let StringPart::Expr(e) = part {
                    collect_expr_idents(e, idents);
                }
            }
        }
        Expr::BinaryOp { lhs, rhs, .. } => {
            collect_expr_idents(lhs, idents);
            collect_expr_idents(rhs, idents);
        }
        Expr::Not(inner) => collect_expr_idents(inner, idents),
        Expr::MemberAccess { expr, .. } => collect_expr_idents(expr, idents),
        Expr::List(items) => {
            for item in items {
                collect_expr_idents(item, idents);
            }
        }
        Expr::Map(entries) => {
            for (_, val) in entries {
                collect_expr_idents(val, idents);
            }
        }
        Expr::VarWithModifier { name, .. } => {
            idents.insert(name.clone());
        }
        Expr::ArithmeticExpansion(inner) => collect_expr_idents(inner, idents),
        Expr::If {
            condition,
            then_body,
            else_body,
        } => {
            collect_expr_idents(condition, idents);
            for stmt in then_body {
                collect_stmt_idents(stmt, idents);
            }
            if let Some(body) = else_body {
                for stmt in body {
                    collect_stmt_idents(stmt, idents);
                }
            }
        }
        _ => {}
    }
}

fn collect_stmt_idents(stmt: &Stmt, idents: &mut FxHashSet<String>) {
    match stmt {
        Stmt::Expr(expr) => collect_expr_idents(expr, idents),
        Stmt::Local {
            expr: Some(expr), ..
        }
        | Stmt::Let { expr, .. }
        | Stmt::Assign { expr, .. }
        | Stmt::Return(expr) => collect_expr_idents(expr, idents),
        Stmt::Local { expr: None, .. } => {}
        Stmt::Update { expr, .. } => collect_expr_idents(expr, idents),
        Stmt::While {
            condition, body, ..
        } => {
            collect_expr_idents(condition, idents);
            for s in body {
                collect_stmt_idents(s, idents);
            }
        }
        Stmt::For { iter, body, .. } => {
            collect_expr_idents(iter, idents);
            for s in body {
                collect_stmt_idents(s, idents);
            }
        }
        Stmt::Match { expr, arms } => {
            collect_expr_idents(expr, idents);
            for arm in arms {
                for s in &arm.body {
                    collect_stmt_idents(s, idents);
                }
            }
        }
        Stmt::And(a, b) | Stmt::Or(a, b) => {
            collect_stmt_idents(a, idents);
            collect_stmt_idents(b, idents);
        }
        Stmt::Source { path, .. } => collect_expr_idents(path, idents),
        Stmt::WithCaps { caps, .. } => {
            for cap in caps {
                collect_expr_idents(cap, idents);
            }
        }
        Stmt::FnDef { body, .. } => {
            for s in body {
                collect_stmt_idents(s, idents);
            }
        }
        Stmt::TryCatch { try_body, .. } => {
            for s in try_body {
                collect_stmt_idents(s, idents);
            }
            // catch_var is the error binding — not a variable reference
        }
        _ => {}
    }
}

/// Helper that takes owned parameters to simplify Send/'static bounds for tokio::spawn.
pub fn execute_pipeline_owned(
    pipeline: Pipeline,
    env: Env,
    tx: PipeSender,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>> {
    Box::pin(async move { execute_pipeline(&pipeline, &env, tx).await })
}

/// Set up tokio inter-stage channels and run pipeline steps.
pub async fn execute_pipeline(
    pipeline: &Pipeline,
    env: &Env,
    tx: PipeSender,
) -> Result<(), String> {
    if pipeline.stages.is_empty() {
        return Ok(());
    }

    // Normalize input redirections: if a PipelineStage::Read/Heredoc/HereString occurs after a stage
    // (from trailing `< file` / `<<EOF` / `<<< word` syntax, e.g. `cmd < file` or `grep pat < file`),
    // place the input stage before the stage it feeds so the stream flows into it.
    let mut stages = Vec::new();
    let mut i = 0;
    while i < pipeline.stages.len() {
        if i + 1 < pipeline.stages.len()
            && matches!(
                pipeline.stages[i + 1],
                PipelineStage::Read { .. }
                    | PipelineStage::Heredoc { .. }
                    | PipelineStage::HereString { .. }
            )
        {
            stages.push(pipeline.stages[i + 1].clone());
            stages.push(pipeline.stages[i].clone());
            i += 2;
        } else {
            stages.push(pipeline.stages[i].clone());
            i += 1;
        }
    }

    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let mut current_rx: Option<PipeStream> = None;

    for (idx, stage) in stages.iter().enumerate() {
        if env.job_control.cancellation.load(Ordering::Acquire) {
            break;
        }
        let (stage_tx, stage_rx) = tokio::sync::mpsc::channel(pipeline_channel_size(env));
        let mut env_clone = env.clone();
        let stage_clone = stage.clone();
        let is_last = idx == stages.len() - 1;
        env_clone.is_last_stage = is_last;
        let out_tx = if is_last { tx.clone() } else { stage_tx };

        match stage_clone {
            PipelineStage::CommandCall {
                name,
                args,
                env: inline_env,
                span,
            } => {
                let is_var = if args.is_empty() {
                    let has_var = {
                        let vars = lock_vars!(env_clone.vars.read());
                        vars.contains_key(&name)
                    };
                    let has_cell = if !has_var
                        && env_clone
                            .reactive
                            .has_cells
                            .load(std::sync::atomic::Ordering::Relaxed)
                    {
                        let cells = lock_reactive!(env_clone.reactive.cells.read());
                        cells.contains_key(&name)
                    } else {
                        false
                    };
                    if has_cell || has_var {
                        let is_user_fn = env_clone.fns.read().contains_key(&name);
                        let is_builtin = env.get_builtin(&name).is_some();
                        let env_path = Some(env_clone.vars.read()).and_then(|vars| {
                            vars.get("env").and_then(|v| {
                                if let Val::Map(map) = v {
                                    map.get(&ustr("PATH")).and_then(|pv| {
                                        if let Val::String(s) = pv {
                                            Some(s.clone())
                                        } else {
                                            None
                                        }
                                    })
                                } else {
                                    None
                                }
                            })
                        });
                        let is_external = is_external_command(&name, env_path.as_deref());
                        !(is_user_fn || is_builtin || is_external)
                    } else {
                        false
                    }
                } else {
                    false
                };

                if name.starts_with('$') || is_var {
                    let var_name = if name.starts_with('$') {
                        name.trim_start_matches('$').to_string()
                    } else {
                        name.clone()
                    };
                    let val = {
                        let mut found = {
                            let vars = lock_vars!(env_clone.vars.read());
                            vars.get(&var_name).cloned()
                        };
                        if found.is_none()
                            && env_clone
                                .reactive
                                .has_cells
                                .load(std::sync::atomic::Ordering::Relaxed)
                        {
                            let cells = lock_reactive!(env_clone.reactive.cells.read());
                            if let Some(rx) = cells.get(&var_name) {
                                env_clone.track_cell(var_name.clone());
                                found = Some(Val::List((**rx.borrow()).clone()));
                            }
                        }
                        found.ok_or_else(|| format!("Undefined variable: {}", var_name))?
                    };
                    tokio::spawn(async move {
                        match val {
                            Val::List(list) => {
                                for item in list {
                                    let _ =
                                        out_tx.send(PipelinePayload::Data(Arc::new(item))).await;
                                }
                            }
                            other => {
                                let _ = out_tx.send(PipelinePayload::Data(Arc::new(other))).await;
                            }
                        }
                    });
                } else {
                    // Alias resolution: expand alias before anything else.
                    // Build the full source string by appending serialized args,
                    // re-parse, and eval in the current environment.
                    //
                    // Skip aliases that shadow a builtin, user function, or
                    // variable — they would just cause confusion or infinite
                    // recursion through eval_expr → collect_pipeline.
                    if !env_clone.prompt.alias_suppressed.load(Ordering::Relaxed)
                        && let Some(expansion) = env_clone.get_alias(&name)
                        && env_clone.get_builtin(&name).is_none()
                        && !env_clone.fns.read().contains_key(&name)
                    {
                        let env_for_alias = env_clone.clone();
                        let out_tx_alias = out_tx.clone();
                        let in_rx_alias = current_rx;
                        let cancel_alias = cancel_rx.clone();
                        let cancel_tx_alias = cancel_tx.clone();

                        // Evaluate extra args so variables are resolved before
                        // we serialize them back into source form.
                        let mut extra_args = Vec::new();
                        for arg in args {
                            extra_args.push(eval_expr(&arg, &env_clone).await?);
                        }

                        tokio::spawn(async move {
                            if *cancel_alias.borrow() {
                                return;
                            }
                            // Suppress alias expansion during recursive eval to prevent infinite recursion.
                            env_for_alias
                                .prompt
                                .alias_suppressed
                                .store(true, Ordering::Relaxed);
                            let full_src = expand_alias_with_args(&expansion, &extra_args);
                            // Forward piped input by executing the expansion in
                            // the same eval_stmt path that handles pipelines.
                            // For now, alias expansion does not forward in_rx
                            // (uncommon for most shell aliases); a future
                            // enhancement can thread it through eval_stmt.
                            drop(in_rx_alias);
                            let mut parser = fshell_core::Parser::new(&full_src);
                            let stmts = match parser.parse_statements() {
                                Ok(s) => s,
                                Err(e) => {
                                    let _ = out_tx_alias
                                        .send(PipelinePayload::Structured(
                                            format!("alias '{}' expansion error: {}", name, e)
                                                .into(),
                                        ))
                                        .await;
                                    return;
                                }
                            };
                            for stmt in stmts {
                                if let Stmt::Expr(expr) = stmt.unpack() {
                                    match expr.unpack() {
                                        Expr::Pipeline(pipeline)
                                        | Expr::InlinePipeline(pipeline) => {
                                            let pipeline_clone = pipeline.clone();
                                            let env_exec = env_for_alias.clone();
                                            let out_tx_exec = out_tx_alias.clone();
                                            let handle = tokio::spawn(execute_pipeline_owned(
                                                pipeline_clone,
                                                env_exec,
                                                out_tx_exec,
                                            ));
                                            if let Ok(Err(e)) = handle.await {
                                                let _ = out_tx_alias
                                                    .send(PipelinePayload::Structured(e.into()))
                                                    .await;
                                                return;
                                            }
                                        }
                                        _ => match eval_expr(expr, &env_for_alias).await {
                                            Ok(Val::List(items)) => {
                                                for item in items {
                                                    if out_tx_alias
                                                        .send(PipelinePayload::Data(Arc::new(item)))
                                                        .await
                                                        .is_err()
                                                    {
                                                        break;
                                                    }
                                                }
                                            }
                                            Ok(other) if other != Val::Null => {
                                                let _ = out_tx_alias
                                                    .send(PipelinePayload::Data(Arc::new(other)))
                                                    .await;
                                            }
                                            Err(e) => {
                                                let _ = out_tx_alias
                                                    .send(PipelinePayload::Structured(
                                                        e.to_string().into(),
                                                    ))
                                                    .await;
                                                return;
                                            }
                                            _ => {}
                                        },
                                    }
                                } else {
                                    if let Err(e) = eval_stmt(&stmt, &env_for_alias, false).await {
                                        let _ = out_tx_alias
                                            .send(PipelinePayload::Structured(e.to_string().into()))
                                            .await;
                                        return;
                                    }
                                }
                            }
                            env_for_alias
                                .prompt
                                .alias_suppressed
                                .store(false, Ordering::Relaxed);
                            drop(cancel_tx_alias);
                        });
                    } else {
                        // Alias shadows builtin/function — surface a diagnostic unless silenced.
                        if !env_clone.options.read().quiet_aliases
                            && env_clone.get_alias(&name).is_some()
                            && (env_clone.get_builtin(&name).is_some()
                                || env_clone.fns.read().contains_key(&name))
                        {
                            let _ = out_tx
                                .send(PipelinePayload::Structured(
                                    format!(
                                        "warning: alias '{}' shadows builtin/function — expansion skipped (setopt quiet_aliases to silence)",
                                        name
                                    )
                                    .into(),
                                ))
                                .await;
                        }
                        // Evaluate and apply inline env vars BEFORE evaluating args
                        // so that $VAR references in args resolve to the inline values.
                        let mut evaluated_env: Vec<(String, String)> = Vec::new();
                        for (key, expr) in &inline_env {
                            let val = eval_expr(expr, &env_clone).await?;
                            let val_str = match &val {
                                Val::String(s) => s.clone(),
                                Val::Int(i) => i.to_string(),
                                Val::Float(f) => f.to_string(),
                                Val::Bool(b) => b.to_string(),
                                Val::Null => String::new(),
                                other => format!("{:?}", other),
                            };
                            evaluated_env.push((key.clone(), val_str));
                        }
                        let mut saved_env_values: Vec<(ustr::Ustr, Option<Val>)> = Vec::new();
                        let mut saved_top_values: Vec<(String, Option<Val>)> = Vec::new();
                        if !evaluated_env.is_empty() {
                            let mut vars = env_clone.vars.write();
                            // Set in env map (for external process visibility)
                            let env_map = vars.entry("env".to_string()).or_insert_with(|| {
                                Val::Map(fshell_core::FxIndexMap::with_hasher(
                                    fshell_hash::FxBuildHasher::default(),
                                ))
                            });
                            if let Val::Map(map) = env_map {
                                for (key, val_str) in &evaluated_env {
                                    let ukey = ustr::ustr(key);
                                    saved_env_values.push((ukey, map.get(&ukey).cloned()));
                                    map.insert(ukey, Val::String(val_str.clone()));
                                }
                            }
                            // Set at top level too (for $VAR resolution in args)
                            for (key, val_str) in &evaluated_env {
                                saved_top_values.push((key.clone(), vars.get(key).cloned()));
                                vars.insert(key.clone(), Val::String(val_str.clone()));
                            }
                        }

                        let mut evaluated_args = Vec::new();
                        for arg in args {
                            evaluated_args.push(eval_expr(&arg, &env_clone).await?);
                        }

                        fn has_glob_or_braces(args: &[Val]) -> bool {
                            for arg in args {
                                if let Val::String(s) = arg
                                    && (s.contains('*')
                                        || s.contains('?')
                                        || s.contains('{')
                                        || s.contains('}')
                                        || s.contains(".."))
                                {
                                    return true;
                                }
                            }
                            false
                        }

                        let evaluated_args = if evaluated_args.is_empty() {
                            evaluated_args
                        } else if has_glob_or_braces(&evaluated_args) {
                            let env_for_glob = env_clone.clone();
                            tokio::task::spawn_blocking(move || {
                                expand_globs(evaluated_args, &env_for_glob)
                            })
                            .await
                            .map_err(|e| format!("Glob expansion task failed: {}", e))??
                        } else {
                            expand_globs(evaluated_args, &env_clone)?
                        };
                        // Helper to restore env values after command execution
                        fn restore_inline_env(
                            env: &Env,
                            saved_env: Vec<(ustr::Ustr, Option<Val>)>,
                            saved_top: Vec<(String, Option<Val>)>,
                        ) {
                            {
                                let mut vars = env.vars.write(); // Restore env map entries
                                if let Some(Val::Map(map)) = vars.get_mut("env") {
                                    for (key, orig_val) in saved_env {
                                        match orig_val {
                                            Some(v) => {
                                                map.insert(key, v);
                                            }
                                            None => {
                                                map.swap_remove(&key);
                                            }
                                        }
                                    }
                                }
                                // Restore top-level entries
                                for (key, orig_val) in saved_top {
                                    match orig_val {
                                        Some(v) => {
                                            vars.insert(key, v);
                                        }
                                        None => {
                                            vars.remove(&key);
                                        }
                                    }
                                }
                            }
                        }

                        let mut name = name;
                        let is_interactive = !crate::is_test_mode()
                            && unsafe {
                                current_rx.is_none()
                                    && is_last
                                    && libc::isatty(0) == 1
                                    && crate::is_stdout_a_tty()
                            };

                        let cnf_debug = std::env::var("FSH_CNF_DEBUG").as_deref() == Ok("1");
                        if is_interactive && env_clone.options.read().did_you_mean {
                            let env_path = Some(env_clone.vars.read()).and_then(|vars| {
                                vars.get("PATH")
                                    .and_then(|pv| {
                                        if let Val::String(s) = pv {
                                            Some(s.clone())
                                        } else {
                                            None
                                        }
                                    })
                                    .or_else(|| {
                                        vars.get("env").and_then(|v| {
                                            if let Val::Map(map) = v {
                                                map.get(&ustr("PATH")).and_then(|pv| {
                                                    if let Val::String(s) = pv {
                                                        Some(s.clone())
                                                    } else {
                                                        None
                                                    }
                                                })
                                            } else {
                                                None
                                            }
                                        })
                                    })
                            });
                            let dym_start = std::time::Instant::now();
                            let is_valid = {
                                let is_user_fn = env_clone.fns.read().contains_key(&name);
                                let is_builtin = env_clone.get_builtin(&name).is_some();
                                let is_external = is_external_command(&name, env_path.as_deref());
                                let is_path =
                                    name.contains('/') || std::path::Path::new(&name).exists();
                                is_user_fn || is_builtin || is_external || is_path
                            };
                            if cnf_debug {
                                eprintln!(
                                    "[cnf_debug] {}:{}: DYM check is_valid={}, took={:?}",
                                    file!(),
                                    line!(),
                                    is_valid,
                                    dym_start.elapsed()
                                );
                            }

                            if !is_valid
                                && let Some(corr) =
                                    get_suggested_command(&name, &env_clone, env_path.as_deref())
                            {
                                use std::io::Write;
                                let suggestion_mode = env_clone.options.read().suggestion_mode;
                                match suggestion_mode {
                                    SuggestionMode::Blocking => {
                                        eprintln!(
                                            "\x1b[1;33mCommand not found: {}. Did you mean '{}'? [y/N]\x1b[0m",
                                            name, corr
                                        );
                                        eprint!("> ");
                                        let _ = std::io::stderr().flush();
                                        let mut ans = String::new();
                                        'read: loop {
                                            if env_clone
                                                .job_control
                                                .sigint_pending
                                                .load(Ordering::Acquire)
                                            {
                                                env_clone
                                                    .job_control
                                                    .sigint_pending
                                                    .store(false, Ordering::SeqCst);
                                                env_clone.report_stage_error_code(127);
                                                return Ok(());
                                            }
                                            let mut pfd = libc::pollfd {
                                                fd: 0,
                                                events: libc::POLLIN,
                                                revents: 0,
                                            };
                                            // SAFETY: poll is async-signal-safe. pfd is a
                                            // stack-local struct, fd 0 is stdin.
                                            let ret = unsafe { libc::poll(&mut pfd, 1, 20) };
                                            if ret > 0 {
                                                let mut buf = [0u8; 256];
                                                // SAFETY: read is async-signal-safe. buf is a
                                                // stack array of known size. fd 0 is stdin.
                                                let n = unsafe {
                                                    libc::read(
                                                        0,
                                                        buf.as_mut_ptr() as *mut libc::c_void,
                                                        buf.len(),
                                                    )
                                                };
                                                if n > 0 {
                                                    let s = std::str::from_utf8(&buf[..n as usize])
                                                        .unwrap_or("");
                                                    ans.push_str(s.trim_end_matches(['\n', '\r']));
                                                }
                                                break 'read;
                                            }
                                        }
                                        let choice = ans.trim().to_lowercase();
                                        if choice == "y" || choice == "yes" {
                                            name = corr;
                                        }
                                    }
                                    SuggestionMode::Deferred => {
                                        let arg_strs: Vec<String> =
                                            evaluated_args.iter().map(|v| v.to_text()).collect();
                                        let full_suggestion = if arg_strs.is_empty() {
                                            corr.clone()
                                        } else {
                                            let quoted: Vec<String> = arg_strs
                                                .iter()
                                                .map(|s| {
                                                    if s.contains(' ')
                                                        || s.contains('\'')
                                                        || s.contains('"')
                                                        || s.is_empty()
                                                    {
                                                        let escaped = s.replace('\'', "'\\''");
                                                        format!("'{}'", escaped)
                                                    } else {
                                                        s.clone()
                                                    }
                                                })
                                                .collect();
                                            format!("{} {}", corr, quoted.join(" "))
                                        };
                                        eprintln!(
                                            "\x1b[1;33mCommand not found: {}. Did you mean '{}'? Enter 'd' to run it, or 'e' to edit it.\x1b[0m",
                                            name, full_suggestion
                                        );
                                        env_clone.report_stage_error_code(127);
                                        *env_clone.prompt.pending_suggestion.write() =
                                            Some(PendingSuggestion {
                                                corrected: corr,
                                                args: arg_strs,
                                            });
                                        env_clone
                                            .prompt
                                            .suggestion_deferred
                                            .store(true, Ordering::Release);
                                        return Ok(());
                                    }
                                }
                            }
                        }

                        // Check user-defined functions (single-lock read to avoid TOCTOU)
                        let user_fn = env_clone.fns.read().get(&name).cloned();
                        if let Some((params, _ret_type, body)) = user_fn {
                            let is_posix_fn = body.len() == 1
                                && matches!(body[0].unpack(), Stmt::Comment(s) if s.starts_with("posix fn "));
                            if is_posix_fn && let Some(handler) = crate::posix_handler() {
                                tokio::spawn(async move {
                                    let evaluated_arg_strs: Vec<String> = evaluated_args
                                        .iter()
                                        .map(|v| match v {
                                            Val::String(s) => s.clone(),
                                            other => other.to_text(),
                                        })
                                        .collect();
                                    let cmd_script = format!("{} \"$@\"", name);
                                    match handler(
                                        cmd_script,
                                        evaluated_arg_strs,
                                        env_clone.clone(),
                                        true,
                                    )
                                    .await
                                    {
                                        Ok((_code, Some(bytes))) => {
                                            let _ = out_tx
                                                .send(PipelinePayload::Bytes(bytes.into()))
                                                .await;
                                        }
                                        Ok((_code, None)) => {}
                                        Err(e) => {
                                            env_clone.report_stage_error();
                                            let _ = out_tx
                                                .send(PipelinePayload::Structured(
                                                    e.to_string().into(),
                                                ))
                                                .await;
                                        }
                                    }
                                });
                                return Ok(());
                            }
                            tokio::spawn(async move {
                                let _fn_guard = crate::profiler::ProfilerState::guard(
                                    &env_clone.profiler,
                                    &format!("fn_call {}", name),
                                    crate::profiler::ProfilerCategory::FnCall,
                                );

                                // Build a properly scoped environment: clone the shared state
                                // but use local_vars for function parameters so we never
                                // clobber global variables with the same names.
                                let mut local_map = FxHashMap::default();
                                for (idx, param) in params.iter().enumerate() {
                                    let arg_val =
                                        evaluated_args.get(idx).cloned().unwrap_or(Val::Null);
                                    if let Err(e) =
                                        check_type_constraint(&arg_val, &param.constraint)
                                    {
                                        env_clone.report_stage_error();
                                        let _ = out_tx
                                            .send(PipelinePayload::Structured(e.into()))
                                            .await;
                                        return;
                                    }
                                    local_map.insert(param.name.clone(), arg_val);
                                }
                                let fn_env = env_clone
                                    .push_scope(Arc::new(fshell_core::RwLock::new(local_map)));
                                let mut last_val: Option<Val> = None;
                                'fn_body: for s in &body {
                                    match s.unpack() {
                                        Stmt::Expr(expr) => match eval_expr(expr, &fn_env).await {
                                            Ok(v) => last_val = Some(v),
                                            Err(e) => {
                                                env_clone.report_stage_error();
                                                let diag =
                                                    fshell_core::diagnostic::FshDiag::from(e);
                                                let _ = out_tx
                                                    .send(PipelinePayload::Structured(diag))
                                                    .await;
                                                return;
                                            }
                                        },
                                        _ => match eval_stmt(s, &fn_env, false).await {
                                            Ok(Flow::Normal) => {}
                                            Ok(Flow::Return(ret)) => {
                                                last_val = Some(ret);
                                                break 'fn_body;
                                            }
                                            Ok(Flow::ConditionFalse) => {
                                                env_clone.report_stage_error_code(1);
                                                let diag = fshell_core::diagnostic::FshDiag::from(
                                                    fshell_core::ShellError::condition_false(),
                                                );
                                                let _ = out_tx
                                                    .send(PipelinePayload::Structured(diag))
                                                    .await;
                                                return;
                                            }
                                            Ok(flow) => {
                                                // Stray `break`/`continue`/`exit`/`return`
                                                // inside a function body called from a
                                                // pipeline stage: report as hard error.
                                                env_clone.report_stage_error();
                                                let msg = flow
                                                    .stray_message()
                                                    .unwrap_or_else(|| "control flow".to_string());
                                                let diag = fshell_core::diagnostic::FshDiag::from(
                                                    fshell_core::ShellError::new(
                                                        fshell_core::diagnostic::ErrorCode::InternalError,
                                                        format!("stray `{msg}` in pipeline function"),
                                                    ),
                                                );
                                                let _ = out_tx
                                                    .send(PipelinePayload::Structured(diag))
                                                    .await;
                                                return;
                                            }
                                            Err(e) => {
                                                env_clone.report_stage_error();
                                                let diag =
                                                    fshell_core::diagnostic::FshDiag::from(e);
                                                let _ = out_tx
                                                    .send(PipelinePayload::Structured(diag))
                                                    .await;
                                                return;
                                            }
                                        },
                                    }
                                }
                                // Forward last expression or return value to pipeline.
                                if let Some(v) = last_val
                                    && v != Val::Null
                                {
                                    let _ = out_tx.send(PipelinePayload::Data(Arc::new(v))).await;
                                }
                                // Restore inline env vars
                                restore_inline_env(&env_clone, saved_env_values, saved_top_values);
                            });
                        } else if let Some(handler) = env_clone.get_builtin(&name) {
                            let handler_span = if span.is_empty() { None } else { Some(span) };
                            let stage_cancel = cancel_rx.clone();
                            let cancel = cancel_tx.clone();
                            tokio::task::spawn_blocking(move || {
                                if *stage_cancel.borrow() {
                                    return;
                                }
                                match handler(
                                    current_rx,
                                    evaluated_args,
                                    &env_clone,
                                    out_tx.clone(),
                                    handler_span,
                                ) {
                                    Ok(()) => {
                                        if env_clone.is_last_stage {
                                            let is_pipefail = env_clone.options.read().pipefail;
                                            let mut ec = env_clone.prompt.last_exit_code.write();
                                            if !is_pipefail || *ec == 0 {
                                                *ec = 0;
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        let _ = cancel.send(true);
                                        env_clone.report_stage_error();
                                        let diag = FshDiag::new(e);
                                        let _ =
                                            out_tx.blocking_send(PipelinePayload::Structured(diag));
                                    }
                                }
                                // Restore inline env vars
                                restore_inline_env(&env_clone, saved_env_values, saved_top_values);
                            });
                        } else if let Some(fallback) = env_clone.get_fallback_handler() {
                            let handler_span = if span.is_empty() { None } else { Some(span) };
                            let stage_cancel = cancel_rx.clone();
                            tokio::task::spawn_blocking(move || {
                                if *stage_cancel.borrow() {
                                    return;
                                }
                                let has_next = !is_last;
                                if let Err(e) = fallback(
                                    &name,
                                    evaluated_args,
                                    current_rx,
                                    &env_clone,
                                    out_tx.clone(),
                                    has_next,
                                    handler_span,
                                ) {
                                    let code = if e.code == fshell_core::ErrorCode::CommandNotFound
                                    {
                                        127
                                    } else {
                                        1
                                    };
                                    env_clone.report_stage_error_code(code);
                                    let _ = out_tx.blocking_send(PipelinePayload::Structured(
                                        fshell_core::diagnostic::FshDiag::from(e),
                                    ));
                                }
                                // Restore inline env vars
                                restore_inline_env(&env_clone, saved_env_values, saved_top_values);
                            });
                        } else {
                            return Err(format!("Command not found: {}", name));
                        }
                    }
                }
            }
            PipelineStage::Filter { condition } => {
                let _stage_cancel = cancel_rx.clone();
                let needed = referenced_idents(&condition);
                tokio::spawn(async move {
                    if let Some(mut rx) = current_rx {
                        // Clone env once and reuse across items to avoid per-item Arc batching
                        let mut sub_env = env_clone.clone();
                        while let Some(payload) = rx.recv().await {
                            if env_clone.job_control.cancellation.load(Ordering::Acquire)
                                || *_stage_cancel.borrow()
                            {
                                break;
                            }
                            match payload {
                                PipelinePayload::Data(val_arc) => {
                                    if let Val::Map(map) = &*val_arc {
                                        if needed.is_empty() {
                                            sub_env.scope.local_vars = None;
                                        } else {
                                            let mut locals = FxHashMap::default();
                                            for (k, v) in map {
                                                if needed.contains(k.as_str()) {
                                                    locals.insert(k.to_string(), v.clone());
                                                }
                                            }
                                            sub_env.scope.local_vars =
                                                Some(Arc::new(fshell_core::RwLock::new(locals)));
                                        }
                                    } else {
                                        sub_env.scope.local_vars = None;
                                    }
                                    match eval_expr(&condition, &sub_env).await {
                                        Ok(Val::Bool(true)) => {
                                            let _ =
                                                out_tx.send(PipelinePayload::Data(val_arc)).await;
                                        }
                                        Ok(_) => {}
                                        Err(e) => {
                                            env_clone.report_stage_error();
                                            let _ = out_tx
                                                .send(PipelinePayload::Structured(
                                                    e.to_string().into(),
                                                ))
                                                .await;
                                            break;
                                        }
                                    }
                                }
                                PipelinePayload::Bytes(b) => {
                                    let text = String::from_utf8_lossy(&b);
                                    for line in text.lines() {
                                        if line.is_empty() {
                                            continue;
                                        }
                                        let val_arc = Arc::new(Val::String(line.to_string()));
                                        sub_env.scope.local_vars = None;
                                        match eval_expr(&condition, &sub_env).await {
                                            Ok(Val::Bool(true)) => {
                                                let _ = out_tx
                                                    .send(PipelinePayload::Data(val_arc))
                                                    .await;
                                            }
                                            Ok(_) => {}
                                            Err(e) => {
                                                env_clone.report_stage_error();
                                                let _ = out_tx
                                                    .send(PipelinePayload::Structured(
                                                        e.to_string().into(),
                                                    ))
                                                    .await;
                                                break;
                                            }
                                        }
                                    }
                                }
                                PipelinePayload::Structured(d) => {
                                    let _ = out_tx.send(PipelinePayload::Structured(d)).await;
                                }
                            }
                        }
                    }
                });
            }
            PipelineStage::Map { projections } => {
                let needed: FxHashSet<String> =
                    projections.iter().flat_map(referenced_idents).collect();
                tokio::spawn(async move {
                    if let Some(mut rx) = current_rx {
                        // Clone env once and reuse across items to avoid per-item Arc batching
                        let mut sub_env = env_clone.clone();
                        while let Some(payload) = rx.recv().await {
                            if env_clone.job_control.cancellation.load(Ordering::Acquire) {
                                break;
                            }
                            match payload {
                                PipelinePayload::Data(val_arc) => {
                                    if let Val::Map(map) = &*val_arc {
                                        if needed.is_empty() {
                                            sub_env.scope.local_vars = None;
                                        } else {
                                            let mut locals = FxHashMap::default();
                                            for (k, v) in map {
                                                if needed.contains(k.as_str()) {
                                                    locals.insert(k.to_string(), v.clone());
                                                }
                                            }
                                            sub_env.scope.local_vars =
                                                Some(Arc::new(fshell_core::RwLock::new(locals)));
                                        }
                                    } else {
                                        sub_env.scope.local_vars = None;
                                    }
                                    let mut new_map = indexmap::IndexMap::with_hasher(
                                        fshell_hash::FxBuildHasher::default(),
                                    );
                                    let mut err = None;
                                    for (col_idx, proj) in projections.iter().enumerate() {
                                        match eval_expr(proj, &sub_env).await {
                                            Ok(val) => {
                                                let key_name = match proj {
                                                    Expr::Variable(name) => name.clone(),
                                                    Expr::Ident(ident) => ident.clone(),
                                                    Expr::MemberAccess { member, .. } => {
                                                        member.clone()
                                                    }
                                                    _ => format!("col_{}", col_idx),
                                                };
                                                new_map.insert(ustr::ustr(&key_name), val);
                                            }
                                            Err(e) => {
                                                err = Some(e);
                                                break;
                                            }
                                        }
                                    }
                                    if let Some(e) = err {
                                        env_clone.report_stage_error();
                                        let _ = out_tx
                                            .send(PipelinePayload::Structured(e.to_string().into()))
                                            .await;
                                        break;
                                    } else {
                                        let _ = out_tx
                                            .send(PipelinePayload::Data(Arc::new(Val::Map(
                                                new_map,
                                            ))))
                                            .await;
                                    }
                                }
                                PipelinePayload::Bytes(b) => {
                                    // Boundary conversion: raw bytes -> line-split string Vals.
                                    forward_bytes_as_lines(&out_tx, &b).await;
                                }
                                PipelinePayload::Structured(d) => {
                                    let _ = out_tx.send(PipelinePayload::Structured(d)).await;
                                }
                            }
                        }
                    }
                });
            }
            PipelineStage::Sort { column, descending } => {
                let sort_max_items = env.options.read().sort_max_items;
                tokio::spawn(async move {
                    if let Some(mut rx) = current_rx {
                        let mut items = Vec::new();
                        while let Some(payload) = rx.recv().await {
                            if env_clone.job_control.cancellation.load(Ordering::Acquire) {
                                break;
                            }
                            match payload {
                                PipelinePayload::Data(val_arc) => {
                                    if items.len() >= sort_max_items {
                                        env_clone.report_stage_error();
                                        // Sort what we have so far and send it
                                        items.sort_by(|a: &Arc<Val>, b: &Arc<Val>| {
                                            let val_a = match &**a {
                                                Val::Map(map) => map
                                                    .get(&ustr::ustr(&column))
                                                    .unwrap_or(&Val::Null),
                                                _ => &Val::Null,
                                            };
                                            let val_b = match &**b {
                                                Val::Map(map) => map
                                                    .get(&ustr::ustr(&column))
                                                    .unwrap_or(&Val::Null),
                                                _ => &Val::Null,
                                            };
                                            let cmp = cmp_vals(val_a, val_b);
                                            if descending { cmp.reverse() } else { cmp }
                                        });
                                        for item in items.drain(..) {
                                            let _ = out_tx.send(PipelinePayload::Data(item)).await;
                                        }
                                        let _ = out_tx
                                            .send(PipelinePayload::Structured(
                                                format!(
                                                    "sort: too many items (limit {})",
                                                    sort_max_items
                                                )
                                                .into(),
                                            ))
                                            .await;
                                        // Drain remaining input to avoid back-pressure deadlock
                                        while rx.recv().await.is_some() {}
                                        return;
                                    }
                                    items.push(val_arc);
                                }
                                PipelinePayload::Bytes(b) => {
                                    let text = String::from_utf8_lossy(&b);
                                    for line in text.lines() {
                                        if !line.is_empty() {
                                            items.push(Arc::new(Val::String(line.to_string())));
                                        }
                                    }
                                }
                                PipelinePayload::Structured(d) => {
                                    let _ = out_tx.send(PipelinePayload::Structured(d)).await;
                                }
                            }
                        }
                        items.sort_by(|a, b| {
                            let val_a = match &**a {
                                Val::Map(map) => {
                                    map.get(&ustr::ustr(&column)).unwrap_or(&Val::Null)
                                }
                                other => other,
                            };
                            let val_b = match &**b {
                                Val::Map(map) => {
                                    map.get(&ustr::ustr(&column)).unwrap_or(&Val::Null)
                                }
                                other => other,
                            };
                            let cmp = cmp_vals(val_a, val_b);
                            if descending { cmp.reverse() } else { cmp }
                        });
                        for item in items {
                            let _ = out_tx.send(PipelinePayload::Data(item)).await;
                        }
                    }
                });
            }
            PipelineStage::Grep { pattern } => {
                tokio::spawn(async move {
                    let pat_val = match eval_expr(&pattern, &env_clone).await {
                        Ok(Val::String(s)) => s,
                        Ok(other) => other.to_text(),
                        Err(e) => {
                            env_clone.report_stage_error();
                            let _ = out_tx
                                .send(PipelinePayload::Structured(e.to_string().into()))
                                .await;
                            return;
                        }
                    };
                    if let Some(mut rx) = current_rx {
                        while let Some(payload) = rx.recv().await {
                            if env_clone.job_control.cancellation.load(Ordering::Acquire) {
                                break;
                            }
                            match payload {
                                PipelinePayload::Data(val_arc) => {
                                    let val_str = match &*val_arc {
                                        Val::String(s) => s.clone(),
                                        Val::Map(map) => {
                                            let mut s = String::new();
                                            for (k, v) in map {
                                                s.push_str(k.as_str());
                                                s.push(' ');
                                                s.push_str(&v.to_text());
                                                s.push(' ');
                                            }
                                            s
                                        }
                                        other => other.to_text(),
                                    };
                                    if val_str.contains(&pat_val) {
                                        let _ = out_tx.send(PipelinePayload::Data(val_arc)).await;
                                    }
                                }
                                PipelinePayload::Bytes(b) => {
                                    let text = String::from_utf8_lossy(&b);
                                    for line in text.lines() {
                                        if line.contains(&pat_val) {
                                            let _ = out_tx
                                                .send(PipelinePayload::Data(Arc::new(Val::String(
                                                    line.to_string(),
                                                ))))
                                                .await;
                                        }
                                    }
                                }
                                PipelinePayload::Structured(d) => {
                                    let _ = out_tx.send(PipelinePayload::Structured(d)).await;
                                }
                            }
                        }
                    }
                });
            }
            PipelineStage::Mark { pattern } => {
                let no_color = !env_clone.options.read().error_color;
                let is_tty = crate::is_stdout_a_tty();
                let use_color = is_tty && !no_color;
                tokio::spawn(async move {
                    let pat_val = match eval_expr(&pattern, &env_clone).await {
                        Ok(Val::String(s)) => s,
                        Ok(other) => other.to_text(),
                        Err(e) => {
                            env_clone.report_stage_error();
                            let _ = out_tx
                                .send(PipelinePayload::Structured(e.to_string().into()))
                                .await;
                            return;
                        }
                    };
                    if let Some(mut rx) = current_rx {
                        while let Some(payload) = rx.recv().await {
                            if env_clone.job_control.cancellation.load(Ordering::Acquire) {
                                break;
                            }
                            match payload {
                                PipelinePayload::Data(val_arc) => {
                                    let val_str = match &*val_arc {
                                        Val::String(s) => s.clone(),
                                        Val::Map(map) => {
                                            let mut s = String::new();
                                            for (k, v) in map {
                                                s.push_str(k.as_str());
                                                s.push(' ');
                                                s.push_str(&v.to_text());
                                                s.push(' ');
                                            }
                                            s
                                        }
                                        other => other.to_text(),
                                    };
                                    if val_str.contains(&pat_val) {
                                        let annotated = if use_color {
                                            format!("\x1b[32m> {}\x1b[0m", val_str)
                                        } else {
                                            format!("> {}", val_str)
                                        };
                                        let _ = out_tx
                                            .send(PipelinePayload::Data(Arc::new(Val::String(
                                                annotated,
                                            ))))
                                            .await;
                                    } else {
                                        let _ = out_tx.send(PipelinePayload::Data(val_arc)).await;
                                    }
                                }
                                PipelinePayload::Bytes(b) => {
                                    // Boundary conversion: raw bytes -> line-split string Vals.
                                    forward_bytes_as_lines(&out_tx, &b).await;
                                }
                                PipelinePayload::Structured(d) => {
                                    let _ = out_tx.send(PipelinePayload::Structured(d)).await;
                                }
                            }
                        }
                    }
                });
            }
            PipelineStage::Count => {
                tokio::spawn(async move {
                    if let Some(mut rx) = current_rx {
                        let mut count = 0;
                        while let Some(payload) = rx.recv().await {
                            if env_clone.job_control.cancellation.load(Ordering::Acquire) {
                                break;
                            }
                            match payload {
                                PipelinePayload::Data(_) => {
                                    count += 1;
                                }
                                PipelinePayload::Bytes(b) => {
                                    let newlines = b.iter().filter(|&&byte| byte == b'\n').count();
                                    count += if newlines == 0 && !b.is_empty() {
                                        1
                                    } else {
                                        newlines as i64
                                    };
                                }
                                PipelinePayload::Structured(d) => {
                                    let _ = out_tx.send(PipelinePayload::Structured(d)).await;
                                }
                            }
                        }
                        let _ = out_tx
                            .send(PipelinePayload::Data(Arc::new(Val::Int(count))))
                            .await;
                    }
                });
            }
            PipelineStage::Hash { mode, per_record } => {
                tokio::spawn(async move {
                    if let Some(mut rx) = current_rx {
                        if per_record {
                            while let Some(payload) = rx.recv().await {
                                if env_clone.job_control.cancellation.load(Ordering::Acquire) {
                                    break;
                                }
                                match payload {
                                    PipelinePayload::Data(mut val_arc) => {
                                        let mut hasher = match mode {
                                            fshell_core::HashMode::Hash256 => {
                                                fshell_hash::Hasher::new(0x00, 16)
                                            }
                                            fshell_core::HashMode::Hash512 => {
                                                fshell_hash::Hasher::new(0x04, 16)
                                            }
                                            fshell_core::HashMode::Xof(_) => {
                                                fshell_hash::Hasher::new(0x02, 16)
                                            }
                                        };
                                        let output_len = match mode {
                                            fshell_core::HashMode::Hash256 => 32,
                                            fshell_core::HashMode::Hash512 => 64,
                                            fshell_core::HashMode::Xof(len) => len,
                                        };
                                        if let Ok(bytes) = serde_json::to_vec(val_arc.as_ref()) {
                                            hasher.update(&bytes);
                                        }
                                        let digest = hasher.finalize(output_len);
                                        let mut hash_hex = String::with_capacity(digest.len() * 2);
                                        for b in digest {
                                            hash_hex.push_str(&format!("{:02x}", b));
                                        }

                                        let val = Arc::make_mut(&mut val_arc);
                                        match val {
                                            Val::Map(m) => {
                                                m.insert(
                                                    ustr::ustr("_hash"),
                                                    Val::String(hash_hex),
                                                );
                                            }
                                            other => {
                                                let mut new_m =
                                                    fshell_core::FxIndexMap::with_hasher(
                                                        fshell_hash::FxBuildHasher::default(),
                                                    );
                                                new_m.insert(ustr::ustr("value"), other.clone());
                                                new_m.insert(
                                                    ustr::ustr("_hash"),
                                                    Val::String(hash_hex),
                                                );
                                                *other = Val::Map(new_m);
                                            }
                                        }

                                        if out_tx
                                            .send(PipelinePayload::Data(val_arc))
                                            .await
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    PipelinePayload::Bytes(b) => {
                                        let mut hasher = match mode {
                                            fshell_core::HashMode::Hash256 => {
                                                fshell_hash::Hasher::new(0x00, 16)
                                            }
                                            fshell_core::HashMode::Hash512 => {
                                                fshell_hash::Hasher::new(0x04, 16)
                                            }
                                            fshell_core::HashMode::Xof(_) => {
                                                fshell_hash::Hasher::new(0x02, 16)
                                            }
                                        };
                                        let output_len = match mode {
                                            fshell_core::HashMode::Hash256 => 32,
                                            fshell_core::HashMode::Hash512 => 64,
                                            fshell_core::HashMode::Xof(len) => len,
                                        };
                                        hasher.update(&b);
                                        let digest = hasher.finalize(output_len);
                                        let hash_hex = digest
                                            .iter()
                                            .map(|x| format!("{:02x}", x))
                                            .collect::<String>();
                                        let mut m = fshell_core::FxIndexMap::with_hasher(
                                            fshell_hash::FxBuildHasher::default(),
                                        );
                                        m.insert(ustr::ustr("_hash"), Val::String(hash_hex));
                                        m.insert(
                                            ustr::ustr("_bytes_len"),
                                            Val::Int(b.len() as i64),
                                        );
                                        let _ = out_tx
                                            .send(PipelinePayload::Data(Arc::new(Val::Map(m))))
                                            .await;
                                    }
                                    PipelinePayload::Structured(d) => {
                                        let _ = out_tx.send(PipelinePayload::Structured(d)).await;
                                    }
                                }
                            }
                        } else {
                            let mut count = 0;
                            let mut hasher = match mode {
                                fshell_core::HashMode::Hash256 => {
                                    fshell_hash::Hasher::new(0x00, 16)
                                }
                                fshell_core::HashMode::Hash512 => {
                                    fshell_hash::Hasher::new(0x04, 16)
                                }
                                fshell_core::HashMode::Xof(_) => fshell_hash::Hasher::new(0x02, 16),
                            };
                            let output_len = match mode {
                                fshell_core::HashMode::Hash256 => 32,
                                fshell_core::HashMode::Hash512 => 64,
                                fshell_core::HashMode::Xof(len) => len,
                            };

                            while let Some(payload) = rx.recv().await {
                                if env_clone.job_control.cancellation.load(Ordering::Acquire) {
                                    break;
                                }
                                match payload {
                                    PipelinePayload::Data(val_arc) => {
                                        count += 1;
                                        if let Ok(bytes) = serde_json::to_vec(val_arc.as_ref()) {
                                            hasher.update(&bytes);
                                        }
                                    }
                                    PipelinePayload::Bytes(b) => {
                                        // Boundary conversion: raw bytes -> line-split string Vals.
                                        forward_bytes_as_lines(&out_tx, &b).await;
                                    }
                                    PipelinePayload::Structured(d) => {
                                        let _ = out_tx.send(PipelinePayload::Structured(d)).await;
                                    }
                                }
                            }

                            let digest = hasher.finalize(output_len);
                            let mut hash_hex = String::with_capacity(digest.len() * 2);
                            for b in digest {
                                hash_hex.push_str(&format!("{:02x}", b));
                            }

                            let mut m = fshell_core::FxIndexMap::with_hasher(
                                fshell_hash::FxBuildHasher::default(),
                            );
                            m.insert(ustr::ustr("_count"), Val::Int(count));
                            m.insert(ustr::ustr("_hash"), Val::String(hash_hex));

                            let _ = out_tx
                                .send(PipelinePayload::Data(Arc::new(Val::Map(m))))
                                .await;
                        }
                    }
                });
            }
            PipelineStage::Limit { amount } => {
                let limit_val = eval_expr(&amount, &env_clone).await?;
                let limit_count = match limit_val {
                    Val::Int(i) if i >= 0 => i as usize,
                    other => {
                        return Err(format!(
                            "limit amount must be a positive integer, got {:?}",
                            other
                        ));
                    }
                };

                tokio::spawn(async move {
                    if let Some(mut rx) = current_rx {
                        let mut yielded = 0;
                        while yielded < limit_count {
                            if env_clone.job_control.cancellation.load(Ordering::Acquire) {
                                break;
                            }
                            if let Some(payload) = rx.recv().await {
                                match payload {
                                    PipelinePayload::Data(val_arc) => {
                                        if out_tx
                                            .send(PipelinePayload::Data(val_arc))
                                            .await
                                            .is_err()
                                        {
                                            break;
                                        }
                                        yielded += 1;
                                    }
                                    PipelinePayload::Bytes(b) => {
                                        let text = String::from_utf8_lossy(&b);
                                        for line in text.lines() {
                                            if yielded >= limit_count {
                                                break;
                                            }
                                            if !line.is_empty() {
                                                if out_tx
                                                    .send(PipelinePayload::Data(Arc::new(
                                                        Val::String(line.to_string()),
                                                    )))
                                                    .await
                                                    .is_err()
                                                {
                                                    break;
                                                }
                                                yielded += 1;
                                            }
                                        }
                                    }
                                    PipelinePayload::Structured(d) => {
                                        if out_tx
                                            .send(PipelinePayload::Structured(d))
                                            .await
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                }
                            } else {
                                break;
                            }
                        }
                    }
                });
            }
            PipelineStage::Traverse { edge_label } => {
                let _stage_cancel = cancel_rx.clone();
                tokio::spawn(async move {
                    if let Some(mut rx) = current_rx {
                        while let Some(payload) = rx.recv().await {
                            if env_clone.job_control.cancellation.load(Ordering::Acquire)
                                || *_stage_cancel.borrow()
                            {
                                break;
                            }
                            match payload {
                                PipelinePayload::Data(val_arc) => match &*val_arc {
                                    Val::ObjectGraph { root, graph } => {
                                        let mut sub_env = env_clone.clone();
                                        let mut scoped_vars = sub_env.vars.read().clone();
                                        if let Some(node_data) = graph.nodes.get(root) {
                                            for (k, v) in &node_data.properties {
                                                scoped_vars.insert(k.to_string(), v.clone());
                                            }
                                        }
                                        sub_env.scope.vars = Arc::new(RwLock::new(scoped_vars));

                                        let label_str = match eval_expr(&edge_label, &sub_env).await
                                        {
                                            Ok(Val::String(s)) => s,
                                            Ok(other) => {
                                                env_clone.report_stage_error();
                                                let _ = out_tx.send(PipelinePayload::Structured(
                                                        format!("traverse edge label must evaluate to a string, got {:?}", other).into()
                                                    )).await;
                                                break;
                                            }
                                            Err(e) => {
                                                env_clone.report_stage_error();
                                                let _ = out_tx
                                                    .send(PipelinePayload::Structured(
                                                        e.to_string().into(),
                                                    ))
                                                    .await;
                                                break;
                                            }
                                        };

                                        if let Some(edges) = graph.edges.get(root) {
                                            let ustr_label = ustr::ustr(&label_str);
                                            for edge in edges {
                                                if edge.label == ustr_label {
                                                    let _ = out_tx
                                                        .send(PipelinePayload::Data(Arc::new(
                                                            Val::ObjectGraph {
                                                                root: edge.target,
                                                                graph: graph.clone(),
                                                            },
                                                        )))
                                                        .await;
                                                }
                                            }
                                        }
                                    }
                                    _ => {
                                        env_clone.report_stage_error();
                                        let _ = out_tx
                                            .send(PipelinePayload::Structured(
                                                "traverse operator requires ObjectGraph inputs"
                                                    .to_string()
                                                    .into(),
                                            ))
                                            .await;
                                        break;
                                    }
                                },
                                PipelinePayload::Bytes(b) => {
                                    // Boundary conversion: raw bytes -> line-split string Vals.
                                    forward_bytes_as_lines(&out_tx, &b).await;
                                }
                                PipelinePayload::Structured(d) => {
                                    let _ = out_tx.send(PipelinePayload::Structured(d)).await;
                                }
                            }
                        }
                    }
                });
            }
            PipelineStage::BoundaryOperator { format } => match format {
                SerializationFormat::Json => run_boundary_operator(
                    current_rx,
                    out_tx,
                    &env_clone,
                    |s| {
                        serde_json::from_str::<Val>(s).or_else(|_| {
                            serde_json::from_str::<serde_json::Value>(s)
                                .map(crate::eval::json_value_to_val)
                                .map_err(|e| format!("JSON parse error: {}", e))
                        })
                    },
                    |b| {
                        serde_json::from_slice::<Val>(b).or_else(|_| {
                            serde_json::from_slice::<serde_json::Value>(b)
                                .map(crate::eval::json_value_to_val)
                                .map_err(|e| format!("JSON parse error: {}", e))
                        })
                    },
                    |v| {
                        serde_json::to_string(&crate::eval::val_to_json_value(&v))
                            .map(|s| PipelinePayload::Data(Arc::new(Val::String(s))))
                            .map_err(|e| format!("JSON serialize error: {}", e))
                    },
                ),
                SerializationFormat::Yaml => run_boundary_operator(
                    current_rx,
                    out_tx,
                    &env_clone,
                    |s| {
                        serde_yaml::from_str::<Val>(s)
                            .map_err(|e| format!("YAML parse error: {}", e))
                    },
                    |b| {
                        serde_yaml::from_slice::<Val>(b)
                            .map_err(|e| format!("YAML parse error: {}", e))
                    },
                    |v| {
                        serde_yaml::to_string(&v)
                            .map(|s| PipelinePayload::Data(Arc::new(Val::String(s))))
                            .map_err(|e| format!("YAML serialize error: {}", e))
                    },
                ),
                SerializationFormat::MsgPack => run_boundary_operator(
                    current_rx,
                    out_tx,
                    &env_clone,
                    |s| {
                        rmp_serde::from_slice::<Val>(s.as_bytes())
                            .map_err(|e| format!("MsgPack parse error: {}", e))
                    },
                    |b| {
                        rmp_serde::from_slice::<Val>(b)
                            .map_err(|e| format!("MsgPack parse error: {}", e))
                    },
                    |v| {
                        rmp_serde::to_vec(&v)
                            .map(|b| PipelinePayload::Data(Arc::new(Val::Blob(b))))
                            .map_err(|e| format!("MsgPack serialize error: {}", e))
                    },
                ),
                SerializationFormat::Text => run_boundary_operator(
                    current_rx,
                    out_tx,
                    &env_clone,
                    |s| Ok(Val::String(s.to_string())),
                    |b| Ok(Val::String(String::from_utf8_lossy(b).into_owned())),
                    |v| Ok(PipelinePayload::Data(Arc::new(Val::String(v.to_text())))),
                ),
                SerializationFormat::Csv => {
                    let local_fields = Arc::new(Mutex::new(None));
                    let lf_clone = local_fields.clone();

                    run_boundary_operator(
                        current_rx,
                        out_tx,
                        &env_clone,
                        move |s: &str| decode_csv_input(s),
                        move |b: &[u8]| {
                            let s = std::str::from_utf8(b)
                                .map_err(|e| format!("CSV bytes not valid UTF-8: {e}"))?;
                            decode_csv_input(s)
                        },
                        move |val: Val| match val {
                            Val::Map(map) => {
                                let mut wtr = csv::Writer::from_writer(Vec::new());
                                let fields = {
                                    let mut guard =
                                        lf_clone.lock().unwrap_or_else(|e| e.into_inner());
                                    if guard.is_none() {
                                        let f = Arc::new(
                                            map.keys()
                                                .map(|k| k.to_string())
                                                .collect::<Vec<String>>(),
                                        );
                                        wtr.write_record(&*f)
                                            .map_err(|e| format!("CSV header error: {e}"))?;
                                        *guard = Some(f);
                                    }
                                    guard
                                        .as_ref()
                                        .cloned()
                                        .ok_or_else(|| "CSV headers not initialized".to_string())?
                                };
                                let row: Vec<String> = fields
                                    .iter()
                                    .map(|k| {
                                        map.get(&ustr(k)).map(|v| v.to_text()).unwrap_or_default()
                                    })
                                    .collect();
                                wtr.write_record(&row)
                                    .map_err(|e| format!("CSV row error: {e}"))?;
                                let data = wtr
                                    .into_inner()
                                    .map_err(|e| format!("CSV flush error: {e}"))?;
                                Ok(PipelinePayload::Data(Arc::new(Val::String(
                                    String::from_utf8_lossy(&data).to_string(),
                                ))))
                            }
                            Val::List(items) => {
                                let mut wtr = csv::Writer::from_writer(Vec::new());
                                let mut fields: Option<Arc<Vec<String>>> = None;
                                for item in items {
                                    let map = match item {
                                        Val::Map(m) => m,
                                        other => {
                                            return Err(format!(
                                                "@csv: expected all list items to be Map, got {}",
                                                other.type_name()
                                            ));
                                        }
                                    };
                                    if fields.is_none() {
                                        let mut guard =
                                            lf_clone.lock().unwrap_or_else(|e| e.into_inner());
                                        if guard.is_none() {
                                            let f = Arc::new(
                                                map.keys()
                                                    .map(|k| k.to_string())
                                                    .collect::<Vec<String>>(),
                                            );
                                            wtr.write_record(&*f)
                                                .map_err(|e| format!("CSV header: {e}"))?;
                                            *guard = Some(f);
                                        }
                                        fields = guard.as_ref().cloned();
                                    }
                                    let f = fields.as_ref().ok_or_else(|| {
                                        "CSV list headers not initialized".to_string()
                                    })?;
                                    let row: Vec<String> = f
                                        .iter()
                                        .map(|k| {
                                            map.get(&ustr(k))
                                                .map(|v| v.to_text())
                                                .unwrap_or_default()
                                        })
                                        .collect();
                                    wtr.write_record(&row)
                                        .map_err(|e| format!("CSV row: {e}"))?;
                                }
                                let data =
                                    wtr.into_inner().map_err(|e| format!("CSV flush: {e}"))?;
                                Ok(PipelinePayload::Data(Arc::new(Val::String(
                                    String::from_utf8_lossy(&data).to_string(),
                                ))))
                            }
                            other => Err(format!(
                                "@csv: expected Map or List, got {}",
                                other.type_name()
                            )),
                        },
                    );
                }
                SerializationFormat::Table => {
                    tokio::spawn(async move {
                        let mut items: Vec<Val> = Vec::new();
                        if let Some(mut rx) = current_rx {
                            while let Some(payload) = rx.recv().await {
                                match payload {
                                    PipelinePayload::Data(val) => {
                                        if items.len() < 100_000 {
                                            match Arc::try_unwrap(val) {
                                                Ok(v) => items.push(v),
                                                Err(arc) => items.push((*arc).clone()),
                                            }
                                        }
                                    }
                                    PipelinePayload::Bytes(b) => {
                                        // Boundary conversion: raw bytes -> line-split string Vals.
                                        forward_bytes_as_lines(&out_tx, &b).await;
                                    }
                                    PipelinePayload::Structured(d) => {
                                        let _ = out_tx.send(PipelinePayload::Structured(d)).await;
                                    }
                                }
                            }
                        }
                        if items.len() >= 100_000 {
                            eprintln!("warning: @table capped at 100,000 records");
                        }
                        let mut rendered = render_table(&items);
                        if items.len() >= 100_000 {
                            rendered.push_str(
                                "\n[warning: @table output truncated at 100,000 records]\n",
                            );
                        }
                        let _ = out_tx
                            .send(PipelinePayload::Data(Arc::new(Val::String(rendered))))
                            .await;
                    });
                }
                SerializationFormat::Bar => {
                    tokio::spawn(async move {
                        let mut items: Vec<Val> = Vec::new();
                        if let Some(mut rx) = current_rx {
                            while let Some(payload) = rx.recv().await {
                                match payload {
                                    PipelinePayload::Data(val) => {
                                        if items.len() < 100_000 {
                                            match Arc::try_unwrap(val) {
                                                Ok(v) => items.push(v),
                                                Err(arc) => items.push((*arc).clone()),
                                            }
                                        }
                                    }
                                    PipelinePayload::Bytes(b) => {
                                        // Boundary conversion: raw bytes -> line-split string Vals.
                                        forward_bytes_as_lines(&out_tx, &b).await;
                                    }
                                    PipelinePayload::Structured(d) => {
                                        let _ = out_tx.send(PipelinePayload::Structured(d)).await;
                                    }
                                }
                            }
                        }
                        if items.len() >= 100_000 {
                            eprintln!("warning: @bar capped at 100,000 records");
                        }
                        let mut rendered = render_bar_chart(&items);
                        if items.len() >= 100_000 {
                            rendered.push_str(
                                "\n[warning: @bar output truncated at 100,000 records]\n",
                            );
                        }
                        let _ = out_tx
                            .send(PipelinePayload::Data(Arc::new(Val::String(rendered))))
                            .await;
                    });
                }
            },
            PipelineStage::FdRedirect { src_fd, dst_fd } => {
                // Handle fd-to-fd redirection (e.g., `2>&1` means stderr -> stdout).
                // In fshell's pipeline model, this means merging the two payload channels.
                tokio::spawn(async move {
                    if let Some(mut rx) = current_rx {
                        while let Some(payload) = rx.recv().await {
                            if env_clone.job_control.cancellation.load(Ordering::Acquire) {
                                break;
                            }
                            match payload {
                                PipelinePayload::Data(val_arc) => {
                                    if src_fd == 2 && dst_fd == 1 {
                                        // 2>&1: stderr goes to stdout -> forward Structured as Data
                                        let _ = out_tx.send(PipelinePayload::Data(val_arc)).await;
                                    } else if src_fd == 1 && dst_fd == 2 {
                                        // 1>&2: stdout goes to stderr -> forward Data as Structured
                                        let _ = out_tx
                                            .send(PipelinePayload::Structured(
                                                val_arc.to_text().into(),
                                            ))
                                            .await;
                                    } else if dst_fd == -1 {
                                        // N>&-: close fd N -> drop the payload silently
                                    } else {
                                        // Default forward
                                        let _ = out_tx.send(PipelinePayload::Data(val_arc)).await;
                                    }
                                }
                                PipelinePayload::Bytes(b) => {
                                    // Boundary conversion: raw bytes -> line-split string Vals.
                                    forward_bytes_as_lines(&out_tx, &b).await;
                                }
                                PipelinePayload::Structured(d) => {
                                    if src_fd == 2 && dst_fd == 1 {
                                        // 2>&1: stderr to stdout -> forward as-is (already structured)
                                        let _ = out_tx.send(PipelinePayload::Structured(d)).await;
                                    } else if src_fd == 1 && dst_fd == 2 {
                                        // 1>&2: stdout to stderr -> forward as Structured
                                        let _ = out_tx.send(PipelinePayload::Structured(d)).await;
                                    } else if dst_fd == -1 {
                                        // N>&-: close fd N -> drop
                                    } else {
                                        let _ = out_tx.send(PipelinePayload::Structured(d)).await;
                                    }
                                }
                            }
                        }
                    }
                });
            }
            PipelineStage::Write {
                path,
                append,
                redirect_stdout,
                redirect_stderr,
            } => {
                // Evaluate path expression eagerly, before consumer spawn.
                let path_val = match eval_expr(&path, &env_clone).await {
                    Ok(Val::String(s)) => s,
                    Ok(other) => {
                        env_clone.report_stage_error();
                        let _ = out_tx
                            .send(PipelinePayload::Structured(
                                format!("redirect target must be a string path, got {:?}", other)
                                    .into(),
                            ))
                            .await;
                        continue;
                    }
                    Err(e) => {
                        env_clone.report_stage_error();
                        let _ = out_tx
                            .send(PipelinePayload::Structured(
                                format!("redirect path evaluation error: {}", e).into(),
                            ))
                            .await;
                        continue;
                    }
                };
                let path_buf = std::path::PathBuf::from(&path_val);
                if let Err(e) =
                    env_clone.enforce_capability("write_redirect", CapAction::WriteFile(path_buf))
                {
                    env_clone.report_stage_error();
                    let _ = out_tx.send(PipelinePayload::Structured(e.into())).await;
                    continue;
                }
                let noclobber = env_clone.options.read().noclobber;
                let is_dev_null = path_val == "/dev/null"
                    || path_val.ends_with('/') && path_val.trim_end_matches('/') == "/dev/null";
                if noclobber && !append && !is_dev_null && std::path::Path::new(&path_val).exists()
                {
                    env_clone.report_stage_error();
                    let _ = out_tx
                        .send(PipelinePayload::Structured(
                            format!("noclobber: file '{}' already exists", path_val).into(),
                        ))
                        .await;
                    continue;
                }
                tokio::spawn(async move {
                    let mut opts = OpenOptions::new();
                    opts.write(true).create(true);
                    if append {
                        opts.append(true);
                    } else {
                        opts.truncate(true);
                    }
                    let mut file = match opts.open(&path_val).await {
                        Ok(f) => f,
                        Err(e) => {
                            env_clone.report_stage_error();
                            let _ = out_tx
                                .send(PipelinePayload::Structured(
                                    format!("failed to open redirect target '{}': {}", path_val, e)
                                        .into(),
                                ))
                                .await;
                            return;
                        }
                    };
                    if let Some(mut rx) = current_rx {
                        while let Some(payload) = rx.recv().await {
                            if env_clone.job_control.cancellation.load(Ordering::Acquire) {
                                break;
                            }
                            match payload {
                                PipelinePayload::Data(val) => {
                                    if redirect_stdout {
                                        if let Val::Blob(b) = &*val {
                                            if let Err(e) = file.write_all(b).await {
                                                env_clone.report_stage_error();
                                                let _ = out_tx
                                                    .send(PipelinePayload::Structured(
                                                        format!("redirect write error: {}", e)
                                                            .into(),
                                                    ))
                                                    .await;
                                                return;
                                            }
                                            continue;
                                        }
                                        let mut text = val.to_text();
                                        let mut no_newline = false;
                                        if let Val::String(s) = &*val
                                            && s.ends_with('\0')
                                        {
                                            text = s[..s.len() - 1].to_string();
                                            no_newline = true;
                                        }
                                        if let Err(e) = file.write_all(text.as_bytes()).await {
                                            env_clone.report_stage_error();
                                            let _ = out_tx
                                                .send(PipelinePayload::Structured(
                                                    format!("redirect write error: {}", e).into(),
                                                ))
                                                .await;
                                            return;
                                        }
                                        if !no_newline
                                            && !text.ends_with('\n')
                                            && let Err(e) = file.write_all(b"\n").await
                                        {
                                            env_clone.report_stage_error();
                                            let _ = out_tx
                                                .send(PipelinePayload::Structured(
                                                    format!("redirect write error: {}", e).into(),
                                                ))
                                                .await;
                                            return;
                                        }
                                    } else {
                                        let _ = out_tx.send(PipelinePayload::Data(val)).await;
                                    }
                                }
                                PipelinePayload::Bytes(b) => {
                                    if redirect_stdout {
                                        if let Err(e) = file.write_all(&b).await {
                                            env_clone.report_stage_error();
                                            let _ = out_tx
                                                .send(PipelinePayload::Structured(
                                                    format!("redirect write error: {}", e).into(),
                                                ))
                                                .await;
                                            return;
                                        }
                                    } else {
                                        let _ = out_tx.send(PipelinePayload::Bytes(b)).await;
                                    }
                                }
                                PipelinePayload::Structured(d) => {
                                    if redirect_stderr {
                                        let err_str = d.to_string();
                                        if let Err(e) = file.write_all(err_str.as_bytes()).await {
                                            env_clone.report_stage_error();
                                            let _ = out_tx
                                                .send(PipelinePayload::Structured(
                                                    format!("redirect write error: {}", e).into(),
                                                ))
                                                .await;
                                            return;
                                        }
                                        if let Err(e) = file.write_all(b"\n").await {
                                            let _ = out_tx
                                                .send(PipelinePayload::Structured(
                                                    format!("redirect write error: {}", e).into(),
                                                ))
                                                .await;
                                            return;
                                        }
                                    } else {
                                        let _ = out_tx.send(PipelinePayload::Structured(d)).await;
                                    }
                                }
                            }
                        }
                    }
                    let _ = file.flush().await;
                });
            }
            PipelineStage::Read { path } => {
                let path_val = match eval_expr(&path, &env_clone).await {
                    Ok(Val::String(s)) => s,
                    Ok(other) => {
                        env_clone.report_stage_error();
                        let _ = out_tx
                            .send(PipelinePayload::Structured(
                                format!(
                                    "input redirect target must be a string path, got {:?}",
                                    other
                                )
                                .into(),
                            ))
                            .await;
                        continue;
                    }
                    Err(e) => {
                        env_clone.report_stage_error();
                        let _ = out_tx
                            .send(PipelinePayload::Structured(
                                format!("input redirect path evaluation error: {}", e).into(),
                            ))
                            .await;
                        continue;
                    }
                };

                let path_str = if path_val == "~" {
                    std::env::var("HOME").unwrap_or(path_val)
                } else if let Some(rest) = path_val.strip_prefix("~/") {
                    let home = std::env::var("HOME").unwrap_or_default();
                    if home.is_empty() {
                        path_val
                    } else {
                        format!("{}/{}", home, rest)
                    }
                } else {
                    path_val
                };
                let path_buf = std::path::PathBuf::from(&path_str);
                if let Err(e) = env_clone
                    .enforce_capability("read_redirect", CapAction::ReadFile(path_buf.clone()))
                {
                    env_clone.report_stage_error();
                    let _ = out_tx.send(PipelinePayload::Structured(e.into())).await;
                    continue;
                }

                let is_dev_null = path_str == "/dev/null"
                    || (path_str.ends_with('/') && path_str.trim_end_matches('/') == "/dev/null");
                if is_dev_null {
                    current_rx = Some(stage_rx);
                    continue;
                }

                tokio::spawn(async move {
                    use tokio::io::AsyncBufReadExt;
                    match tokio::fs::File::open(&path_buf).await {
                        Ok(file) => {
                            let mut reader = tokio::io::BufReader::new(file);
                            let mut buf = Vec::new();
                            while let Ok(n) = reader.read_until(b'\n', &mut buf).await {
                                if n == 0 {
                                    break;
                                }
                                if env_clone.job_control.cancellation.load(Ordering::Acquire) {
                                    break;
                                }
                                let val = match std::str::from_utf8(&buf) {
                                    Ok(s) => Val::String(
                                        s.trim_end_matches(&['\r', '\n'][..]).to_string(),
                                    ),
                                    Err(_) => Val::Blob(buf.clone()),
                                };
                                buf.clear();
                                if crate::send_with_backpressure(
                                    &env_clone,
                                    &out_tx,
                                    PipelinePayload::Data(Arc::new(val)),
                                )
                                .await
                                .is_err()
                                {
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            env_clone.report_stage_error();
                            let _ = out_tx
                                .send(PipelinePayload::Structured(
                                    format!("< {}: {}", path_str, e).into(),
                                ))
                                .await;
                        }
                    }
                });
            }
            PipelineStage::Heredoc { content, .. } => {
                let heredoc_val = match eval_expr(&content, &env_clone).await {
                    Ok(v) => v,
                    Err(e) => {
                        env_clone.report_stage_error();
                        let _ = out_tx
                            .send(PipelinePayload::Structured(
                                format!("heredoc evaluation error: {}", e).into(),
                            ))
                            .await;
                        current_rx = Some(stage_rx);
                        continue;
                    }
                };
                let text = match heredoc_val {
                    Val::String(s) => s,
                    other => other.to_text(),
                };
                tokio::spawn(async move {
                    for line in text.lines() {
                        if env_clone.job_control.cancellation.load(Ordering::Acquire) {
                            break;
                        }
                        let val = Val::String(line.to_string());
                        if crate::send_with_backpressure(
                            &env_clone,
                            &out_tx,
                            PipelinePayload::Data(Arc::new(val)),
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                });
            }
            PipelineStage::HereString { content } => {
                let hs_val = match eval_expr(&content, &env_clone).await {
                    Ok(v) => v,
                    Err(e) => {
                        env_clone.report_stage_error();
                        let _ = out_tx
                            .send(PipelinePayload::Structured(
                                format!("here-string evaluation error: {}", e).into(),
                            ))
                            .await;
                        current_rx = Some(stage_rx);
                        continue;
                    }
                };
                let mut text = match hs_val {
                    Val::String(s) => s,
                    other => other.to_text(),
                };
                // Bash adds a trailing newline if not present
                if !text.ends_with('\n') {
                    text.push('\n');
                }
                // Here-string is single string (may contain newlines); send as one item without trailing \n for pipeline consistency
                let send_text = text.trim_end_matches('\n').to_string();
                tokio::spawn(async move {
                    let _ = crate::send_with_backpressure(
                        &env_clone,
                        &out_tx,
                        PipelinePayload::Data(Arc::new(Val::String(send_text))),
                    )
                    .await;
                });
            }
        }
        current_rx = Some(stage_rx);
    }
    Ok(())
}

fn get_path_helper_paths() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        if std::path::Path::new("/usr/libexec/path_helper").exists()
            && let Ok(output) = std::process::Command::new("/usr/libexec/path_helper")
                .arg("-s")
                .output()
            && output.status.success()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Some(start) = text.find("PATH=\"") {
                let rest = &text[start + 6..];
                if let Some(end) = rest.find('"') {
                    let path_str = &rest[..end];
                    return path_str
                        .split(':')
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .collect();
                }
            }
        }
    }
    Vec::new()
}

fn enrich_path(existing_path: &str) -> String {
    let current_paths: Vec<String> = existing_path
        .split(':')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    let mut seen: std::collections::HashSet<String> = current_paths.iter().cloned().collect();
    let mut to_prepend = Vec::new();

    let home = std::env::var("HOME").ok();

    let mut candidates = Vec::new();
    if let Some(ref h) = home {
        candidates.push(format!("{h}/.local/bin"));
    }
    candidates.push("/opt/homebrew/bin".to_string());
    candidates.push("/opt/homebrew/sbin".to_string());
    candidates.push("/usr/local/bin".to_string());
    candidates.push("/usr/local/sbin".to_string());
    if let Some(ref h) = home {
        candidates.push(format!("{h}/.cargo/bin"));
    }

    for ph_path in get_path_helper_paths() {
        if !candidates.contains(&ph_path) {
            candidates.push(ph_path);
        }
    }

    for cand in candidates {
        if !seen.contains(&cand) && std::path::Path::new(&cand).is_dir() {
            seen.insert(cand.clone());
            to_prepend.push(cand);
        }
    }

    if to_prepend.is_empty() {
        existing_path.to_string()
    } else if existing_path.is_empty() {
        to_prepend.join(":")
    } else {
        format!("{}:{}", to_prepend.join(":"), existing_path)
    }
}

/// Populate an Env's `env` variable from the host OS environment.
pub fn populate_env_from_host(env: &Env) {
    env.ensure_env_populated();
    let mut vars = env.vars.write();

    // Expose the most-used env vars as top-level fshell variables for ergonomics.
    // Users can write `echo $HOME` instead of `echo $env.HOME`.
    for key in &["HOME", "USER", "SHELL", "TERM", "PWD", "LANG"] {
        if let Ok(val) = std::env::var(key) {
            vars.insert(key.to_string(), fshell_core::Val::String(val));
        }
    }

    let host_path = std::env::var("PATH").unwrap_or_default();
    let enriched_path = enrich_path(&host_path);
    unsafe {
        std::env::set_var("PATH", &enriched_path);
    }
    vars.insert("PATH".to_string(), fshell_core::Val::String(enriched_path));
}

/// Parse and evaluate fshell source text (a script or inline command).
/// Prints results for expression statements in plain text.
pub async fn run_script(input: &str, env: &Env) -> Result<Flow, EngineError> {
    // Early POSIX delegation for `find ... -exec ... {} +` which parses as fsh
    // but fails at execution (type mismatch). Route via bash where it is valid.
    if crate::login::looks_like_posix(input)
        && let Some(handler) = crate::posix_handler()
    {
        // Only delegate if fsh would mis-handle it (find -exec) or if fsh parse would fail.
        // We try POSIX first for find -exec, otherwise fall through to fsh.
        if input.contains(" -exec ") {
            let (code, _) = handler(input.to_string(), Vec::new(), env.clone(), false).await?;
            env.set_exit_code(code as i64);
            return Ok(Flow::Normal);
        }
    }
    let mut parser = Parser::new(input);
    let stmts = match parser.parse_statements() {
        Ok(s) => s,
        Err(e) => {
            if crate::login::looks_like_posix(input)
                && let Some(handler) = crate::posix_handler()
            {
                let (code, _) = handler(input.to_string(), Vec::new(), env.clone(), false).await?;
                env.set_exit_code(code as i64);
                return Ok(Flow::Normal);
            }
            return Err(e.into());
        }
    };

    let (noexec, verbose) = {
        let opts = env.options.read();
        (opts.noexec, opts.verbose)
    };
    if verbose {
        eprintln!("{}", input);
    }
    if noexec {
        return Ok(Flow::Normal);
    }

    for stmt in stmts {
        let xtrace = env.options.read().xtrace;
        if xtrace {
            let mut current = &stmt;
            let mut span_opt = None;
            while let Stmt::Spanned { stmt: inner, span } = current {
                span_opt = Some(*span);
                current = inner;
            }
            if let Some(span) = span_opt {
                let start = span.offset();
                let end = start + span.len();
                if end <= input.len() {
                    let cmd_str = &input[start..end];
                    eprintln!("+ {}", cmd_str);
                }
            }
        }
        match run_script_stmt(&stmt, env).await {
            Err(e) => {
                env.set_last_error_with_source(
                    FshDiag::new(e.clone()),
                    input.to_string(),
                    "script".to_string(),
                );
                return Err(e);
            }
            Ok(Flow::Normal) | Ok(Flow::ConditionFalse) => {}
            Ok(flow @ Flow::Break) | Ok(flow @ Flow::Continue) | Ok(flow @ Flow::Return(_)) => {
                let msg = flow
                    .stray_message()
                    .unwrap_or_else(|| "control flow".to_string());
                return Err(EngineError::Generic {
                    message: format!("stray `{msg}` at top level"),
                    span: None,
                });
            }
            Ok(Flow::Exit(code)) => return Ok(Flow::Exit(code)),
        }
    }
    Ok(Flow::Normal)
}

pub(crate) fn run_script_stmt<'a>(
    stmt: &'a Stmt,
    env: &'a Env,
) -> Pin<Box<dyn Future<Output = Result<Flow, EngineError>> + Send + 'a>> {
    Box::pin(async move {
        match stmt.unpack() {
            Stmt::And(a, b) => {
                let old_errexit = {
                    let opts = env.options.read();
                    opts.errexit
                };
                if old_errexit {
                    let mut opts = env.options.write();
                    opts.errexit = false;
                }
                let res = run_script_stmt(a, env).await;
                if old_errexit {
                    let mut opts = env.options.write();
                    opts.errexit = true;
                }
                let flow = res?;
                if !flow.is_normal() {
                    return Ok(flow);
                }
                let last_ec = *env.prompt.last_exit_code.read();
                if last_ec == 0 {
                    run_script_stmt(b, env).await
                } else {
                    Ok(Flow::Normal)
                }
            }
            Stmt::Or(a, b) => {
                let old_errexit = {
                    let opts = env.options.read();
                    opts.errexit
                };
                if old_errexit {
                    let mut opts = env.options.write();
                    opts.errexit = false;
                }
                let res = run_script_stmt(a, env).await;
                if old_errexit {
                    let mut opts = env.options.write();
                    opts.errexit = true;
                }
                match res {
                    Ok(Flow::Normal) => {
                        let last_ec = *env.prompt.last_exit_code.read();
                        if last_ec != 0 {
                            run_script_stmt(b, env).await
                        } else {
                            Ok(Flow::Normal)
                        }
                    }
                    Ok(Flow::ConditionFalse) | Err(_) => run_script_stmt(b, env).await,
                    Ok(flow) => Ok(flow),
                }
            }
            Stmt::Expr(expr) => {
                if let Expr::Pipeline(pipeline) = expr.unpack() {
                    let pipefail = env.options.read().pipefail;
                    {
                        let mut ec = env.prompt.last_exit_code.write();
                        *ec = 0;
                    }
                    let mut rx = spawn_pipeline_stream(pipeline, env);
                    let mut errors: Vec<crate::PipelineFailure> = Vec::new();
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
                                let config = {
                                    let opts = env.options.read();
                                    fshell_render::RenderConfig {
                                        format: opts.error_format,
                                        color: opts.error_color,
                                        is_interactive: false,
                                    }
                                };
                                if crate::is_condition_false_diag(&d) {
                                    errors.push(PipelineFailure::ConditionFalse);
                                } else {
                                    env.set_last_error(d.clone());
                                    errors.push(PipelineFailure::Hard(d.clone()));
                                    let err_str =
                                        fshell_render::render(d, None, "pipeline", &config);
                                    eprintln!("{}", err_str);
                                }
                            }
                        }
                    }
                    let last_ec = *env.prompt.last_exit_code.read();
                    let (exit_code, failure) = crate::pipeline_finalize(errors, last_ec, pipefail);
                    env.set_exit_code(exit_code);
                    if env.options.read().errexit && exit_code != 0 {
                        return Ok(Flow::Exit(exit_code as i32));
                    }
                    match failure {
                        Some(PipelineFailure::ConditionFalse) => return Ok(Flow::ConditionFalse),
                        Some(PipelineFailure::Hard(diag)) => {
                            return Err(crate::engine_error_from_diag(&diag));
                        }
                        None => {}
                    }
                } else {
                    let val = eval_expr(expr, env).await?;
                    if val != Val::Null && !matches!(val, Val::Bool(_)) {
                        println!("{}", val.to_text());
                    }
                    let exit_code = match &val {
                        Val::Bool(false) => 1,
                        _ => 0,
                    };
                    env.set_exit_code(exit_code);
                    let errexit_enabled = env.options.read().errexit;
                    if errexit_enabled && exit_code != 0 {
                        return Ok(Flow::Exit(exit_code as i32));
                    }
                }
                Ok(Flow::Normal)
            }
            _ => {
                match eval_stmt(stmt, env, false).await {
                    Ok(Flow::Normal) => {
                        env.set_exit_code(0);
                    }
                    Ok(Flow::ConditionFalse) => {
                        env.set_exit_code(1);
                        return Ok(Flow::ConditionFalse);
                    }
                    Ok(flow) => return Ok(flow),
                    Err(e) => {
                        env.set_exit_code(1);
                        return Err(e);
                    }
                }
                Ok(Flow::Normal)
            }
        }
    })
}

pub(crate) fn val_type_precedence(val: &Val) -> u8 {
    match val {
        Val::Null => 0,
        Val::Bool(_) => 1,
        Val::Int(_) => 2,
        Val::Float(_) => 3,
        Val::String(_) => 4,
        Val::DateTime(_) => 5,
        Val::Blob(_) => 6,
        Val::List(_) => 7,
        Val::Map(_) => 8,
        Val::ObjectGraph { .. } => 9,
        Val::Capability(_) => 10,
        Val::ReactiveStream(_) => 11,
    }
}
