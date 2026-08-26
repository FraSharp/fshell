#![allow(
    clippy::await_holding_lock,
    unused_must_use,
    unused_imports,
    clippy::needless_borrow
)]

mod common;
use common::*;
use fshell_core::{Expr, ResourceHandle, Stmt};
use fshell_engine::{EngineError, ReactiveEvent, execute_pipeline};
use std::path::PathBuf;

#[tokio::test]
async fn test_integration_simple_math() {
    let ctx = TestContext::new();
    let val = ctx
        .get_var_after_script("let val = 20 * 2 + 10", "val")
        .await;
    assert_eq!(val, Some(Val::Int(50)));
}

#[tokio::test]
async fn test_integration_try_catch() {
    let ctx = TestContext::new();
    let script = "
        try {
            let result = 1 / 0
        } catch |err| {
            let caught = true
        }
    ";
    ctx.eval_script(script).await.unwrap();
    assert_eq!(ctx.get_var("caught"), Some(Val::Bool(true)));
    assert!(ctx.get_var("err").is_some());
}

#[tokio::test]
async fn test_fn_definition_and_call() {
    let ctx = TestContext::new();
    let script = "fn greet(name) { let result = \"Hello, \" + name }; greet \"World\"";
    let val = ctx.get_var_after_script(script, "result").await;
    assert_eq!(val, Some(Val::String("Hello, World".to_string())));
}

// Match execution

#[tokio::test]
async fn test_match_wildcard() {
    let env = setup_test_env();
    let script = "match 42 { _ => { let matched = true } }";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("matched"), Some(&Val::Bool(true)));
}

#[tokio::test]
async fn test_match_literal_int() {
    let env = setup_test_env();
    let script = "match 42 { 42 => { let matched = true }, _ => { } }";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("matched"), Some(&Val::Bool(true)));
}

#[tokio::test]
async fn test_match_literal_string() {
    let env = setup_test_env();
    let script = "match \"hello\" { \"hello\" => { let matched = true }, _ => { } }";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("matched"), Some(&Val::Bool(true)));
}

#[tokio::test]
async fn test_match_map_exact() {
    let env = setup_test_env();
    let script = "match {name: \"test\"} { {name: \"test\"} => { let matched = true }, _ => { } }";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("matched"), Some(&Val::Bool(true)));
}

#[tokio::test]
async fn test_match_map_rest() {
    let env = setup_test_env();
    let script =
        "match {name: \"test\", size: 100} { {name: \"test\", ..} => { let matched = true } }";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("matched"), Some(&Val::Bool(true)));
}

#[tokio::test]
async fn test_match_no_arm() {
    let env = setup_test_env();
    let script = "match 42 { 99 => { let matched = true } }";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    let err = eval_stmt(&stmts[0], &env, false).await.unwrap_err();
    assert!(
        matches!(err, EngineError::MatchNonExhaustive { .. }),
        "expected MatchNonExhaustive error, got: {err}"
    );
}

// WithCaps execution

#[tokio::test]
async fn test_with_caps_grants_and_restores() {
    let env = setup_test_env();
    let orig_has_spawn = env.caps.caps.read().check_process_spawn("test_cmd");
    assert!(!env.caps.caps.read().check_network("any"));

    env.vars.write().insert(
        "extra_cap".to_string(),
        Val::Capability(ResourceHandle::NetworkAll),
    );

    let script = "with caps($extra_cap) { let in_block = true }";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(vars.get("in_block"), Some(&Val::Bool(true)));
    assert_eq!(
        env.caps.caps.read().check_process_spawn("test_cmd"),
        orig_has_spawn,
        "capabilities should be restored after with caps block"
    );
    assert!(
        !env.caps.caps.read().check_network("any"),
        "NetworkAll should not persist after block"
    );
}

// Pipeline map operator

#[tokio::test]
async fn test_string_interpolation() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("name".to_string(), Val::String("World".to_string()));

    let script = r#"let msg = "Hello, {$name}""#;
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(
        vars.get("msg"),
        Some(&Val::String("Hello, World".to_string()))
    );
}

// Chained pipeline operators

#[tokio::test]
async fn test_member_access() {
    let env = setup_test_env();
    let mut map = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    map.insert(ustr::ustr("name"), Val::String("test.txt".to_string()));
    map.insert(ustr::ustr("size"), Val::Int(42));
    env.vars.write().insert("data".to_string(), Val::Map(map));

    let script = "let x = $data.name";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("x"), Some(&Val::String("test.txt".to_string())));
}

