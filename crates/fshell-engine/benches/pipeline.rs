// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use criterion::{Criterion, criterion_group, criterion_main};
use fshell_core::{Expr, Pipeline, PipelineStage, StringPart, Val};
use fshell_engine::{Env, PipelinePayload, execute_pipeline};
use miette::SourceSpan;
use std::hint::black_box;

fn make_env_with_items(count: usize) -> (tokio::runtime::Runtime, Env) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let env = Env::new();
    fshell_builtins::init(&env);
    fshell_bridge::init(&env);

    let mut items = Vec::with_capacity(count);
    for i in 0..count {
        let mut map = fshell_core::FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        map.insert(ustr::ustr("name"), Val::String(format!("item_{}", i)));
        map.insert(
            ustr::ustr("group"),
            Val::String(
                match i % 3 {
                    0 => "alpha",
                    1 => "beta",
                    _ => "gamma",
                }
                .to_string(),
            ),
        );
        map.insert(ustr::ustr("value"), Val::Int(i as i64));
        items.push(Val::Map(map));
    }
    {
        let mut vars = env.vars.write();
        vars.insert("items".to_string(), Val::List(items));
    }

    (runtime, env)
}

fn pipeline_filter(c: &mut Criterion) {
    let (runtime, env) = make_env_with_items(10_000);
    let pipeline = Pipeline {
        stages: vec![
            PipelineStage::CommandCall {
                name: "items".to_string(),
                args: vec![],
                env: vec![],
                span: SourceSpan::new(0.into(), 0),
            },
            PipelineStage::Filter {
                condition: Expr::BinaryOp {
                    op: fshell_core::BinOp::Gt,
                    lhs: Box::new(Expr::Ident("value".to_string())),
                    rhs: Box::new(Expr::Int(5000)),
                },
            },
        ],
    };

    c.bench_function("pipeline/filter_10k", |b| {
        b.to_async(&runtime).iter(|| {
            let pipe = pipeline.clone();
            let e = env.clone();
            async move {
                let (tx, mut rx) = tokio::sync::mpsc::channel(100);
                tokio::spawn(async move {
                    let _ = execute_pipeline(&pipe, &e, tx).await;
                });
                let mut count = 0usize;
                while let Some(payload) = rx.recv().await {
                    if matches!(payload, PipelinePayload::Data(_)) {
                        count += 1;
                    }
                }
                black_box(count)
            }
        })
    });
}

fn pipeline_sort(c: &mut Criterion) {
    let (runtime, env) = make_env_with_items(10_000);
    let pipeline = Pipeline {
        stages: vec![
            PipelineStage::CommandCall {
                name: "items".to_string(),
                args: vec![],
                env: vec![],
                span: SourceSpan::new(0.into(), 0),
            },
            PipelineStage::Sort {
                column: "value".to_string(),
                descending: false,
            },
        ],
    };

    c.bench_function("pipeline/sort_10k", |b| {
        b.to_async(&runtime).iter(|| {
            let pipe = pipeline.clone();
            let e = env.clone();
            async move {
                let (tx, mut rx) = tokio::sync::mpsc::channel(100);
                tokio::spawn(async move {
                    let _ = execute_pipeline(&pipe, &e, tx).await;
                });
                let mut count = 0usize;
                while let Some(payload) = rx.recv().await {
                    if matches!(payload, PipelinePayload::Data(_)) {
                        count += 1;
                    }
                }
                black_box(count)
            }
        })
    });
}

fn pipeline_map(c: &mut Criterion) {
    let (runtime, env) = make_env_with_items(10_000);
    let pipeline = Pipeline {
        stages: vec![
            PipelineStage::CommandCall {
                name: "items".to_string(),
                args: vec![],
                env: vec![],
                span: SourceSpan::new(0.into(), 0),
            },
            PipelineStage::Map {
                projections: vec![Expr::Ident("name".to_string())],
            },
        ],
    };

    c.bench_function("pipeline/map_10k", |b| {
        b.to_async(&runtime).iter(|| {
            let pipe = pipeline.clone();
            let e = env.clone();
            async move {
                let (tx, mut rx) = tokio::sync::mpsc::channel(100);
                tokio::spawn(async move {
                    let _ = execute_pipeline(&pipe, &e, tx).await;
                });
                let mut count = 0usize;
                while let Some(payload) = rx.recv().await {
                    if matches!(payload, PipelinePayload::Data(_)) {
                        count += 1;
                    }
                }
                black_box(count)
            }
        })
    });
}

