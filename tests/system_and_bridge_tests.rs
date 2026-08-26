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
async fn test_integration_cd_and_paths() {
    let _cwd_guard = ProcessLockGuard::acquire();
    let env = setup_test_env();
    let old_dir = env.cwd();

    // Grant capability for the parent directory to allow cd .. in sandbox
    env.caps
        .caps
        .write()
        .grant(fshell_core::ResourceHandle::ReadDir(
            old_dir.parent().unwrap().to_path_buf(),
        ));

    // Test cd ..
    let mut parser = Parser::new("cd ..");
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);

    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let new_dir = env.cwd();

    // It should have successfully moved to the parent directory
    assert_eq!(new_dir, old_dir.parent().unwrap());
}

#[tokio::test]
async fn test_custom_user_tilde_expansion() {
    let env = setup_test_env();

    let mut p = Parser::new("fs-read \"~root\"");
    let stmts = p.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts[0] {
        let res = eval_expr(expr, &env).await.unwrap();
        if let Val::List(items) = res {
            if let Val::Capability(ResourceHandle::ReadDir(path)) = &items[0] {
                assert!(
                    path.to_string_lossy().ends_with("root"),
                    "path was {:?}",
                    path
                );
            } else {
                panic!("Expected ResourceHandle::ReadDir, got {:?}", items[0]);
            }
        }
    }
}

#[tokio::test]
async fn test_alias_expansion_pipeline_output() {
    let mut env = setup_test_env();
    env.is_captured = true;
    env.register_alias("l", "ls -l");

    // Parse a command that triggers the alias
    let mut parser = Parser::new("l");
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);

    // Extract the pipeline from the parsed statement (unwrap Spanned wrappers)
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

    assert!(
        !results.is_empty(),
        "Alias 'l' → 'ls -l' should produce directory entries"
    );
}

#[tokio::test]
async fn test_integration_bridge_fallthrough_echo() {
    let env = setup_test_env();
    let mut parser = Parser::new(r##"echo "hello world""##);
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts[0] {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(list) => {
                assert!(!list.is_empty(), "Expected non-empty list from echo");
                // echo produces Val::String (not Val::Map) — verifies fallthrough
                assert!(
                    matches!(&list[0], Val::String(_)),
                    "Expected Val::String (fallthrough preserved), got {:?}",
                    list[0]
                );
            }
            other => panic!("Expected List, got {:?}", other),
        }
    }
}

#[tokio::test]
async fn test_integration_redirections() {
    let env = setup_test_env();
    let temp_dir = std::env::temp_dir();
    let unique_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let out_file = temp_dir.join(format!("fshell_test_out_{}.txt", unique_id));
    let err_file = temp_dir.join(format!("fshell_test_err_{}.txt", unique_id));
    let _ = std::fs::remove_file(&out_file);
    let _ = std::fs::remove_file(&err_file);

    // Grant write permission to temp dir
    {
        let mut caps = env.caps.caps.write();
        caps.grant(ResourceHandle::WriteDir(temp_dir.clone()));
        caps.grant(ResourceHandle::WriteFile(out_file.clone()));
        caps.grant(ResourceHandle::WriteFile(err_file.clone()));
        caps.grant(ResourceHandle::ReadFile(out_file.clone()));
        caps.grant(ResourceHandle::ReadFile(err_file.clone()));
    }

    // 1. Stdout overwrite >
    let script = format!(
        "let list = [1, 2, 3]; $list > \"{}\"",
        out_file.to_string_lossy().escape_default()
    );
    let mut parser = Parser::new(&script);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    eval_stmt(&stmts[1], &env, false).await.unwrap();

    let content = std::fs::read_to_string(&out_file).unwrap();
    assert_eq!(content, "1\n2\n3\n");

    // 2. Stdout append >>
    let script2 = format!(
        "let list2 = [4]; $list2 >> \"{}\"",
        out_file.to_string_lossy().escape_default()
    );
    let mut parser2 = Parser::new(&script2);
    let stmts2 = parser2.parse_statements().unwrap();
    eval_stmt(&stmts2[0], &env, false).await.unwrap();
    eval_stmt(&stmts2[1], &env, false).await.unwrap();

    let content = std::fs::read_to_string(&out_file).unwrap();
    assert_eq!(content, "1\n2\n3\n4\n");

    // 3. Stderr/Diagnostic redirection 2>
    // A command that produces a diagnostic: cd into a nonexistent directory
    let script3 = format!(
        "cd /nonexistent_dir_foo_bar_xyz 2> \"{}\"",
        err_file.to_string_lossy().escape_default()
    );
    let mut parser3 = Parser::new(&script3);
    let stmts3 = parser3.parse_statements().unwrap();
    let _ = eval_stmt(&stmts3[0], &env, false).await;

    let content_err = std::fs::read_to_string(&err_file).unwrap();
    assert!(content_err.contains("nonexistent_dir_foo_bar_xyz"));

    // Cleanup
    let _ = std::fs::remove_file(&out_file);
    let _ = std::fs::remove_file(&err_file);
}