#[tokio::test]
async fn test_member_access_chain() {
    let env = setup_test_env();
    let mut inner = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    inner.insert(ustr::ustr("name"), Val::String("nested".to_string()));
    let mut outer = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    outer.insert(ustr::ustr("meta"), Val::Map(inner));
    env.vars.write().insert("data".to_string(), Val::Map(outer));

    let script = "let x = $data.meta.name";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("x"), Some(&Val::String("nested".to_string())));
}

// JSON boundary roundtrip

#[tokio::test]
async fn test_integration_type_constraint() {
    let env = setup_test_env();

    // 1. Primitive constraint
    let mut parser = Parser::new("fn my_fn(val: Int) { let x = $val }");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    // Call with valid argument
    let mut parser2 = Parser::new("my_fn 42");
    let stmts2 = parser2.parse_statements().unwrap();
    eval_stmt(&stmts2[0], &env, false).await.unwrap();

    // Call with invalid argument (should fail)
    let mut parser3 = Parser::new("my_fn \"hello\"");
    let stmts3 = parser3.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts3[0] {
        if let Expr::Pipeline(pipeline) = expr.unpack() {
            let res = fshell_engine::collect_pipeline(pipeline, &env).await;
            assert!(res.is_err(), "Expected type constraint error for Int");
        } else {
            panic!("Expected pipeline");
        }
    }

    // 2. Structural constraint
    let mut parser4 =
        Parser::new("fn struct_fn(item: {name: String, size: Int}) { let x = $item }");
    let stmts4 = parser4.parse_statements().unwrap();
    eval_stmt(&stmts4[0], &env, false).await.unwrap();

    // Call with valid struct
    let mut parser5 = Parser::new("struct_fn {name: \"test.txt\", size: 100}");
    let stmts5 = parser5.parse_statements().unwrap();
    eval_stmt(&stmts5[0], &env, false).await.unwrap();

    // Call with missing field (should fail)
    let mut parser6 = Parser::new("struct_fn {name: \"test.txt\"}");
    let stmts6 = parser6.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts6[0]
        && let Expr::Pipeline(pipeline) = expr.unpack()
    {
        let res = fshell_engine::collect_pipeline(pipeline, &env).await;
        assert!(
            res.is_err(),
            "Expected type constraint error for missing field"
        );
    }
}

// Strict Capability Mode & Constructors tests

#[tokio::test]
async fn test_strict_mode_caps_enforced() {
    let env = setup_test_env();

    // Enable strict mode and clear interactive defaults
    {
        let mut caps = env.caps.caps.write();
        caps.strict_mode = true;
        caps.held.clear();
    }

    // ls should now fail because PWD default capabilities are cleared
    let script_fail = "ls";
    let mut parser = Parser::new(script_fail);
    let stmts = parser.parse_statements().unwrap();
    assert!(
        eval_stmt(&stmts[0], &env, false).await.is_err(),
        "ls should fail under strict mode"
    );

    // Let's grant explicit fs-read permission using constructors and with caps
    let current_dir = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
    let dir_str = current_dir.to_string_lossy().to_string();
    env.vars
        .write()
        .insert("my_dir".to_string(), Val::String(dir_str));

    let script_pass = "let read_cap = fs-read $my_dir; with caps($read_cap) { ls }";
    let mut parser = Parser::new(script_pass);
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 2);
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    eval_stmt(&stmts[1], &env, false).await.unwrap();
}

#[tokio::test]
async fn test_capability_constructors_evaluation() {
    let env = setup_test_env();

    // 1. fs-read
    let mut p = Parser::new("fs-read \"/tmp\"");
    let stmts = p.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts[0] {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(
            res,
            Val::List(vec![Val::Capability(ResourceHandle::ReadDir(
                PathBuf::from("/tmp")
            ))])
        );
    }

    // 2. fs-write
    let mut p = Parser::new("fs-write \"/tmp\"");
    let stmts = p.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts[0] {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(
            res,
            Val::List(vec![Val::Capability(ResourceHandle::WriteDir(
                PathBuf::from("/tmp")
            ))])
        );
    }

    // 3. fs-readwrite
    let mut p = Parser::new("fs-readwrite \"/tmp\"");
    let stmts = p.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts[0] {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(
            res,
            Val::List(vec![
                Val::Capability(ResourceHandle::ReadFile(PathBuf::from("/tmp"))),
                Val::Capability(ResourceHandle::WriteFile(PathBuf::from("/tmp"))),
            ])
        );
    }

    // 4. net-connect
    let mut p = Parser::new("net-connect \"example.com\"");
    let stmts = p.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts[0] {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(
            res,
            Val::List(vec![Val::Capability(ResourceHandle::NetworkSocket(
                "example.com".to_string()
            ))])
        );
    }

    // 5. net-all
    let mut p = Parser::new("net-all");
    let stmts = p.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts[0] {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(
            res,
            Val::List(vec![Val::Capability(ResourceHandle::NetworkAll)])
        );
    }

    // 6. env-read
    let mut p = Parser::new("env-read \"TEST\"");
    let stmts = p.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts[0] {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(
            res,
            Val::List(vec![Val::Capability(ResourceHandle::ReadEnv(
                "TEST".to_string()
            ))])
        );
    }

    // 7. env-write
    let mut p = Parser::new("env-write \"TEST\"");
    let stmts = p.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts[0] {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(
            res,
            Val::List(vec![Val::Capability(ResourceHandle::WriteEnv(
                "TEST".to_string()
            ))])
        );
    }

    // 8. process-spawn
    let mut p = Parser::new("process-spawn");
    let stmts = p.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts[0] {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(
            res,
            Val::List(vec![Val::Capability(ResourceHandle::ProcessSpawn)])
        );
    }
}

