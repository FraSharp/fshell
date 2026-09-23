#![cfg(unix)]

use std::io::Read;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use fshell_terminal::input::{CrosstermEventSource, InputPoll};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

const PROBE_ENV: &str = "FSHELL_TERMINAL_PTY_CLOSE_PROBE";
const DIAGNOSTIC_ENV: &str = "FSHELL_TERMINAL_PTY_CLOSE_DIAGNOSTIC";
const READY_MARKER: &[u8] = b"FSHELL_TERMINAL_INPUT_READY";

/// Re-executed under a PTY by `closing_the_pty_unblocks_terminal_input`.
#[test]
fn pty_probe_waits_for_close() {
    if std::env::var_os(PROBE_ENV).is_none() {
        return;
    }

    // The regression is about input EOF, independent of the separate SIGHUP
    // path used by full interactive shells when a PTY master disappears.
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }

    let mut input = CrosstermEventSource::new();
    println!("{}", String::from_utf8_lossy(READY_MARKER));
    let _ = std::io::Write::flush(&mut std::io::stdout());
    match input.poll(Duration::from_secs(30)) {
        Ok(InputPoll::Closed) => std::process::exit(0),
        outcome => {
            if let Some(path) = std::env::var_os(DIAGNOSTIC_ENV) {
                let _ = std::fs::write(path, format!("expected Closed, got {outcome:?}"));
            }
            std::process::exit(2);
        }
    }
}

#[test]
fn closing_the_pty_unblocks_terminal_input() {
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open PTY");

    let executable = std::env::current_exe().expect("test executable path");
    let diagnostic_path = std::env::temp_dir().join(format!(
        "fshell-terminal-pty-close-{}.txt",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&diagnostic_path);
    let mut command = CommandBuilder::new(executable);
    command.arg("--exact");
    command.arg("pty_probe_waits_for_close");
    command.arg("--nocapture");
    command.env(PROBE_ENV, "1");
    command.env(DIAGNOSTIC_ENV, &diagnostic_path);
    command.env("TERM", "xterm-256color");

    let mut child = pair.slave.spawn_command(command).expect("spawn PTY probe");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("clone PTY reader");
    let (ready_tx, ready_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0; 512];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    output.extend_from_slice(&buffer[..read]);
                    if output
                        .windows(READY_MARKER.len())
                        .any(|window| window == READY_MARKER)
                    {
                        // Release this cloned master handle before telling the
                        // parent it is safe to close the final master handle.
                        drop(reader);
                        let _ = ready_tx.send(output);
                        break;
                    }
                }
            }
        }
    });

    let output = ready_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("probe did not reach terminal input polling");
    assert!(
        output
            .windows(READY_MARKER.len())
            .any(|window| window == READY_MARKER),
        "probe readiness marker was missing: {}",
        String::from_utf8_lossy(&output)
    );

    // Closing the final master descriptor produces a true zero-byte read on
    // the slave without relying on SIGHUP to terminate the probe.
    drop(pair.master);

    let deadline = Instant::now() + Duration::from_secs(3);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll PTY probe") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("input source did not return after PTY closure");
        }
        thread::sleep(Duration::from_millis(10));
    };

    let diagnostic = std::fs::read_to_string(&diagnostic_path).unwrap_or_default();
    let _ = std::fs::remove_file(&diagnostic_path);
    assert!(
        status.success(),
        "PTY probe failed with {status}: {diagnostic}"
    );
}
