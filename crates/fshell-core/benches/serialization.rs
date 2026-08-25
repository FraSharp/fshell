// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use criterion::{Criterion, criterion_group, criterion_main};
use fshell_core::{FxIndexMap, Val};
use fshell_hash::FxBuildHasher;
use std::hint::black_box;
use ustr::ustr;

fn make_test_items(count: usize) -> Vec<Val> {
    let mut items = Vec::with_capacity(count);
    for i in 0..count {
        let mut map = FxIndexMap::with_hasher(FxBuildHasher::default());
        map.insert(ustr("name"), Val::String(format!("item_{}", i)));
        map.insert(
            ustr("group"),
            Val::String(
                match i % 3 {
                    0 => "alpha",
                    1 => "beta",
                    _ => "gamma",
                }
                .to_string(),
            ),
        );
        map.insert(ustr("value"), Val::Int(i as i64));
        items.push(Val::Map(map));
    }
    items
}

fn json_encode_10k(c: &mut Criterion) {
    let items = make_test_items(10_000);
    c.bench_function("json/encode_10k", |b| {
        b.iter(|| {
            for item in &items {
                black_box(serde_json::to_string(item).ok());
            }
        })
    });
}

fn json_decode_10k(c: &mut Criterion) {
    let items = make_test_items(10_000);
    let serialized: Vec<String> = items
        .iter()
        .map(|item| serde_json::to_string(item).unwrap())
        .collect();
    c.bench_function("json/decode_10k", |b| {
        b.iter(|| {
            for s in &serialized {
                let val: Result<Val, _> = serde_json::from_str(s);
                black_box(val.ok());
            }
        })
    });
}

fn yaml_encode_1k(c: &mut Criterion) {
    let items = make_test_items(1_000);
    c.bench_function("yaml/encode_1k", |b| {
        b.iter(|| {
            for item in &items {
                black_box(serde_yaml::to_string(item).ok());
            }
        })
    });
}

fn yaml_decode_1k(c: &mut Criterion) {
    let items = make_test_items(1_000);
    let serialized: Vec<String> = items
        .iter()
        .map(|item| serde_yaml::to_string(item).unwrap())
        .collect();
    c.bench_function("yaml/decode_1k", |b| {
        b.iter(|| {
            for s in &serialized {
                let val: Result<Val, _> = serde_yaml::from_str(s);
                black_box(val.ok());
            }
        })
    });
}

fn msgpack_encode_1k(c: &mut Criterion) {
    let items = make_test_items(1_000);
    c.bench_function("msgpack/encode_1k", |b| {
        b.iter(|| {
            for item in &items {
                black_box(rmp_serde::to_vec(item).ok());
            }
        })
    });
}

fn msgpack_decode_1k(c: &mut Criterion) {
    let items = make_test_items(1_000);
    let serialized: Vec<Vec<u8>> = items
        .iter()
        .map(|item| rmp_serde::to_vec(item).unwrap())
        .collect();
    c.bench_function("msgpack/decode_1k", |b| {
        b.iter(|| {
            for bytes in &serialized {
                let val: Result<Val, _> = rmp_serde::from_slice(bytes);
                black_box(val.ok());
            }
        })
    });
}

fn text_encode_10k(c: &mut Criterion) {
    let items = make_test_items(10_000);
    c.bench_function("text/encode_10k", |b| {
        b.iter(|| {
            for item in &items {
                let _ = black_box(item.to_text());
            }
        })
    });
}

fn text_decode_10k(c: &mut Criterion) {
    let items = make_test_items(10_000);
    let serialized: Vec<Vec<u8>> = items
        .iter()
        .map(|item| item.to_text().into_bytes())
        .collect();
    c.bench_function("text/decode_10k", |b| {
        b.iter(|| {
            for bytes in &serialized {
                let s = String::from_utf8_lossy(bytes).into_owned();
                let val = Val::String(s);
                black_box(val);
            }
        })
    });
}

criterion_group!(
    benches,
    json_encode_10k,
    json_decode_10k,
    yaml_encode_1k,
    yaml_decode_1k,
    msgpack_encode_1k,
    msgpack_decode_1k,
    text_encode_10k,
    text_decode_10k,
);
criterion_main!(benches);
