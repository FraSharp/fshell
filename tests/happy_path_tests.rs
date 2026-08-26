//! Happy path tests for fsh — daily-driving scenarios.
//!
//! Every test asserts on actual output or state against the real fsh parser
//! and evaluator.  No "doesn't crash" tests, no mocks, no modifications to
//! fsh source code.

mod common;
use common::*;
use fshell_core::Stmt;

// =========================================================================
// Core language — bare expressions
// =========================================================================

#[tokio::test]
async fn test_happy_int_addition() {
    let env = setup_test_env();
    assert_eq!(eval_script("5 + 3", &env).await, Val::Int(8));
}

#[tokio::test]
async fn test_happy_int_multiplication() {
    let env = setup_test_env();
    assert_eq!(eval_script("6 * 7", &env).await, Val::Int(42));
}

#[tokio::test]
async fn test_happy_int_division_trunc() {
    let env = setup_test_env();
    assert_eq!(eval_script("10 / 3", &env).await, Val::Int(3));
}

#[tokio::test]
async fn test_happy_float_add() {
    let env = setup_test_env();
    assert_eq!(eval_script("1.5 + 2.5", &env).await, Val::Float(4.0));
}

#[tokio::test]
async fn test_happy_boolean_not() {
    let env = setup_test_env();
    assert_eq!(eval_script("!true", &env).await, Val::Bool(false));
}

#[tokio::test]
async fn test_happy_list_literal() {
    let env = setup_test_env();
    assert_eq!(
        eval_script("[1, 2, 3]", &env).await,
        Val::List(vec![Val::Int(1), Val::Int(2), Val::Int(3)])
    );
}

#[tokio::test]
async fn test_happy_map_literal() {
    let env = setup_test_env();
    let res = eval_script(r#"{name: "Alice", age: 30}"#, &env).await;
    if let Val::Map(map) = &res {
        assert_eq!(map.get(&ustr("name")), Some(&Val::String("Alice".into())));
        assert_eq!(map.get(&ustr("age")), Some(&Val::Int(30)));
    } else {
        panic!("expected Val::Map, got {res:?}");
    }
}

// =========================================================================
// Core language — control flow
// =========================================================================

#[tokio::test]
async fn test_happy_while_loop_counter() {
    let env = setup_test_env();
    let script = "let i = 0; while $i < 3 { let i = $i + 1 }; let last = $i";
    let mut p = Parser::new(script);
    let stmts = p.parse_statements().unwrap();
    for s in &stmts {
        eval_stmt(s, &env, false).await.unwrap();
    }
    let vars = env.vars.read();
    assert_eq!(vars.get("i"), Some(&Val::Int(3)));
    assert_eq!(vars.get("last"), Some(&Val::Int(3)));
}

#[tokio::test]
async fn test_happy_for_loop_over_list() {
    let env = setup_test_env();
    let script = "let items = [10, 20, 30]; let sum = 0; for x in $items { let sum = sum + x }";
    let mut p = Parser::new(script);
    let stmts = p.parse_statements().unwrap();
    for s in &stmts {
        eval_stmt(s, &env, false).await.unwrap();
    }
    let vars = env.vars.read();
    assert_eq!(vars.get("sum"), Some(&Val::Int(60)));
}

#[tokio::test]
async fn test_happy_if_else_true() {
    let env = setup_test_env();
    let res = get_var(
        r#"let x = 42; let result = if $x > 10 { "big" } else { "small" }"#,
        "result",
        &env,
    )
    .await;
    assert_eq!(res, Some(Val::String("big".into())));
}

#[tokio::test]
async fn test_happy_if_else_false() {
    let env = setup_test_env();
    let res = get_var(
        r#"let x = 3; let result = if $x > 10 { "big" } else { "small" }"#,
        "result",
        &env,
    )
    .await;
    assert_eq!(res, Some(Val::String("small".into())));
}

#[tokio::test]
async fn test_happy_try_catch_recovers() {
    let env = setup_test_env();
    let script = r#"
        try { let _bad = 1 / 0 } catch |err| { let caught = true }
    "#;
    let mut p = Parser::new(script);
    let stmts = p.parse_statements().unwrap();
    for s in &stmts {
        eval_stmt(s, &env, false).await.unwrap();
    }
    let vars = env.vars.read();
    assert_eq!(vars.get("caught"), Some(&Val::Bool(true)));
    assert!(vars.get("err").is_some(), "catch should bind the error");
}

// =========================================================================
// User-defined functions
// =========================================================================

#[tokio::test]
async fn test_happy_fn_double() {
    let env = setup_test_env();
    // let r = double(21) captures pipeline output as List([42])
    let res = get_var("fn double(x) { $x * 2 }; let r = double(21)", "r", &env).await;
    assert_eq!(res, Some(Val::List(vec![Val::Int(42)])));
}

#[tokio::test]
async fn test_happy_fn_no_args() {
    let env = setup_test_env();
    // fn definition and call — just verify no crash
    let mut p = Parser::new("fn greet() { let _x = 1 }");
    let stmts = p.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    // Calling an fn is done via name — doesn't need a second parse
    // Just verify the fn was registered
    let fns = env.fns.read();
    assert!(fns.contains_key("greet"), "fn greet should be registered");
}

// =========================================================================
// Echo & output
// =========================================================================

#[tokio::test]
async fn test_happy_echo_single_arg() {
    let env = setup_test_env();
    let res = eval_script(r#"echo "hello world""#, &env).await;
    if let Val::List(items) = &res {
        assert!(!items.is_empty());
        assert!(
            items
                .iter()
                .any(|v| matches!(v, Val::String(s) if s.contains("hello world")))
        );
    } else {
        panic!("expected Val::List, got {res:?}");
    }
}

#[tokio::test]
async fn test_happy_echo_two_args() {
    let env = setup_test_env();
    let res = eval_script(r#"echo "hello" "world""#, &env).await;
    if let Val::List(items) = &res {
        // fsh joins echo arguments into one output line
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0], Val::String(s) if s == "hello world"));
    } else {
        panic!("expected Val::List, got {res:?}");
    }
}

