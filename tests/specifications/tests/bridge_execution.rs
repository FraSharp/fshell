//! External-process contracts at the executable boundary: exact byte streams,
//! status observations, typed resolution failures and inherited fixture state.
//!
//! Terminal handoff and incremental stderr delivery need a controlling terminal
//! and belong to the future PTY adapter, not this target.

use fshell_specifications::shell_executable;
use fshell_test_support::fixture::ShellFixture;
use fshell_test_support::process::{ProcessCommand, ProcessOutput};
use std::io;
use std::os::unix::ffi::OsStrExt;

fn command(fixture: &ShellFixture) -> io::Result<ProcessCommand> {
    Ok(fixture
        .command(shell_executable()?)
        .args(["--no-color", "--no-dym"]))
}

fn assert_output(output: &ProcessOutput, code: i32, stdout: &[u8], stderr: &[u8]) {
    assert_eq!(output.status.code(), Some(code), "{output:?}");
    assert_eq!(output.capture.stdout, stdout, "{output:?}");
    assert_eq!(output.capture.stderr, stderr, "{output:?}");
}

#[test]
fn external_process_preserves_exact_streams_and_status() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let output = command(&fixture)?
        .args([
            "--native",
            "-c",
            r#"/bin/sh -c 'printf out; printf err >&2; exit 37'"#,
        ])
        .run()
        .expect("external execution");
    assert_output(&output, 37, b"out", b"err");
    fixture.close()
}

#[test]
fn external_binary_stream_retains_invalid_utf8_nul_and_carriage_returns() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let output = command(&fixture)?
        .args(["--native", "-c", r#"/bin/sh -c 'printf "\377\000x\r\n"'"#])
        .run()
        .expect("binary external execution");
    assert_output(&output, 0, &[0xff, 0x00, b'x', b'\r', b'\n'], b"");
    fixture.close()
}

#[test]
fn unresolved_external_command_is_a_typed_shell_failure() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let output = command(&fixture)?
        .args([
            "--native",
            "--error-format",
            "json",
            "-c",
            "definitely-not-a-bridge-spec-command",
        ])
        .run()
        .expect("resolution failure");
    assert_eq!(output.status.code(), Some(127), "{output:?}");
    assert_eq!(output.capture.stdout, b"", "{output:?}");
    let error: serde_json::Value =
        serde_json::from_slice(&output.capture.stderr).expect("one structured diagnostic");
    assert_eq!(error["code"], "FSH-EXEC-001");
    assert_eq!(error["name"], "CommandNotFound");
    assert_eq!(
        error["message"],
        "Command not found: definitely-not-a-bridge-spec-command"
    );
    fixture.close()
}

#[test]
fn external_pipeline_preserves_bytes_across_stages_without_loss() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let output = command(&fixture)?
        .args([
            "--native",
            "-c",
            r#"/bin/sh -c 'i=0; while [ $i -lt 16384 ]; do printf "0123456789abcde\n"; i=$((i+1)); done' | /bin/cat"#,
        ])
        .run()
        .expect("external pipeline");
    // Inter-stage transport is newline-framed; emit complete records so the
    // expected stream is exact rather than dependent on final-line framing.
    let expected: Vec<u8> = b"0123456789abcde\n"
        .iter()
        .copied()
        .cycle()
        .take(256 * 1024)
        .collect();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(output.capture.stdout, expected, "{output:?}");
    fixture.close()
}

#[test]
fn external_process_inherits_fixture_working_directory_and_environment() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let output = command(&fixture)?
        .args([
            "--native",
            "-c",
            r#"/bin/pwd; /bin/sh -c 'printf %s "$HOME"'"#,
        ])
        .run()
        .expect("external fixture state");
    let mut expected = fixture.workspace().as_os_str().as_bytes().to_vec();
    expected.push(b'\n');
    expected.extend_from_slice(fixture.home().as_os_str().as_bytes());
    assert_output(&output, 0, &expected, b"");
    fixture.close()
}

#[test]
fn nonzero_exit_is_an_observation_without_a_diagnostic() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let output = command(&fixture)?
        .args(["--native", "-c", r#"/bin/sh -c 'exit 3'"#])
        .run()
        .expect("nonzero external observation");
    assert_output(&output, 3, b"", b"");
    fixture.close()
}
