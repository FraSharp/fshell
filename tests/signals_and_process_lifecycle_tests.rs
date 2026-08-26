mod common;
use common::*;
use fshell_core::Val;
use fshell_engine::{run_script, Signal};
use std::sync::atomic::Ordering;

// ---------------------------------------------------------------------------
// 1. Broken Pipe & Early Exit Pipeline Resilience
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pipeline_broken_pipe_limit_terminates_producer() {
    let env = setup_test_env();
    // A loop or stream that generates many elements, piped into limit
    let script = r#"
let items = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]
let taken = ($items | limit 3)
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(
        vars.get("taken"),
        Some(&Val::List(vec![Val::Int(1), Val::Int(2), Val::Int(3)]))
    );
}

#[tokio::test]
async fn test_pipeline_broken_pipe_filter_and_limit() {
    let env = setup_test_env();
    let script = r#"
let data = [
    { id: 1, active: true },
    { id: 2, active: false },
    { id: 3, active: true },
    { id: 4, active: true },
    { id: 5, active: false },
    { id: 6, active: true }
]
let first_two_active = ($data | filter active == true | limit 2)
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    if let Some(Val::List(items)) = vars.get("first_two_active") {
        assert_eq!(items.len(), 2);
    } else {
        panic!("Expected list result");
    }
}

#[tokio::test]
async fn test_external_process_broken_pipe_clean_exit() {
    // Verifies that external process writing to head terminates cleanly
    let output = FshCmd::new()
        .arg("-c")
        .arg("sh { yes 'fshell_stream_data' | head -n 3; }")
        .run()
        .unwrap();

    output.assert_success();
    let lines: Vec<&str> = output.stdout.lines().collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], "fshell_stream_data");
}

// ---------------------------------------------------------------------------
// 2. SIGINT / Cancellation Handling
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_sigint_pending_aborts_infinite_while_loop() {
    let env = setup_test_env();

    // Set sigint_pending flag
    env.job_control.sigint_pending.store(true, Ordering::SeqCst);

    let script = r#"
let counter = 0
while $counter < 1000000 {
    counter = ($counter + 1)
}
"#;
    let res = run_script(script, &env).await;
    assert!(res.is_err());
    let err_msg = res.unwrap_err().to_string();
    assert!(err_msg.contains("Interrupted") || err_msg.contains("Ctrl+C"));
}

#[tokio::test]
async fn test_sigint_pending_aborts_for_loop() {
    let env = setup_test_env();

    env.job_control.sigint_pending.store(true, Ordering::SeqCst);

    let script = r#"
let items = [1, 2, 3, 4, 5]
let collected = 0
for item in $items {
    collected = ($collected + $item)
}
"#;
    let res = run_script(script, &env).await;
    assert!(res.is_err());
}

// ---------------------------------------------------------------------------
// 3. Signal Traps (`trap`)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_posix_trap_set_and_query() {
    let env = setup_test_env();
    let script = r#"
sh {
    trap "echo CLEANUP_CALLED" INT
    trap "echo EXIT_CALLED" EXIT
}
"#;
    run_script(script, &env).await.unwrap();

    let traps = env.posix_traps.read();
    assert_eq!(traps.get(&Signal::Int), Some(&"echo CLEANUP_CALLED".to_string()));
    assert_eq!(traps.get(&Signal::Exit), Some(&"echo EXIT_CALLED".to_string()));
}

#[tokio::test]
async fn test_posix_trap_ignore_and_reset() {
    let env = setup_test_env();
    let script = r#"
sh {
    trap '' HUP
    trap - TERM
}
"#;
    run_script(script, &env).await.unwrap();

    let traps = env.posix_traps.read();
    assert_eq!(traps.get(&Signal::Hup), Some(&"".to_string()));
    assert!(traps.get(&Signal::Term).is_none());
}

#[tokio::test]
async fn test_posix_trap_subshell_isolation() {
    let env = setup_test_env();
    let script = r#"
sh {
    trap "echo TOP_INT" INT
    (
        trap "echo SUBSHELL_INT" INT
    )
}
"#;
    run_script(script, &env).await.unwrap();

    let traps = env.posix_traps.read();
    assert_eq!(traps.get(&Signal::Int), Some(&"echo TOP_INT".to_string()));
}

// ---------------------------------------------------------------------------
// 4. Process Status, Pipefail & Multiple Pipeline Stages
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pipefail_option_reports_first_failure() {
    let env = setup_test_env();
    let script = r#"
setopt pipefail
sh {
    false | true
}
"#;
    let res = run_script(script, &env).await;
    // With pipefail, false | true should yield exit code 1
    assert!(res.is_err() || *env.prompt.last_exit_code.read() != 0);
}

#[tokio::test]
async fn test_no_pipefail_reports_last_stage_exit_code() {
    let env = setup_test_env();
    let script = r#"
unsetopt pipefail
sh {
    false | true
}
"#;
    run_script(script, &env).await.unwrap();
    assert_eq!(*env.prompt.last_exit_code.read(), 0);
}