#[tokio::test]
async fn test_member_access_capabilities_evaluation() {
    let env = setup_test_env();

    // 1. process.spawn
    let mut p = Parser::new("let cap = $process.spawn");
    let stmts = p.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    assert_eq!(
        env.vars.read().get("cap"),
        Some(&Val::Capability(ResourceHandle::ProcessSpawn))
    );

    // 2. net.all
    let mut p = Parser::new("let cap2 = $net.all");
    let stmts = p.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    assert_eq!(
        env.vars.read().get("cap2"),
        Some(&Val::Capability(ResourceHandle::NetworkAll))
    );
}

#[tokio::test]
async fn test_capability_audit_log() {
    let env = setup_test_env();

    // Initially audit log should be empty
    assert!(env.caps.audit_log.lock().unwrap().is_empty());

    // Run `ls` to trigger an audit log
    let mut parser = Parser::new("ls");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    // Now audit log should have at least 1 entry
    let logs = env.caps.audit_log.lock().unwrap().clone();
    assert!(!logs.is_empty());
    assert!(logs[0].contains("ReadDir"), "log was: {}", logs[0]);
    assert!(logs[0].contains("GRANTED"), "log was: {}", logs[0]);

    // Query via `caps-audit` command
    let mut parser2 = Parser::new("caps-audit");
    let stmts2 = parser2.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts2[0] {
        let res = eval_expr(expr, &env).await.unwrap();
        if let Val::List(items) = res {
            assert!(!items.is_empty());
            if let Val::String(log_str) = &items[0] {
                assert!(log_str.contains("ReadDir"));
            } else {
                panic!("Expected Val::String");
            }
        } else {
            panic!("Expected Val::List");
        }
    }
}

#[tokio::test]
async fn test_integration_negation_operator() {
    let env = setup_test_env();

    // !true == false
    let mut p = Parser::new("!true");
    let res = eval_expr(
        &match p.parse_statements().unwrap().remove(0).into_unpack() {
            Stmt::Expr(e) => e,
            _ => panic!(),
        },
        &env,
    )
    .await
    .unwrap();
    assert_eq!(res, Val::Bool(false));

    // !!true == true (double negation)
    let mut p2 = Parser::new("!!true");
    let res2 = eval_expr(
        &match p2.parse_statements().unwrap().remove(0).into_unpack() {
            Stmt::Expr(e) => e,
            _ => panic!(),
        },
        &env,
    )
    .await
    .unwrap();
    assert_eq!(res2, Val::Bool(true));
}

#[tokio::test]
async fn test_integration_variable_mutability_and_parsing() {
    let env = setup_test_env();

    // let a = 10
    let mut p = Parser::new("let a = 10");
    eval_stmt(&p.parse_statements().unwrap().remove(0), &env, false)
        .await
        .unwrap();

    // a = 5 (reassignment)
    let mut p = Parser::new("a = 5");
    eval_stmt(&p.parse_statements().unwrap().remove(0), &env, false)
        .await
        .unwrap();
    assert_eq!(env.vars.read().get("a"), Some(&Val::Int(5)));

    // a += 3 (compound update)
    let mut p = Parser::new("a += 3");
    eval_stmt(&p.parse_statements().unwrap().remove(0), &env, false)
        .await
        .unwrap();
    assert_eq!(env.vars.read().get("a"), Some(&Val::Int(8)));

    // a -= 2
    let mut p = Parser::new("a -= 2");
    eval_stmt(&p.parse_statements().unwrap().remove(0), &env, false)
        .await
        .unwrap();
    assert_eq!(env.vars.read().get("a"), Some(&Val::Int(6)));

    // a *= 3
    let mut p = Parser::new("a *= 3");
    eval_stmt(&p.parse_statements().unwrap().remove(0), &env, false)
        .await
        .unwrap();
    assert_eq!(env.vars.read().get("a"), Some(&Val::Int(18)));

    // a /= 2
    let mut p = Parser::new("a /= 2");
    eval_stmt(&p.parse_statements().unwrap().remove(0), &env, false)
        .await
        .unwrap();
    assert_eq!(env.vars.read().get("a"), Some(&Val::Int(9)));

    // a + 1 (expression parsing fix)
    let mut p = Parser::new("a + 1");
    let res = eval_expr(
        &match p.parse_statements().unwrap().remove(0).into_unpack() {
            Stmt::Expr(e) => e,
            _ => panic!(),
        },
        &env,
    )
    .await
    .unwrap();
    assert_eq!(res, Val::Int(10));
}