#[tokio::test]
async fn test_happy_echo_var_interp() {
    let env = setup_test_env();
    let script = r#"let name = "World"; let res = echo "Hello, $name""#;
    let got = get_var(script, "res", &env).await;
    if let Some(Val::List(items)) = got {
        assert!(
            items
                .iter()
                .any(|v| matches!(v, Val::String(s) if s.contains("Hello, World")))
        );
    } else {
        panic!("expected Some(Val::List), got {got:?}");
    }
}

// =========================================================================
// Builtin commands  (eval_expr returns Val::List for pipeline commands)
// =========================================================================

#[tokio::test]
async fn test_happy_pwd() {
    let env = setup_test_env();
    let res = eval_script("pwd", &env).await;
    if let Val::List(items) = &res {
        assert_eq!(items.len(), 1);
        if let Val::String(cwd) = &items[0] {
            assert!(!cwd.is_empty());
            assert!(std::path::Path::new(cwd).is_absolute());
        } else {
            panic!("expected Val::String in list, got {items:?}");
        }
    } else {
        panic!("expected Val::List, got {res:?}");
    }
}

#[tokio::test]
async fn test_happy_printf() {
    let env = setup_test_env();
    let res = eval_script(r#"printf "Hello %s!" "World""#, &env).await;
    if let Val::List(items) = &res {
        assert!(!items.is_empty());
        assert!(items.iter().any(|v| matches!(v, Val::String(s)
            if s.contains("Hello World!"))));
    } else {
        panic!("expected Val::List, got {res:?}");
    }
}

// =========================================================================
// Introspection
// =========================================================================

#[tokio::test]
async fn test_happy_type_builtin() {
    let env = setup_test_env();
    let res = eval_script(r#"type "echo""#, &env).await;
    // type returns a list containing one map
    if let Val::List(items) = &res {
        assert_eq!(items.len(), 1);
        if let Val::Map(map) = &items[0] {
            assert_eq!(map.get(&ustr("name")), Some(&Val::String("echo".into())));
            assert_eq!(map.get(&ustr("type")), Some(&Val::String("builtin".into())));
        } else {
            panic!("expected Val::Map in list, got {items:?}");
        }
    } else {
        panic!("expected Val::List, got {res:?}");
    }
}

#[tokio::test]
async fn test_happy_which() {
    let env = setup_test_env();
    let res = eval_script("which which", &env).await;
    if let Val::List(items) = &res {
        assert!(!items.is_empty());
        assert!(items.iter().any(|v| matches!(v, Val::String(s)
            if s.contains("which"))));
    } else {
        panic!("expected Val::List, got {res:?}");
    }
}

// =========================================================================
// Alias / Config / Hook  (side-effect + state check)
// =========================================================================

#[tokio::test]
async fn test_happy_alias_crud() {
    let env = setup_test_env();

    // create
    let mut p = Parser::new("alias ll 'ls -la'");
    let stmts = p.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    {
        let a = env.aliases.read();
        assert!(a.contains_key("ll"));
        assert_eq!(a.get("ll").unwrap(), "ls -la");
    }

    // delete
    let mut p2 = Parser::new("alias --delete ll");
    let stmts2 = p2.parse_statements().unwrap();
    eval_stmt(&stmts2[0], &env, false).await.unwrap();
    {
        let a = env.aliases.read();
        assert!(!a.contains_key("ll"));
    }
}

#[tokio::test]
async fn test_happy_config_roundtrip() {
    let env = setup_test_env();

    // set
    let mut p = Parser::new("config set options.autocd true");
    eval_stmt(&p.parse_statements().unwrap()[0], &env, false)
        .await
        .unwrap();
    assert!(env.options.read().autocd);

    // get
    let mut p2 = Parser::new("config get options.autocd");
    let stmts2 = p2.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts2[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        // config get returns List containing the value
        assert_eq!(
            res,
            Val::List(vec![Val::Bool(true)]),
            "config get returned {res:?}"
        );
    }
}

#[tokio::test]
async fn test_happy_hook_precmd() {
    let env = setup_test_env();
    let mut p = Parser::new("hook precmd pwd");
    eval_stmt(&p.parse_statements().unwrap()[0], &env, false)
        .await
        .unwrap();

    let hooks = env.hooks.registry.read();
    assert!(hooks.iter().any(|(event, _)| event == "precmd"));
}

// =========================================================================
// String operations
// =========================================================================

#[tokio::test]
async fn test_happy_string_upper() {
    let env = setup_test_env();
    let got = get_var(
        r#"let text = "hello"; let r = $text | string upper"#,
        "r",
        &env,
    )
    .await;
    assert_eq!(got, Some(Val::List(vec![Val::String("HELLO".into())])));
}

#[tokio::test]
async fn test_happy_string_lower() {
    let env = setup_test_env();
    let got = get_var(
        r#"let text = "WORLD"; let r = $text | string lower"#,
        "r",
        &env,
    )
    .await;
    assert_eq!(got, Some(Val::List(vec![Val::String("world".into())])));
}

#[tokio::test]
async fn test_happy_string_trim() {
    let env = setup_test_env();
    let got = get_var(
        r##"let txt = "  hi  "; let r = $txt | string trim"##,
        "r",
        &env,
    )
    .await;
    assert_eq!(got, Some(Val::List(vec![Val::String("hi".into())])));
}

#[tokio::test]
async fn test_happy_string_contains() {
    let env = setup_test_env();
    // string contains via pipe
    let got = get_var(
        r##"let txt = "hello world"; let r = $txt | string contains world"##,
        "r",
        &env,
    )
    .await;
    assert_eq!(got, Some(Val::List(vec![Val::Bool(true)])));
}

// =========================================================================
// Pipelines — external commands piped to builtins
// =========================================================================

#[tokio::test]
async fn test_happy_seq_count() {
    let env = setup_test_env();
    // seq is external, count is builtin — Verona integration
    let got = get_var("let r = seq 5 | count", "r", &env).await;
    assert_eq!(got, Some(Val::List(vec![Val::Int(5)])));
}

#[tokio::test]
async fn test_happy_sort_field() {
    let env = setup_test_env();
    // seed, sort by size
    env.vars.write().insert(
        "files".into(),
        Val::List(vec![
            mk_file("c.txt", 100i64),
            mk_file("a.txt", 10),
            mk_file("b.txt", 50),
        ]),
    );
    let res = get_var("let r = $files | sort size", "r", &env).await;
    if let Some(Val::List(items)) = &res {
        assert_eq!(items.len(), 3);
        if let Val::Map(m) = &items[0] {
            assert_eq!(m.get(&ustr("name")), Some(&Val::String("a.txt".into())));
        } else {
            panic!("expected map, got {items:?}");
        }
    } else {
        panic!("expected Some(List), got {res:?}");
    }
}

#[tokio::test]
async fn test_happy_filter_field() {
    let env = setup_test_env();
    env.vars.write().insert(
        "files".into(),
        Val::List(vec![
            mk_file("a.txt", 10i64),
            mk_file("b.txt", 50),
            mk_file("c.txt", 100),
        ]),
    );
    let res = get_var(r#"let r = $files | filter size >= 50"#, "r", &env).await;
    if let Some(Val::List(items)) = &res {
        assert_eq!(items.len(), 2);
    } else {
        panic!("expected Some(List), got {res:?}");
    }
}

#[tokio::test]
async fn test_happy_map_projection() {
    let env = setup_test_env();
    env.vars.write().insert(
        "files".into(),
        Val::List(vec![mk_file("a.txt", 10i64), mk_file("b.txt", 50)]),
    );
    let res = get_var("let r = $files | map name", "r", &env).await;
    if let Some(Val::List(items)) = &res {
        assert_eq!(items.len(), 2);
        for item in items {
            if let Val::Map(m) = item {
                assert_eq!(m.len(), 1);
                assert!(m.contains_key(&ustr("name")));
            } else {
                panic!("expected map, got {item:?}");
            }
        }
    } else {
        panic!("expected Some(List), got {res:?}");
    }
}

fn mk_file(name: &str, size: i64) -> Val {
    let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    m.insert(ustr("name"), Val::String(name.into()));
    m.insert(ustr("size"), Val::Int(size));
    Val::Map(m)
}

// =========================================================================
// Exit codes & boolean
// =========================================================================

#[tokio::test]
async fn test_happy_true_exit_zero() {
    let env = setup_test_env();
    let mut p = Parser::new("true");
    eval_stmt(&p.parse_statements().unwrap()[0], &env, false)
        .await
        .unwrap();
    assert_eq!(env.vars.read().get("?"), Some(&Val::Int(0)));
}

#[tokio::test]
async fn test_happy_false_exit_one() {
    let env = setup_test_env();
    {
        let mut opts = env.options.write();
        opts.errexit = false;
    }
    let mut p = Parser::new("false");
    let _ = eval_stmt(&p.parse_statements().unwrap()[0], &env, false).await;
    assert_eq!(env.vars.read().get("?"), Some(&Val::Int(1)));
}

#[tokio::test]
async fn test_happy_and_chain() {
    let env = setup_test_env();
    let mut p = Parser::new("true && true");
    eval_stmt(&p.parse_statements().unwrap()[0], &env, false)
        .await
        .unwrap();
    assert_eq!(env.vars.read().get("?"), Some(&Val::Int(0)));
}

#[tokio::test]
async fn test_happy_or_chain() {
    let env = setup_test_env();
    let mut p = Parser::new("false || true");
    eval_stmt(&p.parse_statements().unwrap()[0], &env, false)
        .await
        .unwrap();
    assert_eq!(env.vars.read().get("?"), Some(&Val::Int(0)));
}

// =========================================================================
// Environment — export to process
// =========================================================================

#[tokio::test]
async fn test_happy_export() {
    let env = setup_test_env();
    // export uses FOO=bar syntax and stores in env.vars["env"] map
    let mut p = Parser::new(r#"export FSH_TEST=hello"#);
    eval_stmt(&p.parse_statements().unwrap()[0], &env, false)
        .await
        .unwrap();

    let vars = env.vars.read();
    let env_val = vars.get("env").unwrap();
    if let Val::Map(map) = env_val {
        assert_eq!(
            map.get(&ustr("FSH_TEST")),
            Some(&Val::String("hello".into()))
        );
    } else {
        panic!("expected env to be a Map, got {env_val:?}");
    }
}

// =========================================================================
// Daily Driver Workflows & Structured Pipelines
// =========================================================================

#[tokio::test]
async fn test_happy_file_pipeline_filter_and_sort() {
    let env = setup_test_env();
    let script = r#"
let files = [
    { name: "small.log", size: 10 },
    { name: "medium.txt", size: 500 },
    { name: "large.bin", size: 5000 }
]
let big_files = $files | filter size > 100 | sort size desc
"#;
    fshell_engine::run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    if let Some(Val::List(items)) = vars.get("big_files") {
        assert_eq!(items.len(), 2);
        if let Val::Map(first) = &items[0] {
            assert_eq!(
                first.get(&ustr("name")),
                Some(&Val::String("large.bin".to_string()))
            );
            assert_eq!(first.get(&ustr("size")), Some(&Val::Int(5000)));
        } else {
            panic!("Expected Map in big_files");
        }
    } else {
        panic!("Expected List for big_files");
    }
}

#[tokio::test]
async fn test_happy_nested_data_property_access() {
    let env = setup_test_env();
    let script = r#"
let app_config = {
    server: {
        host: "localhost",
        port: 8080,
        ssl: true
    },
    database: {
        connections: [
            { name: "primary", pool_size: 20 },
            { name: "replica", pool_size: 5 }
        ]
    }
}
let host = $app_config.server.host
let port = $app_config.server.port
"#;
    fshell_engine::run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(
        vars.get("host"),
        Some(&Val::String("localhost".to_string()))
    );
    assert_eq!(vars.get("port"), Some(&Val::Int(8080)));
}

#[tokio::test]
async fn test_happy_data_serialization_json_and_csv() {
    let env = setup_test_env();
    let script = r#"
let users = [
    { id: 1, name: "Alice", role: "admin" },
    { id: 2, name: "Bob", role: "developer" }
]
let json_str = $users | @json
let csv_str = $users | @csv
"#;
    fshell_engine::run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    if let Some(Val::List(items)) = vars.get("json_str") {
        let json_text = items[0].to_text();
        assert!(json_text.contains("Alice"));
        assert!(json_text.contains("admin"));
    } else {
        panic!("Expected List of json output");
    }

    if let Some(Val::List(items)) = vars.get("csv_str") {
        let csv_text = items
            .iter()
            .map(|v| v.to_text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(csv_text.contains("Alice"));
        assert!(csv_text.contains("developer"));
    } else {
        panic!("Expected List of csv output");
    }
}

#[tokio::test]
async fn test_happy_string_manipulation_pipeline() {
    let env = setup_test_env();
    let script = r#"
let raw_input = "  frontend, backend, database  "
let trimmed = (string trim $raw_input)
let upper = (string upper $trimmed)
let parts = (string split ", " $upper)
"#;
    fshell_engine::run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(
        vars.get("trimmed"),
        Some(&Val::List(vec![Val::String(
            "frontend, backend, database".to_string()
        )]))
    );
    assert_eq!(
        vars.get("upper"),
        Some(&Val::List(vec![Val::String(
            "FRONTEND, BACKEND, DATABASE".to_string()
        )]))
    );
}

#[tokio::test]
async fn test_happy_directory_stack_pushd_popd() {
    let env = setup_test_env();
    let temp_dir = tempfile::tempdir().unwrap();
    let dir_a = temp_dir.path().join("dir_a");
    let dir_b = temp_dir.path().join("dir_b");
    std::fs::create_dir(&dir_a).unwrap();
    std::fs::create_dir(&dir_b).unwrap();

    let dir_a_str = dir_a.to_string_lossy();
    let dir_b_str = dir_b.to_string_lossy();

    let script = format!(
        r#"
cd "{dir_a_str}"
pushd "{dir_b_str}"
let in_b = (pwd)
popd
let in_a = (pwd)
"#
    );
    fshell_engine::run_script(&script, &env).await.unwrap();

    let vars = env.vars.read();
    assert!(vars.get("in_b").is_some());
    assert!(vars.get("in_a").is_some());
}

#[tokio::test]
async fn test_happy_heredoc_config_generation() {
    let env = setup_test_env();
    let temp_dir = tempfile::tempdir().unwrap();
    let config_file = temp_dir.path().join("generated_config.yaml");
    let config_file_str = config_file.to_string_lossy();

    let script = format!(
        r#"
let SERVICE_NAME = "auth-service"
let PORT_NUM = "9090"
sh {{
    cat <<EOF > "{config_file_str}"
service:
  name: $SERVICE_NAME
  port: $PORT_NUM
  active: true
EOF
}}
"#
    );
    fshell_engine::run_script(&script, &env).await.unwrap();

    let content = std::fs::read_to_string(&config_file).unwrap();
    assert!(content.contains("name: auth-service"));
    assert!(content.contains("port: 9090"));
    assert!(content.contains("active: true"));
}

#[tokio::test]
async fn test_happy_reactive_cell_declaration_and_query() {
    let env = setup_test_env();
    let script = r#"
$= live_stream = echo "event_ok"
"#;
    fshell_engine::run_script(script, &env).await.unwrap();

    let reactive = env.reactive.pipelines.read();
    assert!(reactive.contains_key("live_stream"));
}
