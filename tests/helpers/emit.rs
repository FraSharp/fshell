//! `emit` — a deterministic stdout/stderr/exit-status emitter for conformance tests.
//!
//! Redirection, stream-merging and exit-status cases need a command whose
//! output and status are entirely under the test's control. Real utilities are
//! unsuitable: `ls` is a *builtin* in fsh with its own error wording, so its
//! stderr does not match the `ls` the reference shell runs, and the comparison
//! would flag a wording difference instead of a redirection difference.
//!
//!     $ emit --stdout out --stderr err --exit 3
//!
//! Writes `out` and a trailing newline to stdout, `err` and a trailing newline
//! to stderr, then exits with the requested status. Streams are always written
//! in that order so a merged stream (`2>&1`) has a stable, comparable layout.

use std::io::Write as _;

fn main() {
    let mut stdout_text: Option<String> = None;
    let mut stderr_text: Option<String> = None;
    let mut exit_code: i32 = 0;

    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--stdout" => stdout_text = args.next(),
            "--stderr" => stderr_text = args.next(),
            "--exit" => {
                exit_code = args
                    .next()
                    .and_then(|value| value.parse::<i32>().ok())
                    .unwrap_or(0);
            }
            other => {
                eprintln!("emit: unrecognised argument {other:?}");
                std::process::exit(64);
            }
        }
    }

    if let Some(text) = stdout_text {
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        let _ = writeln!(handle, "{text}");
        let _ = handle.flush();
    }

    if let Some(text) = stderr_text {
        let stderr = std::io::stderr();
        let mut handle = stderr.lock();
        let _ = writeln!(handle, "{text}");
        let _ = handle.flush();
    }

    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}