#[tokio::test]
async fn test_integration_variable_command_collision() {
    let env = setup_test_env();

    // let ls = 3
    let mut p = Parser::new("let ls = 3");
    eval_stmt(&p.parse_statements().unwrap().remove(0), &env, false)
        .await
        .unwrap();

    // Execute pipeline "ls". It should run the builtin 'ls', NOT return Val::Int(3)!
    let mut p2 = Parser::new("ls");
    let stmts = p2.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        if let fshell_core::Expr::Pipeline(pipeline) = expr.unpack() {
            let (tx, mut rx) = tokio::sync::mpsc::channel(100);
            let env_clone = env.clone();
            let pipeline_clone = pipeline.clone();

            tokio::spawn(async move {
                fshell_engine::execute_pipeline(&pipeline_clone, &env_clone, tx)
                    .await
                    .unwrap();
            });

            let mut results = Vec::new();
            while let Some(payload) = rx.recv().await {
                if let fshell_engine::PipelinePayload::Data(val) = payload {
                    results.push((*val).clone());
                }
            }
            // It ran the actual 'ls' builtin, so results contains directory entries, not the int 3!
            if !results.is_empty() {
                assert_ne!(results[0], Val::Int(3));
            }
        } else {
            panic!("Expected Expr::Pipeline");
        }
    } else {
        panic!("Expected Stmt::Expr");
    }
}

// ---------------------------------------------------------------------------
// v0.1 feature tests: rm, cp, mv, source, if/else, while, string escapes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_eval_source_statement() {
    let env = setup_test_env();
    // Write a temp script file that sets a variable
    let tmpdir = std::env::temp_dir();
    let script_path = tmpdir.join("fshell_test_source.fsh");
    std::fs::write(&script_path, "let sourced_val = 99\n").unwrap();

    let source_stmt = Stmt::Source {
        path: fshell_core::Expr::String(vec![fshell_core::StringPart::Lit(
            script_path.to_string_lossy().to_string(),
        )]),
        bash: false,
    };
    eval_stmt(&source_stmt, &env, false)
        .await
        .expect("source should succeed");

    let vars = env.vars.read();
    assert_eq!(vars.get("sourced_val"), Some(&Val::Int(99)));

    let _ = std::fs::remove_file(&script_path);
}

#[tokio::test]
async fn test_eval_if_else() {
    let env = setup_test_env();

    // Construct `if true { 42 } else { 0 }` and assign to `result` via `let result = ...`
    let if_true = fshell_core::Expr::If {
        condition: Box::new(fshell_core::Expr::Bool(true)),
        then_body: vec![Stmt::Expr(fshell_core::Expr::Int(42))],
        else_body: Some(vec![Stmt::Expr(fshell_core::Expr::Int(0))]),
    };
    let stmt = Stmt::Let {
        name: "result".to_string(),
        expr: if_true,
    };
    eval_stmt(&stmt, &env, false).await.unwrap();
    {
        // Drop the read guard before the next eval (which takes a write lock)
        let vars = env.vars.read();
        assert_eq!(vars.get("result"), Some(&Val::Int(42)));
    }

    // `if false { 1 } else { 2 }` should produce 2
    let if_false = fshell_core::Expr::If {
        condition: Box::new(fshell_core::Expr::Bool(false)),
        then_body: vec![Stmt::Expr(fshell_core::Expr::Int(1))],
        else_body: Some(vec![Stmt::Expr(fshell_core::Expr::Int(2))]),
    };
    let stmt2 = Stmt::Let {
        name: "result2".to_string(),
        expr: if_false,
    };
    eval_stmt(&stmt2, &env, false).await.unwrap();
    {
        let vars = env.vars.read();
        assert_eq!(vars.get("result2"), Some(&Val::Int(2)));
    }

    // `if false { 1 }` (no else) should produce Val::Null
    let if_no_else = fshell_core::Expr::If {
        condition: Box::new(fshell_core::Expr::Bool(false)),
        then_body: vec![Stmt::Expr(fshell_core::Expr::Int(1))],
        else_body: None,
    };
    let stmt3 = Stmt::Let {
        name: "result3".to_string(),
        expr: if_no_else,
    };
    eval_stmt(&stmt3, &env, false).await.unwrap();
    {
        let vars = env.vars.read();
        assert_eq!(vars.get("result3"), Some(&Val::Null));
    }
}

