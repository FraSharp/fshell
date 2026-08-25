// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Configuration types for directory listing.
//!
//! NOTE: This is adapted from rrls's args.rs, stripped of CLI argument parsing.
//! Config is constructed programmatically by fshell's builtin ls.

use std::path::PathBuf;

/// File sorting mode.
#[derive(PartialEq, Copy, Clone, Debug)]
pub enum SortMode {
    Name,
    Size,
    Time,
}

/// Git status for a file.
#[derive(PartialEq, Copy, Clone, Default, Debug)]
pub enum GitStatus {
    #[default]
    Clean,
    Modified,
    New,
    Deleted,
    Renamed,
    Ignored,
    Untracked,
    Conflicted,
}

/// Configuration for directory listing.
#[derive(Clone, Debug)]
pub struct Config {
    pub path: PathBuf,
    pub show_all: bool,
    pub list_dirs: bool,
    pub long_listing: bool,
    pub one_per_line: bool,
    pub human_readable: bool,
    pub raw: bool,
    pub show_inode: bool,
    pub sort_mode: SortMode,
    pub reverse_sort: bool,
    pub use_color: bool,
    pub tree: bool,
    pub tree_depth: Option<usize>,
    pub group_directories_first: bool,
    pub show_icons: bool,
    pub git: bool,
    pub dereference: bool,
    pub recursive: bool,
    pub verbose: bool,
}
