// SPDX-License-Identifier: GPL-3.0-or-later
// Generated from the target's own libarchive headers. The only unsafe calls
// live in ArchiveReader in lib.rs; no raw FFI types escape that module.
#![allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    clippy::upper_case_acronyms
)]
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