#[tokio::test]
async fn test_eval_while_loop() {
    let env = setup_test_env();
    // while loop that counts to 5
    env.vars.write().insert("counter".to_string(), Val::Int(0));

    // Construct the while-loop AST directly
    let while_stmt = Stmt::While {
        condition: fshell_core::Expr::BinaryOp {
            op: fshell_core::BinOp::Lt,
            lhs: Box::new(fshell_core::Expr::Variable("counter".to_string())),
            rhs: Box::new(fshell_core::Expr::Int(5)),
        },
        body: vec![Stmt::Assign {
            name: "counter".to_string(),
            expr: fshell_core::Expr::BinaryOp {
                op: fshell_core::BinOp::Add,
                lhs: Box::new(fshell_core::Expr::Variable("counter".to_string())),
                rhs: Box::new(fshell_core::Expr::Int(1)),
            },
        }],
    };
    eval_stmt(&while_stmt, &env, false).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(vars.get("counter"), Some(&Val::Int(5)));
}

#[tokio::test]
async fn test_eval_while_loop_parsed_from_text() {
    let env = setup_test_env();
    // Verify that while-loop with < comparison parses correctly from source text
    env.vars.write().insert("i".to_string(), Val::Int(0));

    let script = "while $i < 3 { i = $i + 1 }";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().expect("while loop should parse");
    assert_eq!(stmts.len(), 1);
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(vars.get("i"), Some(&Val::Int(3)));
}

#[tokio::test]
async fn test_eval_if_else_if_chaining() {
    let env = setup_test_env();
    // Construct:
    // if false { 1 } else if true { 2 } else { 3 }
    // The else-if is represented by putting another Expr::If inside the else_body.
    let inner_if = fshell_core::Expr::If {
        condition: Box::new(fshell_core::Expr::Bool(true)),
        then_body: vec![Stmt::Expr(fshell_core::Expr::Int(2))],
        else_body: Some(vec![Stmt::Expr(fshell_core::Expr::Int(3))]),
    };
    let outer_if = fshell_core::Expr::If {
        condition: Box::new(fshell_core::Expr::Bool(false)),
        then_body: vec![Stmt::Expr(fshell_core::Expr::Int(1))],
        else_body: Some(vec![Stmt::Expr(inner_if)]),
    };
    let stmt = Stmt::Let {
        name: "val".to_string(),
        expr: outer_if,
    };
    eval_stmt(&stmt, &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("val"), Some(&Val::Int(2)));
}

