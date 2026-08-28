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
async fn test_integration_hash_builtin() {
    let env = setup_test_env();

    // 1. Hash a file
    let tmp = tempfile::tempdir().unwrap();
    let file_path = tmp.path().join("test.txt");
    std::fs::write(&file_path, "hello").unwrap();
    let file_path_str = file_path.to_string_lossy().to_string();

    let digest = fshell_hash::fhash256(b"hello");
    let expected_hash = digest
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();

    let cmd = format!("hash {:?}", file_path_str);
    let mut parser = Parser::new(&cmd);
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        if let Val::List(items) = res {
            assert_eq!(items.len(), 1);
            if let Val::String(ref s) = items[0] {
                // Should look like "hexdigest  filename"
                assert!(s.len() > 64);
                assert!(s.starts_with(&expected_hash));
            } else {
                panic!("Expected String, got {:?}", items[0]);
            }
        } else {
            panic!("Expected List");
        }
    }

    // 2. Alias cksum on a file (resolves to hash)
    let cmd_cksum = format!("cksum {:?}", file_path_str);
    let mut parser = Parser::new(&cmd_cksum);
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        if let Val::List(items) = res {
            assert_eq!(items.len(), 1);
            if let Val::String(ref s) = items[0] {
                assert!(s.starts_with(&expected_hash));
            } else {
                panic!("Expected String, got {:?}", items[0]);
            }
        } else {
            panic!("Expected List");
        }
    }
}

#[tokio::test]
async fn test_integration_json_serialize() {
    let env = setup_test_env();
    // Seed a structured Val, then pipe through @json to serialize
    let raw_data = Val::Map({
        let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        m.insert(ustr::ustr("name"), Val::String("test".to_string()));
        m.insert(ustr::ustr("value"), Val::Int(42));
        m
    });
    env.vars.write().insert("data".to_string(), raw_data);
    let mut parser = Parser::new("$data | @json");
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(items) if items.len() == 1 => match &items[0] {
                Val::String(s) => {
                    assert!(s.contains("name"), "JSON should contain name");
                    assert!(s.contains("test"), "JSON should contain test");
                    assert!(s.contains("42"), "JSON should contain 42");
                }
                other => panic!("Expected Val::String in pipeline output, got {:?}", other),
            },
            other => panic!("Expected Val::List from pipeline, got {:?}", other),
        }
    } else {
        panic!("Expected Stmt::Expr");
    }
}

#[tokio::test]
async fn test_integration_json_deserialize() {
    let env = setup_test_env();
    // Seed a JSON string in Val's serde-tagged format (Map serializes as [[key, val], ...])
    let json_str = Val::String(
        r#"{"type":"Map","value":[["name",{"type":"String","value":"test"}],["value",{"type":"Int","value":42}]]}"#.to_string(),
    );
    env.vars.write().insert("json_input".to_string(), json_str);
    let mut parser = Parser::new("$json_input | @json");
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(items) if items.len() == 1 => match &items[0] {
                Val::Map(m) => {
                    assert_eq!(
                        m.get(&ustr::ustr("name")),
                        Some(&Val::String("test".to_string()))
                    );
                    assert_eq!(m.get(&ustr::ustr("value")), Some(&Val::Int(42)));
                }
                other => panic!(
                    "Expected Val::Map from JSON deserialization, got {:?}",
                    other
                ),
            },
            other => panic!("Expected Val::List from pipeline, got {:?}", other),
        }
    } else {
        panic!("Expected Stmt::Expr");
    }
}

#[tokio::test]
async fn test_integration_json_invalid() {
    let env = setup_test_env();
    // Seed an invalid JSON string, pipe through @json, expect error
    let bad_json = Val::String("not valid json".to_string());
    env.vars.write().insert("bad_json".to_string(), bad_json);
    let mut parser = Parser::new("$bad_json | @json");
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let result = eval_expr(expr, &env).await;
        assert!(
            result.is_err(),
            "Expected error from invalid JSON, got {:?}",
            result
        );
        assert!(
            result.unwrap_err().contains("JSON parse error"),
            "Expected JSON parse error message"
        );
    } else {
        panic!("Expected Stmt::Expr");
    }
}

#[tokio::test]
async fn test_integration_csv_roundtrip() {
    let env = setup_test_env();
    // Seed a CSV string into a variable and pipe through @csv to parse
    let csv_data = Val::String("name,size\nmain.rs,14230\nlib.rs,89201\n".to_string());
    env.vars.write().insert("csv_input".to_string(), csv_data);
    let mut parser = Parser::new("$csv_input | @csv");
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(outer) => {
                assert_eq!(outer.len(), 1, "pipeline output should have 1 item");
                // The single pipeline item is the Val::List of CSV records
                if let Val::List(items) = &outer[0] {
                    assert!(!items.is_empty(), "CSV parse should produce records");
                    if let Val::Map(map) = &items[0] {
                        assert_eq!(
                            map.get(&ustr::ustr("name")),
                            Some(&Val::String("main.rs".into()))
                        );
                        assert_eq!(map.get(&ustr::ustr("size")), Some(&Val::Int(14230)));
                    } else {
                        panic!("Expected Val::Map");
                    }
                } else {
                    panic!("Expected inner Val::List, got {:?}", outer[0]);
                }
            }
            other => panic!("Expected Val::List, got {:?}", other),
        }
    } else {
        panic!("Expected Stmt::Expr");
    }
}

