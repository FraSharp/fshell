//! Actual binary startup, frontend dispatch, diagnostics and observable effects.

use fshell_specifications::shell_executable;
use fshell_test_support::fixture::ShellFixture;
use fshell_test_support::process::{ProcessCommand, ProcessOutput};
use std::io;

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
fn native_command_prints_the_value_and_uses_success_status() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let output = command(&fixture)?
        .args(["--native", "-c", "2 + 3 * 4"])
        .run()
        .expect("native command execution");
    assert_output(&output, 0, b"14\n", b"");
    fixture.close()
}

#[test]
fn native_declaration_is_silent() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let output = command(&fixture)?
        .args(["--native", "-c", "let answer = 42"])
        .run()
        .expect("native declaration");
    assert_output(&output, 0, b"", b"");
    fixture.close()
}

#[test]
fn posix_exit_stops_the_script_and_preserves_the_requested_status() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let output = command(&fixture)?
        .args([
            "--posix",
            "-c",
            "printf 'before\n'; exit 37; printf 'after\n'",
        ])
        .run()
        .expect("POSIX exit execution");
    assert_output(&output, 37, b"before\n", b"");
    fixture.close()
}

#[test]
fn native_math_error_has_a_structured_diagnostic_and_nonzero_status() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let output = command(&fixture)?
        .args(["--native", "--error-format", "json", "-c", "9 / 0"])
        .run()
        .expect("native error execution");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(output.capture.stdout, b"");
    let error: serde_json::Value =
        serde_json::from_slice(&output.capture.stderr).expect("one structured diagnostic");
    assert_eq!(error["code"], "FSH-MATH-001");
    assert_eq!(error["category"], "arithmetic");
    assert_eq!(error["message"], "Division by zero");
    assert!(
        !error["labels"]
            .as_array()
            .expect("diagnostic labels")
            .is_empty()
    );
    fixture.close()
}

#[test]
fn malformed_native_declaration_is_not_reinterpreted_as_a_posix_command() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let output = command(&fixture)?
        .args(["--error-format", "json", "-c", "let answer ="])
        .run()
        .expect("malformed native input");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(output.capture.stdout, b"");
    let error: serde_json::Value =
        serde_json::from_slice(&output.capture.stderr).expect("one parse diagnostic");
    assert_eq!(error["code"], "FSH-PARSE-003");
    fixture.close()
}

#[test]
fn native_file_execution_preserves_spaced_and_empty_positional_arguments() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let file = fixture.create_file("arguments.fsh", b"echo \"$1\"; echo \"$2\"\n")?;
    let output = command(&fixture)?
        .arg("--native")
        .arg(file)
        .args(["a b", ""])
        .run()
        .expect("native script execution");
    assert_output(&output, 0, b"a b\n\n", b"");
    fixture.close()
}

#[test]
fn posix_shebang_selects_the_file_frontend_without_a_mode_flag() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let file = fixture.create_file("script.sh", b"#!/bin/sh\nprintf 'shebang-ok\\n'\n")?;
    let trace = fixture.workspace().join("routing");
    let output = command(&fixture)?
        .env("FSH_ENGINE_TRACE", &trace)
        .arg(file)
        .run()
        .expect("shebang script execution");
    assert_output(&output, 0, b"shebang-ok\n", b"");
    assert_eq!(
        fixture.read_file("routing")?,
        b"engine=posix reason=posix-shebang\n"
    );
    fixture.close()
}

#[test]
fn auto_mode_keeps_native_safe_input_in_the_native_frontend() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let trace = fixture.workspace().join("routing");
    let output = command(&fixture)?
        .env("FSH_ENGINE_TRACE", trace)
        .args(["-c", "let answer = 42; echo $answer"])
        .run()
        .expect("auto native execution");
    assert_output(&output, 0, b"42\n", b"");
    assert_eq!(
        fixture.read_file("routing")?,
        b"engine=native reason=native-safe\n"
    );
    fixture.close()
}

#[test]
fn posix_fallback_is_decided_before_any_side_effect_executes() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let trace = fixture.workspace().join("routing");
    let script = "printf 'once\\n' >> effect; for value in one; do printf '%s\\n' \"$value\"; done";
    let output = command(&fixture)?
        .env("FSH_ENGINE_TRACE", trace)
        .args(["-c", script])
        .run()
        .expect("auto POSIX execution");
    assert_output(&output, 0, b"one\n", b"");
    assert_eq!(
        fixture.read_file("effect")?,
        b"once\n",
        "dispatch must not execute the prefix twice"
    );
    assert_eq!(
        fixture.read_file("routing")?,
        b"engine=posix reason=native-parse-failed\n"
    );
    fixture.close()
}

#[test]
fn posix_read_consumes_explicit_stdin_without_losing_internal_spaces() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let output = command(&fixture)?
        .args([
            "--posix",
            "-c",
            "read -r value; printf '[%s]\\n' \"$value\"",
        ])
        .stdin(b"alpha  beta\n".to_vec())
        .run()
        .expect("POSIX stdin execution");
    assert_output(&output, 0, b"[alpha  beta]\n", b"");
    assert_eq!(output.capture.stdin_written, b"alpha  beta\n".len());
    fixture.close()
}

#[test]
fn posix_redirection_keeps_stdout_stderr_and_file_effects_distinct() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let output = command(&fixture)?
        .args([
            "--posix",
            "-c",
            "printf 'file' > effect; printf 'out'; printf 'err' >&2",
        ])
        .run()
        .expect("POSIX redirection execution");
    assert_output(&output, 0, b"out", b"err");
    assert_eq!(fixture.read_file("effect")?, b"file");
    fixture.close()
}

#[test]
fn empty_command_exits_without_entering_interactive_startup() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let output = command(&fixture)?
        .args(["-c", ""])
        .run()
        .expect("empty command execution");
    assert_output(&output, 0, b"", b"");
    assert!(!fixture.state().join("history.db").exists());
    fixture.close()
}
