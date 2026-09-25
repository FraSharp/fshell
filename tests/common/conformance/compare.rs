//! Structured comparison of two outcomes, and classification of a difference.
//!
//! The class matters as much as the mismatch: an fsh that explicitly rejects
//! syntax it cannot run (`UnsupportedSyntax`) is in a different state from one
//! that runs it and produces the wrong effect. Only the latter is a correctness
//! bug, and the spec requires the two be told apart.

use super::outcome::{Outcome, Termination};

/// Why two outcomes differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// The shell exceeded its deadline.
    Timeout,
    /// The shell was killed by a signal.
    SignalMismatch,
    /// The shell could not be run at all.
    Crash,
    /// Exit statuses differ.
    ExitStatusMismatch,
    /// Standard output differs.
    StdoutMismatch,
    /// Standard error differs.
    StderrMismatch,
    /// Captured file contents differ.
    FilesystemMismatch,
    /// fsh reported the syntax as unsupported; the reference shell ran it.
    UnsupportedSyntax,
}

impl FailureClass {
    pub fn label(self) -> &'static str {
        match self {
            FailureClass::Timeout => "Timeout",
            FailureClass::SignalMismatch => "SignalMismatch",
            FailureClass::Crash => "Crash",
            FailureClass::ExitStatusMismatch => "ExitStatusMismatch",
            FailureClass::StdoutMismatch => "StdoutMismatch",
            FailureClass::StderrMismatch => "StderrMismatch",
            FailureClass::FilesystemMismatch => "FilesystemMismatch",
            FailureClass::UnsupportedSyntax => "UnsupportedSyntax",
        }
    }
}

/// A classified difference between expected and actual outcomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub class: FailureClass,
    pub detail: String,
}

impl Mismatch {
    fn new(class: FailureClass, detail: impl Into<String>) -> Self {
        Self {
            class,
            detail: detail.into(),
        }
    }
}

/// Substrings that indicate fsh refused syntax rather than mis-executing it.
const UNSUPPORTED_MARKERS: &[&str] = &[
    "not supported",
    "unsupported",
    "not implemented",
    "unimplemented",
    "posix parse error",
];

/// Compare `actual` against `expected`, returning the first difference found.
///
/// Ordering is deliberate, and follows the priority the spec sets:
///
/// 1. a timeout is reported as a timeout, never flattened into a stream diff;
/// 2. an explicit unsupported-syntax rejection is reported as a capability gap
///    rather than as a wrong result — provided the reference shell *succeeded*,
///    which is what makes it a gap;
/// 3. then termination, stdout, stderr and captured files, in that order.
pub fn compare(expected: &Outcome, actual: &Outcome, compare_stderr: bool) -> Option<Mismatch> {
    if actual.termination == Termination::TimedOut {
        return Some(Mismatch::new(
            FailureClass::Timeout,
            format!(
                "fsh did not finish within {:?}; expected {}",
                actual.duration,
                expected.termination.label()
            ),
        ));
    }

    if let Some(gap) = unsupported_syntax_gap(expected, actual) {
        return Some(gap);
    }

    if actual.termination != expected.termination {
        let detail = format!(
            "termination differs: expected {}, got {}",
            expected.termination.label(),
            actual.termination.label()
        );
        // A shell killed by a signal before producing anything is a crash, not
        // merely a different exit code.
        if matches!(actual.termination, Termination::Signaled(_)) && actual.stdout.is_empty() {
            return Some(Mismatch::new(FailureClass::Crash, detail));
        }
        let class = match actual.termination {
            Termination::Signaled(_) => FailureClass::SignalMismatch,
            _ => FailureClass::ExitStatusMismatch,
        };
        return Some(Mismatch::new(class, detail));
    }

    if expected.stdout != actual.stdout {
        return Some(Mismatch::new(
            FailureClass::StdoutMismatch,
            format!(
                "stdout differs:\n    expected {:?}\n    actual   {:?}",
                expected.stdout, actual.stdout
            ),
        ));
    }

    if compare_stderr && expected.stderr != actual.stderr {
        return Some(Mismatch::new(
            FailureClass::StderrMismatch,
            format!(
                "stderr differs:\n    expected {:?}\n    actual   {:?}",
                expected.stderr, actual.stderr
            ),
        ));
    }

    if expected.files != actual.files {
        return Some(Mismatch::new(
            FailureClass::FilesystemMismatch,
            format!(
                "captured files differ:\n    expected {:?}\n    actual   {:?}",
                expected.files, actual.files
            ),
        ));
    }

    None
}

fn unsupported_marker(stderr: &str) -> Option<&'static str> {
    let lower = stderr.to_lowercase();
    UNSUPPORTED_MARKERS
        .iter()
        .copied()
        .find(|marker| lower.contains(marker))
}

/// An fsh refusal to run syntax the reference shell ran successfully.
///
/// This is a *capability gap*: fsh declined the syntax rather than
/// mis-executing it, which the spec treats as a different class of problem from
/// a wrong result. Only counted when the reference succeeded — if it failed too,
/// there is no gap.
fn unsupported_syntax_gap(expected: &Outcome, actual: &Outcome) -> Option<Mismatch> {
    if expected.exit_code() != Some(0) {
        return None;
    }
    let marker = unsupported_marker(&actual.stderr)?;
    Some(Mismatch::new(
        FailureClass::UnsupportedSyntax,
        format!("fsh reported unsupported syntax ({marker:?}) where the reference shell succeeded"),
    ))
}
