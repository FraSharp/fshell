//! Integration tests for the semantic layer and the `intent` builtin.
//!
//! These tests are the contract for the eventual model evaluation: they pin the meaning
//! of every corpus prompt, the lowering of representative actions to both targets, and the
//! safety behaviour of executing a structured action.

mod common;
use common::*;

use fshell_semantic::{
    Action, FindFiles, Os, Platform, UtilitySet, lower_fsh, render_fsh, render_posix,
    validate_intent,
};
use std::path::PathBuf;

fn lowering_platform() -> Platform {
    Platform::new(
        Os::MacOs,
        UtilitySet::of([
            "rg", "grep", "find", "lsof", "df", "du", "vm_stat", "uname", "docker",
        ]),
    )
}

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/semantic")
}

// --- corpus ---------------------------------------------------------------

#[test]
fn corpus_cases_validate_and_lower_cleanly() {
    let cases = SemanticCase::discover(&corpus_dir()).expect("discover semantic corpus");
    assert!(
        cases.len() >= 10,
        "expected a representative corpus, found {}",
        cases.len()
    );

    let platform = lowering_platform();
    for case in &cases {
        let intent = case.intent();

        let mut actual: Vec<String> = validate_intent(&intent)
            .iter()
            .map(|issue| issue.param().to_string())
            .collect();
        actual.sort();
        let mut expected = case.expect_issues.clone();
        expected.sort();
        assert_eq!(actual, expected, "case {}: unexpected issues", case.id);

        // Every complete action must lower to both targets without error.
        if let Some(action) = &intent.action
            && expected.is_empty()
        {
            assert!(
                render_fsh(action, &platform).is_ok(),
                "case {}: fsh lowering failed",
                case.id
            );
            assert!(
                render_posix(action, &platform).is_ok(),
                "case {}: posix lowering failed",
                case.id
            );
        }
    }
}

#[test]
fn ambiguous_cases_are_flagged() {
    let cases = SemanticCase::discover(&corpus_dir()).expect("discover semantic corpus");
    let ambiguous: Vec<&str> = cases
        .iter()
        .filter(|case| case.ambiguous)
        .map(|case| case.id.as_str())
        .collect();
    assert!(
        ambiguous.contains(&"remove_chrome_ambiguous"),
        "the ambiguous reading of 'remove chrome' should be represented"
    );
}

// --- semantics preserved through lowering ---------------------------------

#[test]
fn one_action_renders_differently_per_target() {
    let case = SemanticCase::discover(&corpus_dir())
        .expect("discover semantic corpus")
        .into_iter()
        .find(|case| case.id == "find_logs")
        .expect("find_logs case");
    let action = case.intent().action.expect("find_logs has an action");

    let fsh = render_fsh(&action, &lowering_platform()).unwrap();
    let posix = render_posix(&action, &lowering_platform()).unwrap();

    // fsh prefers the native finder + native operators; POSIX uses `find`.
    assert!(fsh.starts_with("ff "), "fsh rendering: {fsh}");
    assert!(fsh.contains("filter (name ~"), "fsh rendering: {fsh}");
    assert!(posix.starts_with("find "), "posix rendering: {posix}");
    assert!(
        posix.contains("-size +500000000c"),
        "posix rendering: {posix}"
    );
    assert_ne!(fsh, posix);
}

#[tokio::test(flavor = "multi_thread")]
async fn lowered_fsh_pipeline_executes_against_the_real_engine() {
    let env = setup_test_env();
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.log"), "x").unwrap();
    std::fs::write(dir.path().join("b.log"), "y").unwrap();
    std::fs::write(dir.path().join("c.txt"), "z").unwrap();

    let action = Action::FindFiles(FindFiles {
        root: Some(dir.path().display().to_string()),
        extension: Some("log".to_string()),
        ..Default::default()
    });

    let platform = Platform::new(Os::current(), UtilitySet::empty());
    let pipeline = lower_fsh(&action, &platform).expect("lower find_files");
    let results = fshell_engine::collect_pipeline(&pipeline, &env)
        .await
        .expect("execute lowered pipeline");

    assert_eq!(results.len(), 2, "only .log files should be returned");
}

// --- the intent builtin ----------------------------------------------------

