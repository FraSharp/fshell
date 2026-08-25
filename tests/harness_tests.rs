mod common;
use common::*;

#[tokio::test]
async fn test_harness_test_context_basic() {
    let ctx = TestContext::new();
    let val = ctx.eval_ok("20 * 3 + 4").await;
    assert_val_int!(val, 64);
}

#[tokio::test]
async fn test_harness_test_context_variables() {
    let ctx = TestContext::new();
    ctx.eval_script("let greeting = 'hello world'")
        .await
        .unwrap();
    let val = ctx.get_var("greeting");
    assert_eq!(val, Some(Val::String("hello world".into())));
}

#[tokio::test]
async fn test_harness_test_context_temp_file() {
    let ctx = TestContext::new();
    let file_path = ctx
        .create_file("test_data/input.txt", "sample content")
        .unwrap();
    assert!(file_path.exists());
    let read_back = std::fs::read_to_string(&file_path).unwrap();
    assert_eq!(read_back, "sample content");
}

#[tokio::test]
async fn test_harness_cwd_guard_restoration() {
    let orig_cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
    {
        let guard = CwdGuard::new_temp();
        let cur_in_guard = std::env::current_dir().unwrap().canonicalize().unwrap();
        assert_ne!(cur_in_guard, orig_cwd);
        assert_eq!(cur_in_guard, guard.path());
    }
    // After dropping the guard, original cwd is guaranteed to be restored
    let cur_after_guard = std::env::current_dir().unwrap().canonicalize().unwrap();
    assert_eq!(cur_after_guard, orig_cwd);
}

#[tokio::test]
async fn test_harness_env_var_guard_restoration() {
    let test_key = "FSHELL_TEST_HARNESS_VAR_XYZ";
    fshell_core::remove_var(test_key);
    assert!(std::env::var(test_key).is_err());

    {
        let mut guard = EnvVarGuard::new();
        guard.set(test_key, "temporary_val");
        assert_eq!(std::env::var(test_key).unwrap(), "temporary_val");
    }

    // After dropping the guard, variable is restored to previous unset state
    assert!(std::env::var(test_key).is_err());
}

#[tokio::test]
async fn test_harness_pipeline_collection() {
    let ctx = TestContext::new();
    ctx.set_var("items", make_file_items());
    let filtered = ctx
        .collect_pipeline("$items | filter size > 100")
        .await
        .unwrap();
    assert_eq!(filtered.len(), 2);
}

#[test]
fn test_harness_subprocess_eval_inline() {
    let output = FshCmd::new()
        .cmd("echo 'subprocess harness test' | count")
        .run()
        .expect("subprocess execution failed");

    output.assert_success();
    output.assert_stdout_contains("1");
}

#[test]
fn test_harness_subprocess_tempfile_and_stdin() {
    let cmd = FshCmd::new();
    let _path = cmd
        .create_file("demo.txt", "line1\nline2\nline3\n")
        .unwrap();
    let output = cmd
        .cmd("cat demo.txt | count")
        .run()
        .expect("subprocess execution failed");

    output.assert_success();
    output.assert_stdout_contains("3");
}

#[test]
fn test_harness_subprocess_multicall_ls() {
    let (cmd, _symlink) = FshCmd::multicall("ls");
    let output = cmd.run().expect("multicall execution failed");
    output.assert_success();
}