#[tokio::test]
async fn test_integration_redirections_both() {
    let env = setup_test_env();
    let temp_dir = std::env::temp_dir();
    let out_file = temp_dir.join("fshell_both_out.txt");
    let _ = std::fs::remove_file(&out_file);

    // Grant write permission to temp dir
    {
        let mut caps = env.caps.caps.write();
        caps.grant(ResourceHandle::WriteDir(temp_dir.clone()));
        caps.grant(ResourceHandle::WriteFile(out_file.clone()));
        caps.grant(ResourceHandle::ReadFile(out_file.clone()));
    }

    // 1. &> redirection for both stdout and stderr
    let script = format!(
        "let list = [42]; $list &> \"{}\"",
        out_file.to_string_lossy().escape_default()
    );
    let mut parser = Parser::new(&script);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    eval_stmt(&stmts[1], &env, false).await.unwrap();

    let content = std::fs::read_to_string(&out_file).unwrap();
    assert_eq!(content, "42\n");
    let _ = std::fs::remove_file(&out_file);

    // 2. >& redirection for both stdout and stderr
    let script2 = format!(
        "let list2 = [24]; $list2 >& \"{}\"",
        out_file.to_string_lossy().escape_default()
    );
    let mut parser2 = Parser::new(&script2);
    let stmts2 = parser2.parse_statements().unwrap();
    eval_stmt(&stmts2[0], &env, false).await.unwrap();
    eval_stmt(&stmts2[1], &env, false).await.unwrap();

    let content2 = std::fs::read_to_string(&out_file).unwrap();
    assert_eq!(content2, "24\n");
    let _ = std::fs::remove_file(&out_file);

    // 3. Stderr-only redirection 2> should not discard stdout
    let script3 = format!(
        "let list3 = [100, 200]; $list3 2> \"{}\"",
        out_file.to_string_lossy().escape_default()
    );
    let mut parser3 = Parser::new(&script3);
    let stmts3 = parser3.parse_statements().unwrap();
    eval_stmt(&stmts3[0], &env, false).await.unwrap();

    let pipeline = match stmts3[1].clone().into_unpack() {
        Stmt::Expr(expr) => match expr.into_unpack() {
            Expr::Pipeline(p) => p,
            other => panic!("Expected pipeline, got {:?}", other),
        },
        other => panic!("Expected Expr, got {:?}", other),
    };

    let collected = fshell_engine::collect_pipeline(&pipeline, &env)
        .await
        .unwrap();
    assert_eq!(collected.len(), 2);
    assert_eq!(collected[0], Val::Int(100));
    assert_eq!(collected[1], Val::Int(200));

    // Cleanup
    let _ = std::fs::remove_file(&out_file);
}

