// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use criterion::{Criterion, criterion_group, criterion_main};
use fshell_core::{
    BinOp, Expr, FxIndexMap, LiteralPattern, MatchArm, MatchPattern, Stmt, StringPart, Val,
};
use fshell_engine::{Env, eval_expr, eval_stmt};
use fshell_hash::FxBuildHasher;
use std::hint::black_box;
use ustr::ustr;

fn make_env() -> (tokio::runtime::Runtime, Env) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let env = Env::new();
    fshell_builtins::init(&env);
    fshell_bridge::init(&env);
    {
        let mut vars = env.vars.write();
        vars.insert("x".to_string(), Val::Int(42));
        vars.insert("name".to_string(), Val::String("fshell".to_string()));
        vars.insert("flag".to_string(), Val::Bool(true));
        let mut map = FxIndexMap::with_hasher(FxBuildHasher::default());
        map.insert(ustr("a"), Val::Int(1));
        let mut inner = FxIndexMap::with_hasher(FxBuildHasher::default());
        inner.insert(ustr("b"), Val::Int(2));
        map.insert(ustr("sub"), Val::Map(inner));
        vars.insert("obj".to_string(), Val::Map(map));
        vars.insert(
            "items".to_string(),
            Val::List(vec![Val::Int(1), Val::Int(2), Val::Int(3)]),
        );
    }
    (runtime, env)
}

fn eval_literals_and_expressions(c: &mut Criterion) {
    let (runtime, env) = make_env();
    let exprs = vec![
        Expr::Null,
        Expr::Bool(true),
        Expr::Int(42),
        Expr::Float(1.5),
        Expr::Ident("x".to_string()),
        Expr::Ident("unknown".to_string()),
        Expr::String(vec![StringPart::Lit("hello".to_string())]),
        Expr::String(vec![
            StringPart::Lit("Hello, ".to_string()),
            StringPart::Expr(Box::new(Expr::Variable("name".to_string()))),
            StringPart::Lit("! Value is ".to_string()),
            StringPart::Expr(Box::new(Expr::Variable("x".to_string()))),
        ]),
        Expr::List(vec![Expr::Int(1), Expr::Int(2), Expr::Int(3)]),
        Expr::Map(vec![("a".to_string(), Expr::Int(1))]),
        Expr::Variable("x".to_string()),
        Expr::Not(Box::new(Expr::Bool(false))),
        Expr::Not(Box::new(Expr::Int(0))),
        Expr::MemberAccess {
            expr: Box::new(Expr::Variable("obj".to_string())),
            member: "sub".to_string(),
        },
        Expr::If {
            condition: Box::new(Expr::Bool(true)),
            then_body: vec![Stmt::Expr(Expr::Int(1))],
            else_body: None,
        },
        Expr::If {
            condition: Box::new(Expr::Bool(false)),
            then_body: vec![Stmt::Expr(Expr::Int(1))],
            else_body: Some(vec![Stmt::Expr(Expr::Int(2))]),
        },
    ];
    c.bench_function("eval/literals_and_expressions", |b| {
        b.to_async(&runtime).iter(|| async {
            for expr in &exprs {
                let _ = black_box(eval_expr(black_box(expr), black_box(&env)).await.ok());
            }
        })
    });
}

