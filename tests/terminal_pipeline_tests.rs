use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Run with terminal stdin, but capture PTY output without blocking the test
/// indefinitely if grep incorrectly falls back to reading the terminal.
fn native_terminal_output(script: &str, cwd: &std::path::Path) -> (bool, String) {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open PTY");
    let config = tempfile::tempdir().expect("temporary config");
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_fsh"));
    command.arg("--native");
    command.arg("--no-color");
    command.arg("-c");
    command.arg(script);
    command.cwd(cwd);
    command.env("FSH_CONFIG_DIR", config.path());
    command.env("TERM", "xterm-256color");
    command.env_remove("CI");
    command.env_remove("FSH_TEST_ENV");
    let mut child = pair.slave.spawn_command(command).expect("spawn fsh");
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().expect("PTY reader");
    let (output_tx, output_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0u8; 1024];
        while let Ok(n) = reader.read(&mut buffer) {
            if n == 0 || output_tx.send(buffer[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut output = Vec::new();
    loop {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "command hung: {script}: {}",
                String::from_utf8_lossy(&output)
            );
        }
        match output_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(bytes) => output.extend(bytes),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let status = child.wait().expect("wait for fsh");
                return (
                    status.success(),
                    String::from_utf8_lossy(&output).replace("\r\n", "\n"),
                );
            }
        }
    }
}

#[test]
fn builtin_grep_defaults_to_cwd_only_without_stream_or_explicit_paths() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("nested")).unwrap();
    std::fs::write(tmp.path().join("nested/match.txt"), "needle from file\n").unwrap();
    std::fs::write(tmp.path().join(".hidden"), "needle hidden\n").unwrap();
    std::fs::write(tmp.path().join("ignored.txt"), "needle ignored\n").unwrap();
    std::fs::write(tmp.path().join(".gitignore"), "ignored.txt\n").unwrap();
    for (script, expected) in [
        ("grep needle", "./nested/match.txt:needle from file\n"),
        ("grep -r needle", "./nested/match.txt:needle from file\n"),
        ("grep -R needle", "./nested/match.txt:needle from file\n"),
        ("grep -c needle", "./nested/match.txt:1\n"),
        ("grep -q needle", ""),
        ("grep -l needle", "./nested/match.txt\n"),
        ("grep needle | count", "1\n"),
        ("grep --hidden --no-ignore needle | count", "3\n"),
        ("cd nested; grep needle", "./match.txt:needle from file\n"),
        ("grep needle nested/match.txt", "needle from file\n"),
        (
            "echo 'needle from pipe' | grep needle",
            "needle from pipe\n",
        ),
        (
            "echo 'needle from pipe' | grep --stdin needle",
            "needle from pipe\n",
        ),
    ] {
        let (success, output) = native_terminal_output(script, tmp.path());
        assert!(success, "{script}: {output}");
        assert_eq!(output, expected, "{script}");
    }
    for script in [
        "grep absent",
        "grep '['",
        "echo unrelated | grep needle",
        "let items = []; $items | grep needle",
        "grep needle nested",
        "grep --stdin needle nested/match.txt",
        "grep --stdin=yes needle",
    ] {
        let (success, output) = native_terminal_output(script, tmp.path());
        assert!(!success, "{script} unexpectedly succeeded: {output}");
        assert!(!output.contains("needle from file"), "{script}: {output}");
    }
    let redirected = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(redirected.path(), "needle redirected\n").unwrap();
    for flags in ["", "--stdin"] {
        let script = format!("grep {flags} needle < '{}'", redirected.path().display());
        let (success, output) = native_terminal_output(&script, tmp.path());
        assert!(success, "{script}: {output}");
        assert_eq!(output, "needle redirected\n");
    }
}

