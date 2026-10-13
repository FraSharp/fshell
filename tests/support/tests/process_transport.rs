//! Fault probes for transport guarantees, independent of the shell implementation.

use fshell_test_support::process::{
    ExecutionLimits, OutputStream, ProcessCommand, ProcessErrorKind,
};
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::process::ExitStatusExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn command(dir: &tempfile::TempDir, operation: &str) -> ProcessCommand {
    ProcessCommand::new(env!("CARGO_BIN_EXE_fshell-process-probe"), dir.path()).arg(operation)
}

fn short_deadline() -> ExecutionLimits {
    ExecutionLimits {
        deadline: Duration::from_millis(500),
        ..ExecutionLimits::default()
    }
}

#[test]
fn binary_streams_are_captured_without_lossy_decoding() {
    let dir = tempfile::tempdir().expect("scratch directory");
    let output = command(&dir, "bytes").run().expect("probe execution");
    assert!(output.status.success());
    assert_eq!(output.capture.stdout, [0, 255, 128, b'\n']);
    assert_eq!(output.capture.stderr, [254, b'\n']);
}

#[test]
fn arguments_preserve_empty_whitespace_and_non_unicode_values() {
    let dir = tempfile::tempdir().expect("scratch directory");
    let args = [
        OsString::from(""),
        OsString::from("a b\nc"),
        OsString::from_vec(vec![255, 128]),
    ];
    let output = command(&dir, "argv")
        .args(&args)
        .run()
        .expect("probe execution");
    let expected: Vec<u8> = args
        .iter()
        .flat_map(|arg| {
            let bytes = arg.as_bytes();
            (bytes.len() as u64)
                .to_be_bytes()
                .into_iter()
                .chain(bytes.iter().copied())
        })
        .collect();
    assert!(output.status.success());
    assert_eq!(output.capture.stdout, expected);
    assert_eq!(output.capture.stderr, []);
}

#[test]
fn parent_environment_is_not_inherited() {
    let dir = tempfile::tempdir().expect("scratch directory");
    // PATH is an ordinary parent variable, not a specially chosen absent key.
    assert!(
        std::env::var_os("PATH").is_some(),
        "test runner must have a PATH"
    );
    let output = command(&dir, "environment")
        .arg("PATH")
        .run()
        .expect("probe execution");
    assert!(output.status.success());
    assert_eq!(output.capture.stdout, [0]);
}

#[test]
fn explicit_environment_preserves_non_unicode_values() {
    let dir = tempfile::tempdir().expect("scratch directory");
    let value = OsString::from_vec(vec![255, b'x']);
    let output = command(&dir, "environment")
        .arg("PROBE_VALUE")
        .env("PROBE_VALUE", value)
        .run()
        .expect("probe execution");
    assert!(output.status.success());
    assert_eq!(output.capture.stdout, [1, 255, b'x']);
}

#[test]
fn working_directory_is_child_only() {
    let original = std::env::current_dir().expect("parent cwd");
    let dir = tempfile::tempdir().expect("scratch directory");
    let output = command(&dir, "cwd").run().expect("probe execution");
    assert!(output.status.success());
    // macOS temp paths may have a /var -> /private/var symlink.
    assert_eq!(
        output.capture.stdout,
        dir.path()
            .canonicalize()
            .expect("canonical scratch path")
            .as_os_str()
            .as_bytes()
    );
    assert_eq!(std::env::current_dir().expect("parent cwd"), original);
}

#[test]
fn empty_stdin_is_eof_not_an_inherited_terminal() {
    let dir = tempfile::tempdir().expect("scratch directory");
    let output = command(&dir, "echo-input").run().expect("probe execution");
    assert!(output.status.success());
    assert_eq!(output.capture.stdout, []);
    assert_eq!(output.capture.stdin_written, 0);
}

#[test]
fn simultaneous_large_input_and_both_outputs_do_not_deadlock() {
    let dir = tempfile::tempdir().expect("scratch directory");
    let size = 1024 * 1024;
    let output = command(&dir, "duplex")
        .arg(size.to_string())
        .stdin(vec![b'I'; size])
        .run()
        .expect("duplex execution");
    assert!(output.status.success());
    assert_eq!(output.capture.stdout, vec![b'O'; size]);
    assert_eq!(output.capture.stderr, vec![b'E'; size]);
    assert_eq!(output.capture.stdin_written, size);
}

#[test]
fn nonzero_exit_is_a_completed_observation() {
    let dir = tempfile::tempdir().expect("scratch directory");
    let output = command(&dir, "exit")
        .arg("37")
        .run()
        .expect("probe execution");
    assert_eq!(output.status.code(), Some(37));
}

#[test]
fn signal_is_not_flattened_into_an_exit_code() {
    let dir = tempfile::tempdir().expect("scratch directory");
    let output = command(&dir, "signal").run().expect("probe execution");
    assert_eq!(output.status.signal(), Some(libc::SIGTERM));
    assert_eq!(output.status.code(), None);
}

#[test]
fn deadline_covers_a_child_that_never_reads_large_stdin() {
    let dir = tempfile::tempdir().expect("scratch directory");
    let error = command(&dir, "block-input")
        .stdin(vec![b'I'; 1024 * 1024])
        .limits(short_deadline())
        .run()
        .expect_err("blocked input must time out");
    assert!(matches!(error.kind, ProcessErrorKind::Deadline));
    assert_eq!(error.capture.stdout, b"READY\n");
    assert!(error.capture.stdin_written < 1024 * 1024);
    assert_eq!(
        error.status.expect("reaped child").signal(),
        Some(libc::SIGKILL)
    );
    assert!(error.cleanup_errors.is_empty(), "{error}");
}

