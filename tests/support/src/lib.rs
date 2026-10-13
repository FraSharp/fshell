//! Mechanical support for source-aligned fshell specifications.
//!
//! No fshell initialization, parent environment mutation, hidden artifact builds,
//! or dependency on the legacy harness. Process groups are not a kernel sandbox:
//! descendants creating new groups/sessions require a separate containment adapter.

#![cfg(unix)]
#![deny(clippy::unwrap_used, clippy::panic)]

pub mod process;
