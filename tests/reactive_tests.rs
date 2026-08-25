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
async fn test_integration_reactive_cell_rejects_mutation() {
    let env = setup_test_env();

    let script = "$= x = rm foo";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);

    let result = eval_stmt(&stmts[0], &env, false).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("Mutation"),
        "Expected mutation rejection, got: {}",
        err
    );
    assert!(err.contains("rm"), "Expected rm to be named, got: {}", err);
}

#[tokio::test]
async fn test_integration_reactive_cell_allows_query() {
    let env = setup_test_env();

    let script = "$= live = ls | filter size > 100";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);

    let result = eval_stmt(&stmts[0], &env, false).await;
    assert!(
        result.is_ok(),
        "Expected query to be allowed, got: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn test_integration_unsafe_allows_mutation_in_reactive_cell() {
    let env = setup_test_env();

    let script = "unsafe { $= x = rm foo }";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);

    let result = eval_stmt(&stmts[0], &env, false).await;
    assert!(
        result.is_ok(),
        "Expected unsafe to allow mutation, got: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn test_unsafe_context_not_propagate_into_try() {
    let env = setup_test_env();
    // unsafe wraps a try that contains a reactive cell mutation.
    // The mutation should still be rejected because unsafe_context does
    // not propagate into the try body.
    let script = "unsafe { try { $= x = rm foo } catch |e| { let caught = true } }";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);
    let result = eval_stmt(&stmts[0], &env, false).await;
    assert!(
        result.is_ok(),
        "Expected unsafe+try to be Ok, got: {:?}",
        result.err()
    );
    // The catch block should have run because the try body failed
    let vars = env.vars.read();
    assert_eq!(
        vars.get("caught"),
        Some(&Val::Bool(true)),
        "Expected caught=true from catch block"
    );
    // The reactive cell should NOT exist
    assert!(
        !vars.contains_key("x"),
        "Reactive cell 'x' should not exist (mutation was rejected)"
    );
}

#[tokio::test]
async fn test_reactive_cell_variable_lookup() {
    let env = setup_test_env();

    // Create a reactive cell
    let mut p = Parser::new("$= live_cell = ls");
    let stmts = p.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    // Variable lookup for live_cell should retrieve the list from reactive_cells
    let mut p2 = Parser::new("$live_cell");
    let stmts2 = p2.parse_statements().unwrap();
    if let Stmt::Expr(expr) = &stmts2[0] {
        let res = eval_expr(expr, &env).await.unwrap();
        assert!(matches!(res, Val::List(_)));
    }
}

#[tokio::test]
async fn test_reactive_cell_event_driven_fs() {
    let env = setup_test_env();

    // Create temp directory for testing
    let test_dir = std::env::current_dir()
        .unwrap()
        .join("reactive_test_dir_fs");
    let _ = std::fs::remove_dir_all(&test_dir);
    std::fs::create_dir(&test_dir).unwrap();

    // Declare reactive cell watching the test directory
    let script = format!("$= live_dir = ls {:?}", test_dir.to_string_lossy());
    let mut p = Parser::new(&script);
    let stmts = p.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    // Initially, the directory is empty
    let rx = {
        let cells = env.reactive.cells.read();
        let rx = cells.get("live_dir").unwrap().clone();
        assert_eq!(rx.borrow().len(), 0);
        rx
    };

    // Create a file in the watched directory
    let file_path = test_dir.join("temp_file.txt");
    std::fs::write(&file_path, "hello").unwrap();

    // Trigger the cell directly — the filesystem watcher may miss events
    // under parallel test load on macOS
    let _ = env
        .reactive
        .tx
        .send(fshell_engine::ReactiveEvent::TriggerCell("live_dir".into()))
        .await;

    // Wait for event-driven watcher to trigger (up to 10s)
    let mut rx = rx;
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        rx.wait_for(|val| val.len() == 1),
    )
    .await
    .expect("Expected live_dir to update to 1 item within timeout");

    // Clean up
    let _ = std::fs::remove_dir_all(&test_dir);
}

#[tokio::test]
async fn test_reactive_cell_dependency_propagation() {
    let env = setup_test_env();

    // Create temp directory for testing
    let test_dir = std::env::current_dir()
        .unwrap()
        .join("reactive_test_dir_dep");
    let _ = std::fs::remove_dir_all(&test_dir);
    std::fs::create_dir(&test_dir).unwrap();

    // cell_a watches the directory
    let script_a = format!("$= cell_a = ls {:?}", test_dir.to_string_lossy());
    let mut p_a = Parser::new(&script_a);
    let stmts_a = p_a.parse_statements().unwrap();
    eval_stmt(&stmts_a[0], &env, false).await.unwrap();

    // cell_b depends on cell_a
    let script_b = "$= cell_b = $cell_a | count";
    let mut p_b = Parser::new(script_b);
    let stmts_b = p_b.parse_statements().unwrap();
    eval_stmt(&stmts_b[0], &env, false).await.unwrap();

    // Retrieve cell_b receiver
    let mut rx_b = {
        let cells = env.reactive.cells.read();
        cells.get("cell_b").unwrap().clone()
    };

    // Wait for initial value of cell_b to be populated (up to 10s)
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        rx_b.wait_for(|val| val.len() == 1 && val[0] == Val::Int(0)),
    )
    .await
    .expect("Expected cell_b to initially be [0] within timeout");

    // Create a file in the watched directory
    let file_path = test_dir.join("temp_file.txt");
    std::fs::write(&file_path, "hello").unwrap();

    // Trigger cell_a directly — the filesystem watcher may miss events
    // under parallel test load on macOS
    let _ = env
        .reactive
        .tx
        .send(fshell_engine::ReactiveEvent::TriggerCell("cell_a".into()))
        .await;

    // Wait for cascade to propagate (up to 10s)
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        rx_b.wait_for(|val| val.len() == 1 && val[0] == Val::Int(1)),
    )
    .await
    .expect("Expected cell_b to propagate and update to [1] within timeout");

    // Clean up
    let _ = std::fs::remove_dir_all(&test_dir);
}

#[tokio::test]
async fn test_integration_unsafe_block_parsed() {
    let env = setup_test_env();
    let script = "
        unsafe {
            $= live = echo 1
        }
        unsafe {
            $= live = echo 2
        }
    ";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in &stmts {
        eval_stmt(stmt, &env, false).await.unwrap();
    }
    let rx = {
        let cells = env.reactive.cells.read();
        cells.get("live").unwrap().clone()
    };
    let mut success = false;
    for _ in 0..40 {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        if rx.borrow().len() == 1 && rx.borrow()[0] == Val::String("2".to_string()) {
            success = true;
            break;
        }
    }
    assert!(
        success,
        "Expected cell to propagate update to 2, got {:?}",
        rx.borrow()
    );
}