#[tokio::test]
async fn test_integration_table_output() {
    let env = setup_test_env();
    // Seed structured data and pipe through @table
    let data = Val::List(vec![Val::Map({
        let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        m.insert(ustr::ustr("name"), Val::String("test.rs".to_string()));
        m.insert(ustr::ustr("size"), Val::Int(42));
        m
    })]);
    env.vars.write().insert("data".to_string(), data);
    let mut parser = Parser::new("$data | @table");
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(items) if items.len() == 1 => {
                if let Val::String(table) = &items[0] {
                    assert!(table.contains("name"));
                    assert!(table.contains("test.rs"));
                    assert!(table.contains("42"));
                } else {
                    panic!("Expected Val::String");
                }
            }
            other => panic!("Expected single Val::String in list, got {:?}", other),
        }
    }
}

#[tokio::test]
async fn test_integration_bar_output() {
    let env = setup_test_env();
    let data = Val::List(vec![Val::Map({
        let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        m.insert(ustr::ustr("ext"), Val::String("rs".to_string()));
        m.insert(ustr::ustr("count"), Val::Int(100));
        m
    })]);
    env.vars.write().insert("data".to_string(), data);
    let mut parser = Parser::new("$data | @bar");
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(items) if items.len() == 1 => {
                if let Val::String(chart) = &items[0] {
                    assert!(chart.contains("rs"));
                    assert!(chart.contains("100"));
                } else {
                    panic!("Expected Val::String");
                }
            }
            other => panic!("Expected single Val::String, got {:?}", other),
        }
    }
}

// Function definitions and calls

#[tokio::test]
async fn test_integration_help() {
    let env = setup_test_env();

    // Test help command parsing and execution (no args = overview)
    let mut parser = Parser::new("help");
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    // Test help with a topic
    let mut parser_ls = Parser::new("help ls");
    let stmts_ls = parser_ls.parse_statements().unwrap();
    eval_stmt(&stmts_ls[0], &env, false).await.unwrap();

    // Test help --topics
    let mut parser_topics = Parser::new("help --topics");
    let stmts_topics = parser_topics.parse_statements().unwrap();
    eval_stmt(&stmts_topics[0], &env, false).await.unwrap();
}

#[tokio::test]
async fn test_integration_z_builtin() {
    // Use an isolated temp database so tests don't pollute the user's real z database
    let tmp_db = std::env::temp_dir().join("fshell_test_z_integration.json");
    let _ = std::fs::remove_file(&tmp_db);
    set_var("FSH_Z_DB_PATH", &tmp_db.to_string_lossy());

    let env = setup_test_env();
    let tmp = std::env::temp_dir();
    let target = tmp.join("fshell_test_z_target");
    let _ = std::fs::create_dir_all(&target);

    // Grant directory read/write capability to target path
    {
        let mut caps = env.caps.caps.write();
        caps.grant(ResourceHandle::ReadDir(target.clone()));
        caps.grant(ResourceHandle::WriteDir(target.clone()));
        caps.grant(ResourceHandle::ReadFile(target.clone()));
        caps.grant(ResourceHandle::WriteFile(target.clone()));
    }

    // Log a visit to target path
    fshell_builtins::log_frecency_visit(&target).unwrap();

    // Call z_builtin with a fragment of the directory name
    let fragment = target.file_name().unwrap().to_string_lossy().to_string();
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    fshell_builtins::z_builtin(None, vec![Val::String(fragment)], &env, tx, None).unwrap();

    // Verify PWD is changed to the target path
    let current = env.cwd();
    assert_eq!(
        current.canonicalize().unwrap(),
        target.canonicalize().unwrap()
    );

    // Clean up
    let _ = std::fs::remove_file(&tmp_db);
    remove_var("FSH_Z_DB_PATH");
}

#[tokio::test]
async fn test_integration_extract_builtin_capabilities() {
    let env = setup_test_env();
    {
        let mut caps = env.caps.caps.write();
        caps.held.clear();
        caps.strict_mode = true; // enable strict mode so capability checks enforce
    }
    let tmp = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let archive = tmp.join("test_archive.zip");
    std::fs::write(&archive, b"PK\x03\x04mock_zip_content").unwrap();

    // Call extract without capabilities first -> should fail with permission error
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let res = fshell_builtins::extract_builtin(
        None,
        vec![Val::String(archive.to_string_lossy().to_string())],
        &env,
        tx,
        None,
    );
    assert!(
        res.is_err(),
        "should fail due to missing read file capability"
    );
    assert!(res.unwrap_err().message.contains("Capability denied"));

    // Grant read capability to the archive path but NOT write capability to destination or process-spawn
    {
        let mut caps = env.caps.caps.write();
        caps.grant(ResourceHandle::ReadFile(archive.clone()));
    }
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let res = fshell_builtins::extract_builtin(
        None,
        vec![Val::String(archive.to_string_lossy().to_string())],
        &env,
        tx,
        None,
    );
    assert!(
        res.is_err(),
        "should fail due to missing write capability to destination PWD"
    );
    assert!(res.unwrap_err().message.contains("Capability denied"));

    // Grant write capability to destination PWD but NOT process-spawn
    let pwd = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
    {
        let mut caps = env.caps.caps.write();
        caps.grant(ResourceHandle::WriteDir(pwd));
    }
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let res = fshell_builtins::extract_builtin(
        None,
        vec![Val::String(archive.to_string_lossy().to_string())],
        &env,
        tx,
        None,
    );
    match res {
        Ok(_) => panic!("should fail due to missing process-spawn capability"),
        Err(err) => {
            assert!(
                err.message.contains("process-spawn capability is required"),
                "Expected process-spawn error, but got: {}",
                err.message
            );
        }
    }

    // Cleanup
    let _ = std::fs::remove_file(&archive);
}

