// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use fshell_search::{
    Control,
    content::{CompiledMatcher, MatchOptions, ScanOptions, Scanner},
    traversal::{FileKind, Hooks, NamePredicate, TraversalPlan, walk},
};
use std::{
    hint::black_box,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

fn buffered_counts(c: &mut Criterion) {
    let mut bytes = Vec::with_capacity(1024 * 1024);
    for i in 0..16384 {
        bytes.extend_from_slice(if i % 100 == 0 { b"needle " } else { b"plain  " });
        bytes.extend(std::iter::repeat_n(b'x', 56));
        bytes.push(b'\n');
    }
    let mut group = c.benchmark_group("buffered_count_1mib");
    group.throughput(Throughput::Bytes(bytes.len() as u64));
    for (name, pattern, expected) in [
        ("dense", "x", 16384),
        ("sparse", "needle", 164),
        ("none", "absenttoken", 0),
    ] {
        let matcher = CompiledMatcher::compile(
            &[pattern.into()],
            &MatchOptions {
                fixed: true,
                ..MatchOptions::default()
            },
        )
        .expect("fixture matcher must compile");
        let mut scanner = Scanner::new(ScanOptions {
            count_only: true,
            ..ScanOptions::default()
        })
        .expect("count scanner must initialize");
        let control = Control::default();
        let scan = |scanner: &mut Scanner| {
            scanner
                .search(
                    &matcher,
                    black_box(bytes.as_slice()),
                    &control,
                    || false,
                    |_| Err(std::io::Error::other("count unexpectedly emitted a result")),
                )
                .expect("count scan must succeed without constructing results")
                .matches
        };
        assert_eq!(scan(&mut scanner), expected);
        group.bench_function(name, |b| b.iter(|| black_box(scan(&mut scanner))));
    }
    group.finish();
}

fn name_discovery(c: &mut Criterion) {
    let fixture = tempfile::tempdir().expect("traversal fixture directory must be created");
    for directory in 0..50 {
        let path = fixture.path().join(format!("d{directory:03}"));
        std::fs::create_dir(&path).expect("fixture subdirectory must be created");
        for file in 0..200 {
            std::fs::write(path.join(format!("f{file:03}.rs")), b"")
                .expect("fixture file must be created");
        }
    }
    let mut group = c.benchmark_group("names_only_10k");
    group.throughput(Throughput::Elements(10000));
    for workers in [1, 8] {
        let mut plan = TraversalPlan::new(fixture.path().into());
        plan.threads = workers;
        plan.respect_ignore = false;
        plan.kind = Some(FileKind::File);
        plan.names
            .push(NamePredicate::compile("*.rs", false, false).expect("fixture glob must compile"));
        let count = AtomicU64::new(0);
        let run = || {
            count.store(0, Ordering::Relaxed);
            walk(
                &plan,
                Hooks::unrestricted(),
                Arc::new(Control::default()),
                |entry| {
                    assert!(entry.metadata.is_none());
                    count.fetch_add(1, Ordering::Relaxed);
                    Ok(true)
                },
            )
            .expect("fixture traversal must succeed");
            count.load(Ordering::Relaxed)
        };
        assert_eq!(run(), 10000);
        group.bench_with_input(BenchmarkId::new("workers", workers), &workers, |b, _| {
            b.iter(|| black_box(run()))
        });
    }
    group.finish();
}

criterion_group!(benches, buffered_counts, name_discovery);
criterion_main!(benches);
