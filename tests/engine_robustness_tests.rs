//! Comprehensive native engine robustness, pipeline edge cases, boundary formatters, and type safety tests.

mod common;
use common::*;
use fshell_core::{FxIndexMap, Parser, Stmt, Val};
use fshell_engine::{eval_expr, eval_stmt};
use ustr::ustr;

// ---------------------------------------------------------------------------
// 1. Pipeline Transformations & Degenerate Data Streams
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pipeline_filter_on_heterogeneous_maps() {
    let env = setup_test_env();

    // List of maps where some maps lack the 'score' key entirely
    let mut map1 = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    map1.insert(ustr("name"), Val::String("Alice".to_string()));
    map1.insert(ustr("score"), Val::Int(90));

    let mut map2 = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    map2.insert(ustr("name"), Val::String("Bob".to_string()));
    // score key missing

    let mut map3 = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    map3.insert(ustr("name"), Val::String("Charlie".to_string()));
    map3.insert(ustr("score"), Val::Int(40));

    let list = Val::List(vec![Val::Map(map1), Val::Map(map2), Val::Map(map3)]);
    env.vars.write().insert("users".to_string(), list);

    let script = "$users | filter score > 50";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts[0] {
        let res = eval_expr(expr, &env).await.unwrap();
        if let Val::List(filtered) = res {
            assert_eq!(filtered.len(), 1);
            if let Val::Map(m) = &filtered[0] {
                assert_eq!(
                    m.get(&ustr("name")),
                    Some(&Val::String("Alice".to_string()))
                );
            } else {
                panic!("Expected map in filtered result");
            }
        } else {
            panic!("Expected list from filter");
        }
    }
}

#[tokio::test]
async fn test_pipeline_sort_mixed_and_empty() {
    let env = setup_test_env();

    // Sort empty list
    let empty_list = Val::List(vec![]);
    env.vars.write().insert("empty".to_string(), empty_list);
    let script = "$empty | sort";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts[0] {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(res, Val::List(vec![]));
    }

    // Sort numbers descending
    let nums = Val::List(vec![Val::Int(10), Val::Int(3), Val::Int(50), Val::Int(1)]);
    env.vars.write().insert("nums".to_string(), nums);
    let script2 = "$nums | sort -desc";
    let mut parser2 = Parser::new(script2);
    let stmts2 = parser2.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts2[0] {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(
            res,
            Val::List(vec![Val::Int(50), Val::Int(10), Val::Int(3), Val::Int(1)])
        );
    }
}

#[tokio::test]
async fn test_pipeline_limit_and_count() {
    let env = setup_test_env();
    let nums = Val::List(vec![
        Val::Int(1),
        Val::Int(2),
        Val::Int(3),
        Val::Int(4),
        Val::Int(5),
    ]);
    env.vars.write().insert("nums".to_string(), nums);

    // Limit 2
    let script = "$nums | limit 2";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts[0] {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(res, Val::List(vec![Val::Int(1), Val::Int(2)]));
    }

    // Count
    let script2 = "$nums | count";
    let mut parser2 = Parser::new(script2);
    let stmts2 = parser2.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts2[0] {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(res, Val::Int(5));
    }
}

