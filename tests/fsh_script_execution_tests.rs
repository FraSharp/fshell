mod common;
use common::*;
use fshell_core::Val;
use fshell_engine::run_script;
use ustr::ustr;

// ---------------------------------------------------------------------------
// 1. Native FSH Script Control Flow & Data Structures
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_fsh_script_function_calls_and_returns() {
    let env = setup_test_env();
    let script = r#"
fn compute_volume(width, height, depth) {
    let base = ($width * $height)
    let vol = ($base * $depth)
    return $vol
}

let vol1 = compute_volume 3 4 5
let vol2 = compute_volume 10 2 3
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(vars.get("vol1"), Some(&Val::List(vec![Val::Int(60)])));
    assert_eq!(vars.get("vol2"), Some(&Val::List(vec![Val::Int(60)])));
}

#[tokio::test]
async fn test_fsh_script_loops_break_continue() {
    let env = setup_test_env();
    let script = r#"
let sum = 0
let counter = 0

while $counter < 10 {
    counter = ($counter + 1)
    if $counter == 3 {
        continue
    }
    if $counter == 7 {
        break
    }
    sum = ($sum + $counter)
}
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(vars.get("sum"), Some(&Val::Int(1 + 2 + 4 + 5 + 6)));
}

#[tokio::test]
async fn test_fsh_script_for_in_list_and_range() {
    let env = setup_test_env();
    let script = r#"
let items = ["alpha", "beta", "gamma"]
let collected = ""

for item in $items {
    let upper = ${item:u}
    collected = ($collected + $upper)
}
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(
        vars.get("collected"),
        Some(&Val::String("ALPHABETAGAMMA".to_string()))
    );
}

#[tokio::test]
async fn test_fsh_script_match_expressions() {
    let env = setup_test_env();
    let script = r#"
let code1 = 200
let desc1 = ""
match $code1 {
    200 => { let desc1 = "OK" },
    404 => { let desc1 = "Not Found" },
    _ => { let desc1 = "Unknown" }
}

let code2 = 404
let desc2 = ""
match $code2 {
    200 => { let desc2 = "OK" },
    404 => { let desc2 = "Not Found" },
    _ => { let desc2 = "Unknown" }
}

let code3 = 999
let desc3 = ""
match $code3 {
    200 => { let desc3 = "OK" },
    404 => { let desc3 = "Not Found" },
    _ => { let desc3 = "Unknown" }
}
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(vars.get("desc1"), Some(&Val::String("OK".to_string())));
    assert_eq!(
        vars.get("desc2"),
        Some(&Val::String("Not Found".to_string()))
    );
    assert_eq!(vars.get("desc3"), Some(&Val::String("Unknown".to_string())));
}

#[tokio::test]
async fn test_fsh_script_nested_map_mutations() {
    let env = setup_test_env();
    let script = r#"
let user = {
    name: "Ferris",
    metadata: {
        role: "admin",
        level: 42
    },
    tags: ["rust", "shell"]
}

let user_role = $user.metadata.role
let user_level = $user.metadata.level
let user_tags = $user.tags
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(
        vars.get("user_role"),
        Some(&Val::String("admin".to_string()))
    );
    assert_eq!(vars.get("user_level"), Some(&Val::Int(42)));
    assert_eq!(
        vars.get("user_tags"),
        Some(&Val::List(vec![
            Val::String("rust".to_string()),
            Val::String("shell".to_string())
        ]))
    );
}

// ---------------------------------------------------------------------------
// 2. Mixed POSIX Blocks Inside FSH Scripts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_fsh_mixed_posix_bidirectional_variable_sharing() {
    let env = setup_test_env();
    let script = r#"
let fsh_initial = "hello_posix"
let base_num = 25

sh {
    test "$fsh_initial" = "hello_posix"
    test "$base_num" -eq 25
    computed_val=$((base_num * 4))
    export POSIX_RESULT="computed_$computed_val"
    export POSIX_MUTATION="mutated_by_embedded_sh"
}

let fsh_captured_result = $POSIX_RESULT
let fsh_captured_mutation = $POSIX_MUTATION
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(
        vars.get("fsh_captured_result"),
        Some(&Val::String("computed_100".to_string()))
    );
    assert_eq!(
        vars.get("fsh_captured_mutation"),
        Some(&Val::String("mutated_by_embedded_sh".to_string()))
    );
}

#[tokio::test]
async fn test_fsh_mixed_posix_inside_loops() {
    let env = setup_test_env();
    let script = r#"
let numbers = [10, 20, 30]
let total_sum = ""

for n in $numbers {
    let doubled = ""
    sh {
        export doubled=$((n * 2))
    }
    total_sum = ($total_sum + $doubled)
}
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(
        vars.get("total_sum"),
        Some(&Val::String("204060".to_string()))
    );
}

#[tokio::test]
async fn test_fsh_mixed_posix_inside_functions() {
    let env = setup_test_env();
    let script = r#"
fn process_artifact(name, ext) {
    let slug = ""
    sh {
        raw_slug=$(printf "%s_processed.%s" "$name" "$ext" | tr '[:upper:]' '[:lower:]')
        export slug="$raw_slug"
    }
    let res = {
        name: $name,
        ext: $ext,
        output_file: $slug
    }
    return $res
}

let artifact = process_artifact "MyDocument" "PDF"
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    if let Some(Val::List(items)) = vars.get("artifact")
        && let Some(Val::Map(m)) = items.first()
    {
        assert_eq!(
            m.get(&ustr("output_file")),
            Some(&Val::String("mydocument_processed.pdf".to_string()))
        );
    } else {
        panic!("Expected Map artifact result in list");
    }
}

