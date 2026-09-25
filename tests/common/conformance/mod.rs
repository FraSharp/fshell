//! Differential conformance harness.
//!
//! The point of this module is to make shell compatibility *measurable* rather
//! than discovered: a case is executed in a deterministic fixture, judged
//! against a reference shell (or a literal expectation for fsh-native
//! behaviour), and any difference is classified. Bugs found this way become
//! permanent, strict regressions.
//!
//! Layout:
//!
//! * [`fixture`] — the reproducible sandbox and its controlled environment.
//! * [`outcome`] — structured capture of one invocation (streams, status, files).
//! * [`oracle`] — discovery of the reference shells.
//! * [`case`] — what a conformance case is.
//! * [`compare`] — structured comparison and failure classification.
//! * [`runner`] — expectation resolution, execution, and reporting.
//!
//! fsh has two engines and they are tested as two engines. A case names the
//! [`Engine`]s it should run in, so a defect in the native engine is never
//! hidden behind a correct POSIX result (or the reverse).

pub mod case;
pub mod compare;
pub mod fixture;
pub mod oracle;
pub mod outcome;
pub mod runner;

pub use case::{Case, Engine, KnownFailure, Oracle};
pub use compare::{FailureClass, Mismatch};
pub use fixture::Fixture;
pub use oracle::{ReferenceShell, posix_references};
pub use outcome::{Invocation, Outcome, Termination, capture};
pub use runner::{CaseReport, EngineResult, assert_case, assert_suite, run_case};
