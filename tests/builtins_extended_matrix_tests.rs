mod common;
use common::*;
use fshell_core::Val;
use fshell_engine::run_script;

// ---------------------------------------------------------------------------
// 1. Hash Provider Builtin Verification
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_hash_builtin_sha256_and_sha512() {
    let env = setup_test_env();
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = temp_dir.path().join("input.txt");
    std::fs::write(&file_path, "hello world\n").unwrap();
    let file_str = file_path.to_string_lossy();

    // Default 256
    let script = format!("let h256 = (hash \"{file_str}\")");
    run_script(&script, &env).await.unwrap();

    {
        let vars = env.vars.read();
        if let Some(Val::List(items)) = vars.get("h256") {
            let line = items[0].to_text();
            let hash_hex = line.split_whitespace().next().unwrap();
            assert_eq!(hash_hex.len(), 64); // 256 bits = 64 hex characters
        } else {
            panic!("Expected List of hash items");
        }
    }

    // 512
    let script512 = format!("let h512 = (hash -a 512 \"{file_str}\")");
    run_script(&script512, &env).await.unwrap();

    {
        let vars = env.vars.read();
        if let Some(Val::List(items)) = vars.get("h512") {
            let line = items[0].to_text();
            let hash_hex = line.split_whitespace().next().unwrap();
            assert_eq!(hash_hex.len(), 128); // 512 bits = 128 hex characters
        } else {
            panic!("Expected List of hash items");
        }
    }
}

// ---------------------------------------------------------------------------
// 2. Type & Which Introspection Matrix
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_type_and_which_builtin_resolution() {
    let env = setup_test_env();

    // 1. Builtin resolution
    let script = r#"
let type_cd = (type cd)
let which_ls = (which ls)
"#;
    run_script(script, &env).await.unwrap();

    {
        let vars = env.vars.read();
        assert!(vars.get("type_cd").is_some());
        assert!(vars.get("which_ls").is_some());
    }

    // 2. User function resolution
    let script_fn = r#"
fn custom_tool(x) { $x + 1 }
let type_tool = (type custom_tool)
"#;
    run_script(script_fn, &env).await.unwrap();

    {
        let vars = env.vars.read();
        if let Some(Val::List(items)) = vars.get("type_tool") {
            let text = items[0].to_text();
            assert!(text.contains("function") || text.contains("custom_tool"));
        }
    }
}

// ---------------------------------------------------------------------------
// 3. Text Search & Replace Builtins
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_replace_builtin_in_files() {
    let env = setup_test_env();
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = temp_dir.path().join("code.rs");
    std::fs::write(&file_path, "let old_name = 100;\n").unwrap();
    let file_str = file_path.to_string_lossy();

    let script = format!("replace old_name new_name in \"{file_str}\"");
    run_script(&script, &env).await.unwrap();

    // Small delay for async file write
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let content = std::fs::read_to_string(&file_path).unwrap();
    assert!(content.contains("new_name"));
    assert!(!content.contains("old_name"));
}

// ---------------------------------------------------------------------------
// 4. Data Serialization Formats (@yaml, @text, @table)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_serialization_yaml_and_text_boundaries() {
    let env = setup_test_env();
    let script = r#"
let items = [
    { title: "Task 1", priority: "high" },
    { title: "Task 2", priority: "low" }
]
let yaml_out = $items | @yaml
let text_out = $items | @text
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    if let Some(Val::List(items)) = vars.get("yaml_out") {
        let yaml_str = items
            .iter()
            .map(|v| v.to_text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(yaml_str.contains("Task 1"));
        assert!(yaml_str.contains("Task 2"));
    } else {
        panic!("Expected List for yaml_out");
    }

    if let Some(Val::List(items)) = vars.get("text_out") {
        assert!(!items.is_empty());
    } else {
        panic!("Expected List for text_out");
    }
}