#[tokio::test]
async fn test_integration_shortcut_reminders() {
    let env = setup_test_env();

    // 1. Declare shortcut function
    let mut parser = Parser::new("fn ll() { ls -la }");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    // 2. Parse equivalent manual command
    let mut parser_manual = Parser::new("ls -la");
    let stmts_manual = parser_manual.parse_statements().unwrap();

    // 3. Verify AST structurally matches
    let fns = env.fns.read();
    let (params, _, body) = fns.get("ll").unwrap();
    assert!(params.is_empty());
    assert_eq!(body.len(), 1);
    match (body[0].unpack(), stmts_manual[0].unpack()) {
        (Stmt::Expr(e1), Stmt::Expr(e2)) => assert_eq!(e1.unpack(), e2.unpack()),
        _ => panic!("Expected Stmt::Expr on both sides"),
    }
}

#[tokio::test]
async fn test_integration_persistent_caps() {
    let (tmp, orig, _guard) = setup_config_test();
    let path = tmp.path().join(".config/fsh/caps.json");

    let env = setup_test_env();
    let action = fshell_engine::CapAction::ReadFile(std::path::PathBuf::from("/etc/hosts"));
    let handle = action.to_resource_handle();

    // Simulate saving capability persistently
    {
        let mut caps = env.caps.caps.write();
        caps.grant(handle.clone());
        if let Ok(content) = serde_json::to_string(&caps.held) {
            let _ = std::fs::create_dir_all(path.parent().unwrap());
            let _ = std::fs::write(&path, content);
        }
    }

    // Load new registry and verify it loaded the persistent capability
    let new_caps =
        fshell_capabilities::CapsRegistry::new_with_defaults(std::path::PathBuf::from("."));
    assert!(new_caps.check_read_file(std::path::Path::new("/etc/hosts")));

    teardown_config_test(&orig);
}

#[tokio::test]
async fn test_history_builtin_in_pipeline() {
    // Override the DB path for testing
    let temp_dir = std::env::temp_dir();
    let test_db_file = temp_dir.join(format!(
        "fshell_test_hist_pipeline_{}.db",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    set_var("FSH_TEST_DB_PATH", &test_db_file.to_string_lossy());

    // Initialize DB and register builtins
    let env = setup_test_env();
    fshell_repl::init(&env); // Registers "history" builtin and inits db!

    // Log some test commands in history
    fshell_repl::history::log_command(
        "cargo build",
        "/workspace",
        1000,
        150,
        Some(0),
        "myhost",
        "myuser",
        "sess1",
    )
    .unwrap();
    fshell_repl::history::log_command(
        "cargo test",
        "/workspace",
        2000,
        320,
        Some(0),
        "myhost",
        "myuser",
        "sess1",
    )
    .unwrap();
    fshell_repl::history::log_command(
        "invalid_cmd",
        "/workspace",
        3000,
        50,
        Some(1),
        "myhost",
        "myuser",
        "sess1",
    )
    .unwrap();

    // Execute pipeline: history --global | filter exit_code == 0
    let mut parser = Parser::new("history --global | filter exit_code == 0");
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);

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

            // We logged 3 commands, 2 of them have exit_code = 0
            assert_eq!(results.len(), 2);

            // Let's verify details of the first returned command (ordered DESC by timestamp, so "cargo test" is first)
            if let Val::Map(m) = &results[0] {
                assert_eq!(
                    m.get(&ustr::ustr("command")),
                    Some(&Val::String("cargo test".to_string()))
                );
                assert_eq!(m.get(&ustr::ustr("exit_code")), Some(&Val::Int(0)));
            } else {
                panic!("Expected Val::Map");
            }
        } else {
            panic!("Expected Expr::Pipeline");
        }
    } else {
        panic!("Expected Stmt::Expr");
    }

    // Clean up
    let _ = std::fs::remove_file(&test_db_file);
    remove_var("FSH_TEST_DB_PATH");
}

#[tokio::test]
async fn test_integration_which_builtin() {
    let env = setup_test_env();
    let mut parser = Parser::new("which ls");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(fshell_core::ast::Expr::Pipeline(pipeline)) = &stmts[0] {
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
        assert!(!results.is_empty());
        if let Val::String(path) = &results[0] {
            assert!(path.contains("bin/ls") || path.contains("/ls"));
        } else {
            panic!("Expected String path");
        }
    }
}