#[test]
fn builtin_grep_ctrl_c_stops_idle_and_partial_terminal_reads() {
    for (mode, options, input) in [
        ("--native", "--stdin", ""),
        ("--native", "--stdin", "unfinished"),
        ("--native", "--stdin -c", ""),
        ("--native", "--stdin -q", ""),
        // POSIX grep must still read terminal input instead of searching cwd.
        ("--posix", "", ""),
    ] {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open PTY");
        let config = tempfile::tempdir().expect("temporary config");
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_fsh"));
        command.arg(mode);
        command.arg("-c");
        command.arg(format!("echo GREP_READY; grep {options} needle"));
        command.env("FSH_CONFIG_DIR", config.path());
        command.env("TERM", "xterm-256color");
        command.env_remove("CI");
        command.env_remove("FSH_TEST_ENV");
        let mut child = pair.slave.spawn_command(command).expect("spawn fsh");
        let mut writer = pair.master.take_writer().expect("PTY writer");
        let mut reader = pair.master.try_clone_reader().expect("PTY reader");
        let (output_tx, output_rx) = mpsc::channel();
        thread::spawn(move || {
            let mut buffer = [0u8; 1024];
            while let Ok(n) = reader.read(&mut buffer) {
                if n == 0 || output_tx.send(buffer[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut output = Vec::new();
        let mut interrupted_at = None;
        loop {
            if Instant::now() >= deadline
                || interrupted_at
                    .is_some_and(|time: Instant| time.elapsed() > Duration::from_secs(3))
            {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "grep {options} did not stop on Ctrl+C (input={input:?}): {}",
                    String::from_utf8_lossy(&output)
                );
            }
            if let Ok(bytes) = output_rx.recv_timeout(Duration::from_millis(50)) {
                output.extend(bytes);
            }
            if interrupted_at.is_none()
                && output
                    .windows(b"GREP_READY".len())
                    .any(|w| w == b"GREP_READY")
            {
                // Let grep enter its read, rather than interrupting before it starts.
                writer
                    .write_all(input.as_bytes())
                    .expect("write partial input");
                thread::sleep(Duration::from_millis(200));
                writer.write_all(b"\x03").expect("send Ctrl+C");
                writer.flush().expect("flush Ctrl+C");
                interrupted_at = Some(Instant::now());
            }
            if child.try_wait().expect("poll fsh").is_some() {
                assert!(interrupted_at.is_some(), "grep exited before Ctrl+C");
                break;
            }
        }
    }
}

#[test]
fn piped_external_stage_can_read_password_from_controlling_terminal() {
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open PTY");

    let script = r#"echo "$(sh -c 'printf substitution-ok')" | cat; sh -c 'printf "TTY_READY\n" >/dev/tty; sleep 0.2; IFS= read -r value </dev/tty; printf "TTY_GOT:%s\n" "$value" >/dev/tty' | cat"#;
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_fsh"));
    command.arg("-c");
    command.arg(script);
    command.env("TERM", "xterm-256color");
    // The terminal handoff under test is exactly what test mode declines, so
    // the ambient CI and FSH_TEST_ENV flags must not reach the child shell.
    command.env_remove("CI");
    command.env_remove("FSH_TEST_ENV");
    let mut child = pair.slave.spawn_command(command).expect("spawn fsh");

    let mut writer = pair.master.take_writer().expect("PTY writer");
    let mut reader = pair.master.try_clone_reader().expect("PTY reader");
    let (output_tx, output_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0u8; 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if output_tx.send(buffer[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut output = Vec::new();
    let mut replied = false;
    let status = loop {
        if Instant::now() >= deadline {
            let _ = child.kill();
            std::mem::forget(child);
            panic!(
                "timed out waiting for pipeline completion; replied={replied}, output={}",
                String::from_utf8_lossy(&output)
            );
        }
        match output_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(bytes) => output.extend(bytes),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(status) = child.try_wait().expect("poll fsh") {
                    break status;
                }
                continue;
            }
        }
        if !replied
            && output
                .windows(b"TTY_READY".len())
                .any(|w| w == b"TTY_READY")
        {
            writer
                .write_all(b"test-secret\n")
                .expect("write terminal input");
            replied = true;
        }
        if output
            .windows(b"TTY_GOT:test-secret".len())
            .any(|w| w == b"TTY_GOT:test-secret")
        {
            if let Some(status) = child.try_wait().expect("poll fsh") {
                break status;
            }
        } else if let Some(status) = child.try_wait().expect("poll fsh") {
            break status;
        }
    };
    let output = String::from_utf8_lossy(&output);
    assert!(
        output.contains("substitution-ok"),
        "command substitution did not flow through the pipeline: {output}"
    );
    assert!(
        !output.contains("FSH-EXEC-005"),
        "external command spawn failed after command substitution: {output}"
    );
    assert!(replied, "pipeline never reached terminal read: {output}");
    assert!(
        output.contains("TTY_GOT:test-secret"),
        "unexpected PTY output: {output}"
    );
    assert!(status.success(), "fsh exited with {status}: {output}");
    assert!(
        !output.contains("Suspended"),
        "pipeline was suspended: {output}"
    );
}
