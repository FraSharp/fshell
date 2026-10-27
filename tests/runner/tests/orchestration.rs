//! Real runner control flow against explicit, bounded tool stand-ins.
//! No nested builds, inherited tool lookup or parent environment changes.

use fshell_test_support::fixture::ShellFixture;
use fshell_test_support::process::{ProcessCommand, ProcessOutput};
use serde_json::Value;
use std::io;

fn fixture(mode: &str) -> io::Result<ShellFixture> {
    let fixture = ShellFixture::new()?;
    fixture.create_file("mode", mode.as_bytes())?;
    fixture.create_file("fake-fsh", b"not executed")?;
    Ok(fixture)
}

fn runner(fixture: &ShellFixture, args: &[&str]) -> ProcessOutput {
    ProcessCommand::new(
        env!("CARGO_BIN_EXE_fshell-test-runner"),
        fixture.workspace(),
    )
    .env("CARGO", env!("CARGO_BIN_EXE_fshell-orchestration-probe"))
    .env("RUSTC", env!("CARGO_BIN_EXE_fshell-orchestration-probe"))
    .env("FSHELL_ORCHESTRATION_PROBE_ROOT", fixture.workspace())
    .env("FSHELL_TEST_EXECUTABLE", "/stale/caller/fsh")
    .args(args)
    .run()
    .expect("bounded runner invocation")
}

fn calls(fixture: &ShellFixture) -> io::Result<Vec<Value>> {
    fixture
        .read_file("calls")?
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).map_err(io::Error::other))
        .collect()
}

fn operation(call: &Value) -> String {
    let args = call["args"].as_array().expect("argument array");
    let first = args[0].as_str().expect("tool operation");
    match args.get(1).and_then(Value::as_str) {
        Some(second) if first == "nextest" => format!("nextest {second}"),
        _ => first.to_string(),
    }
}

fn operations(calls: &[Value]) -> Vec<String> {
    calls.iter().map(operation).collect()
}

fn has_pair(call: &Value, option: &str, value: &str) -> bool {
    call["args"]
        .as_array()
        .expect("argument array")
        .windows(2)
        .any(|pair| pair[0] == option && pair[1] == value)
}

fn has_flag(call: &Value, flag: &str) -> bool {
    call["args"]
        .as_array()
        .expect("argument array")
        .iter()
        .any(|argument| argument == flag)
}

fn filter_expression(call: &Value) -> &str {
    call["args"]
        .as_array()
        .expect("argument array")
        .windows(2)
        .find(|pair| pair[0] == "--filter-expr")
        .map(|pair| pair[1].as_str().expect("filter value"))
        .expect("filter expression")
}

#[test]
fn library_selection_never_builds_or_passes_a_shell_executable() -> io::Result<()> {
    let fixture = fixture("success")?;
    let output = runner(&fixture, &["--suite", "library", "--full", "--release"]);
    assert!(output.status.success(), "{output:?}");
    let calls = calls(&fixture)?;
    assert_eq!(
        operations(&calls),
        [
            "nextest --version",
            "-vV",
            "metadata",
            "nextest list",
            "nextest run"
        ]
    );
    let listing = &calls[3];
    assert!(has_pair(listing, "--message-format", "json"));
    assert!(has_pair(
        listing,
        "--features",
        "fshell-specifications/full"
    ));
    assert!(!has_flag(listing, "--filter-expr"));
    assert!(!has_flag(listing, "--profile"));
    let run = calls.last().expect("nextest run");
    assert_eq!(run["executable"], Value::Null);
    assert!(has_pair(run, "--profile", "library"));
    assert!(has_pair(run, "--features", "fshell-specifications/full"));
    assert!(has_pair(run, "--test", "native_evaluation"));
    assert!(!has_pair(run, "--test", "cli_execution"));
    assert!(
        run["args"]
            .as_array()
            .expect("args")
            .contains(&Value::from("--release"))
    );
    let filter = filter_expression(run);
    assert!(filter.contains("package(=fshell-posix)"), "{filter}");
    assert!(
        filter.contains("test(eval::capture_transport::)"),
        "{filter}"
    );
    assert!(
        filter.contains("package(=fshell-specifications)"),
        "{filter}"
    );
    fixture.close()
}

#[test]
fn executable_selection_uses_only_the_selected_cargo_artifact_and_cli_target() -> io::Result<()> {
    let fixture = fixture("success")?;
    let output = runner(&fixture, &["--suite", "executable", "--full", "--release"]);
    assert!(output.status.success(), "{output:?}");
    let calls = calls(&fixture)?;
    assert_eq!(
        operations(&calls),
        [
            "nextest --version",
            "-vV",
            "metadata",
            "build",
            "nextest run"
        ]
    );
    let build = &calls[3];
    let run = &calls[4];
    for call in [build, run] {
        assert!(has_pair(call, "--target", "aarch64-unknown-linux-gnu"));
        assert!(
            call["args"]
                .as_array()
                .expect("args")
                .contains(&Value::from("--locked"))
        );
        assert!(
            call["args"]
                .as_array()
                .expect("args")
                .contains(&Value::from("--release"))
        );
    }
    assert!(has_pair(build, "--features", "full"));
    assert!(has_pair(run, "--features", "fshell-specifications/full"));
    assert!(has_pair(run, "--profile", "executable"));
    assert!(has_pair(run, "--test", "cli_execution"));
    assert!(!has_pair(run, "--test", "native_evaluation"));
    assert_eq!(
        run["executable"],
        fixture
            .workspace()
            .join("fake-fsh")
            .to_str()
            .expect("fixture path")
    );
    fixture.close()
}