fn pipeline_multi_stage(c: &mut Criterion) {
    let (runtime, env) = make_env_with_items(10_000);
    let pipeline = Pipeline {
        stages: vec![
            PipelineStage::CommandCall {
                name: "items".to_string(),
                args: vec![],
                env: vec![],
                span: SourceSpan::new(0.into(), 0),
            },
            PipelineStage::Filter {
                condition: Expr::BinaryOp {
                    op: fshell_core::BinOp::Gt,
                    lhs: Box::new(Expr::Ident("value".to_string())),
                    rhs: Box::new(Expr::Int(2000)),
                },
            },
            PipelineStage::Sort {
                column: "value".to_string(),
                descending: false,
            },
            PipelineStage::Map {
                projections: vec![Expr::Ident("name".to_string())],
            },
            PipelineStage::Count,
        ],
    };

    c.bench_function("pipeline/multi_stage_10k", |b| {
        b.to_async(&runtime).iter(|| {
            let pipe = pipeline.clone();
            let e = env.clone();
            async move {
                let (tx, mut rx) = tokio::sync::mpsc::channel(100);
                tokio::spawn(async move {
                    let _ = execute_pipeline(&pipe, &e, tx).await;
                });
                let mut count = 0usize;
                while let Some(payload) = rx.recv().await {
                    if matches!(payload, PipelinePayload::Data(_)) {
                        count += 1;
                    }
                }
                black_box(count)
            }
        })
    });
}

fn pipeline_grep_10k(c: &mut Criterion) {
    let (runtime, env) = make_env_with_items(10_000);
    let pipeline = Pipeline {
        stages: vec![
            PipelineStage::CommandCall {
                name: "items".to_string(),
                args: vec![],
                env: vec![],
                span: SourceSpan::new(0.into(), 0),
            },
            PipelineStage::Grep {
                pattern: Expr::String(vec![StringPart::Lit("item_5".to_string())]),
            },
        ],
    };
    c.bench_function("pipeline/grep_10k", |b| {
        b.to_async(&runtime).iter(|| {
            let pipe = pipeline.clone();
            let e = env.clone();
            async move {
                let (tx, mut rx) = tokio::sync::mpsc::channel(100);
                tokio::spawn(async move {
                    let _ = execute_pipeline(&pipe, &e, tx).await;
                });
                let mut count = 0usize;
                while let Some(payload) = rx.recv().await {
                    if matches!(payload, PipelinePayload::Data(_)) {
                        count += 1;
                    }
                }
                black_box(count)
            }
        })
    });
}

fn pipeline_count_10k(c: &mut Criterion) {
    let (runtime, env) = make_env_with_items(10_000);
    let pipeline = Pipeline {
        stages: vec![
            PipelineStage::CommandCall {
                name: "items".to_string(),
                args: vec![],
                env: vec![],
                span: SourceSpan::new(0.into(), 0),
            },
            PipelineStage::Count,
        ],
    };
    c.bench_function("pipeline/count_10k", |b| {
        b.to_async(&runtime).iter(|| {
            let pipe = pipeline.clone();
            let e = env.clone();
            async move {
                let (tx, mut rx) = tokio::sync::mpsc::channel(100);
                tokio::spawn(async move {
                    let _ = execute_pipeline(&pipe, &e, tx).await;
                });
                let mut count = 0usize;
                while let Some(payload) = rx.recv().await {
                    if matches!(payload, PipelinePayload::Data(_)) {
                        count += 1;
                    }
                }
                black_box(count)
            }
        })
    });
}

fn pipeline_limit_10k_100(c: &mut Criterion) {
    let (runtime, env) = make_env_with_items(10_000);
    let pipeline = Pipeline {
        stages: vec![
            PipelineStage::CommandCall {
                name: "items".to_string(),
                args: vec![],
                env: vec![],
                span: SourceSpan::new(0.into(), 0),
            },
            PipelineStage::Limit {
                amount: Expr::Int(100),
            },
        ],
    };
    c.bench_function("pipeline/limit_10k_100", |b| {
        b.to_async(&runtime).iter(|| {
            let pipe = pipeline.clone();
            let e = env.clone();
            async move {
                let (tx, mut rx) = tokio::sync::mpsc::channel(100);
                tokio::spawn(async move {
                    let _ = execute_pipeline(&pipe, &e, tx).await;
                });
                let mut count = 0usize;
                while let Some(payload) = rx.recv().await {
                    if matches!(payload, PipelinePayload::Data(_)) {
                        count += 1;
                    }
                }
                black_box(count)
            }
        })
    });
}

