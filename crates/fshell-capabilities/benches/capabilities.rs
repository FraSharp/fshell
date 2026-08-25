// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![allow(deprecated)]
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use fshell_capabilities::CapsRegistry;
use fshell_core::ResourceHandle;
use std::path::{Path, PathBuf};

fn caps_check_read_dir_hit(c: &mut Criterion) {
    let mut registry = CapsRegistry::new();
    for i in 0..100 {
        registry.grant(ResourceHandle::ReadDir(PathBuf::from(format!(
            "/dir/{}",
            i
        ))));
    }
    let test_path = Path::new("/dir/50/sub/file.txt");
    c.bench_function("caps/check_read_dir_hit", |b| {
        b.iter(|| registry.check_read_dir(black_box(test_path)))
    });
}

fn caps_check_read_dir_miss(c: &mut Criterion) {
    let mut registry = CapsRegistry::new();
    for i in 0..100 {
        registry.grant(ResourceHandle::ReadDir(PathBuf::from(format!(
            "/dir/{}",
            i
        ))));
    }
    let test_path = Path::new("/other/99/sub/file.txt");
    c.bench_function("caps/check_read_dir_miss", |b| {
        b.iter(|| registry.check_read_dir(black_box(test_path)))
    });
}

fn caps_check_write_dir_hit(c: &mut Criterion) {
    let mut registry = CapsRegistry::new();
    for i in 0..100 {
        registry.grant(ResourceHandle::WriteDir(PathBuf::from(format!(
            "/dir/{}",
            i
        ))));
    }
    let test_path = Path::new("/dir/50/sub/file.txt");
    c.bench_function("caps/check_write_dir_hit", |b| {
        b.iter(|| registry.check_write_dir(black_box(test_path)))
    });
}

fn caps_check_network_all(c: &mut Criterion) {
    let mut registry = CapsRegistry::new();
    registry.grant(ResourceHandle::NetworkAll);
    c.bench_function("caps/check_network_all", |b| {
        b.iter(|| registry.check_network(black_box("any.host.example.com")))
    });
}

fn caps_check_network_socket(c: &mut Criterion) {
    let mut registry = CapsRegistry::new();
    for i in 0..100 {
        registry.grant(ResourceHandle::NetworkSocket(format!(
            "host{}.example.com",
            i
        )));
    }
    c.bench_function("caps/check_network_socket", |b| {
        b.iter(|| registry.check_network(black_box("host50.example.com")))
    });
}

fn caps_check_env_read_wildcard(c: &mut Criterion) {
    let mut registry = CapsRegistry::new();
    registry.grant(ResourceHandle::ReadEnv("*".to_string()));
    c.bench_function("caps/check_env_read_wildcard", |b| {
        b.iter(|| registry.check_env_read(black_box("PATH")))
    });
}

fn caps_check_env_read_specific(c: &mut Criterion) {
    let mut registry = CapsRegistry::new();
    for i in 0..100 {
        registry.grant(ResourceHandle::ReadEnv(format!("VAR_{}", i)));
    }
    c.bench_function("caps/check_env_read_specific", |b| {
        b.iter(|| registry.check_env_read(black_box("VAR_50")))
    });
}

fn caps_check_process_spawn_granted(c: &mut Criterion) {
    let mut registry = CapsRegistry::new();
    registry.grant(ResourceHandle::ProcessSpawn);
    c.bench_function("caps/check_process_spawn_granted", |b| {
        b.iter(|| registry.check_process_spawn("test_cmd"))
    });
}

fn caps_check_process_spawn_denied(c: &mut Criterion) {
    let registry = CapsRegistry::new();
    c.bench_function("caps/check_process_spawn_denied", |b| {
        b.iter(|| registry.check_process_spawn("test_cmd"))
    });
}

fn caps_grant_100(c: &mut Criterion) {
    c.bench_function("caps/grant_100", |b| {
        b.iter(|| {
            let mut registry = CapsRegistry::new();
            for i in 0..100 {
                registry.grant(ResourceHandle::ReadDir(PathBuf::from(format!(
                    "/dir/{}",
                    i
                ))));
            }
            black_box(registry)
        })
    });
}

fn caps_revoke_100(c: &mut Criterion) {
    c.bench_function("caps/revoke_100", |b| {
        b.iter(|| {
            let mut registry = CapsRegistry::new();
            let handles: Vec<ResourceHandle> = (0..100)
                .map(|i| ResourceHandle::ReadDir(PathBuf::from(format!("/dir/{}", i))))
                .collect();
            for h in &handles {
                registry.grant(h.clone());
            }
            for h in &handles {
                registry.revoke(h);
            }
            black_box(registry)
        })
    });
}

criterion_group!(
    benches,
    caps_check_read_dir_hit,
    caps_check_read_dir_miss,
    caps_check_write_dir_hit,
    caps_check_network_all,
    caps_check_network_socket,
    caps_check_env_read_wildcard,
    caps_check_env_read_specific,
    caps_check_process_spawn_granted,
    caps_check_process_spawn_denied,
    caps_grant_100,
    caps_revoke_100,
);
criterion_main!(benches);
