//! Mechanical support for source-aligned fshell specifications.
//!
//! No fshell initialization, parent environment mutation or hidden artifact builds.
//! Process groups are not a kernel sandbox:
//! descendants creating new groups/sessions require a separate containment adapter.

#![cfg(unix)]
#![deny(clippy::unwrap_used, clippy::panic)]

pub mod fixture;
pub mod process;