#[tokio::test]
async fn test_integration_touch_rm_mkdir() {
    let env = setup_test_env();
    let tmp = std::env::temp_dir().join(format!("fshell_int_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    {
        let mut caps = env.caps.caps.write();
        caps.grant(fshell_core::ResourceHandle::ReadFile(tmp.clone()));
        caps.grant(fshell_core::ResourceHandle::WriteFile(tmp.clone()));
        caps.grant(fshell_core::ResourceHandle::ReadDir(tmp.clone()));
        caps.grant(fshell_core::ResourceHandle::WriteDir(tmp.clone()));
    }

    let test_file = tmp.join("test.txt");
    let test_dir = tmp.join("newdir");

    let touch_cmd = format!("touch {:?}", test_file.to_string_lossy());
    let mut parser = Parser::new(&touch_cmd);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    assert!(test_file.exists());

    let rm_cmd = format!("rm {:?}", test_file.to_string_lossy());
    let mut parser = Parser::new(&rm_cmd);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    assert!(!test_file.exists());

    let mkdir_cmd = format!("mkdir {:?}", test_dir.to_string_lossy());
    let mut parser = Parser::new(&mkdir_cmd);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    assert!(test_dir.is_dir());

    let test_file2 = tmp.join("test2.txt");
    std::fs::write(&test_file2, "hello\nworld").unwrap();
    let cat_cmd = format!("cat {:?}", test_file2.to_string_lossy());
    let mut parser = Parser::new(&cat_cmd);
    let stmts = parser.parse_statements().unwrap();
    let results = if let Stmt::Expr(expr) = stmts[0].unpack()
        && let Expr::Pipeline(pipeline) = expr.unpack()
    {
        fshell_engine::collect_pipeline(pipeline, &env)
            .await
            .unwrap()
    } else {
        panic!("Expected pipeline statement, got {:?}", stmts[0].unpack());
    };
    assert_eq!(results.len(), 2);
    assert_eq!(results[0], Val::String("hello".to_string()));
    assert_eq!(results[1], Val::String("world".to_string()));

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn test_integration_pushd_popd_dirs() {
    let _cwd_guard = CwdGuard::new_temp();
    let env = setup_test_env();
    let tmp = std::env::temp_dir();
    let dir1 = tmp.join("fshell_test_dir1");
    let dir2 = tmp.join("fshell_test_dir2");
    let _ = std::fs::create_dir_all(&dir1);
    let _ = std::fs::create_dir_all(&dir2);

    // Grant caps
    {
        let mut caps = env.caps.caps.write();
        caps.grant(ResourceHandle::ReadDir(dir1.clone()));
        caps.grant(ResourceHandle::ReadDir(dir2.clone()));
        caps.grant(ResourceHandle::ReadDir(tmp.clone()));
    }

    let initial_dir = env.cwd();

    // 1. pushd to dir1
    let (tx1, _rx1) = tokio::sync::mpsc::channel(100);
    fshell_builtins::pushd_builtin(
        None,
        vec![Val::String(dir1.to_string_lossy().to_string())],
        &env,
        tx1,
        None,
    )
    .unwrap();
    assert_eq!(
        env.cwd().canonicalize().unwrap(),
        dir1.canonicalize().unwrap()
    );

    // 2. pushd to dir2
    let (tx2, _rx2) = tokio::sync::mpsc::channel(100);
    fshell_builtins::pushd_builtin(
        None,
        vec![Val::String(dir2.to_string_lossy().to_string())],
        &env,
        tx2,
        None,
    )
    .unwrap();
    assert_eq!(
        env.cwd().canonicalize().unwrap(),
        dir2.canonicalize().unwrap()
    );

    // 3. popd
    let (tx3, _rx3) = tokio::sync::mpsc::channel(100);
    fshell_builtins::popd_builtin(None, vec![], &env, tx3, None).unwrap();
    assert_eq!(
        env.cwd().canonicalize().unwrap(),
        dir1.canonicalize().unwrap()
    );

    // 4. popd
    let (tx4, _rx4) = tokio::sync::mpsc::channel(100);
    fshell_builtins::popd_builtin(None, vec![], &env, tx4, None).unwrap();
    assert_eq!(
        env.cwd().canonicalize().unwrap(),
        initial_dir.canonicalize().unwrap()
    );
}

#[tokio::test]
async fn test_integration_read_builtin_timeout() {
    let env = setup_test_env();
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);
    let (_in_tx, in_rx) = tokio::sync::mpsc::channel(100);

    // Call with a 1 second timeout. Since no data is sent on in_rx, it will time out
    // and produce an empty string.
    fshell_builtins::read::read_builtin(
        Some(in_rx),
        vec![
            Val::String("-t".to_string()),
            Val::String("1".to_string()),
            Val::String("TEST_VAR".to_string()),
        ],
        &env,
        tx,
        None,
    )
    .unwrap();

    // Wait for the spawned task to complete
    let mut val = None;
    while let Some(payload) = rx.recv().await {
        if let PipelinePayload::Data(v) = payload {
            val = Some((*v).clone());
        }
    }

    assert!(val.is_some());
    // The variable TEST_VAR should be set in env
    let vars = env.vars.read();
    let var_val = vars.get("TEST_VAR").unwrap();
    assert!(matches!(var_val, Val::String(_)));
}

#[tokio::test]
async fn test_integration_cat_short_circuit() {
    let env = setup_test_env();
    let temp_dir = std::env::temp_dir();
    let test_file = temp_dir.join("fshell_cat_test.txt");
    let _ = std::fs::remove_file(&test_file);

    std::fs::write(&test_file, "hello\nworld\n").unwrap();

    // Grant read permission
    {
        let mut caps = env.caps.caps.write();
        caps.grant(ResourceHandle::ReadFile(test_file.clone()));
    }

    // Command: cat <test_file>
    let script = format!("cat \"{}\"", test_file.to_string_lossy().escape_default());
    let mut parser = Parser::new(&script);
    let stmts = parser.parse_statements().unwrap();
    let pipeline = match stmts[0].clone().into_unpack() {
        Stmt::Expr(expr) => match expr.into_unpack() {
            Expr::Pipeline(p) => p,
            _ => panic!("Expected Expr::Pipeline"),
        },
        _ => panic!("Expected Stmt::Expr"),
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel(100);
    execute_pipeline(&pipeline, &env, tx).await.unwrap();

    let mut results = Vec::new();
    while let Some(payload) = rx.recv().await {
        match payload {
            PipelinePayload::Data(v) => results.push((*v).clone()),
            PipelinePayload::Bytes(b) => {
                results.push(Val::String(String::from_utf8_lossy(&b).into_owned()))
            }
            PipelinePayload::Structured(d) => panic!("Unexpected error: {:?}", d),
        }
    }

    assert_eq!(results.len(), 2);
    assert_eq!(results[0], Val::String("hello".to_string()));
    assert_eq!(results[1], Val::String("world".to_string()));

    let _ = std::fs::remove_file(&test_file);
}

#[tokio::test]
async fn test_integration_ls_drain() {
    let env = setup_test_env();
    let mut parser = Parser::new("ls");
    let stmts = parser.parse_statements().unwrap();
    let pipeline = match stmts[0].unpack() {
        Stmt::Expr(expr) => match expr.unpack() {
            Expr::Pipeline(p) => p.clone(),
            _ => panic!("Expected pipeline"),
        },
        _ => panic!("Expected expression"),
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel(100);
    let tx_err = tx.clone();
    let mut env_clone = env.clone();
    env_clone.is_captured = true;
    let pipeline_clone = pipeline.clone();

    tokio::spawn(async move {
        if let Err(e) = execute_pipeline(&pipeline_clone, &env_clone, tx).await {
            let _ = tx_err.send(PipelinePayload::Structured(e.into())).await;
        }
    });

    let mut results = Vec::new();
    while let Some(payload) = rx.recv().await {
        match payload {
            PipelinePayload::Data(v) => results.push((*v).clone()),
            PipelinePayload::Bytes(b) => {
                results.push(Val::String(String::from_utf8_lossy(&b).into_owned()))
            }
            PipelinePayload::Structured(_) => {}
        }
    }
    assert!(!results.is_empty());
}

#[tokio::test]
async fn test_phase3_builtins_integration() {
    let env = setup_test_env();

    // 1. test and [ ]
    let script = "let t1 = $(test -n \"\")\n\
                  let t2 = $(test -z \"\")\n\
                  let t3 = $([ \"a\" = \"a\" ])\n\
                  let t4 = $([ \"a\" != \"b\" ])\n\
                  let t5 = $(test 3 -gt 1)";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in &stmts {
        eval_stmt(stmt, &env, false).await.unwrap();
    }
    {
        let vars = env.vars.read();
        assert_eq!(vars.get("t1"), Some(&Val::List(vec![Val::Bool(false)])));
        assert_eq!(vars.get("t2"), Some(&Val::List(vec![Val::Bool(true)])));
        assert_eq!(vars.get("t3"), Some(&Val::List(vec![Val::Bool(true)])));
        assert_eq!(vars.get("t4"), Some(&Val::List(vec![Val::Bool(true)])));
        assert_eq!(vars.get("t5"), Some(&Val::List(vec![Val::Bool(true)])));
    }

    // 2. printf
    let script = "let p1 = $(printf \"hello %s %d\" \"world\" 42)";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    {
        let vars = env.vars.read();
        assert_eq!(
            vars.get("p1"),
            Some(&Val::List(vec![Val::String("hello world 42".to_string())]))
        );
    }

    // 3. true / false
    let script = "let tr = $(true)\n\
                  let fa = $(false)";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in &stmts {
        eval_stmt(stmt, &env, false).await.unwrap();
    }
    {
        let vars = env.vars.read();
        assert_eq!(vars.get("tr"), Some(&Val::List(vec![Val::Bool(true)])));
        assert_eq!(vars.get("fa"), Some(&Val::List(vec![Val::Bool(false)])));
    }

    // 4. sleep
    let script = "sleep 5ms";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    let start = std::time::Instant::now();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    assert!(start.elapsed() >= std::time::Duration::from_millis(5));

    // 5. echo options
    let script = "let e1 = $(echo -n \"hello\")\n\
                  let e2 = $(echo -e \"hello\\tworld\")";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in &stmts {
        eval_stmt(stmt, &env, false).await.unwrap();
    }
    {
        let vars = env.vars.read();
        // `echo -n` appends a NUL sentinel only for terminal writers; captured
        // values must be clean (the sentinel is stripped at the capture boundary).
        assert_eq!(
            vars.get("e1"),
            Some(&Val::List(vec![Val::String("hello".to_string())]))
        );
        assert_eq!(
            vars.get("e2"),
            Some(&Val::List(vec![Val::String("hello\tworld".to_string())]))
        );
    }
}

#[tokio::test]
async fn test_string_split() {
    let env = setup_test_env();
    let script = r#"let text = "hello world"; let result = $text | string split " ""#;
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in &stmts {
        eval_stmt(stmt, &env, false).await.unwrap();
    }
    let vars = env.vars.read();
    assert_eq!(
        vars.get("result"),
        Some(&Val::List(vec![Val::List(vec![
            Val::String("hello".to_string()),
            Val::String("world".to_string()),
        ])]))
    );
}

#[tokio::test]
async fn test_string_upper() {
    let env = setup_test_env();
    let script = r#"let text = "hello"; let result = $text | string upper"#;
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in &stmts {
        eval_stmt(stmt, &env, false).await.unwrap();
    }
    let vars = env.vars.read();
    assert_eq!(
        vars.get("result"),
        Some(&Val::List(vec![Val::String("HELLO".to_string())]))
    );
}

#[tokio::test]
async fn test_string_lower() {
    let env = setup_test_env();
    let script = r#"let text = "WORLD"; let result = $text | string lower"#;
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in &stmts {
        eval_stmt(stmt, &env, false).await.unwrap();
    }
    let vars = env.vars.read();
    assert_eq!(
        vars.get("result"),
        Some(&Val::List(vec![Val::String("world".to_string())]))
    );
}

#[tokio::test]
async fn test_string_trim() {
    let env = setup_test_env();
    let script = r#"let text = "  hello  "; let result = $text | string trim"#;
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in &stmts {
        eval_stmt(stmt, &env, false).await.unwrap();
    }
    let vars = env.vars.read();
    assert_eq!(
        vars.get("result"),
        Some(&Val::List(vec![Val::String("hello".to_string())]))
    );
}

#[tokio::test]
async fn test_string_contains() {
    let env = setup_test_env();
    let script = r#"let text = "hello world"; let result = $text | string contains "world""#;
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in &stmts {
        eval_stmt(stmt, &env, false).await.unwrap();
    }
    let vars = env.vars.read();
    assert_eq!(vars.get("result"), Some(&Val::List(vec![Val::Bool(true)])));
}

#[tokio::test]
async fn test_string_starts_with() {
    let env = setup_test_env();
    let script = r#"let text = "hello"; let result = $text | string starts-with "hel""#;
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in &stmts {
        eval_stmt(stmt, &env, false).await.unwrap();
    }
    let vars = env.vars.read();
    assert_eq!(vars.get("result"), Some(&Val::List(vec![Val::Bool(true)])));
}

#[tokio::test]
async fn test_string_ends_with() {
    let env = setup_test_env();
    let script = r#"let text = "hello"; let result = $text | string ends-with "llo""#;
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in &stmts {
        eval_stmt(stmt, &env, false).await.unwrap();
    }
    let vars = env.vars.read();
    assert_eq!(vars.get("result"), Some(&Val::List(vec![Val::Bool(true)])));
}

#[tokio::test]
async fn test_string_substring() {
    let env = setup_test_env();
    let script = r#"let text = "hello world"; let result = $text | string substring 0 5"#;
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in &stmts {
        eval_stmt(stmt, &env, false).await.unwrap();
    }
    let vars = env.vars.read();
    assert_eq!(
        vars.get("result"),
        Some(&Val::List(vec![Val::String("hello".to_string())]))
    );
}

#[tokio::test]
async fn test_integration_completions() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();

    // Setup temporary config directory
    let temp_dir = tempfile::tempdir().unwrap();
    let config_dir_path = temp_dir.path().to_path_buf();

    let orig_config_dir = std::env::var("FSH_CONFIG_DIR").ok();
    fshell_core::set_var("FSH_CONFIG_DIR", config_dir_path.to_str().unwrap());

    // Register completions using eval_stmt
    let script = r#"complete docker -c "run" -a "(__fsh_docker_images)""#;
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    // Check registry in env
    {
        let reg = env.completions.read();
        let docker_comp = reg.get("docker").expect("docker completion not found");
        assert_eq!(docker_comp.dynamic_providers.len(), 1);
        assert_eq!(
            docker_comp.dynamic_providers[0].command,
            "(__fsh_docker_images)"
        );
        assert_eq!(
            docker_comp.dynamic_providers[0].parent_subcmds,
            vec!["run".to_string()]
        );
    }

    // Check that completions.toml was saved
    let toml_path = config_dir_path.join("completions.toml");
    assert!(toml_path.exists());
    let toml_content = std::fs::read_to_string(&toml_path).unwrap();
    assert!(toml_content.contains("docker"));
    assert!(toml_content.contains("(__fsh_docker_images)"));

    // Clear in-memory completions registry
    {
        let mut reg = env.completions.write();
        reg.clear();
    }

    // Load completions from disk
    fshell_engine::load_completions(&env).unwrap();

    // Verify it is loaded back
    {
        let reg = env.completions.read();
        let docker_comp = reg.get("docker").expect("docker completion not loaded");
        assert_eq!(docker_comp.dynamic_providers.len(), 1);
        assert_eq!(
            docker_comp.dynamic_providers[0].command,
            "(__fsh_docker_images)"
        );
    }

    // Clean up environment variable
    if let Some(orig) = orig_config_dir {
        fshell_core::set_var("FSH_CONFIG_DIR", &orig);
    } else {
        fshell_core::remove_var("FSH_CONFIG_DIR");
    }
}

#[tokio::test]
async fn test_integration_funcsave_builtin() {
    let _guard = ProcessLockGuard::acquire();
    let orig = std::env::var("FSH_CONFIG_DIR").ok();
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path().to_string_lossy().to_string();
    set_var("FSH_CONFIG_DIR", &config_dir);

    // 1. Create env and define a function
    let env = setup_test_env();
    let script = "fn my_persisted_func(x) { let y = $x + 1; return $y }";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    // Verify it is defined in memory
    {
        let fns = env.fns.read();
        assert!(fns.contains_key("my_persisted_func"));
    }

    // 2. Save it using funcsave
    let save_script = "funcsave \"my_persisted_func\"";
    let mut parser2 = Parser::new(save_script);
    let stmts2 = parser2.parse_statements().unwrap();
    eval_stmt(&stmts2[0], &env, false).await.unwrap();

    // Verify file exists and has correct content
    let init_path = tmp.path().join("init.fsh");
    assert!(init_path.exists());
    let content = std::fs::read_to_string(&init_path).unwrap();
    assert!(content.contains("fn my_persisted_func"));
    assert!(content.contains("let y = $x + 1"));

    // 3. Create a new environment, which should load the config script if we source it
    let env2 = setup_test_env();
    // Initially not defined in the new environment
    {
        let fns2 = env2.fns.read();
        assert!(!fns2.contains_key("my_persisted_func"));
    }

    // Source the init.fsh file
    let source_script = format!("source \"{}\"", init_path.to_string_lossy());
    let mut parser3 = Parser::new(&source_script);
    let stmts3 = parser3.parse_statements().unwrap();
    eval_stmt(&stmts3[0], &env2, false).await.unwrap();

    // Verify the function is now loaded and executable in env2
    {
        let fns2 = env2.fns.read();
        assert!(fns2.contains_key("my_persisted_func"));
    }

    // Run the function and verify results
    let run_script = "let res = my_persisted_func(41)";
    let mut parser4 = Parser::new(run_script);
    let stmts4 = parser4.parse_statements().unwrap();
    eval_stmt(&stmts4[0], &env2, false).await.unwrap();
    {
        let vars = env2.vars.read();
        assert_eq!(vars.get("res"), Some(&Val::List(vec![Val::Int(42)])));
    }

    if let Some(o) = orig {
        set_var("FSH_CONFIG_DIR", &o);
    } else {
        remove_var("FSH_CONFIG_DIR");
    }
}

#[tokio::test]
async fn test_profile_builtin_basic() {
    let env = setup_test_env();
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);

    fshell_builtins::profile_builtin(None, vec![Val::String("on".into())], &env, tx.clone(), None)
        .unwrap();
    let payload = rx.recv().await.unwrap();
    if let PipelinePayload::Data(val) = payload {
        assert!(val.to_text().contains("Profiling enabled"));
    } else {
        panic!("Expected Data payload");
    }

    let (tx2, mut rx2) = tokio::sync::mpsc::channel(32);
    fshell_builtins::profile_builtin(None, vec![], &env, tx2.clone(), None).unwrap();
    let payload = rx2.recv().await.unwrap();
    if let PipelinePayload::Data(val) = payload {
        let text = val.to_text();
        assert!(text.contains("calls") || text.contains("No profiling data"));
    } else {
        panic!("Expected Data payload");
    }
}

#[tokio::test]
async fn test_integration_disabled_builtins() {
    let env = setup_test_env();

    // By default, echo is a builtin
    assert!(env.get_builtin("echo").is_some());
    assert!(env.get_all_builtins().contains(&"echo".to_string()));

    // Disable echo
    {
        let mut opts = env.options.write();
        opts.disabled_builtins = vec!["echo".to_string()];
        env.invalidate_builtins_cache();
    }

    // Now echo is not a builtin
    assert!(env.get_builtin("echo").is_none());
    assert!(!env.get_all_builtins().contains(&"echo".to_string()));
}

#[tokio::test]
async fn test_integration_pushd_popd_dirs_streaming() {
    let _cwd_guard = CwdGuard::new_temp();
    let env = setup_test_env();

    // pushd /tmp | ...
    let script = "let p = $| pushd /tmp |";
    let mut parser = fshell_core::Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    fshell_engine::eval_stmt(&stmts[0], &env, false)
        .await
        .unwrap();

    let vars = env.vars.read();
    let p = vars.get("p").unwrap();
    if let Val::List(items) = p {
        assert_eq!(items.len(), 1);
        if let Val::String(s) = &items[0] {
            assert!(s.contains("/tmp") || s.contains("/private/tmp"));
        } else {
            panic!("Expected Val::String in p items, got {:?}", items[0]);
        }
    } else {
        panic!("Expected Val::List for p, got {:?}", p);
    }
}

#[tokio::test]
async fn test_explain_builtin_specific_code_and_list() {
    let env = setup_test_env();

    // Test explain with specific code
    let mut parser = fshell_core::Parser::new("explain FSH-TYPE-001");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack()
        && let Expr::Pipeline(pipeline) = expr.unpack()
    {
        let res = fshell_engine::collect_pipeline(pipeline, &env)
            .await
            .unwrap();
        assert_eq!(res.len(), 1);
        let text = res[0].to_text();
        assert!(text.contains("TypeError: FSH-TYPE-001"));
        if option_env!("FSHELL_DOCS_BASE").is_some() {
            assert!(text.contains("FSH-TYPE-001") && text.contains("docs:"));
        } else {
            assert!(!text.contains("↳ docs:"));
        }
    } else {
        panic!("Expected pipeline statement");
    }

    // Test explain --list
    let mut parser_list = fshell_core::Parser::new("explain --list");
    let stmts_list = parser_list.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts_list[0].unpack()
        && let Expr::Pipeline(pipeline) = expr.unpack()
    {
        let res_list = fshell_engine::collect_pipeline(pipeline, &env)
            .await
            .unwrap();
        assert_eq!(res_list.len(), 1);
        if let Val::List(items) = &res_list[0] {
            assert!(items.len() >= 40);
            let first_map = &items[0];
            if let Val::Map(m) = first_map {
                assert!(m.contains_key(&ustr::ustr("code")));
                assert!(m.contains_key(&ustr::ustr("name")));
                if option_env!("FSHELL_DOCS_BASE").is_some() {
                    assert!(m.contains_key(&ustr::ustr("docs_url")));
                }
            } else {
                panic!("Expected map item in explain --list output");
            }
        } else {
            panic!("Expected list from explain --list");
        }
    } else {
        panic!("Expected pipeline statement");
    }
}

#[tokio::test]
async fn test_bind_and_keybindings_architecture() {
    let env = setup_test_env();

    // 1. Switch to vi mode and back to emacs
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    fshell_builtins::builtin_bind(
        None,
        vec![Val::String("-v".to_string())],
        &env,
        tx.clone(),
        None,
    )
    .unwrap();
    assert_eq!(
        env.keybindings.read().active_mode,
        fshell_engine::keybindings::KeyMapMode::ViInsert
    );

    fshell_builtins::builtin_bind(
        None,
        vec![Val::String("-e".to_string())],
        &env,
        tx.clone(),
        None,
    )
    .unwrap();
    assert_eq!(
        env.keybindings.read().active_mode,
        fshell_engine::keybindings::KeyMapMode::Emacs
    );

    // 2. Bind custom widget
    fshell_builtins::builtin_bind(
        None,
        vec![
            Val::String("ctrl-o".to_string()),
            Val::String("kill-buffer".to_string()),
        ],
        &env,
        tx.clone(),
        None,
    )
    .unwrap();

    let chord = fshell_engine::keybindings::KeyChord::parse("ctrl-o").unwrap();
    assert_eq!(
        env.keybindings
            .read()
            .get_action(fshell_engine::keybindings::KeyMapMode::Emacs, &chord),
        Some(&fshell_engine::keybindings::KeyAction::Widget(
            "kill-buffer".to_string()
        ))
    );

    // 3. Bind macro
    fshell_builtins::builtin_bind(
        None,
        vec![
            Val::String("-s".to_string()),
            Val::String("ctrl-g".to_string()),
            Val::String("git status\n".to_string()),
        ],
        &env,
        tx.clone(),
        None,
    )
    .unwrap();

    let chord_g = fshell_engine::keybindings::KeyChord::parse("ctrl-g").unwrap();
    assert_eq!(
        env.keybindings
            .read()
            .get_action(fshell_engine::keybindings::KeyMapMode::Emacs, &chord_g),
        Some(&fshell_engine::keybindings::KeyAction::Macro(
            "git status\n".to_string()
        ))
    );

    // 4. Unbind key
    fshell_builtins::builtin_bind(
        None,
        vec![
            Val::String("-r".to_string()),
            Val::String("ctrl-o".to_string()),
        ],
        &env,
        tx.clone(),
        None,
    )
    .unwrap();

    assert_eq!(
        env.keybindings
            .read()
            .get_action(fshell_engine::keybindings::KeyMapMode::Emacs, &chord),
        None
    );

    // 5. Query and list
    fshell_builtins::builtin_bind(
        None,
        vec![Val::String("-l".to_string())],
        &env,
        tx.clone(),
        None,
    )
    .unwrap();
    fshell_builtins::builtin_bind(None, vec![], &env, tx.clone(), None).unwrap();
}

#[tokio::test]
async fn test_compgen_builtin() {
    let env = setup_test_env();
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    // 1. Wordlist completion
    fshell_builtins::complete::compgen_builtin(
        None,
        vec![
            Val::String("-W".to_string()),
            Val::String("alpha alpine amazon beta".to_string()),
            Val::String("al".to_string()),
        ],
        &env,
        tx,
        None,
    )
    .unwrap();

    let mut words = Vec::new();
    while let Some(payload) = rx.recv().await {
        if let fshell_engine::PipelinePayload::Data(val) = payload {
            words.push(val.to_text());
        }
    }
    assert_eq!(words, vec!["alpha", "alpine"]);

    // 2. Builtins completion
    let (tx2, mut rx2) = tokio::sync::mpsc::channel(100);
    fshell_builtins::complete::compgen_builtin(
        None,
        vec![Val::String("-b".to_string()), Val::String("wh".to_string())],
        &env,
        tx2,
        None,
    )
    .unwrap();

    let mut builtins = Vec::new();
    while let Some(payload) = rx2.recv().await {
        if let fshell_engine::PipelinePayload::Data(val) = payload {
            builtins.push(val.to_text());
        }
    }
    assert!(builtins.contains(&"which".to_string()));
}
