// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![allow(
    deprecated,
    clippy::collapsible_if,
    clippy::field_reassign_with_default
)]
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use fshell_core::Val;
use fshell_engine::{Env, PipelinePayload};
use fshell_sandbox::{SandboxConfig, SandboxMode};

fn make_test_env() -> Env {
    let env = Env::new();
    {
        let mut vars = env.vars.write();
        vars.insert("env".to_string(), Val::empty_map());
    }
    env
}

fn find_true_command() -> &'static str {
    if std::path::Path::new("/bin/true").exists() {
        "/bin/true"
    } else {
        "/usr/bin/true"
    }
}

fn sandbox_spawn_off(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let env = make_test_env();
    let true_cmd = find_true_command();

    let mut config = SandboxConfig::default();
    config.mode = SandboxMode::Off;

    c.bench_function("sandbox/spawn_off", |b| {
        b.to_async(&runtime).iter(|| {
            let e = env.clone();
            let cfg = config.clone();
            async move {
                let (tx, mut rx) = tokio::sync::mpsc::channel(100);
                fshell_sandbox::run_sandboxed(true_cmd, &[], None, &e, tx, &cfg).unwrap();
                while let Some(payload) = rx.recv().await {
                    if let PipelinePayload::Data(data) = payload {
                        if let Val::String(s) = &*data {
                            if s.starts_with("\0exit:") {
                                break;
                            }
                        }
                    }
                }
                black_box(())
            }
        })
    });
}

fn sandbox_spawn_kernel(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let env = make_test_env();
    let true_cmd = find_true_command();

    let config = SandboxConfig::default();

    c.bench_function("sandbox/spawn_kernel", |b| {
        b.to_async(&runtime).iter(|| {
            let e = env.clone();
            let cfg = config.clone();
            async move {
                let (tx, mut rx) = tokio::sync::mpsc::channel(100);
                fshell_sandbox::run_sandboxed(true_cmd, &[], None, &e, tx, &cfg).unwrap();
                while let Some(payload) = rx.recv().await {
                    if let PipelinePayload::Data(data) = payload {
                        if let Val::String(s) = &*data {
                            if s.starts_with("\0exit:") {
                                break;
                            }
                        }
                    }
                }
                black_box(())
            }
        })
    });
}

criterion_group!(benches, sandbox_spawn_off, sandbox_spawn_kernel,);
criterion_main!(benches);
