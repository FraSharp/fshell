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
use std::io::Write;

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
    if write_output(&mut handle, out.as_bytes()).is_err() {
        std::process::exit(1);
    }
}

fn write_output<W: Write>(writer: &mut W, output: &[u8]) -> std::io::Result<()> {
    writer.write_all(output)?;
    writer.flush()
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

#[cfg(test)]
mod tests {
    use super::{quoted, write_output};
    use std::io::{self, Write};

    #[test]
    fn quoted_escapes_control_characters_and_special_delimiters() {
        assert_eq!(
            quoted("\0\u{1f}\u{7f}\t\n\r\\\""),
            "\"\\x00\\x1f\\x7f\\t\\n\\r\\\\\\\"\""
        );
    }

    #[test]
    fn write_output_returns_write_failure_without_relying_on_flush_failure() {
        struct WriteFails;

        impl Write for WriteFails {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "write failed"))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let error = write_output(&mut WriteFails, b"output")
            .expect_err("write failure must be returned even if flush succeeds");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn write_output_returns_flush_failure_after_a_successful_write() {
        struct FlushFails;

        impl Write for FlushFails {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::other("flush failed"))
            }
        }

        let error =
            write_output(&mut FlushFails, b"output").expect_err("flush failure must be returned");
        assert_eq!(error.kind(), io::ErrorKind::Other);
    }
}
