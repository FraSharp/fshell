use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

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

    let script = r#"echo "$(command -v fsh)" | cat; sh -c 'printf "TTY_READY\n" >/dev/tty; sleep 0.2; IFS= read -r value </dev/tty; printf "TTY_GOT:%s\n" "$value" >/dev/tty' | cat"#;
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_fsh"));
    command.arg("-c");
    command.arg(script);
    command.env("TERM", "xterm-256color");
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
        output.lines().any(|line| line.trim_end().ends_with("/fsh")),
        "command substitution did not emit the fsh path: {output}"
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