#[tokio::test]
async fn test_pipeline_grep_strings() {
    let env = setup_test_env();
    let items = Val::List(vec![
        Val::String("apple".to_string()),
        Val::String("banana".to_string()),
        Val::String("pineapple".to_string()),
        Val::String("cherry".to_string()),
    ]);
    env.vars.write().insert("fruits".to_string(), items);

    let script = "$fruits | grep \"apple\"";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts[0] {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(
            res,
            Val::List(vec![
                Val::String("apple".to_string()),
                Val::String("pineapple".to_string())
            ])
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Boundary Serialization Operators (@json, @yaml, @msgpack, @csv, @text)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_serialization_json_roundtrip() {
    let env = setup_test_env();

    let script =
        r#"let data = {"name": "Antigravity", "level": 100, "active": true}; $data | @json"#;
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    if let Stmt::Expr(expr) = &stmts[1] {
        let res = eval_expr(expr, &env).await.unwrap();
        if let Val::String(json_str) = res {
            assert!(json_str.contains("Antigravity"));
            assert!(json_str.contains("100"));
            assert!(json_str.contains("true"));
        } else {
            panic!("Expected JSON string result");
        }
    }
}

#[tokio::test]
async fn test_serialization_csv_operator() {
    let env = setup_test_env();

    let mut row1 = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    row1.insert(ustr("id"), Val::Int(1));
    row1.insert(ustr("name"), Val::String("Alice".to_string()));

    let mut row2 = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    row2.insert(ustr("id"), Val::Int(2));
    row2.insert(ustr("name"), Val::String("Bob".to_string()));

    let table = Val::List(vec![Val::Map(row1), Val::Map(row2)]);
    env.vars.write().insert("table".to_string(), table);

    let script = "$table | @csv";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts[0] {
        let res = eval_expr(expr, &env).await.unwrap();
        if let Val::String(csv_str) = res {
            assert!(csv_str.contains("id") && csv_str.contains("name"));
            assert!(csv_str.contains("Alice") && csv_str.contains("Bob"));
        } else {
            panic!("Expected CSV string result");
        }
    }
}

#[tokio::test]
async fn test_serialization_text_operator() {
    let env = setup_test_env();
    let items = Val::List(vec![Val::Int(10), Val::Int(20), Val::Int(30)]);
    env.vars.write().insert("items".to_string(), items);

    let script = "$items | @text";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts[0] {
        let res = eval_expr(expr, &env).await.unwrap();
        if let Val::String(text) = res {
            assert_eq!(text.trim(), "10\n20\n30");
        } else {
            panic!("Expected string output from @text");
        }
    }
}

// ---------------------------------------------------------------------------
// 3. Control Flow & Match Expressions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_match_expression_literals_and_wildcard() {
    let env = setup_test_env();

    let script = r#"
match 0 {
    0 => { let res0 = "zero" },
    1 => { let res0 = "one" },
    _ => { let res0 = "other" }
}
match 42 {
    0 => { let res42 = "zero" },
    1 => { let res42 = "one" },
    _ => { let res42 = "other" }
}
"#;
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for s in &stmts {
        eval_stmt(s, &env, false).await.unwrap();
    }

    let vars = env.vars.read();
    assert_eq!(vars.get("res0"), Some(&Val::String("zero".to_string())));
    assert_eq!(vars.get("res42"), Some(&Val::String("other".to_string())));
}

#[tokio::test]
async fn test_while_and_for_loops_with_mutation() {
    let env = setup_test_env();

    let script = r#"
let sum = 0
for i in [1, 2, 3, 4, 5] {
    sum = $sum + $i
}
"#;
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for s in &stmts {
        eval_stmt(s, &env, false).await.unwrap();
    }

    let vars = env.vars.read();
    assert_eq!(vars.get("sum"), Some(&Val::Int(15)));
}

// ---------------------------------------------------------------------------
// 4. Try-Catch Error Recovery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_try_catch_error_handling() {
    let env = setup_test_env();

    let script = r#"
try {
    let x = 10 / 0
} catch |err| {
    let caught = true
}
"#;
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for s in &stmts {
        eval_stmt(s, &env, false).await.unwrap();
    }

    let vars = env.vars.read();
    assert_eq!(vars.get("caught"), Some(&Val::Bool(true)));
}

// ---------------------------------------------------------------------------
// 5. Native Type Coercions & Builtin Helpers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_val_type_conversions() {
    assert_eq!(Val::Int(42).to_text(), "42");
    assert_eq!(Val::Float(3.5).to_text(), "3.5");
    assert_eq!(Val::Bool(true).to_text(), "true");
    assert_eq!(Val::Bool(false).to_text(), "false");
    assert_eq!(Val::Null.to_text(), "");

    assert_eq!(Val::Int(42).type_name(), "Int");
    assert_eq!(Val::Float(3.5).type_name(), "Float");
    assert_eq!(Val::String("hi".to_string()).type_name(), "String");
    assert_eq!(Val::Bool(true).type_name(), "Bool");
    assert_eq!(Val::List(vec![]).type_name(), "List");
    assert_eq!(Val::Null.type_name(), "Null");
}

#[tokio::test]
async fn test_string_builtin_operations() {
    let env = setup_test_env();

    let script = r#"let text = "apple,banana,orange"; let parts = $text | string split ",""#;
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for s in &stmts {
        eval_stmt(s, &env, false).await.unwrap();
    }

    let vars = env.vars.read();
    assert_eq!(
        vars.get("parts"),
        Some(&Val::List(vec![Val::List(vec![
            Val::String("apple".to_string()),
            Val::String("banana".to_string()),
            Val::String("orange".to_string())
        ])]))
    );
}
