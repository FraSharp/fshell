// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use criterion::{Criterion, criterion_group, criterion_main};
use fshell_core::{FxIndexMap, ResourceHandle, Val};
use fshell_engine::Env;
use fshell_hash::FxBuildHasher;
use std::hint::black_box;
use ustr::ustr;

fn bridge_external_spawn(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let env = Env::new();
    fshell_builtins::init(&env);
    fshell_bridge::init(&env);

    // Grant process spawn capability
    {
        let mut caps = env.caps.caps.write();
        caps.grant(ResourceHandle::ProcessSpawn);
    }

    c.bench_function("bridge/echo_spawn", |b| {
        b.to_async(&runtime).iter(|| {
            let e = env.clone();
            async move {
                let (tx, mut rx) = tokio::sync::mpsc::channel(10);
                let args = vec![Val::String("hello".to_string())];
                let _ = fshell_bridge::run_external("echo", args, None, &e, tx, false, None);
                while rx.recv().await.is_some() {}
                black_box(())
            }
        })
    });
}

// Sync benchmarks

fn coerce_string(c: &mut Criterion) {
    let val = Val::String("hello world".to_string());
    c.bench_function("bridge/coerce_string", |b| {
        b.iter(|| black_box(fshell_bridge::coerce_val_to_bytes(black_box(&val))))
    });
}

fn coerce_int(c: &mut Criterion) {
    let val = Val::Int(42);
    c.bench_function("bridge/coerce_int", |b| {
        b.iter(|| black_box(fshell_bridge::coerce_val_to_bytes(black_box(&val))))
    });
}

fn coerce_float(c: &mut Criterion) {
    let val = Val::Float(1.5);
    c.bench_function("bridge/coerce_float", |b| {
        b.iter(|| black_box(fshell_bridge::coerce_val_to_bytes(black_box(&val))))
    });
}

fn coerce_list_100(c: &mut Criterion) {
    let items: Vec<Val> = (0..100).map(Val::Int).collect();
    let val = Val::List(items);
    c.bench_function("bridge/coerce_list_100", |b| {
        b.iter(|| black_box(fshell_bridge::coerce_val_to_bytes(black_box(&val))))
    });
}

fn coerce_map_10(c: &mut Criterion) {
    let mut map = FxIndexMap::with_hasher(FxBuildHasher::default());
    for i in 0..10 {
        map.insert(ustr(&format!("key_{}", i)), Val::Int(i));
    }
    let val = Val::Map(map);
    c.bench_function("bridge/coerce_map_10", |b| {
        b.iter(|| black_box(fshell_bridge::coerce_val_to_bytes(black_box(&val))))
    });
}

fn path_resolution_hit(c: &mut Criterion) {
    c.bench_function("bridge/path_resolution_hit", |b| {
        b.iter(|| black_box(fshell_engine::is_external_command(black_box("ls"), None)))
    });
}

fn path_resolution_miss(c: &mut Criterion) {
    c.bench_function("bridge/path_resolution_miss", |b| {
        b.iter(|| {
            black_box(fshell_engine::is_external_command(
                black_box("nonexistent_binary_xyz_12345"),
                None,
            ))
        })
    });
}

criterion_group!(
    benches,
    bridge_external_spawn,
    coerce_string,
    coerce_int,
    coerce_float,
    coerce_list_100,
    coerce_map_10,
    path_resolution_hit,
    path_resolution_miss,
);
criterion_main!(benches);