#[test]
fn early_stdin_closure_is_an_error_with_delivery_progress() {
    let dir = tempfile::tempdir().expect("scratch directory");
    let error = command(&dir, "close-input")
        .stdin(vec![b'I'; 1024 * 1024])
        .run()
        .expect_err("input delivery must fail");
    assert!(matches!(error.kind, ProcessErrorKind::InputClosed));
    assert!(error.capture.stdin_written < 1024 * 1024);
    assert_eq!(
        error.status.expect("reaped child").signal(),
        Some(libc::SIGKILL)
    );
    assert!(error.cleanup_errors.is_empty(), "{error}");
}

#[test]
fn output_limits_fail_with_the_retained_prefix_for_either_stream() {
    for (operation, stream) in [
        ("stdout", OutputStream::Stdout),
        ("stderr", OutputStream::Stderr),
    ] {
        let dir = tempfile::tempdir().expect("scratch directory");
        let error = command(&dir, operation)
            .arg("65536")
            .limits(ExecutionLimits {
                output_bytes: 1024,
                ..ExecutionLimits::default()
            })
            .run()
            .expect_err("overflow must fail");
        assert!(matches!(error.kind, ProcessErrorKind::OutputLimit(actual) if actual == stream));
        let bytes = if stream == OutputStream::Stdout {
            &error.capture.stdout
        } else {
            &error.capture.stderr
        };
        assert_eq!(
            bytes,
            &vec![
                if stream == OutputStream::Stdout {
                    b'O'
                } else {
                    b'E'
                };
                1024
            ]
        );
        assert!(error.status.is_some());
        assert!(error.cleanup_errors.is_empty(), "{error}");
    }
}

#[test]
fn child_exit_is_not_mistaken_for_eof_when_a_descendant_holds_pipes() {
    let dir = tempfile::tempdir().expect("scratch directory");
    let error = command(&dir, "hold-pipes")
        .limits(short_deadline())
        .run()
        .expect_err("retained pipe must fail");
    assert!(matches!(error.kind, ProcessErrorKind::Deadline));
    assert_eq!(error.status.expect("direct child status").code(), Some(0));
    let text = std::str::from_utf8(&error.capture.stdout).expect("probe protocol is UTF-8");
    let pid: i32 = text
        .trim()
        .strip_prefix("DESCENDANT:")
        .expect("descendant identity")
        .parse()
        .expect("PID");
    // Cleanup kills the launch group even after the leader has exited. A killed
    // orphan may briefly be a zombie until the OS reaper collects it.
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        // SAFETY: signal 0 inspects a PID, it sends no signal.
        if unsafe { libc::kill(pid, 0) } < 0 {
            break;
        }
        #[cfg(target_os = "linux")]
        if std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            stat.rsplit_once(") ")
                .is_some_and(|(_, fields)| fields.starts_with('Z'))
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "descendant {pid} survived launch-group cleanup"
        );
        std::thread::yield_now();
    }
    assert!(error.cleanup_errors.is_empty(), "{error}");
}

#[test]
fn cancellation_after_a_start_handshake_kills_and_reaps_the_child() {
    let dir = tempfile::tempdir().expect("scratch directory");
    let marker = dir.path().join("ready");
    let flag = Arc::new(AtomicBool::new(false));
    let command = command(&dir, "ready-file")
        .arg(&marker)
        .cancellation(flag.clone());
    let worker = std::thread::spawn(move || command.run());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() && Instant::now() < deadline && !worker.is_finished() {
        std::thread::yield_now();
    }
    let ready = marker.exists();
    flag.store(true, Ordering::Release);
    let error = worker
        .join()
        .expect("transport worker")
        .expect_err("cancelled process must fail");
    assert!(ready, "probe did not reach startup handshake: {error}");
    assert!(matches!(error.kind, ProcessErrorKind::Cancelled));
    assert_eq!(
        error.status.expect("reaped child").signal(),
        Some(libc::SIGKILL)
    );
    assert!(error.cleanup_errors.is_empty(), "{error}");
}

#[test]
fn pre_cancelled_execution_does_not_spawn() {
    let dir = tempfile::tempdir().expect("scratch directory");
    let error = command(&dir, "idle")
        .cancellation(Arc::new(AtomicBool::new(true)))
        .run()
        .expect_err("cancelled before spawn");
    assert!(matches!(error.kind, ProcessErrorKind::Cancelled));
    assert!(error.status.is_none());
}

#[test]
fn relative_paths_and_zero_limits_are_rejected_before_spawn() {
    let dir = tempfile::tempdir().expect("scratch directory");
    let error = ProcessCommand::new("relative-program", dir.path())
        .run()
        .expect_err("reject relative executable");
    assert!(matches!(error.kind, ProcessErrorKind::InvalidInvocation(_)));
    let error = command(&dir, "idle")
        .limits(ExecutionLimits {
            deadline: Duration::ZERO,
            ..ExecutionLimits::default()
        })
        .run()
        .expect_err("reject zero deadline");
    assert!(matches!(error.kind, ProcessErrorKind::InvalidInvocation(_)));
}

#[test]
fn spawn_failure_is_an_infrastructure_error() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("scratch directory");
    let executable = dir.path().join("invalid-executable");
    // Existing and executable, but its interpreter does not exist.
    std::fs::write(&executable, b"#!/nonexistent/fshell-test-interpreter\n")
        .expect("invalid executable");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
        .expect("permissions");
    let error = ProcessCommand::new(&executable, dir.path())
        .run()
        .expect_err("spawn must fail");
    assert!(matches!(
        error.kind,
        ProcessErrorKind::Io {
            operation: "spawn",
            ..
        }
    ));
    assert!(error.status.is_none());
}
