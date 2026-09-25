//! `argvdump` — a deterministic argv inspector for the conformance suite.
//!
//! Word expansion is the foundation every other compatibility property rests
//! on, and observing it with a real utility is unreliable: the utility's own
//! behaviour (and, for fsh, the existence of a *builtin* of the same name)
//! obscures what the shell actually produced. `argvdump` does nothing but
//! report the arguments it received, so any difference in its output is a
//! difference in the shell's expansion.
//!
//!     $ argvdump '*.rs' "$HOME"
//!     argc=2
//!     arg[0]="*.rs"
//!     arg[1]="/home/user"
//!
//! Two properties are deliberate:
//!
//! * **The output is not JSON.** fsh's native engine decodes JSON on an
//!   external command's stdout and re-encodes it in its own tagged `Val`
//!   representation, so a JSON instrument would be rewritten before any
//!   comparison could see it. A plain line-oriented format survives verbatim
//!   through both engines; see the `native/json-stdout-is-reencoded` case,
//!   which pins that behaviour down separately.
//! * **`argv[0]` is excluded.** Shells disagree about how the command word is
//!   reported and only the argument vector is under test.
//!
//! Arguments are rendered with Rust-style escaping, so embedded quotes, tabs
//! and newlines stay unambiguous and the output is stable for every shell.

use std::fmt::Write as _;
use std::io::Write as _;

fn main() {
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

    let mut out = String::with_capacity(32 + argv.iter().map(String::len).sum::<usize>());
    let _ = writeln!(out, "argc={}", argv.len());
    for (index, arg) in argv.iter().enumerate() {
        let _ = writeln!(out, "arg[{index}]={}", quoted(arg));
    }

    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    if handle.write_all(out.as_bytes()).is_err() || handle.flush().is_err() {
        std::process::exit(1);
    }
}

/// Render `value` as an unambiguous, escaped, double-quoted literal.
fn quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 || ch as u32 == 0x7f => {
                let _ = write!(out, "\\x{:02x}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}
