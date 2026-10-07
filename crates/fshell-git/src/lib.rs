// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Fshell-owned Git metadata and status types backed by gitoxide.
//!
//! Repository discovery, object access, references, index decoding, ignore
//! matching, and status comparison use `gix`. Consumers depend on this crate's
//! domain types rather than gitoxide types. The supported repository format is
//! non-bare SHA-1; status reports sparse indexes as an explicit unsupported
//! feature because the selected gitoxide status diff cannot compare them.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::panic))]
#![allow(clippy::manual_repeat_n)]
pub mod branch;
pub mod config;
pub mod head;
pub mod ignore;
pub mod index;
pub mod objects;
pub mod refs;
pub mod repo;
pub mod status;

#[cfg(test)]
mod test_support;
