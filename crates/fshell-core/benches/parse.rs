// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use criterion::{Criterion, criterion_group, criterion_main};
use fshell_core::Parser;
use std::hint::black_box;

fn parse_literals(c: &mut Criterion) {
    let inputs = vec![
        "null",
        "true",
        "42",
        "3.14",
        "foo",
        "\"hello\"",
        r#""Hello {name} value is {x}""#,
        "[1, 2, 3]",
        "{a: 1, b: 2}",
        "$x",
    ];
    c.bench_function("parse/literals", |b| {
        b.iter(|| {
            for input in &inputs {
                let _ = black_box(Parser::new(black_box(input)).parse_statements().ok());
            }
        })
    });
}

fn parse_expressions(c: &mut Criterion) {
    let inputs = vec![
        "!flag",
        "obj.field",
        "if x > 5 { \"high\" } else { \"low\" }",
        "1 == 2",
        "1 != 2",
        "x < 5",
        "x <= 5",
        "x > 5",
        "x >= 5",
        "true and false",
        "true or false",
    ];
    c.bench_function("parse/expressions", |b| {
        b.iter(|| {
            for input in &inputs {
                let _ = black_box(Parser::new(black_box(input)).parse_statements().ok());
            }
        })
    });
}

fn parse_statements(c: &mut Criterion) {
    let inputs = vec![
        "let x = 42",
        "x = 42",
        "x += 1",
        "while x > 0 { x -= 1 }",
        "for i in list { echo $i }",
        "break",
        "continue",
        "return 42",
        "try { risky() } catch e { echo $e }",
        "with caps [$process.spawn] { cmd }",
        "unsafe { cmd }",
        "source \"config.fsh\"",
        "match x { 1 => \"one\", _ => \"other\" }",
        "fn greet(name) { let msg = \"Hello, \" + name; echo msg }",
    ];
    c.bench_function("parse/statements", |b| {
        b.iter(|| {
            for input in &inputs {
                let _ = black_box(Parser::new(black_box(input)).parse_statements().ok());
            }
        })
    });
}

fn parse_pipeline(c: &mut Criterion) {
    let input = r#"ls "/usr/bin" | filter type == "file" | sort name | count"#;
    c.bench_function("parse/pipeline", |b| {
        b.iter(|| Parser::new(black_box(input)).parse_statements().ok())
    });
}

fn parse_complex(c: &mut Criterion) {
    let input = r#"
let items = seq 1 100 | collect
let filtered = items | filter value > 50
let result = filtered | sort value | map name
for item in result {
    echo $item
    if item > 75 {
        break
    }
}
match item {
    100 => "max",
    _ => "other"
}"#;
    c.bench_function("parse/complex", |b| {
        b.iter(|| Parser::new(black_box(input)).parse_statements().ok())
    });
}

criterion_group!(
    benches,
    parse_literals,
    parse_expressions,
    parse_statements,
    parse_pipeline,
    parse_complex,
);
criterion_main!(benches);