#[tokio::test]
async fn test_string_escape_sequences() {
    let env = setup_test_env();

    // \\n escape — the parser sees literal backslash-n and converts to newline
    let mut p = Parser::new(r##"let s = "hello\nworld""##);
    let stmts = p.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    {
        let vars = env.vars.read();
        assert_eq!(
            vars.get("s"),
            Some(&Val::String("hello\nworld".to_string()))
        );
    }

    // \\x hex escapes: \\x48 = H, \\x65 = e, \\x6c = l, \\x6f = o
    let mut p2 = Parser::new(r##"let t = "\x48\x65\x6c\x6c\x6f""##);
    let stmts2 = p2.parse_statements().unwrap();
    eval_stmt(&stmts2[0], &env, false).await.unwrap();
    {
        let vars = env.vars.read();
        assert_eq!(vars.get("t"), Some(&Val::String("Hello".to_string())));
    }

    // \\t escape
    let mut p3 = Parser::new(r##"let u = "col1\tcol2""##);
    let stmts3 = p3.parse_statements().unwrap();
    eval_stmt(&stmts3[0], &env, false).await.unwrap();
    {
        let vars = env.vars.read();
        match vars.get("u") {
            Some(Val::String(s)) => assert!(s.contains('\t')),
            other => panic!("expected Val::String, got {:?}", other),
        }
    }
}

#[tokio::test]
async fn test_exit_code_tracking() {
    let env = setup_test_env();
    let (tx, _rx) = tokio::sync::mpsc::channel(100);

    // Run external command that succeeds
    fshell_bridge::run_external("true", vec![], None, &env, tx, false, None).unwrap();

    // Wait for the background waitpid thread (spawn_job_waiter) to set $?
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let vars = env.vars.read();
    assert_eq!(
        vars.get("?"),
        Some(&Val::Int(0)),
        "true should set exit code 0"
    );
}

#[tokio::test]
async fn test_exit_code_tracking_failure() {
    let env = setup_test_env();
    let (tx, _rx) = tokio::sync::mpsc::channel(100);

    // Run external command that fails with exit code 42
    fshell_bridge::run_external(
        "sh",
        vec![
            Val::String("-c".to_string()),
            Val::String("exit 42".to_string()),
        ],
        None,
        &env,
        tx,
        false,
        None,
    )
    .unwrap();

    // Wait for the background waitpid thread (spawn_job_waiter) to set $?
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let vars = env.vars.read();
    assert_eq!(
        vars.get("?"),
        Some(&Val::Int(42)),
        "exit 42 should set exit code 42"
    );
}

#[tokio::test]
async fn test_string_interpolation_display() {
    let env = setup_test_env();

    // Verify that non-string values in string interpolation use natural display
    // (not Rust debug format like "Int(42)" or "Bool(true)")
    env.vars.write().insert("n".to_string(), Val::Int(42));

    let mut parser = Parser::new(r#"let s = "value is {$n}""#);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(
        vars.get("s"),
        Some(&Val::String("value is 42".to_string())),
        "integer should interpolate as '42' not 'Int(42)'"
    );
}

#[tokio::test]
async fn test_integration_env_capability_checks() {
    let env = setup_test_env();
    {
        let mut caps = env.caps.caps.write();
        caps.strict_mode = true;
        caps.revoke(&fshell_core::ResourceHandle::WriteEnv("*".to_string()));
        caps.revoke(&fshell_core::ResourceHandle::ReadEnv("*".to_string()));
    }

    let mut parser = Parser::new("export NEW_TEST_VAR \"value\"");
    let stmts = parser.parse_statements().unwrap();
    let res = eval_stmt(&stmts[0], &env, false).await;
    assert!(res.is_err());
    assert!(res.unwrap_err().contains("Capability denied"));
}

#[tokio::test]
async fn test_integration_if_else_expression_parsed() {
    let env = setup_test_env();
    let script = "
        let x = 10
        let result = if x > 5 { true } else { false }
    ";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in &stmts {
        eval_stmt(stmt, &env, false).await.unwrap();
    }
    let vars = env.vars.read();
    assert_eq!(vars.get("result"), Some(&Val::Bool(true)));
}

#[tokio::test]
async fn test_exit_statement_no_code() {
    let env = setup_test_env();
    let script = "exit";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);
    let result = eval_stmt(&stmts[0], &env, false).await;
    assert!(matches!(result, Ok(fshell_engine::Flow::Exit(0))));
}

#[tokio::test]
async fn test_exit_statement_with_code() {
    let env = setup_test_env();
    let script = "exit 42";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);
    let result = eval_stmt(&stmts[0], &env, false).await;
    assert!(matches!(result, Ok(fshell_engine::Flow::Exit(42))));
}

#[tokio::test]
async fn test_cmd_substitution_syntax() {
    let _env = setup_test_env();
    let script = "let x = $(ls | count)";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);
    if let Stmt::Let { expr, .. } = stmts[0].unpack() {
        assert!(matches!(expr.unpack(), Expr::InlinePipeline(_)));
    } else {
        panic!("Expected Let with InlinePipeline");
    }
}

#[tokio::test]
async fn test_backtick_substitution_syntax() {
    let _env = setup_test_env();
    let script = "let x = `ls | count`";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);
    if let Stmt::Let { expr, .. } = stmts[0].unpack() {
        assert!(matches!(expr.unpack(), Expr::InlinePipeline(_)));
    } else {
        panic!("Expected Let with InlinePipeline from backtick");
    }
}

#[tokio::test]
async fn test_on_signal_with_function() {
    let env = setup_test_env();
    let script = "
        fn cleanup() { echo done }
        on exit cleanup
    ";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 2);
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    eval_stmt(&stmts[1], &env, false).await.unwrap();
    let hooks = env.hooks.registry.read();
    let exit_hooks = hooks.get("exit");
    assert!(exit_hooks.is_some(), "exit hooks should exist");
    assert!(exit_hooks.unwrap().contains(&"cleanup".to_string()));
}

#[tokio::test]
async fn test_on_signal_with_block() {
    let env = setup_test_env();
    let script = "on sigint { echo interrupted }";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let fns = env.fns.read();
    assert!(
        fns.contains_key("_on_sigint"),
        "synthetic function should exist"
    );
    let hooks = env.hooks.registry.read();
    let sigint_hooks = hooks.get("sigint");
    assert!(sigint_hooks.is_some(), "sigint hooks should exist");
    assert!(sigint_hooks.unwrap().contains(&"_on_sigint".to_string()));
}