#[test]
fn support_selection_neither_queries_specifications_nor_builds_the_shell() -> io::Result<()> {
    let fixture = fixture("success")?;
    let output = runner(&fixture, &["--suite", "support"]);
    assert!(output.status.success(), "{output:?}");
    let calls = calls(&fixture)?;
    assert_eq!(
        operations(&calls),
        ["nextest --version", "-vV", "nextest run"]
    );
    let run = calls.last().expect("nextest run");
    assert_eq!(run["executable"], Value::Null);
    assert!(has_pair(run, "--profile", "support"));
    fixture.close()
}

#[cfg(target_os = "linux")]
#[test]
fn containment_selection_has_no_shell_artifact_or_specification_metadata_dependency()
-> io::Result<()> {
    let fixture = fixture("success")?;
    let output = runner(&fixture, &["--suite", "containment"]);
    assert!(output.status.success(), "{output:?}");
    let calls = calls(&fixture)?;
    assert_eq!(
        operations(&calls),
        ["nextest --version", "-vV", "nextest run"]
    );
    let run = calls.last().expect("nextest run");
    assert_eq!(run["executable"], Value::Null);
    assert!(has_pair(run, "--profile", "containment"));
    assert!(has_pair(run, "--test", "namespace_containment"));
    fixture.close()
}

#[test]
fn invalid_mechanical_full_feature_requests_fail_before_any_tool_invocation() -> io::Result<()> {
    for suite in ["support", "containment"] {
        let fixture = fixture("success")?;
        let output = runner(&fixture, &["--suite", suite, "--full"]);
        assert!(!output.status.success(), "{output:?}");
        assert_eq!(
            fixture
                .read_file("calls")
                .expect_err("no tools should execute")
                .kind(),
            io::ErrorKind::NotFound
        );
        fixture.close()?;
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
#[test]
fn unsupported_containment_hosts_fail_before_any_tool_invocation() -> io::Result<()> {
    let fixture = fixture("success")?;
    let output = runner(&fixture, &["--suite", "containment"]);
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(
        fixture
            .read_file("calls")
            .expect_err("no tools should execute")
            .kind(),
        io::ErrorKind::NotFound
    );
    fixture.close()
}

#[test]
fn unclassified_missing_or_ambiguous_specification_targets_fail_before_building() -> io::Result<()>
{
    for mode in [
        "metadata-unknown",
        "metadata-missing",
        "metadata-ambiguous",
        "metadata-fail",
    ] {
        let fixture = fixture(mode)?;
        let output = runner(&fixture, &["--suite", "executable"]);
        assert!(!output.status.success(), "{mode}: {output:?}");
        let calls = calls(&fixture)?;
        assert_eq!(
            operations(&calls),
            ["nextest --version", "-vV", "metadata"],
            "{mode}"
        );
        fixture.close()?;
    }
    Ok(())
}

#[test]
fn failed_builds_and_absent_artifacts_never_start_nextest_execution() -> io::Result<()> {
    for mode in ["build-fail", "artifact-missing"] {
        let fixture = fixture(mode)?;
        let output = runner(&fixture, &["--suite", "executable"]);
        assert!(!output.status.success(), "{mode}: {output:?}");
        let calls = calls(&fixture)?;
        assert_eq!(
            operations(&calls),
            ["nextest --version", "-vV", "metadata", "build"],
            "{mode}"
        );
        fixture.close()?;
    }
    Ok(())
}

#[test]
fn listing_guard_fails_before_build_or_execution_when_a_family_disappears() -> io::Result<()> {
    let fixture = fixture("listing-gap")?;
    let output = runner(&fixture, &["--suite", "library"]);
    assert!(!output.status.success(), "{output:?}");
    let calls = calls(&fixture)?;
    assert_eq!(
        operations(&calls),
        ["nextest --version", "-vV", "metadata", "nextest list"]
    );
    let stderr = std::str::from_utf8(&output.capture.stderr).expect("runner stderr");
    assert!(stderr.contains("lost curated families"), "{stderr}");
    assert!(
        stderr.contains("posix_builtins::read_cmd::tests::"),
        "{stderr}"
    );
    assert!(!operations(&calls).contains(&"nextest run".to_string()));
    fixture.close()
}

#[test]
fn failed_prerequisites_and_nextest_verdicts_propagate_to_the_runner_exit() -> io::Result<()> {
    for (mode, expected) in [
        ("version-fail", vec!["nextest --version"]),
        ("rustc-fail", vec!["nextest --version", "-vV"]),
        (
            "nextest-fail",
            vec![
                "nextest --version",
                "-vV",
                "metadata",
                "nextest list",
                "nextest run",
            ],
        ),
    ] {
        let fixture = fixture(mode)?;
        let output = runner(&fixture, &["--suite", "library"]);
        assert!(!output.status.success(), "{mode}: {output:?}");
        assert_eq!(operations(&calls(&fixture)?), expected, "{mode}");
        fixture.close()?;
    }
    Ok(())
}
