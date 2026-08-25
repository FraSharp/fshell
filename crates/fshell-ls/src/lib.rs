// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::panic))]
#![cfg(unix)]

use std::io;

pub mod args;
pub mod colors;
pub mod file;
pub mod platform;
pub mod render;
pub mod scan;
pub mod tree;
pub mod utils;

// Convenience re-exports for the most common API surface
pub use args::{Config, GitStatus, SortMode};
pub use file::{Entry, FileInfo, Metadata};
pub use scan::{ListResult, list_dir};

/// Pretty-print directory listing results to stdout.
///
/// Dispatches to column view, long listing, or tree format based on config.
/// This is the terminal rendering path — use `list_dir()` first, then pass
/// the result to this function when writing to a terminal.
///
/// NOTE: In tree mode (`config.tree`), the tree renderer opens directories
/// itself via libc calls. Capability checks for both the root path and
/// all subdirectories are enforced via the `check_read_dir` closure.
pub fn render<F>(result: &ListResult, config: &Config, check_read_dir: F) -> io::Result<()>
where
    F: Fn(&std::path::Path) -> bool,
{
    if config.tree {
        return tree::print_tree(config, check_read_dir);
    }

    let ListResult { entries, arena } = result;

    if config.long_listing {
        render::print_long_listing(
            entries,
            arena,
            config.use_color,
            config.show_inode,
            config.human_readable,
            config.git,
        )
    } else if config.one_per_line {
        render::print_one_per_line(entries, arena, config.use_color, config.show_inode)
    } else {
        let term_width = render::get_terminal_width();
        let use_icons = config.show_icons && entries.len() <= 500;
        render::print_columns(
            entries,
            arena,
            term_width,
            config.use_color,
            config.show_inode,
            use_icons,
        )
    }
}
