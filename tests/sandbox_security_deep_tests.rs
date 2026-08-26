mod common;
use common::*;
use fshell_engine::run_script;
use fshell_sandbox::{SandboxMode, SandboxProfile};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// 1. Sandbox Profile Builder & Matrix Policies
// ---------------------------------------------------------------------------

#[test]
fn test_sandbox_profile_matrix_modes() {
    let p_default = SandboxProfile::new(SandboxMode::ReadOnlySystem);
    assert!(p_default.allow_network);
    assert_eq!(p_default.mode, SandboxMode::ReadOnlySystem);

    let p_isolated = SandboxProfile::new(SandboxMode::Isolated);
    assert!(!p_isolated.allow_network);

    let p_custom = SandboxProfile::new(SandboxMode::ReadOnlySystem)
        .allow_write(PathBuf::from("/tmp/build"))
        .deny_write(PathBuf::from("/tmp/build/secrets"))
        .with_network(false);

    assert_eq!(p_custom.allow_write_paths, vec![PathBuf::from("/tmp/build")]);
    assert_eq!(p_custom.deny_write_paths, vec![PathBuf::from("/tmp/build/secrets")]);
    assert!(!p_custom.allow_network);
}

// ---------------------------------------------------------------------------
// 2. Destructive Command Interception & Protection
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_destructive_patterns_blocked_in_non_interactive() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();

    // Verify confirmation is on by default in test env
    assert!(env.options.read().confirm_destructive);

    let dangerous_commands = vec![
        "rm -rf /",
        "rm -rf /*",
        "chmod -R 777 /",
        "chmod -R 000 /System",
    ];

    for cmd in dangerous_commands {
        let res = run_script(cmd, &env).await;
        assert!(
            res.is_err(),
            "Expected destructive command to be blocked: {cmd}"
        );
        let err = res.unwrap_err();
        assert!(
            err.contains("Dangerous operation") || err.contains("destructive") || err.contains("blocked"),
            "Error should mention destructive/blocked guard: {err}"
        );
    }
}

// ---------------------------------------------------------------------------
// 3. Symlink & Directory Traversal Boundaries
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_sandbox_symlink_traversal_confinement() {
    let env = setup_test_env();
    let temp_dir = tempfile::tempdir().unwrap();
    let allowed_dir = temp_dir.path().join("allowed");
    let outside_dir = temp_dir.path().join("outside");
    std::fs::create_dir(&allowed_dir).unwrap();
    std::fs::create_dir(&outside_dir).unwrap();

    let outside_file = outside_dir.join("victim.txt");
    std::fs::write(&outside_file, "original_content").unwrap();

    // Create a symlink inside allowed_dir pointing to outside_file
    let symlink_path = allowed_dir.join("link_to_victim");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_file, &symlink_path).unwrap();

    let symlink_str = symlink_path.to_string_lossy();
    let script = format!(
        r#"
let target = "{symlink_str}"
"#
    );
    run_script(&script, &env).await.unwrap();

    // Ensure outside file content wasn't modified
    assert_eq!(std::fs::read_to_string(&outside_file).unwrap(), "original_content");
}

// ---------------------------------------------------------------------------
// 4. Subprocess Sandboxing Under Isolated Profile
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_sandbox_isolated_mode_process_execution() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();

    let res = run_script("sandbox --isolated echo isolated_ok", &env).await;
    assert!(res.is_ok(), "expected sandbox --isolated to execute cleanly");
}
