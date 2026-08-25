// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![allow(deprecated, clippy::all)]
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use fshell_core::Val;
use fshell_engine::Env;
use fshell_repl::{FshellCompleter, fuzzy};
use reedline::Completer;

fn make_completer_with_vars() -> FshellCompleter {
    let env = Env::new();
    fshell_builtins::init(&env);
    fshell_bridge::init(&env);

    // Add some variables for autocompletion testing
    {
        let mut vars = env.vars.write();
        vars.insert("my_test_var_1".to_string(), Val::Int(1));
        vars.insert(
            "my_test_var_2".to_string(),
            Val::String("hello".to_string()),
        );
        vars.insert("another_variable".to_string(), Val::Bool(true));
    }

    FshellCompleter { env }
}

fn autocomplete_bench(c: &mut Criterion) {
    let mut completer = make_completer_with_vars();

    c.bench_function("repl/complete_builtin", |b| {
        b.iter(|| {
            let results = completer.complete(black_box("he"), 2);
            black_box(results);
        })
    });

    c.bench_function("repl/complete_keyword", |b| {
        b.iter(|| {
            let results = completer.complete(black_box("le"), 2);
            black_box(results);
        })
    });

    c.bench_function("repl/complete_operator", |b| {
        b.iter(|| {
            let results = completer.complete(black_box(" |"), 2);
            black_box(results);
        })
    });

    c.bench_function("repl/complete_builtin_flags", |b| {
        b.iter(|| {
            let results = completer.complete(black_box("ls -"), 4);
            black_box(results);
        })
    });

    c.bench_function("repl/complete_variables", |b| {
        b.iter(|| {
            let results = completer.complete(black_box("$my_"), 4);
            black_box(results);
        })
    });
}

fn fuzzy_matching_bench(c: &mut Criterion) {
    let query = "ls";
    let candidate = "crates/fshell-repl/src/lib.rs";

    c.bench_function("repl/fuzzy_score_simple", |b| {
        b.iter(|| {
            let score = fuzzy::fuzzy_score(
                black_box(query),
                black_box(candidate),
                fuzzy::FuzzyKind::Simple,
            );
            black_box(score);
        })
    });

    c.bench_function("repl/fuzzy_score_smart", |b| {
        b.iter(|| {
            let score = fuzzy::fuzzy_score(
                black_box(query),
                black_box(candidate),
                fuzzy::FuzzyKind::Smart,
            );
            black_box(score);
        })
    });
}

fn history_sqlite_bench(c: &mut Criterion) {
    // Set up temp database path for history testing to avoid polluting real history
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test_history.db");
    unsafe {
        std::env::set_var("FSH_TEST_DB_PATH", db_path.to_str().unwrap());
    }

    // Initialize the DB
    fshell_repl::history::clear_connection_cache();
    fshell_repl::history::init_db().unwrap();

    c.bench_function("repl/history_insert", |b| {
        let mut i = 0i64;
        b.iter(|| {
            i += 1;
            let res = fshell_repl::history::log_command(
                &format!("echo hello {}", i),
                "/tmp",
                1600000000000 + i,
                5,
                Some(0),
                "localhost",
                "user",
                "sess_123",
            );
            black_box(res.unwrap());
        })
    });

    // Seed 1,000 items to query
    for i in 1..1000 {
        let cmd = if i % 10 == 0 {
            format!("cargo build --release {}", i)
        } else {
            format!("ls -la {}", i)
        };
        fshell_repl::history::log_command(
            &cmd,
            if i % 2 == 0 { "/tmp" } else { "/usr/bin" },
            1600000000000 + i as i64,
            10,
            Some(if i % 5 == 0 { 1 } else { 0 }),
            "localhost",
            "user",
            &format!("sess_{}", i % 5),
        )
        .unwrap();
    }

    c.bench_function("repl/history_query_all", |b| {
        b.iter(|| {
            let results = fshell_repl::history::query_history(
                black_box(None),
                black_box(None),
                black_box(None),
                black_box(None),
                black_box(None),
                black_box(None),
            );
            black_box(results.unwrap());
        })
    });

    c.bench_function("repl/history_query_filtered", |b| {
        b.iter(|| {
            let results = fshell_repl::history::query_history(
                black_box(Some(100)),
                black_box(Some("cargo")),
                black_box(Some("/tmp")),
                black_box(None),
                black_box(None),
                black_box(Some(0)),
            );
            black_box(results.unwrap());
        })
    });

    unsafe {
        std::env::remove_var("FSH_TEST_DB_PATH");
    }
}

criterion_group!(
    benches,
    autocomplete_bench,
    fuzzy_matching_bench,
    history_sqlite_bench,
);
criterion_main!(benches);