fn eval_binary_ops(c: &mut Criterion) {
    let (runtime, env) = make_env();
    let exprs = vec![
        Expr::BinaryOp {
            op: BinOp::Add,
            lhs: Box::new(Expr::Int(1)),
            rhs: Box::new(Expr::Int(2)),
        },
        Expr::BinaryOp {
            op: BinOp::Add,
            lhs: Box::new(Expr::Float(1.5)),
            rhs: Box::new(Expr::Float(2.5)),
        },
        Expr::BinaryOp {
            op: BinOp::Add,
            lhs: Box::new(Expr::Int(1)),
            rhs: Box::new(Expr::Float(2.5)),
        },
        Expr::BinaryOp {
            op: BinOp::Add,
            lhs: Box::new(Expr::String(vec![StringPart::Lit("hello".to_string())])),
            rhs: Box::new(Expr::String(vec![StringPart::Lit("world".to_string())])),
        },
        Expr::BinaryOp {
            op: BinOp::Sub,
            lhs: Box::new(Expr::Int(10)),
            rhs: Box::new(Expr::Int(3)),
        },
        Expr::BinaryOp {
            op: BinOp::Mul,
            lhs: Box::new(Expr::Int(6)),
            rhs: Box::new(Expr::Int(7)),
        },
        Expr::BinaryOp {
            op: BinOp::Div,
            lhs: Box::new(Expr::Int(10)),
            rhs: Box::new(Expr::Int(3)),
        },
        Expr::BinaryOp {
            op: BinOp::Div,
            lhs: Box::new(Expr::Int(5)),
            rhs: Box::new(Expr::Int(0)),
        },
        Expr::BinaryOp {
            op: BinOp::Eq,
            lhs: Box::new(Expr::Int(1)),
            rhs: Box::new(Expr::Int(1)),
        },
        Expr::BinaryOp {
            op: BinOp::Eq,
            lhs: Box::new(Expr::Int(1)),
            rhs: Box::new(Expr::Int(2)),
        },
        Expr::BinaryOp {
            op: BinOp::Neq,
            lhs: Box::new(Expr::Int(1)),
            rhs: Box::new(Expr::Int(2)),
        },
        Expr::BinaryOp {
            op: BinOp::Lt,
            lhs: Box::new(Expr::Int(3)),
            rhs: Box::new(Expr::Int(7)),
        },
        Expr::BinaryOp {
            op: BinOp::Lte,
            lhs: Box::new(Expr::Int(5)),
            rhs: Box::new(Expr::Int(5)),
        },
        Expr::BinaryOp {
            op: BinOp::Gt,
            lhs: Box::new(Expr::Int(7)),
            rhs: Box::new(Expr::Int(3)),
        },
        Expr::BinaryOp {
            op: BinOp::Gte,
            lhs: Box::new(Expr::Int(5)),
            rhs: Box::new(Expr::Int(5)),
        },
        Expr::BinaryOp {
            op: BinOp::And,
            lhs: Box::new(Expr::Bool(true)),
            rhs: Box::new(Expr::Bool(true)),
        },
        Expr::BinaryOp {
            op: BinOp::And,
            lhs: Box::new(Expr::Bool(false)),
            rhs: Box::new(Expr::Bool(true)),
        },
        Expr::BinaryOp {
            op: BinOp::Or,
            lhs: Box::new(Expr::Bool(false)),
            rhs: Box::new(Expr::Bool(false)),
        },
        Expr::BinaryOp {
            op: BinOp::Or,
            lhs: Box::new(Expr::Bool(true)),
            rhs: Box::new(Expr::Bool(false)),
        },
    ];
    c.bench_function("eval/binary_ops", |b| {
        b.to_async(&runtime).iter(|| async {
            for expr in &exprs {
                let _ = black_box(eval_expr(black_box(expr), black_box(&env)).await.ok());
            }
        })
    });
}

fn eval_statements(c: &mut Criterion) {
    let (runtime, env) = make_env();
    let stmts = vec![
        Stmt::Let {
            name: "y".to_string(),
            expr: Expr::Int(99),
        },
        Stmt::Assign {
            name: "x".to_string(),
            expr: Expr::Int(99),
        },
        Stmt::Assign {
            name: "z".to_string(),
            expr: Expr::Int(99),
        },
        Stmt::Update {
            name: "x".to_string(),
            op: BinOp::Add,
            expr: Expr::Int(1),
        },
        Stmt::FnDef {
            name: "f".to_string(),
            params: vec![],
            ret_type: None,
            body: vec![Stmt::Expr(Expr::Int(1))],
        },
        Stmt::Match {
            expr: Expr::Int(42),
            arms: vec![MatchArm {
                pattern: MatchPattern::Wildcard,
                body: vec![],
            }],
        },
        Stmt::Match {
            expr: Expr::Int(42),
            arms: vec![MatchArm {
                pattern: MatchPattern::Literal(LiteralPattern::Int(42)),
                body: vec![],
            }],
        },
        Stmt::TryCatch {
            try_body: vec![Stmt::Expr(Expr::Int(42))],
            catch_var: "e".to_string(),
            catch_body: vec![],
        },
        Stmt::TryCatch {
            try_body: vec![Stmt::Expr(Expr::BinaryOp {
                op: BinOp::Div,
                lhs: Box::new(Expr::Int(5)),
                rhs: Box::new(Expr::Int(0)),
            })],
            catch_var: "e".to_string(),
            catch_body: vec![Stmt::Expr(Expr::Int(0))],
        },
        Stmt::WithCaps {
            caps: vec![],
            body: vec![Stmt::Expr(Expr::Int(42))],
        },
        Stmt::Unsafe {
            body: vec![Stmt::Expr(Expr::Int(42))],
        },
        Stmt::While {
            condition: Expr::Bool(false),
            body: vec![Stmt::Break],
        },
        Stmt::For {
            var: "i".to_string(),
            iter: Expr::Variable("items".to_string()),
            body: vec![],
        },
        Stmt::For {
            var: "i".to_string(),
            iter: Expr::List(vec![Expr::Int(1), Expr::Int(2), Expr::Int(3)]),
            body: vec![Stmt::Break],
        },
        Stmt::For {
            var: "i".to_string(),
            iter: Expr::List(vec![Expr::Int(1), Expr::Int(2), Expr::Int(3)]),
            body: vec![Stmt::Continue, Stmt::Break],
        },
        Stmt::Return(Expr::Int(42)),
        Stmt::Expr(Expr::Int(42)),
    ];
    c.bench_function("eval/statements", |b| {
        b.to_async(&runtime).iter(|| async {
            for stmt in &stmts {
                let _ = black_box(
                    eval_stmt(black_box(stmt), black_box(&env), false)
                        .await
                        .ok(),
                );
            }
        })
    });
}

criterion_group!(
    benches,
    eval_literals_and_expressions,
    eval_binary_ops,
    eval_statements,
);
criterion_main!(benches);
