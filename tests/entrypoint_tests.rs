mod common;

use common::*;
use std::process::Command;

#[test]
fn empty_inline_command_exits_successfully() {
    let output = FshCmd::new()
        .arg("--command")
        .arg(" \n\t ")
        .run()
        .expect("empty command invocation should start");

    output.assert_exit_code(0);
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn missing_inline_command_is_a_cli_error() {
    let output = FshCmd::new()
        .arg("-c")
        .run()
        .expect("missing command invocation should start");

    output.assert_exit_code(2);
    output.assert_stderr_contains("COMMAND");
    assert!(!output.stderr.contains("panicked"));
}

#[test]
fn command_like_script_arguments_do_not_replace_the_script() {
    let cmd = FshCmd::new();
    let script = cmd
        .create_file("exit.fsh", "exit 7\n")
        .expect("script fixture should be written");
    let output = cmd
        .arg(script)
        .arg("--")
        .arg("-c")
        .arg(" ")
        .run()
        .expect("script invocation should start");

    output.assert_exit_code(7);
}

#[test]
fn non_login_script_sees_fsh_login_false() {
    let cmd = FshCmd::new();
    let script = cmd
        .create_file("login-state.fsh", "echo $FSH_LOGIN\n")
        .expect("script fixture should be written");
    let output = cmd
        .arg(script)
        .run()
        .expect("script invocation should start");

    output.assert_success().assert_stdout_trimmed_eq("false");
}

#[test]
fn fshell_multicall_name_runs_the_shell() {
    let (cmd, _symlink) = FshCmd::multicall("fshell");
    let output = cmd
        .cmd("echo fshell-alias")
        .run()
        .expect("fshell alias invocation should start");

    output
        .assert_success()
        .assert_stdout_trimmed_eq("fshell-alias");
}

#[test]
fn trace_records_startup_spans_with_their_finished_outcomes() {
    let cmd = FshCmd::new();
    let trace_dir = tempfile::tempdir().expect("trace directory should be created");
    let trace_path = trace_dir.path().join("trace.jsonl");
    let trace_path = trace_path.to_string_lossy().into_owned();
    let output = cmd
        .env("FSH_TRACE_FILE", trace_path.clone())
        .cmd("echo traced")
        .run()
        .expect("traced command should start");

    output.assert_success().assert_stdout_trimmed_eq("traced");
    let trace = std::fs::read_to_string(std::path::Path::new(&trace_path))
        .expect("trace file should be written");
    let records: Vec<serde_json::Value> = trace
        .lines()
        .map(|line| serde_json::from_str(line).expect("trace record should be valid JSON"))
        .collect();

    for name in ["cli.parse", "startup.core_init"] {
        let record = records
            .iter()
            .find(|record| record["name"] == name)
            .expect("trace should contain the expected startup span");
        assert_eq!(record["outcome"], "ok", "span {name} should finish cleanly");
    }
}

#[test]
fn standalone_ls_preserves_explicit_color_on_piped_output() {
    let (cmd, _symlink) = FshCmd::multicall("ls");
    let listing = cmd.temp_path().join("color-listing");
    std::fs::create_dir_all(listing.join("subdirectory"))
        .expect("listing fixture should be created");
    let output = cmd
        .arg("--color=always")
        .arg(&listing)
        .run()
        .expect("multicall ls should start");

    output
        .assert_success()
        .assert_stdout_contains("subdirectory");
    assert!(
        output.stdout.contains("\x1b["),
        "--color=always should preserve the renderer's ANSI output"
    );
}

#[tokio::test]
async fn public_run_entrypoint_child_body() {
    if std::env::var_os("FSH_RUN_ENTRYPOINT_CHILD_BODY").is_some() {
        fshell::run().await;
    }
}

#[test]
fn public_run_entrypoint_delegates_to_the_shared_router() {
    let output = Command::new(std::env::current_exe().expect("test executable path should exist"))
        .args(["--exact", "public_run_entrypoint_child_body", "--nocapture"])
        .env("FSH_RUN_ENTRYPOINT_CHILD_BODY", "1")
        .output()
        .expect("run entrypoint child should start");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not available as a standalone utility"),
        "run() should delegate into the utility route; stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn setup_panic_hook_child_body() {
    if std::env::var_os("FSH_PANIC_HOOK_CHILD").is_some() {
        fshell::setup_panic_hook();
        std::panic::panic_any("panic-hook-sentinel");
    }
}

#[test]
fn setup_panic_hook_reports_a_panic_in_a_child_process() {
    let output = Command::new(std::env::current_exe().expect("test executable path should exist"))
        .args(["--exact", "setup_panic_hook_child_body", "--nocapture"])
        .env("FSH_PANIC_HOOK_CHILD", "1")
        .env_remove("RUST_BACKTRACE")
        .output()
        .expect("panic hook child should start");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fshell encountered an internal panic"),
        "{stderr}"
    );
    assert!(stderr.contains("panic-hook-sentinel"), "{stderr}");
}