#[tokio::test]
async fn test_fsh_mixed_posix_try_catch_error_handling() {
    let env = setup_test_env();
    let script = r#"
let caught = false
let error_msg = ""

try {
    sh {
        echo "About to fail"
        exit 12
    }
} catch |err| {
    let caught = true
    let error_msg = $err
}
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(vars.get("caught"), Some(&Val::Bool(true)));
    assert!(vars.get("error_msg").is_some());
}

#[tokio::test]
async fn test_fsh_mixed_posix_subshell_isolation() {
    let env = setup_test_env();
    let script = r#"
let outer_state = "initial"

sh {
    # Subshell should not leak environment changes
    (
        export LEAKED_SUBSHELL="leaked"
        outer_state="subshell_mutated"
    )
    # Top-level POSIX execution should mutate
    export TOP_LEVEL_POSIX="persisted"
}
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(
        vars.get("outer_state"),
        Some(&Val::String("initial".to_string()))
    );
    assert!(vars.get("LEAKED_SUBSHELL").is_none());
    assert_eq!(
        vars.get("TOP_LEVEL_POSIX"),
        Some(&Val::String("persisted".to_string()))
    );
}

#[tokio::test]
async fn test_fsh_mixed_posix_heredoc_file_orchestration() {
    let env = setup_test_env();
    let temp_dir = tempfile::tempdir().unwrap();
    let target_file = temp_dir.path().join("orchestrated_data.csv");
    let target_file_str = target_file.to_string_lossy();

    let script = format!(
        r#"
let out_path = "{target_file_str}"

sh {{
    cat << 'EOF' > "$out_path"
id,name,score
1,Alpha,88
2,Beta,94
3,Gamma,72
EOF
}}
"#
    );

    run_script(&script, &env).await.unwrap();

    assert!(target_file.exists());
    let content = std::fs::read_to_string(&target_file).unwrap();
    assert!(content.contains("Alpha,88"));
    assert!(content.contains("Beta,94"));
}

#[tokio::test]
async fn test_fsh_mixed_posix_sourcing_script_and_fn_delegation() {
    let env = setup_test_env();
    let temp_dir = tempfile::tempdir().unwrap();
    let helper_script = temp_dir.path().join("helpers.sh");

    std::fs::write(
        &helper_script,
        r#"#!/bin/sh
export SOURCED_CONFIG="active_v2"

posix_compute() {
    echo 40
}
"#,
    )
    .unwrap();

    let helper_path_str = helper_script.to_string_lossy();
    let script = format!(
        r#"
source "{helper_path_str}"

let config_val = $SOURCED_CONFIG

let computed_res = ""
sh {{
    val=$(posix_compute)
    export computed_res="$val"
}}
"#
    );

    run_script(&script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(
        vars.get("config_val"),
        Some(&Val::String("active_v2".to_string()))
    );
    assert_eq!(
        vars.get("computed_res"),
        Some(&Val::String("40".to_string()))
    );
}

// ---------------------------------------------------------------------------
// 3. Subprocess .fsh Script File Execution with Mixed POSIX
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_subprocess_fsh_script_execution() {
    let temp_dir = tempfile::tempdir().unwrap();
    let script_file = temp_dir.path().join("main_pipeline.fsh");

    let script_content = r#"
let message = "fsh_subprocess_ok"
let count = 42

sh {
    test "$message" = "fsh_subprocess_ok"
    test "$count" -eq 42
    printf "SUBPROCESS_OUTPUT_OK\n"
}
"#;
    std::fs::write(&script_file, script_content).unwrap();

    let output = FshCmd::new()
        .arg(script_file.to_str().unwrap())
        .run()
        .unwrap();

    output.assert_success();
    assert!(output.stdout.contains("SUBPROCESS_OUTPUT_OK"));
}

#[tokio::test]
async fn test_subprocess_fsh_script_with_arguments() {
    let temp_dir = tempfile::tempdir().unwrap();
    let script_file = temp_dir.path().join("args_pipeline.fsh");

    let script_content = r#"
let first = $1
let second = $2

sh {
    test "$first" = "foo"
    test "$second" = "bar"
    printf "ARGS_VERIFIED:%s:%s\n" "$first" "$second"
}
"#;
    std::fs::write(&script_file, script_content).unwrap();

    let output = FshCmd::new()
        .arg(script_file.to_str().unwrap())
        .arg("foo")
        .arg("bar")
        .run()
        .unwrap();

    output.assert_success();
    assert!(output.stdout.contains("ARGS_VERIFIED:foo:bar"));
}

#[tokio::test]
async fn test_fsh_mixed_posix_pipeline_output_transformation() {
    let env = setup_test_env();
    let script = r#"
let records = [
    { id: 1, name: "nginx", active: true },
    { id: 2, name: "redis", active: false },
    { id: 3, name: "postgres", active: true }
]

let active_services = ($records | filter active == true)

sh {
    test "$active_services" != ""
    echo "SERVICES_COUNT=$active_services" > /dev/null
}

let count_active = ($active_services | count)
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(
        vars.get("count_active"),
        Some(&Val::List(vec![Val::Int(2)]))
    );
}
