// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

// crates/fshell-hash/benches/hasher_comparison.rs

use core::hash::Hasher;
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use std::hint::black_box;

fn bench_hasher_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("hasher_comparison");

    for size in [8, 32, 64, 256, 1024] {
        let data = vec![0xABu8; size];
        group.throughput(criterion::Throughput::Bytes(size as u64));

        group.bench_function(format!("fxhash/{size}B"), |b| {
            b.iter(|| {
                let mut h = fxhash::FxHasher::default();
                h.write(black_box(&data));
                h.finish()
            });
        });

        group.bench_function(format!("map_hasher/{size}B"), |b| {
            b.iter(|| {
                let mut h = fshell_hash::MapHasher::new();
                h.write(black_box(&data));
                h.finish()
            });
        });

        group.bench_function(format!("fhash4/{size}B"), |b| {
            b.iter(|| {
                let mut h = fshell_hash::FhashHasher::new();
                h.write(black_box(&data));
                h.finish()
            });
        });
    }

    // Typical fshell workload: short string keys
    let short_keys: Vec<&[u8]> = vec![
        b"name", b"value", b"result", b"items", b"config", b"output", b"status", b"error",
        b"count", b"total",
    ];
    let short_key_bytes = short_keys.iter().map(|key| key.len() as u64).sum();
    group.throughput(criterion::Throughput::Bytes(short_key_bytes));

    group.bench_function("fxhash/10_short_keys", |b| {
        b.iter(|| {
            let mut h = fxhash::FxHasher::default();
            for key in &short_keys {
                h.write(black_box(key));
            }
            h.finish()
        });
    });

    group.bench_function("map_hasher/10_short_keys", |b| {
        b.iter(|| {
            let mut h = fshell_hash::MapHasher::new();
            for key in &short_keys {
                h.write(black_box(key));
            }
            h.finish()
        });
    });

    group.bench_function("fhash4/10_short_keys", |b| {
        b.iter(|| {
            let mut h = fshell_hash::FhashHasher::new();
            for key in &short_keys {
                h.write(black_box(key));
            }
            h.finish()
        });
    });

    group.finish();
}

fn bench_map_operations(c: &mut Criterion) {
    let keys: Vec<String> = (0..128).map(|index| format!("field-{index:04}")).collect();
    let mut deterministic =
        std::collections::HashMap::with_hasher(fshell_hash::MapBuildHasher::default());
    let mut randomized = fshell_hash::RandomHashMap::default();
    for (index, key) in keys.iter().cloned().enumerate() {
        deterministic.insert(key.clone(), index);
        randomized.insert(key, index);
    }

    let mut group = c.benchmark_group("map_operations_128_keys");
    group.bench_function("lookup/deterministic_fast", |b| {
        b.iter(|| {
            for key in &keys {
                black_box(deterministic.get(black_box(key)));
            }
        });
    });
    group.bench_function("lookup/randomized", |b| {
        b.iter(|| {
            for key in &keys {
                black_box(randomized.get(black_box(key)));
            }
        });
    });
    group.bench_function("insert/deterministic_fast", |b| {
        b.iter_batched(
            || keys.clone(),
            |owned_keys| {
                let mut map =
                    std::collections::HashMap::with_hasher(fshell_hash::MapBuildHasher::default());
                for (index, key) in owned_keys.into_iter().enumerate() {
                    map.insert(key, index);
                }
                black_box(map);
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("insert/randomized", |b| {
        b.iter_batched(
            || keys.clone(),
            |owned_keys| {
                let mut map = fshell_hash::RandomHashMap::default();
                for (index, key) in owned_keys.into_iter().enumerate() {
                    map.insert(key, index);
                }
                black_box(map);
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(benches, bench_hasher_comparison, bench_map_operations);
criterion_main!(benches);
