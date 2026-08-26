mod common;
use common::*;
use fshell_core::{FxIndexMap, Param, Stmt, Val};
use fshell_engine::handoff::{HandoffState, load_handoff};
use fshell_engine::run_script;
use fshell_hash::{FxBuildHasher, FxHashMap};
use ustr::ustr;

// ---------------------------------------------------------------------------
// 1. Full Roundtrip In-Memory & File Serialization
// ---------------------------------------------------------------------------

#[test]
fn test_handoff_state_json_roundtrip_all_val_types() {
    let mut vars = FxHashMap::default();
    vars.insert("int_val".to_string(), Val::Int(12345));
    vars.insert("float_val".to_string(), Val::Float(3.125));
    vars.insert(
        "str_val".to_string(),
        Val::String("hello_handoff".to_string()),
    );
    vars.insert("bool_val".to_string(), Val::Bool(true));
    vars.insert(
        "list_val".to_string(),
        Val::List(vec![Val::Int(1), Val::String("two".to_string())]),
    );

    let mut map = FxIndexMap::with_hasher(FxBuildHasher::default());
    map.insert(ustr("key1"), Val::Int(42));
    map.insert(ustr("key2"), Val::String("val2".to_string()));
    vars.insert("map_val".to_string(), Val::Map(map));

    let mut fns = FxHashMap::default();
    fns.insert(
        "greet".to_string(),
        (
            vec![Param {
                name: "name".to_string(),
                constraint: fshell_core::TypeConstraint::Any,
            }],
            None,
            vec![Stmt::Comment("greet fn".to_string())],
        ),
    );

    let mut hooks = FxHashMap::default();
    hooks.insert("precmd".to_string(), vec!["my_precmd_hook".to_string()]);

    let state = HandoffState {
        vars,
        fns,
        caps_held: std::collections::HashSet::new(),
        caps_strict_mode: false,
        reactive_pipelines: FxHashMap::default(),
        session_id: "test-session-123".to_string(),
        cwd: "/tmp/test_handoff_dir".to_string(),
        options: fshell_engine::ShellOptions::default(),
        hooks,
        last_exit_code: 0,
        last_duration_secs: 0.123,
    };

    // Serialize to JSON string
    let json = serde_json::to_string_pretty(&state).unwrap();
    assert!(json.contains("hello_handoff"));
    assert!(json.contains("test-session-123"));

    // Deserialize back
    let restored: HandoffState = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.session_id, "test-session-123");
    assert_eq!(restored.vars.get("int_val"), Some(&Val::Int(12345)));
    assert_eq!(
        restored.vars.get("str_val"),
        Some(&Val::String("hello_handoff".to_string()))
    );
    assert_eq!(
        restored.hooks.get("precmd"),
        Some(&vec!["my_precmd_hook".to_string()])
    );
}

#[test]
fn test_save_and_load_handoff_file_lifecycle() {
    let temp_dir = tempfile::tempdir().unwrap();
    let handoff_file = temp_dir.path().join("handoff.json");

    let mut vars = FxHashMap::default();
    vars.insert(
        "restored_key".to_string(),
        Val::String("restored_val".to_string()),
    );

    let state = HandoffState {
        vars,
        fns: FxHashMap::default(),
        caps_held: std::collections::HashSet::new(),
        caps_strict_mode: false,
        reactive_pipelines: FxHashMap::default(),
        session_id: "lifecycle-session".to_string(),
        cwd: "/tmp".to_string(),
        options: fshell_engine::ShellOptions::default(),
        hooks: FxHashMap::default(),
        last_exit_code: 42,
        last_duration_secs: 1.5,
    };

    // Write state to file
    let json_content = serde_json::to_string_pretty(&state).unwrap();
    std::fs::write(&handoff_file, json_content).unwrap();

    // Load handoff file
    let loaded = load_handoff(&handoff_file).unwrap();
    assert_eq!(loaded.session_id, "lifecycle-session");
    assert_eq!(loaded.last_exit_code, 42);
    assert_eq!(
        loaded.vars.get("restored_key"),
        Some(&Val::String("restored_val".to_string()))
    );

    // load_handoff should have deleted the consumed handoff file
    assert!(!handoff_file.exists());
}

// ---------------------------------------------------------------------------
// 2. Corrupted & Missing Handoff Recovery
// ---------------------------------------------------------------------------

#[test]
fn test_load_handoff_corrupted_json_handling() {
    let temp_dir = tempfile::tempdir().unwrap();
    let handoff_file = temp_dir.path().join("corrupted_handoff.json");

    // Write incomplete / corrupted JSON
    std::fs::write(&handoff_file, "{\"vars\": {\"incomplete\": ").unwrap();

    let res = load_handoff(&handoff_file);
    assert!(res.is_err());

    // Corrupted file should be renamed to .json.corrupted
    let corrupted_backup = temp_dir.path().join("corrupted_handoff.json.corrupted");
    assert!(corrupted_backup.exists());
    assert!(!handoff_file.exists());
}

#[test]
fn test_load_handoff_non_existent_file() {
    let temp_dir = tempfile::tempdir().unwrap();
    let non_existent = temp_dir.path().join("does_not_exist.json");

    let res = load_handoff(&non_existent);
    assert!(res.is_err());
}

// ---------------------------------------------------------------------------
// 3. Live Environment Restoration & Execution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_restored_handoff_state_executes_in_env() {
    let env = setup_test_env();

    // Populate env with handoff data
    let mut vars = FxHashMap::default();
    vars.insert("USER_NAME".to_string(), Val::String("alice".to_string()));
    vars.insert("CONFIG_LVL".to_string(), Val::Int(3));

    {
        let mut env_vars = env.vars.write();
        for (k, v) in vars {
            env_vars.insert(k, v);
        }
    }

    // Run script that uses restored variables
    let script = r#"
let greeting = ("Hello " + $USER_NAME)
let next_lvl = ($CONFIG_LVL + 1)
"#;
    run_script(script, &env).await.unwrap();

    let read_vars = env.vars.read();
    assert_eq!(
        read_vars.get("greeting"),
        Some(&Val::String("Hello alice".to_string()))
    );
    assert_eq!(read_vars.get("next_lvl"), Some(&Val::Int(4)));
}
