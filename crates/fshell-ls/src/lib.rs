// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::panic))]
#![cfg(unix)]

use std::io::{self, BufWriter, Write};

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
pub use scan::{
    GitStatusCache, ListResult, RootIdentity, list_dir, list_dir_with_git_status_cache,
};

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
        return tree::print_tree_with_result(config, result, check_read_dir);
    }

    let capacity = if config.long_listing {
        crate::utils::calculate_output_buffer_size(&result.entries, &result.arena, true)
    } else {
        crate::utils::calculate_output_buffer_size(&result.entries, &result.arena, false)
            .max(crate::utils::determine_buffer_size(result.entries.len()))
    };
    let stdout = io::stdout();
    let mut out = BufWriter::with_capacity(capacity, stdout.lock());
    let rendered = render_to_with_width(
        result,
        config,
        &mut out,
        render::get_terminal_width(),
        check_read_dir,
    );
    match rendered {
        Ok(()) => out.flush(),
        Err(err) => Err(err),
    }
}

/// Render a listing to the supplied writer without adding output buffering.
pub fn render_to<W, F>(
    result: &ListResult,
    config: &Config,
    out: &mut W,
    check_read_dir: F,
) -> io::Result<()>
where
    W: Write + ?Sized,
    F: Fn(&std::path::Path) -> bool,
{
    render_to_with_width(
        result,
        config,
        out,
        render::get_terminal_width(),
        check_read_dir,
    )
}

/// Render with an explicit terminal width, allowing deterministic layout in
/// non-terminal consumers and benchmarks.
pub fn render_to_with_width<W, F>(
    result: &ListResult,
    config: &Config,
    out: &mut W,
    term_width: usize,
    check_read_dir: F,
) -> io::Result<()>
where
    W: Write + ?Sized,
    F: Fn(&std::path::Path) -> bool,
{
    let entries = &result.entries;
    let arena = &result.arena;

    if config.tree {
        tree::render_tree_with_result(config, result, out, check_read_dir)
    } else if config.long_listing {
        render::render_long_listing_to(
            entries,
            arena,
            config.use_color,
            config.show_inode,
            config.human_readable,
            config.git,
            out,
        )
    } else if config.one_per_line {
        render::render_one_per_line_to(entries, arena, config.use_color, config.show_inode, out)
    } else {
        let use_icons = config.show_icons && entries.len() <= 500;
        render::render_columns_to(
            entries,
            arena,
            term_width,
            config.use_color,
            config.show_inode,
            use_icons,
            out,
        )
    }
}
