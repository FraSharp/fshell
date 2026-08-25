// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

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