#[tokio::test]
async fn test_integration_legacy_function_delegation() {
    let env = setup_test_env();
    let temp_dir = std::env::temp_dir();
    let script_file = temp_dir.join("fshell_test_func.sh");
    let _ = std::fs::remove_file(&script_file);

    let content = r#"
my_fshell_test_func() {
    echo "val1"
    echo "val2"
}
"#;
    std::fs::write(&script_file, content).unwrap();

    {
        let mut caps = env.caps.caps.write();
        caps.grant(ResourceHandle::ReadFile(script_file.clone()));
        caps.grant(ResourceHandle::ProcessSpawn);
    }

    let source_script = format!(
        "source --bash \"{}\"",
        script_file.to_string_lossy().escape_default()
    );
    let mut parser = Parser::new(&source_script);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    let call_script = "my_fshell_test_func";
    let mut parser2 = Parser::new(call_script);
    let stmts2 = parser2.parse_statements().unwrap();
    let pipeline2 = match stmts2[0].clone().into_unpack() {
        Stmt::Expr(expr) => match expr.into_unpack() {
            Expr::Pipeline(p) => p,
            _ => panic!("Expected Expr::Pipeline"),
        },
        _ => panic!("Expected Stmt::Expr"),
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel(100);
    execute_pipeline(&pipeline2, &env, tx).await.unwrap();

    let mut results = Vec::new();
    while let Some(payload) = rx.recv().await {
        match payload {
            PipelinePayload::Data(v) => {
                let is_exit_token = if let Val::String(s) = &*v {
                    s.starts_with("\0exit:")
                } else {
                    false
                };
                if !is_exit_token {
                    match &*v {
                        Val::Blob(b) => {
                            let s = String::from_utf8_lossy(b);
                            for line in s.lines() {
                                if !line.is_empty() {
                                    results.push(Val::String(line.to_string()));
                                }
                            }
                        }
                        _ => {
                            results.push((*v).clone());
                        }
                    }
                }
            }
            PipelinePayload::Bytes(b) => {
                let s = String::from_utf8_lossy(&b).into_owned();
                for line in s.lines() {
                    if !line.is_empty() {
                        results.push(Val::String(line.to_string()));
                    }
                }
            }
            PipelinePayload::Structured(d) => panic!("Unexpected error: {:?}", d),
        }
    }

    assert_eq!(results.len(), 2);
    assert_eq!(results[0], Val::String("val1".to_string()));
    assert_eq!(results[1], Val::String("val2".to_string()));

    let _ = std::fs::remove_file(&script_file);
}

#[tokio::test]
async fn test_heredoc_integration() {
    let env = setup_test_env();
    let script = "let data = <<EOF\nhello world\nEOF";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let val = env.vars.read().get("data").cloned();
    assert_eq!(val, Some(Val::String("hello world".to_string())));
}

#[tokio::test]
async fn test_background_job_integration() {
    let env = setup_test_env();
    let script = "let x = 1\nlet x = 2 &\nwait";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in &stmts {
        eval_stmt(stmt, &env, false).await.unwrap();
    }
}

#[tokio::test]
async fn test_integration_command_binaries_override() {
    let env = setup_test_env();

    // Set an override for "fshell-test-echo" to point to "/bin/echo" or similar standard utility
    {
        let mut opts = env.options.write();
        opts.command_binaries
            .insert("fshell-test-echo".to_string(), "/bin/echo".to_string());
    }

    // Execute the command in the engine, which should invoke /bin/echo and print its arguments
    let mut parser = Parser::new("fshell-test-echo 'hello world'");
    let stmts = parser.parse_statements().unwrap();
    let pipeline = match stmts[0].clone().into_unpack() {
        Stmt::Expr(expr) => match expr.into_unpack() {
            Expr::Pipeline(p) => p,
            _ => panic!("Expected Expr::Pipeline"),
        },
        _ => panic!("Expected Stmt::Expr"),
    };

    let (tx, rx) = tokio::sync::mpsc::channel(32);

    // We execute in background thread because it spawns blocking process spawn
    let handle = tokio::spawn(async move {
        let mut out_rx = rx;
        let mut output = String::new();
        while let Some(payload) = out_rx.recv().await {
            if let PipelinePayload::Data(val) = payload {
                output.push_str(&val.to_text());
            }
        }
        output
    });

    fshell_engine::execute_pipeline(&pipeline, &env, tx)
        .await
        .unwrap();
    let output = handle.await.unwrap();
    assert!(
        output.contains("hello world"),
        "Expected output to contain 'hello world', got: {}",
        output
    );
}

#[tokio::test]
async fn test_mock_installer_pipeline() {
    let env = setup_test_env();

    // Mimic curl emitting 500 lines of script, piped into sh where sh executes the script,
    // prints banner/prompt text, and exits cleanly.
    let script = r#"sh -c 'for i in $(seq 1 500); do echo "echo Line $i"; done; echo "echo Pi Installer Mock"; echo "echo Install Node? [Y/n]"' | sh"#;

    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();

    let pipeline = match stmts[0].clone().into_unpack() {
        Stmt::Expr(expr) => match expr.into_unpack() {
            Expr::Pipeline(pipe) => pipe,
            _ => panic!("Expected Expr::Pipeline"),
        },
        _ => panic!("Expected Stmt::Expr"),
    };

    let (tx, rx) = tokio::sync::mpsc::channel(64);

    let handle = tokio::spawn(async move {
        let mut out_rx = rx;
        let mut count = 0;
        let mut saw_banner = false;
        let mut saw_prompt = false;

        while let Some(payload) = out_rx.recv().await {
            let text = match payload {
                PipelinePayload::Data(val) => val.to_text(),
                PipelinePayload::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
                PipelinePayload::Structured(_) => continue,
            };
            if text.contains("Line ") {
                // Count lines that actually contain "Line N"
                count += text.matches("Line ").count();
                if count > 500 {
                    count = 500;
                }
            }
            if text.contains("Pi Installer Mock") {
                saw_banner = true;
            }
            if text.contains("Install Node? [Y/n]") {
                saw_prompt = true;
            }
        }
        (count, saw_banner, saw_prompt)
    });

    fshell_engine::execute_pipeline(&pipeline, &env, tx)
        .await
        .unwrap();

    let (count, saw_banner, saw_prompt) = handle.await.unwrap();

    assert_eq!(
        count, 500,
        "Expected 500 script lines processed through pipeline"
    );
    assert!(saw_banner, "Expected installer banner output");
    assert!(saw_prompt, "Expected installer prompt output");
}

#[test]
fn test_cli_mock_installer_pipeline() {
    let output = FshCmd::new()
        .cmd("sh -c 'for i in $(seq 1 100); do echo \"echo Line $i\"; done; echo \"echo Pi Installer Mock\"; echo \"echo Install Node? [Y/n]\"' | sh")
        .run()
        .expect("Failed to execute fsh CLI");

    output
        .assert_success()
        .assert_stdout_contains("Pi Installer Mock")
        .assert_stdout_contains("Install Node? [Y/n]")
        .assert_stdout_not_contains("pipeline channel full");
}

#[tokio::test]
async fn test_integration_input_redirection() {
    let env = setup_test_env();
    let temp_dir = tempfile::tempdir().unwrap();
    let in_file = temp_dir.path().join("input.txt");
    let out_file = temp_dir.path().join("output.txt");

    std::fs::write(&in_file, "hello input redirection\nline 2\n").unwrap();

    let script = format!(
        "cat < {} > {}",
        in_file.to_string_lossy(),
        out_file.to_string_lossy()
    );
    let mut parser = fshell_core::Parser::new(&script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in stmts {
        fshell_engine::eval_stmt(&stmt, &env, false).await.unwrap();
    }

    // Allow tokio spawned tasks to finish writing
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let out_content = std::fs::read_to_string(&out_file).unwrap();
    assert_eq!(out_content, "hello input redirection\nline 2\n");
}

#[tokio::test]
async fn test_integration_standalone_input_redirection() {
    let env = setup_test_env();
    let temp_dir = tempfile::tempdir().unwrap();
    let in_file = temp_dir.path().join("standalone.txt");
    let out_file = temp_dir.path().join("standalone_out.txt");

    std::fs::write(&in_file, "line A\nline B\n").unwrap();

    let script = format!(
        "< {} > {}",
        in_file.to_string_lossy(),
        out_file.to_string_lossy()
    );
    let mut parser = fshell_core::Parser::new(&script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in stmts {
        fshell_engine::eval_stmt(&stmt, &env, false).await.unwrap();
    }

    // Allow tokio spawned tasks to finish writing
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let out_content = std::fs::read_to_string(&out_file).unwrap();
    assert_eq!(out_content, "line A\nline B\n");
}

#[tokio::test]
async fn test_integration_unset_propagation_to_child_process() {
    let env = setup_test_env();

    // Set an initial variable in host environment
    unsafe {
        std::env::set_var("FSH_TEST_UNSET_VAR", "present");
    }

    // Evaluate unset in fshell
    let script = "unset FSH_TEST_UNSET_VAR";
    let mut parser = fshell_core::Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in stmts {
        fshell_engine::eval_stmt(&stmt, &env, false).await.unwrap();
    }

    // Verify in env map
    let vars = env.vars.read();
    if let Some(Val::Map(m)) = vars.get("env") {
        assert!(m.get(&ustr::ustr("FSH_TEST_UNSET_VAR")).is_none());
    }

    // Clean up host env
    unsafe {
        std::env::remove_var("FSH_TEST_UNSET_VAR");
    }
}

#[tokio::test]
async fn test_integration_cmd_input_redirection() {
    let env = setup_test_env();
    let temp_dir = std::env::temp_dir();
    let in_file = temp_dir.join("fsh_test_cmd_in.txt");
    let out_file = temp_dir.join("fsh_test_cmd_out.txt");
    std::fs::write(&in_file, "apple\nbanana\ncherry\n").unwrap();

    let mut caps = env.caps.caps.write();
    caps.grant(ResourceHandle::ReadFile(in_file.clone()));
    caps.grant(ResourceHandle::WriteFile(out_file.clone()));
    caps.grant(ResourceHandle::ReadFile(out_file.clone()));
    drop(caps);

    let script = format!(
        "cat < \"{}\" > \"{}\"",
        in_file.to_string_lossy().escape_default(),
        out_file.to_string_lossy().escape_default()
    );
    let mut parser = fshell_core::Parser::new(&script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in stmts {
        fshell_engine::eval_stmt(&stmt, &env, false).await.unwrap();
    }

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let out_content = std::fs::read_to_string(&out_file).unwrap();
    assert_eq!(out_content, "apple\nbanana\ncherry\n");
    let _ = std::fs::remove_file(&out_file);

    let script2 = format!(
        "grep banana < \"{}\" > \"{}\"",
        in_file.to_string_lossy().escape_default(),
        out_file.to_string_lossy().escape_default()
    );
    let mut parser2 = fshell_core::Parser::new(&script2);
    let stmts2 = parser2.parse_statements().unwrap();
    for stmt in stmts2 {
        fshell_engine::eval_stmt(&stmt, &env, false).await.unwrap();
    }

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let out_content2 = std::fs::read_to_string(&out_file).unwrap();
    assert_eq!(out_content2, "banana\n");

    let _ = std::fs::remove_file(in_file);
    let _ = std::fs::remove_file(out_file);
}

#[tokio::test]
async fn test_integration_export_syntax_variants() {
    let env = setup_test_env();

    // 1. export KEY="VALUE"
    let script1 = "export TEST_EXP_A=\"hello world\"";
    let mut p1 = fshell_core::Parser::new(script1);
    for stmt in p1.parse_statements().unwrap() {
        fshell_engine::eval_stmt(&stmt, &env, false).await.unwrap();
    }

    // 2. export KEY=VALUE
    let script2 = "export TEST_EXP_B=foobar";
    let mut p2 = fshell_core::Parser::new(script2);
    for stmt in p2.parse_statements().unwrap() {
        fshell_engine::eval_stmt(&stmt, &env, false).await.unwrap();
    }

    // 3. export KEY = "VALUE"
    let script3 = "export TEST_EXP_C = \"spaced\"";
    let mut p3 = fshell_core::Parser::new(script3);
    for stmt in p3.parse_statements().unwrap() {
        fshell_engine::eval_stmt(&stmt, &env, false).await.unwrap();
    }

    let vars = env.vars.read();
    assert_eq!(
        vars.get("TEST_EXP_A"),
        Some(&Val::String("hello world".to_string()))
    );
    assert_eq!(
        vars.get("TEST_EXP_B"),
        Some(&Val::String("foobar".to_string()))
    );
    assert_eq!(
        vars.get("TEST_EXP_C"),
        Some(&Val::String("spaced".to_string()))
    );
}

#[tokio::test]
async fn test_integration_process_substitution() {
    let env = setup_test_env();
    let script = "let p = <(echo hello)";
    let mut parser = fshell_core::Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    fshell_engine::eval_stmt(&stmts[0], &env, false)
        .await
        .unwrap();

    let path_str = {
        let vars = env.vars.read();
        match vars.get("p") {
            Some(Val::String(s)) => s.clone(),
            other => panic!("Expected Val::String, got {:?}", other),
        }
    };

    // The file should exist and contain "hello\n"
    assert!(
        std::path::Path::new(&path_str).exists(),
        "Process substitution temp file must exist on disk"
    );
    let content = std::fs::read_to_string(&path_str).unwrap();
    assert_eq!(content.trim(), "hello");

    // Cleaning temp files unlinks it
    env.clear_temp_files();
    assert!(
        !std::path::Path::new(&path_str).exists(),
        "Process substitution temp file should be cleaned up"
    );
}

#[tokio::test]
async fn test_integration_tilde_expansion_in_command_arguments() {
    let env = setup_test_env();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());

    let script = "let res = $| echo ~/test_tilde_path |";
    let mut parser = fshell_core::Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    fshell_engine::eval_stmt(&stmts[0], &env, false)
        .await
        .unwrap();

    let vars = env.vars.read();
    let res = vars.get("res").unwrap();
    if let Val::List(items) = res {
        assert_eq!(items.len(), 1);
        let expected = format!("{}/test_tilde_path", home);
        assert_eq!(items[0], Val::String(expected));
    } else {
        panic!("Expected Val::List, got {:?}", res);
    }
}

#[tokio::test]
async fn test_integration_set_flags_and_positional_params() {
    let env = setup_test_env();

    // 1. Test set -e and set +e
    let script1 = "set -e";
    let mut parser = fshell_core::Parser::new(script1);
    let stmts = parser.parse_statements().unwrap();
    fshell_engine::eval_stmt(&stmts[0], &env, false)
        .await
        .unwrap();
    assert!(
        env.options.read().errexit,
        "errexit should be true after set -e"
    );

    let script2 = "set +e";
    let mut parser = fshell_core::Parser::new(script2);
    let stmts = parser.parse_statements().unwrap();
    fshell_engine::eval_stmt(&stmts[0], &env, false)
        .await
        .unwrap();
    assert!(
        !env.options.read().errexit,
        "errexit should be false after set +e"
    );

    // 2. Test set -o pipefail and set +o pipefail
    let script3 = "set -o pipefail";
    let mut parser = fshell_core::Parser::new(script3);
    let stmts = parser.parse_statements().unwrap();
    fshell_engine::eval_stmt(&stmts[0], &env, false)
        .await
        .unwrap();
    assert!(
        env.options.read().pipefail,
        "pipefail should be true after set -o pipefail"
    );

    // 3. Test set -- foo bar baz
    let script4 = "set -- foo bar baz";
    let mut parser = fshell_core::Parser::new(script4);
    let stmts = parser.parse_statements().unwrap();
    fshell_engine::eval_stmt(&stmts[0], &env, false)
        .await
        .unwrap();

    let vars = env.vars.read();
    assert_eq!(vars.get("1"), Some(&Val::String("foo".to_string())));
    assert_eq!(vars.get("2"), Some(&Val::String("bar".to_string())));
    assert_eq!(vars.get("3"), Some(&Val::String("baz".to_string())));
    assert_eq!(vars.get("#"), Some(&Val::Int(3)));
}

#[tokio::test]
async fn test_integration_alias_syntax_variants() {
    let env = setup_test_env();

    // 1. POSIX quoted syntax: alias ll="ls -la"
    let script1 = "alias ll=\"ls -la\"";
    let mut parser = fshell_core::Parser::new(script1);
    let stmts = parser.parse_statements().unwrap();
    fshell_engine::eval_stmt(&stmts[0], &env, false)
        .await
        .unwrap();
    assert_eq!(env.get_alias("ll"), Some("ls -la".to_string()));

    // 2. Space separated syntax: alias gs git status
    let script2 = "alias gs git status";
    let mut parser = fshell_core::Parser::new(script2);
    let stmts = parser.parse_statements().unwrap();
    fshell_engine::eval_stmt(&stmts[0], &env, false)
        .await
        .unwrap();
    assert_eq!(env.get_alias("gs"), Some("git status".to_string()));

    // 3. Query alias: alias ll
    let script3 = "let q = $| alias ll |";
    let mut parser = fshell_core::Parser::new(script3);
    let stmts = parser.parse_statements().unwrap();
    fshell_engine::eval_stmt(&stmts[0], &env, false)
        .await
        .unwrap();
    let vars = env.vars.read();
    let q = vars.get("q").unwrap();
    if let Val::List(items) = q {
        assert_eq!(items.len(), 1);
        assert_eq!(items[0], Val::String("alias ll = \"ls -la\"".to_string()));
    } else {
        panic!("Expected Val::List, got {:?}", q);
    }
}

#[tokio::test]
async fn test_integration_inline_posix_block() {
    let env = setup_test_env();

    let script = r#"
let prefix = "hello"
sh {
    export GREETING="${prefix}_world"
    COUNT=$((5 + 5))
}
"#;
    let mut parser = fshell_core::Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 2);

    for stmt in &stmts {
        fshell_engine::eval_stmt(stmt, &env, false).await.unwrap();
    }

    let vars = env.vars.read();
    assert_eq!(
        vars.get("GREETING"),
        Some(&Val::String("hello_world".to_string()))
    );
    assert_eq!(
        vars.get("COUNT"),
        Some(&Val::String("10".to_string()))
            .or_else(|| vars.get("COUNT").filter(|v| matches!(v, Val::Int(10))))
    );
}

#[tokio::test]
async fn test_integration_inline_posix_control_flow() {
    let env = setup_test_env();

    let script = r#"
posix {
    val=0
    for i in 1 2 3 4; do
        val=$((val + i))
    done
}
"#;
    let mut parser = fshell_core::Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in &stmts {
        fshell_engine::eval_stmt(stmt, &env, false).await.unwrap();
    }

    let vars = env.vars.read();
    assert_eq!(
        vars.get("val"),
        Some(&Val::String("10".to_string()))
            .or_else(|| vars.get("val").filter(|v| matches!(v, Val::Int(10))))
    );
}

#[tokio::test]
async fn test_process_substitution_input_and_output() {
    let env = setup_test_env();

    // 1. Input process substitution <(echo "hello fshell")
    let code = r#"
        let p = <(echo "hello substitution")
        cat $p
    "#;
    let res = fshell_engine::run_script(code, &env).await;
    assert!(res.is_ok());

    // 2. Output process substitution >(cat)
    let code_out = r#"
        let p = >(cat)
        echo "output subst" > $p
    "#;
    let res_out = fshell_engine::run_script(code_out, &env).await;
    assert!(res_out.is_ok());
}

#[tokio::test]
async fn test_native_cli_integrations() {
    let env = setup_test_env();
    let (tx, _rx) = tokio::sync::mpsc::channel(100);

    // 1. Direnv init
    fshell_builtins::direnv_init_builtin(None, vec![], &env, tx.clone(), None).unwrap();
    assert!(fshell_engine::get_hooks("chpwd", &env).contains(&"eval_direnv".to_string()));
    assert!(fshell_engine::get_hooks("precmd", &env).contains(&"eval_direnv".to_string()));

    // 2. Zoxide init
    fshell_builtins::zoxide_init_builtin(None, vec![], &env, tx.clone(), None).unwrap();
    assert!(fshell_engine::get_hooks("chpwd", &env).contains(&"zoxide_hook".to_string()));

    // 3. Starship init
    fshell_builtins::starship_init_builtin(None, vec![], &env, tx.clone(), None).unwrap();
    assert!(fshell_engine::get_hooks("precmd", &env).contains(&"starship_precmd".to_string()));
    assert!(fshell_engine::get_hooks("preexec", &env).contains(&"starship_preexec".to_string()));

    // 4. FZF init
    fshell_builtins::fzf_init_builtin(None, vec![], &env, tx.clone(), None).unwrap();
    let chord = fshell_engine::keybindings::KeyChord::parse("ctrl-t").unwrap();
    assert_eq!(
        env.keybindings
            .read()
            .get_action(fshell_engine::keybindings::KeyMapMode::Emacs, &chord),
        Some(&fshell_engine::keybindings::KeyAction::Widget(
            "fzf-file-widget".to_string()
        ))
    );
}