#[test]
fn intent_lists_supported_actions() {
    let output = FshCmd::new().cmd("intent --actions").run().unwrap();
    output.assert_success();
    output.assert_stdout_contains("find_files");
    output.assert_stdout_contains("signal_process");
    output.assert_stdout_contains("run_container");
}

#[test]
fn intent_schema_is_valid_tool_json() {
    let output = FshCmd::new().cmd("intent --schema").run().unwrap();
    output.assert_success();
    let json = output.stdout_json_val().expect("schema is valid JSON");
    let tools = json["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 18);
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"find_files"));
}

#[test]
fn intent_plan_shows_both_targets() {
    let cmd = FshCmd::new();
    let plan = cmd
        .create_file(
            "plan.json",
            r#"{"kind":"find_files","root":"/tmp","extension":"log"}"#,
        )
        .unwrap();

    let output = cmd
        .cmd(&format!("intent --file {} --explain", plan.display()))
        .run()
        .unwrap();
    output.assert_success();
    output.assert_stdout_contains("fsh:");
    output.assert_stdout_contains("posix:");
    output.assert_stdout_contains("ff ");
    output.assert_stdout_contains("find ");
}

#[test]
fn intent_reports_missing_information() {
    let cmd = FshCmd::new();
    let plan = cmd
        .create_file(
            "plan.json",
            r#"{"kind":"run_container","ports":[{"host_port":8000}]}"#,
        )
        .unwrap();

    let output = cmd
        .cmd(&format!("intent --file {}", plan.display()))
        .run()
        .unwrap();
    output.assert_success();
    output.assert_stdout_contains("image");
    output.assert_stdout_contains("Understood so far");
}

#[test]
fn intent_executes_and_composes_in_a_pipeline() {
    let cmd = FshCmd::new();
    cmd.create_file("logs/a.log", "x").unwrap();
    cmd.create_file("logs/b.log", "y").unwrap();
    cmd.create_file("logs/notes.txt", "z").unwrap();
    let plan = cmd
        .create_file(
            "plan.json",
            r#"{"kind":"find_files","root":"logs","extension":"log"}"#,
        )
        .unwrap();

    let output = cmd
        .cmd(&format!("intent --file {} --run | count", plan.display()))
        .run()
        .unwrap();
    output.assert_success();
    output.assert_stdout_trimmed_eq("2");
}

#[test]
fn intent_executes_via_the_posix_target() {
    let cmd = FshCmd::new();
    cmd.create_file("logs/a.log", "x").unwrap();
    cmd.create_file("logs/b.log", "y").unwrap();
    let plan = cmd
        .create_file(
            "plan.json",
            r#"{"kind":"find_files","root":"logs","extension":"log"}"#,
        )
        .unwrap();

    let output = cmd
        .cmd(&format!(
            "intent --file {} --target posix --run | count",
            plan.display()
        ))
        .run()
        .unwrap();
    output.assert_success();
    output.assert_stdout_trimmed_eq("2");
}

#[test]
fn intent_answers_informational_questions() {
    let cmd = FshCmd::new();
    let plan = cmd
        .create_file(
            "plan.json",
            r#"{"mode":"inform","info":{"kind":"signal","signal":"KILL"}}"#,
        )
        .unwrap();

    let output = cmd
        .cmd(&format!("intent --file {}", plan.display()))
        .run()
        .unwrap();
    output.assert_success();
    output.assert_stdout_contains("SIGKILL");
}

#[test]
fn intent_refuses_destructive_action_without_confirmation() {
    // Kept in a `TempDir` owned by this test: `FshCmd::run` consumes and drops its own
    // `TempDir`, which would remove the victim before we could check it survived.
    let dir = tempfile::tempdir().expect("tempdir");
    let victim = dir.path().join("victim.txt");
    std::fs::write(&victim, "keep me").unwrap();
    let plan = dir.path().join("plan.json");
    std::fs::write(
        &plan,
        format!(r#"{{"kind":"delete_path","path":"{}"}}"#, victim.display()),
    )
    .unwrap();

    let output = FshCmd::new()
        .cmd(&format!("intent --file {} --run", plan.display()))
        .run()
        .unwrap();
    output.assert_success();
    output.assert_stdout_contains("cancelled");
    assert!(
        victim.exists(),
        "a destructive action must not run without confirmation"
    );
}