fn pipeline_sort_desc_10k(c: &mut Criterion) {
    let (runtime, env) = make_env_with_items(10_000);
    let pipeline = Pipeline {
        stages: vec![
            PipelineStage::CommandCall {
                name: "items".to_string(),
                args: vec![],
                env: vec![],
                span: SourceSpan::new(0.into(), 0),
            },
            PipelineStage::Sort {
                column: "value".to_string(),
                descending: true,
            },
        ],
    };
    c.bench_function("pipeline/sort_desc_10k", |b| {
        b.to_async(&runtime).iter(|| {
            let pipe = pipeline.clone();
            let e = env.clone();
            async move {
                let (tx, mut rx) = tokio::sync::mpsc::channel(100);
                tokio::spawn(async move {
                    let _ = execute_pipeline(&pipe, &e, tx).await;
                });
                let mut count = 0usize;
                while let Some(payload) = rx.recv().await {
                    if matches!(payload, PipelinePayload::Data(_)) {
                        count += 1;
                    }
                }
                black_box(count)
            }
        })
    });
}

fn pipeline_filter_string_10k(c: &mut Criterion) {
    let (runtime, env) = make_env_with_items(10_000);
    let pipeline = Pipeline {
        stages: vec![
            PipelineStage::CommandCall {
                name: "items".to_string(),
                args: vec![],
                env: vec![],
                span: SourceSpan::new(0.into(), 0),
            },
            PipelineStage::Filter {
                condition: Expr::BinaryOp {
                    op: fshell_core::BinOp::Eq,
                    lhs: Box::new(Expr::Ident("group".to_string())),
                    rhs: Box::new(Expr::String(vec![StringPart::Lit("alpha".to_string())])),
                },
            },
        ],
    };
    c.bench_function("pipeline/filter_string_10k", |b| {
        b.to_async(&runtime).iter(|| {
            let pipe = pipeline.clone();
            let e = env.clone();
            async move {
                let (tx, mut rx) = tokio::sync::mpsc::channel(100);
                tokio::spawn(async move {
                    let _ = execute_pipeline(&pipe, &e, tx).await;
                });
                let mut count = 0usize;
                while let Some(payload) = rx.recv().await {
                    if matches!(payload, PipelinePayload::Data(_)) {
                        count += 1;
                    }
                }
                black_box(count)
            }
        })
    });
}

fn pipeline_multi_stage_all_10k(c: &mut Criterion) {
    let (runtime, env) = make_env_with_items(10_000);
    let pipeline = Pipeline {
        stages: vec![
            PipelineStage::CommandCall {
                name: "items".to_string(),
                args: vec![],
                env: vec![],
                span: SourceSpan::new(0.into(), 0),
            },
            PipelineStage::Filter {
                condition: Expr::BinaryOp {
                    op: fshell_core::BinOp::Gt,
                    lhs: Box::new(Expr::Ident("value".to_string())),
                    rhs: Box::new(Expr::Int(2000)),
                },
            },
            PipelineStage::Sort {
                column: "value".to_string(),
                descending: false,
            },
            PipelineStage::Grep {
                pattern: Expr::Ident("item".to_string()),
            },
            PipelineStage::Map {
                projections: vec![Expr::Ident("name".to_string())],
            },
            PipelineStage::Limit {
                amount: Expr::Int(100),
            },
            PipelineStage::Count,
        ],
    };
    c.bench_function("pipeline/multi_stage_all_10k", |b| {
        b.to_async(&runtime).iter(|| {
            let pipe = pipeline.clone();
            let e = env.clone();
            async move {
                let (tx, mut rx) = tokio::sync::mpsc::channel(100);
                tokio::spawn(async move {
                    let _ = execute_pipeline(&pipe, &e, tx).await;
                });
                let mut count = 0usize;
                while let Some(payload) = rx.recv().await {
                    if matches!(payload, PipelinePayload::Data(_)) {
                        count += 1;
                    }
                }
                black_box(count)
            }
        })
    });
}

criterion_group!(
    benches,
    pipeline_filter,
    pipeline_sort,
    pipeline_map,
    pipeline_multi_stage,
    pipeline_grep_10k,
    pipeline_count_10k,
    pipeline_limit_10k_100,
    pipeline_sort_desc_10k,
    pipeline_filter_string_10k,
    pipeline_multi_stage_all_10k,
);
criterion_main!(benches);
