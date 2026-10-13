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

fn operation(call: &Value) -> &str {
    call["args"][0].as_str().expect("tool operation")
}

fn has_pair(call: &Value, option: &str, value: &str) -> bool {
    call["args"]
        .as_array()
        .expect("argument array")
        .windows(2)
        .any(|pair| pair[0] == option && pair[1] == value)
}

#[test]
fn library_selection_never_builds_or_passes_a_shell_executable() -> io::Result<()> {
    let fixture = fixture("success")?;
    let output = runner(&fixture, &["--suite", "library", "--full", "--release"]);
    assert!(output.status.success(), "{output:?}");
    let calls = calls(&fixture)?;
    assert_eq!(
        calls.iter().map(operation).collect::<Vec<_>>(),
        ["nextest", "-vV", "metadata", "nextest"]
    );
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
    fixture.close()
}

#[test]
fn executable_selection_uses_only_the_selected_cargo_artifact_and_cli_target() -> io::Result<()> {
    let fixture = fixture("success")?;
    let output = runner(&fixture, &["--suite", "executable", "--full", "--release"]);
    assert!(output.status.success(), "{output:?}");
    let calls = calls(&fixture)?;
    assert_eq!(
        calls.iter().map(operation).collect::<Vec<_>>(),
        ["nextest", "-vV", "metadata", "build", "nextest"]
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
        calls.iter().map(operation).collect::<Vec<_>>(),
        ["nextest", "-vV", "nextest"]
    );
    let run = calls.last().expect("nextest run");
    assert_eq!(run["executable"], Value::Null);
    assert!(has_pair(run, "--profile", "support"));
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
            calls.iter().map(operation).collect::<Vec<_>>(),
            ["nextest", "-vV", "metadata"],
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
            calls.iter().map(operation).collect::<Vec<_>>(),
            ["nextest", "-vV", "metadata", "build"],
            "{mode}"
        );
        fixture.close()?;
    }
    Ok(())
}

#[test]
fn failed_prerequisites_and_nextest_verdicts_propagate_to_the_runner_exit() -> io::Result<()> {
    for (mode, expected) in [
        ("version-fail", vec!["nextest"]),
        ("rustc-fail", vec!["nextest", "-vV"]),
        (
            "nextest-fail",
            vec!["nextest", "-vV", "metadata", "nextest"],
        ),
    ] {
        let fixture = fixture(mode)?;
        let output = runner(&fixture, &["--suite", "library"]);
        assert!(!output.status.success(), "{mode}: {output:?}");
        assert_eq!(
            calls(&fixture)?.iter().map(operation).collect::<Vec<_>>(),
            expected,
            "{mode}"
        );
        fixture.close()?;
    }
    Ok(())
}
