// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use criterion::{Criterion, criterion_group, criterion_main};
use fshell_core::Val;
use fshell_engine::{Env, PipelinePayload};
use std::hint::black_box;
use std::sync::Arc;

fn make_env() -> (tokio::runtime::Runtime, Env) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let env = Env::new();
    fshell_builtins::init(&env);
    fshell_bridge::init(&env);
    (runtime, env)
}

fn builtin_echo_empty(c: &mut Criterion) {
    let (runtime, env) = make_env();
    let echo = env.get_builtin("echo").unwrap();
    c.bench_function("builtins/echo_empty", |b| {
        b.to_async(&runtime).iter(|| {
            let e = env.clone();
            let echo = echo.clone();
            async move {
                let (tx, _rx) = tokio::sync::mpsc::channel(10);
                black_box(echo(None, vec![], &e, tx, None).ok());
            }
        })
    });
}

fn builtin_echo_string(c: &mut Criterion) {
    let (runtime, env) = make_env();
    let echo = env.get_builtin("echo").unwrap();
    let args = vec![Val::String("hello world".to_string())];
    c.bench_function("builtins/echo_string", |b| {
        b.to_async(&runtime).iter(|| {
            let e = env.clone();
            let echo = echo.clone();
            let a = args.clone();
            async move {
                let (tx, _rx) = tokio::sync::mpsc::channel(10);
                black_box(echo(None, a, &e, tx, None).ok());
            }
        })
    });
}

fn builtin_echo_multi(c: &mut Criterion) {
    let (runtime, env) = make_env();
    let echo = env.get_builtin("echo").unwrap();
    let args = vec![
        Val::String("a".to_string()),
        Val::String("b".to_string()),
        Val::String("c".to_string()),
        Val::String("d".to_string()),
        Val::String("e".to_string()),
    ];
    c.bench_function("builtins/echo_multi", |b| {
        b.to_async(&runtime).iter(|| {
            let e = env.clone();
            let echo = echo.clone();
            let a = args.clone();
            async move {
                let (tx, _rx) = tokio::sync::mpsc::channel(10);
                black_box(echo(None, a, &e, tx, None).ok());
            }
        })
    });
}

fn builtin_pwd(c: &mut Criterion) {
    let (runtime, env) = make_env();
    let pwd = env.get_builtin("pwd").unwrap();
    c.bench_function("builtins/pwd", |b| {
        b.to_async(&runtime).iter(|| {
            let e = env.clone();
            let p = pwd.clone();
            async move {
                let (tx, _rx) = tokio::sync::mpsc::channel(10);
                black_box(p(None, vec![], &e, tx, None).ok());
            }
        })
    });
}

fn builtin_head_stream(c: &mut Criterion) {
    let (runtime, env) = make_env();
    let head = env.get_builtin("head").unwrap();
    c.bench_function("builtins/head_stream", |b| {
        b.to_async(&runtime).iter(|| {
            let e = env.clone();
            let h = head.clone();
            async move {
                let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(100);
                let (in_tx, in_rx) = tokio::sync::mpsc::channel(100);
                let _ = h(
                    Some(in_rx),
                    vec![Val::String("-n".to_string()), Val::Int(100)],
                    &e,
                    out_tx,
                    None,
                );
                for i in 0..10_000 {
                    let _ = in_tx
                        .send(PipelinePayload::Data(Arc::new(Val::Int(i))))
                        .await;
                }
                drop(in_tx);
                while out_rx.recv().await.is_some() {}
                black_box(())
            }
        })
    });
}

fn builtin_type_found(c: &mut Criterion) {
    let (runtime, env) = make_env();
    let type_builtin = env.get_builtin("type").unwrap();
    c.bench_function("builtins/type_found", |b| {
        b.to_async(&runtime).iter(|| {
            let e = env.clone();
            let t = type_builtin.clone();
            async move {
                let (tx, _rx) = tokio::sync::mpsc::channel(10);
                black_box(t(None, vec![Val::String("ls".to_string())], &e, tx, None).ok());
            }
        })
    });
}

fn builtin_type_not_found(c: &mut Criterion) {
    let (runtime, env) = make_env();
    let type_builtin = env.get_builtin("type").unwrap();
    c.bench_function("builtins/type_not_found", |b| {
        b.to_async(&runtime).iter(|| {
            let e = env.clone();
            let t = type_builtin.clone();
            async move {
                let (tx, _rx) = tokio::sync::mpsc::channel(10);
                black_box(
                    t(
                        None,
                        vec![Val::String("nonexistent_cmd_xyz".to_string())],
                        &e,
                        tx,
                        None,
                    )
                    .ok(),
                );
            }
        })
    });
}

criterion_group!(
    benches,
    builtin_echo_empty,
    builtin_echo_string,
    builtin_echo_multi,
    builtin_pwd,
    builtin_head_stream,
    builtin_type_found,
    builtin_type_not_found,
);
criterion_main!(benches);
