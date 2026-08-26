mod common;
use common::*;
use fshell_engine::run_script;
use fshell_repl::history::{init_db, log_command, query_history};

static TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ---------------------------------------------------------------------------
// 1. History Logging & Querying
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_sqlite_history_insert_and_query() {
    let _lock = TEST_MUTEX.lock().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test_history.db");

    // Set test history DB
    unsafe {
        std::env::set_var("FSH_TEST_DB_PATH", &db_path);
    }

    init_db().unwrap();

    log_command(
        "cargo test --all",
        "/home/user/project",
        1000000,
        250,
        Some(0),
        "test-host",
        "test-user",
        "sess-1",
    )
    .unwrap();

    log_command(
        "git status",
        "/home/user/project",
        1005000,
        15,
        Some(0),
        "test-host",
        "test-user",
        "sess-1",
    )
    .unwrap();

    let all_entries = query_history(Some(10), None, None, None, None, None).unwrap();
    assert_eq!(all_entries.len(), 2);
    // Ordered by timestamp desc
    assert_eq!(all_entries[0].command, "git status");
    assert_eq!(all_entries[1].command, "cargo test --all");

    // Filter by command pattern
    let filtered = query_history(Some(10), Some("cargo"), None, None, None, None).unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].command, "cargo test --all");

    // Clean up env
    unsafe {
        std::env::remove_var("FSH_TEST_DB_PATH");
    }
}

// ---------------------------------------------------------------------------
// 2. Frecency Directory Tracking & Scoring
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_frecency_recording_and_matching() {
    let env = setup_test_env();

    let temp_dir = tempfile::tempdir().unwrap();
    let dir_a = temp_dir.path().join("dev_rust_my_project");
    let dir_b = temp_dir.path().join("dev_python_script_tool");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();

    let dir_a_str = dir_a.to_string_lossy();
    let dir_b_str = dir_b.to_string_lossy();

    // Record visits
    let script = format!(
        r#"
cd "{dir_a_str}"
cd "{dir_b_str}"
cd "{dir_a_str}"
"#
    );
    run_script(&script, &env).await.unwrap();

    let current_dir = env.cwd();
    // Resolve canonicalized paths to match MacOS symlink /var vs /private/var
    assert_eq!(
        current_dir.canonicalize().unwrap_or(current_dir),
        dir_a.canonicalize().unwrap_or(dir_a)
    );
}