#[tokio::test]
async fn test_exit_code_stored_in_prompt() {
    let env = setup_test_env();
    let script = "exit 7";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    let result = eval_stmt(&stmts[0], &env, false).await;
    assert!(matches!(result, Ok(fshell_engine::Flow::Exit(7))));
    let ec = *env.prompt.last_exit_code.read();
    assert_eq!(ec, 7);
}

#[tokio::test]
async fn test_export_kv_syntax() {
    let env = setup_test_env();
    let mut parser = Parser::new("export FOO=bar");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    // Check in env map
    let vars = env.vars.read();
    let env_val = vars.get("env").unwrap();
    if let Val::Map(map) = env_val {
        assert_eq!(
            map.get(&ustr::ustr("FOO")),
            Some(&Val::String("bar".to_string()))
        );
    } else {
        panic!("env must be a map");
    }
}

#[tokio::test]
async fn test_unset_variable() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("TESTVAR".to_string(), Val::String("value".to_string()));

    let mut parser = Parser::new("unset TESTVAR");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    let vars = env.vars.read();
    assert!(!vars.contains_key("TESTVAR"));
}

#[tokio::test]
async fn test_integration_and_or_short_circuit() {
    let env = setup_test_env();

    // Test: false && (let x = 1) -> x should NOT be set
    let script = "false && let x = 1";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert!(vars.get("x").is_none());
    drop(vars);

    // Test: true && let y = 2 -> y should be set to 2
    let script = "true && let y = 2";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("y"), Some(&Val::Int(2)));
    drop(vars);

    // Test: true || let z = 3 -> z should NOT be set
    let script = "true || let z = 3";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert!(vars.get("z").is_none());
    drop(vars);

    // Test: false || let w = 4 -> w should be set to 4
    let script = "false || let w = 4";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("w"), Some(&Val::Int(4)));
}

#[tokio::test]
async fn test_arithmetic_expansion_basic() {
    let env = setup_test_env();
    let mut parser = Parser::new("let result = $((1 + 2))");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("result"), Some(&Val::Int(3)));
}

#[tokio::test]
async fn test_arithmetic_expansion_with_variable() {
    let env = setup_test_env();
    env.vars.write().insert("x".to_string(), Val::Int(5));
    let mut parser = Parser::new("let result = $((x * 2))");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("result"), Some(&Val::Int(10)));
}

#[tokio::test]
async fn test_arithmetic_expansion_nested() {
    let env = setup_test_env();
    let mut parser = Parser::new("let result = $(((2 + 3) * 4))");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("result"), Some(&Val::Int(20)));
}

#[tokio::test]
async fn test_special_var_random() {
    let env = setup_test_env();
    let mut parser = Parser::new("let result = $RANDOM");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    let val = vars.get("result").unwrap();
    assert!(val.to_text().parse::<i64>().is_ok());
}

#[tokio::test]
async fn test_special_var_ostype() {
    let env = setup_test_env();
    let mut parser = Parser::new("let result = $OSTYPE");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    let os = vars.get("result").unwrap().to_text();
    assert!(os == "macos" || os == "linux");
}

#[tokio::test]
async fn test_special_var_fshpid() {
    let env = setup_test_env();
    let mut parser = Parser::new("let result = $FSHPID");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(
        vars.get("result"),
        Some(&Val::Int(std::process::id() as i64))
    );
}

#[tokio::test]
async fn test_trap_set_and_list() {
    let env = setup_test_env();
    let mut parser = Parser::new(r#"trap "echo bye" EXIT"#);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    let traps = env.posix_traps.read();
    assert_eq!(
        traps.get(&fshell_engine::Signal::Exit),
        Some(&"echo bye".to_string())
    );
}

#[tokio::test]
async fn test_trap_remove() {
    let env = setup_test_env();
    let mut parser = Parser::new(r#"trap "echo hi" INT"#);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    let mut parser2 = Parser::new(r#"trap "-" INT"#);
    let stmts2 = parser2.parse_statements().unwrap();
    eval_stmt(&stmts2[0], &env, false).await.unwrap();

    let traps = env.posix_traps.read();
    assert!(!traps.contains_key(&fshell_engine::Signal::Int));
}

#[test]
fn test_expand_braces_single_word() {
    let result = fshell_core::expand_braces("test {x} end");
    println!("expand_braces('test {{x}} end') = {:?}", result);
}

#[test]
fn test_glob_expand_single_word() {
    let result = fshell_core::glob_utils::expand_glob_with_options("test {x} end", false, false);
    println!("expand_glob('test {{x}} end') = {:?}", result);
}

#[tokio::test]
async fn test_integration_exit_code_variables() {
    let mut env = setup_test_env();
    env.is_last_stage = true;
    let (tx, _rx) = tokio::sync::mpsc::channel(100);

    // 1. Run external command that fails with 42
    fshell_bridge::run_external(
        "sh",
        vec![
            Val::String("-c".to_string()),
            Val::String("exit 42".to_string()),
        ],
        None,
        &env,
        tx.clone(),
        false,
        None,
    )
    .unwrap();

    // Wait for the background waitpid thread (spawn_job_waiter) to set last_exit_code
    tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;

    // Check $? and $status via eval_expr
    let mut parser = Parser::new("$?");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(res, Val::Int(42));
    }

    let mut parser = Parser::new("$status");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(res, Val::Int(42));
    }

    // 2. Run external command that succeeds
    fshell_bridge::run_external("true", vec![], None, &env, tx, false, None).unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;

    // Check $? and $status are now 0
    let mut parser = Parser::new("$?");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(res, Val::Int(0));
    }

    let mut parser = Parser::new("$status");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(res, Val::Int(0));
    }
}

// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_integration_loop_brace_expansion() {
    let env = setup_test_env();

    // 1. Range expansion: {1..5}
    let script = "let sum = 0; for i in {1..5} { sum = $sum + $i }";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in &stmts {
        eval_stmt(stmt, &env, false).await.unwrap();
    }
    {
        let vars = env.vars.read();
        assert_eq!(vars.get("sum"), Some(&Val::Int(15)));
    }

    // 2. List/comma expansion: {a,b,c}
    let script2 = "let concat = \"\"; for s in {a,b,c} { concat = $concat + $s }";
    let mut parser2 = Parser::new(script2);
    let stmts2 = parser2.parse_statements().unwrap();
    for stmt in &stmts2 {
        eval_stmt(stmt, &env, false).await.unwrap();
    }
    {
        let vars2 = env.vars.read();
        assert_eq!(vars2.get("concat"), Some(&Val::String("abc".to_string())));
    }
}

#[tokio::test]
async fn test_integration_function_local_variables() {
    let env = setup_test_env();

    // Define global x = 10
    let script = "let x = 10\nfn test_scope() {\n  local x = 99\n  local y = 42\n  x += 1\n  return $x\n}\ntest_scope";
    let mut parser = fshell_core::Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in stmts {
        fshell_engine::eval_stmt(&stmt, &env, false).await.unwrap();
    }

    let vars = env.vars.read();
    // Global x must remain 10 (not clobbered by local x)
    assert_eq!(vars.get("x"), Some(&Val::Int(10)));
    // Local y must not exist in global scope
    assert_eq!(vars.get("y"), None);
}

#[tokio::test]
async fn test_diagnostic_try_catch_structured_map() {
    let env = setup_test_env();
    let script = r#"
        try {
            let result = 1 / 0
        } catch |err| {
            let err_code = $err.code
            let err_name = $err.name
            let err_cat = $err.category
            let err_docs = $err.docs_url
        }
    "#;
    let mut parser = fshell_core::Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in &stmts {
        fshell_engine::eval_stmt(stmt, &env, false).await.unwrap();
    }

    let vars = env.vars.read();
    assert_eq!(
        vars.get("err_code"),
        Some(&Val::String("FSH-MATH-001".to_string()))
    );
    assert_eq!(
        vars.get("err_name"),
        Some(&Val::String("DivisionByZero".to_string()))
    );
    assert_eq!(
        vars.get("err_cat"),
        Some(&Val::String("arithmetic".to_string()))
    );
    // docs URL is only present when FSHELL_DOCS_BASE is configured
    if option_env!("FSHELL_DOCS_BASE").is_some() {
        let docs = vars
            .get("err_docs")
            .expect("err_docs present when base set");
        assert!(docs.to_text().contains("FSH-MATH-001"));
    } else {
        assert!(
            vars.get("err_docs").is_none() || vars.get("err_docs") == Some(&Val::Null),
            "err_docs should be missing or Null when docs base not set, got {:?}",
            vars.get("err_docs")
        );
    }
}

#[tokio::test]
async fn test_syntax_error_diagnostics_invalid_operator_sequence() {
    let mut parser = fshell_core::Parser::new("1++");
    let err = parser.parse_statements().unwrap_err();
    assert!(
        matches!(err, fshell_core::ParseError::ExpectedToken { ref expected, ref found, .. } if expected == "expression" && found == "'+'")
    );

    let validation = fshell_core::validate_input("1++");
    assert!(matches!(
        validation,
        fshell_core::ValidationResult::Invalid { .. }
    ));
}
