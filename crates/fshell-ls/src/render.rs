// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![allow(clippy::unnecessary_cast)]
use crate::colors::{BLUE, CYAN, GREEN, RESET};
use crate::file::FileInfo;
use crate::utils::{
    calculate_output_buffer_size, determine_buffer_size, escape_name, format_size,
    format_time_with_now, get_group_name, get_mode_string, get_user_name,
};
use fshell_hash::FxHashMap;
use libc::{S_IFLNK, S_IFMT, S_IXUSR};
use std::env;
use std::io::{self, BufWriter, Write};
use terminal_size::{Width, terminal_size};
use unicode_width::UnicodeWidthStr;

const SPACES: &str = "                                                                                                                                                                                                                                                                                                                        ";

const ICON_DIR: &str = "d";
const ICON_FILE: &str = " ";
const ICON_EXECUTABLE: &str = "*";

#[inline]
fn is_executable(mode: u32) -> bool {
    mode & ((S_IXUSR | libc::S_IXGRP | libc::S_IXOTH) as u32) != 0
}

fn entry_name<'a>(item: &FileInfo, arena: &'a [u8]) -> io::Result<&'a [u8]> {
    let range = item.entry.range(arena.len()).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "file entry points outside the filename arena",
        )
    })?;
    arena.get(range).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "file entry points outside the filename arena",
        )
    })
}

fn write_left_aligned<W: Write>(out: &mut W, value: &str, width: usize) -> io::Result<()> {
    out.write_all(value.as_bytes())?;
    let mut padding = width.saturating_sub(value.width());
    while padding > 0 {
        let chunk = padding.min(SPACES.len());
        out.write_all(&SPACES.as_bytes()[..chunk])?;
        padding -= chunk;
    }
    Ok(())
}

fn get_icon_for_file(name_bytes: &[u8], is_dir: bool, is_exec: bool) -> &'static str {
    if is_dir {
        return ICON_DIR;
    }

    let name = match std::str::from_utf8(name_bytes) {
        Ok(n) => n,
        Err(_) => return ICON_FILE,
    };

    if is_exec {
        return ICON_EXECUTABLE;
    }

    if name.ends_with(".exe") || name.ends_with(".app") {
        return ICON_EXECUTABLE;
    }

    ICON_FILE
}

const PADDING_SMALL: usize = 2;
const PADDING_MEDIUM: usize = 3;
const PADDING_LARGE: usize = 4;

#[inline]
fn calculate_padding(n: usize) -> usize {
    match n {
        0..20 => PADDING_SMALL,
        20..50 => PADDING_MEDIUM,
        _ => PADDING_LARGE,
    }
}

#[inline]
pub fn num_digits(n: u64) -> usize {
    if n < 10 {
        return 1;
    }
    let mut digits = 1;
    let mut value = n;
    while value >= 10 {
        value /= 10;
        digits += 1;
    }
    digits
}

/// Get the current terminal width in columns.
///
/// Returns the terminal width from termios if available, otherwise falls back
/// to the `COLUMNS` environment variable, or 80 as a final default.
#[inline]
pub fn get_terminal_width() -> usize {
    if let Some((Width(w), _)) = terminal_size() {
        w as usize
    } else {
        env::var("COLUMNS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(80)
    }
}

/// Print files in column format, optimized for terminal width.
///
/// Automatically calculates the optimal number of columns to fit the terminal width.
/// Uses buffered output and pre-allocated buffers for performance.
///
/// # Arguments
///
/// * `items` - File information to display
/// * `arena` - Arena containing filename bytes
/// * `term_width` - Terminal width in columns
/// * `use_color` - Whether to colorize output (directories=blue, symlinks=cyan, executables=green)
/// * `show_inode` - Whether to display inode numbers
pub fn print_columns(
    items: &[FileInfo],
    arena: &[u8],
    term_width: usize,
    use_color: bool,
    show_inode: bool,
    show_icons: bool,
) -> io::Result<()> {
    let stdout = io::stdout();
    let capacity =
        calculate_output_buffer_size(items, arena, false).max(determine_buffer_size(items.len()));
    let mut out = BufWriter::with_capacity(capacity, stdout.lock());
    match render_columns_to(
        items, arena, term_width, use_color, show_inode, show_icons, &mut out,
    ) {
        Ok(()) => out.flush(),
        Err(err) => Err(err),
    }
}

/// Render column output to a caller-provided writer without adding buffering.
pub fn render_columns_to<W: Write + ?Sized>(
    items: &[FileInfo],
    arena: &[u8],
    term_width: usize,
    use_color: bool,
    show_inode: bool,
    show_icons: bool,
    mut out: &mut W,
) -> io::Result<()> {
    if items.is_empty() {
        return Ok(());
    }

    let n = items.len();

    // Calculate item lengths for column fitting
    // Pre-compute name widths to avoid calling String::from_utf8_lossy twice per item
    let mut name_widths = Vec::with_capacity(n);
    let mut display_names = Vec::with_capacity(n);
    let mut item_lens = Vec::with_capacity(n);
    for item in items {
        let name_bytes = entry_name(item, arena)?;
        let display_name = escape_name(name_bytes);
        let name_width = display_name.width();
        name_widths.push(name_width);
        display_names.push(display_name);
        let mut item_len = name_width;
        if show_icons {
            item_len += get_icon_for_file(
                name_bytes,
                item.entry.is_dir(),
                item.metadata
                    .as_ref()
                    .is_some_and(|m| is_executable(m.mode)),
            )
            .width()
                + 1;
        }
        if show_inode {
            if let Some(meta) = &item.metadata {
                item_len += num_digits(meta.ino) + 1;
            } else {
                item_len += 2;
            }
        }
        item_lens.push(item_len);
    }

    let pad = calculate_padding(n);
    let min_item_len = 1;
    let col_width_estimate = min_item_len + pad;
    let max_possible_cols = (term_width / col_width_estimate).max(1).min(n);

    // Optimized column search: pre-allocate column widths buffer
    let mut best_cols = 1;
    let mut best_rows = n;
    let mut best_col_widths = vec![item_lens.iter().copied().max().unwrap_or(1)];

    let mut col_widths = vec![0; max_possible_cols.max(2)]; // Pre-allocate once for all iterations

    for num_cols in (2..=max_possible_cols).rev() {
        let num_rows = n.div_ceil(num_cols);
        col_widths.clear();
        col_widths.resize(num_cols, 0);

        let mut fits = true;
        let mut total_width = 0;

        for (col_idx, col_width) in col_widths.iter_mut().take(num_cols).enumerate() {
            let mut max_width = 0;
            for row_idx in 0..num_rows {
                let idx = col_idx * num_rows + row_idx;
                if idx < n {
                    max_width = max_width.max(item_lens[idx]);
                }
            }
            *col_width = max_width;
            total_width += max_width;
            if col_idx < num_cols - 1 {
                total_width += pad;
            }

            if total_width > term_width {
                fits = false;
                break;
            }
        }

        if fits {
            best_cols = num_cols;
            best_rows = num_rows;
            best_col_widths = col_widths.clone();
            break;
        }
    }

    for row in 0..best_rows {
        for (col, &col_width) in best_col_widths.iter().enumerate() {
            let idx = col * best_rows + row;
            if idx < n {
                let item = items.get(idx).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid column index")
                })?;
                let entry = item.entry;
                let name_bytes = entry_name(item, arena)?;
                let display_name = &display_names[idx];

                let is_exec = item
                    .metadata
                    .as_ref()
                    .map(|m| is_executable(m.mode))
                    .unwrap_or(false);

                let mut printed_len = name_widths[idx];

                if show_icons {
                    let icon = get_icon_for_file(name_bytes, entry.is_dir(), is_exec);
                    out.write_all(icon.as_bytes())?;
                    out.write_all(b" ")?;
                    printed_len += icon.width() + 1;
                }

                if show_inode {
                    if let Some(meta) = &item.metadata {
                        write!(out, "{} ", meta.ino)?;
                        printed_len += num_digits(meta.ino) + 1;
                    } else {
                        out.write_all(b"? ")?;
                        printed_len += 2;
                    }
                }

                if use_color {
                    if entry.is_dir() {
                        BLUE.write_to(&mut out)?;
                        out.write_all(display_name.as_bytes())?;
                        out.write_all(RESET.as_bytes())?;
                    } else if let Some(meta) = &item.metadata {
                        let mode = meta.mode;
                        if (mode & (S_IFMT as u32)) == (S_IFLNK as u32) {
                            CYAN.write_to(&mut out)?;
                            out.write_all(display_name.as_bytes())?;
                            out.write_all(RESET.as_bytes())?;
                        } else if is_executable(mode) {
                            GREEN.write_to(&mut out)?;
                            out.write_all(display_name.as_bytes())?;
                            out.write_all(RESET.as_bytes())?;
                        } else {
                            out.write_all(display_name.as_bytes())?;
                        }
                    } else {
                        out.write_all(display_name.as_bytes())?;
                    }
                } else {
                    out.write_all(display_name.as_bytes())?;
                }

                if col + 1 < best_cols {
                    let mut rem = (col_width + pad).saturating_sub(printed_len);
                    while rem > 0 {
                        let chunk = rem.min(SPACES.len());
                        out.write_all(&SPACES.as_bytes()[..chunk])?;
                        rem -= chunk;
                    }
                }
            }
        }
        out.write_all(b"\n")?;
    }

    Ok(())
}

/// Print files in long listing format (like `ls -l`).
///
/// Displays detailed file metadata including permissions, owner, group, size,
/// modification time, and filename. Automatically formats output with aligned
/// columns and optional human-readable file sizes.
///
/// # Arguments
///
/// * `items` - File information to display
/// * `arena` - Arena containing filename bytes
/// * `use_color` - Whether to colorize output
/// * `show_inode` - Whether to display inode numbers
/// * `human_readable` - Whether to format sizes as human-readable (e.g., 1K, 234M, 2G)
pub fn print_long_listing(
    items: &[FileInfo],
    arena: &[u8],
    use_color: bool,
    show_inode: bool,
    human_readable: bool,
    show_git: bool,
) -> io::Result<()> {
    let stdout = io::stdout();
    let capacity = calculate_output_buffer_size(items, arena, true);
    let mut out = BufWriter::with_capacity(capacity, stdout.lock());
    match render_long_listing_to(
        items,
        arena,
        use_color,
        show_inode,
        human_readable,
        show_git,
        &mut out,
    ) {
        Ok(()) => out.flush(),
        Err(err) => Err(err),
    }
}

/// Render long-listing output to a caller-provided writer without buffering.
pub fn render_long_listing_to<W: Write + ?Sized>(
    items: &[FileInfo],
    arena: &[u8],
    use_color: bool,
    show_inode: bool,
    human_readable: bool,
    show_git: bool,
    mut out: &mut W,
) -> io::Result<()> {
    if items.iter().any(|item| item.metadata.is_none()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "long listing requires metadata for every entry",
        ));
    }
    let mut max_nlink_width = 0;
    let mut max_user_width = 0;
    let mut max_group_width = 0;
    let mut max_size_width = 0;
    let mut max_inode_width = 0;
    let mut total_blocks = 0;

    let mut user_cache = FxHashMap::default();
    let mut group_cache = FxHashMap::default();
    let mut buf = [0u8; 256];

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);

    for item in items {
        if let Some(meta) = &item.metadata {
            max_nlink_width = max_nlink_width.max(num_digits(meta.nlink));

            let user = get_user_name(meta.uid, &mut user_cache);
            max_user_width = max_user_width.max(escape_name(user.as_bytes()).width());

            let group = get_group_name(meta.gid, &mut group_cache);
            max_group_width = max_group_width.max(escape_name(group.as_bytes()).width());

            let size_s = format_size(meta.size, human_readable, &mut buf);
            max_size_width = max_size_width.max(size_s.len());

            if show_inode {
                max_inode_width = max_inode_width.max(num_digits(meta.ino));
            }
            total_blocks += meta.blocks;
        }
    }

    writeln!(out, "total {}", total_blocks)?;

    for item in items {
        if let Some(meta) = &item.metadata {
            if show_inode {
                write!(out, "{:>width$} ", meta.ino, width = max_inode_width)?;
            }

            let mut buf = [0u8; 256];
            let mode_str = get_mode_string(meta.mode, &mut buf);
            out.write_all(mode_str.as_bytes())?;
            out.write_all(b" ")?;

            if show_git {
                let git_marker = match meta.git_status {
                    crate::args::GitStatus::Modified => "M",
                    crate::args::GitStatus::New => "N",
                    crate::args::GitStatus::Deleted => "D",
                    crate::args::GitStatus::Renamed => "R",
                    crate::args::GitStatus::Ignored => "I",
                    crate::args::GitStatus::Untracked => "?",
                    crate::args::GitStatus::Conflicted => "U",
                    crate::args::GitStatus::Clean => " ",
                };
                out.write_all(git_marker.as_bytes())?;
                out.write_all(b" ")?;
            }

            write!(out, "{:>width$} ", meta.nlink, width = max_nlink_width)?;

            let user = escape_name(get_user_name(meta.uid, &mut user_cache).as_bytes());
            write_left_aligned(&mut out, &user, max_user_width)?;
            out.write_all(b" ")?;

            let group = escape_name(get_group_name(meta.gid, &mut group_cache).as_bytes());
            write_left_aligned(&mut out, &group, max_group_width)?;
            out.write_all(b" ")?;

            let mut size_buf = [0u8; 64];
            let size_str = format_size(meta.size, human_readable, &mut size_buf);
            write!(out, "{:>width$} ", size_str, width = max_size_width)?;

            let mut time_buf = [0u8; 64];
            let time_str = format_time_with_now(meta.mtime, now, &mut time_buf);
            out.write_all(b" ")?;
            out.write_all(time_str.as_bytes())?;
            out.write_all(b" ")?;

            let name_bytes = entry_name(item, arena)?;
            let display_name = escape_name(name_bytes);

            if use_color {
                if item.entry.is_dir() {
                    BLUE.write_to(&mut out)?;
                    out.write_all(display_name.as_bytes())?;
                    out.write_all(RESET.as_bytes())?;
                } else if (meta.mode & (S_IFMT as u32)) == (S_IFLNK as u32) {
                    CYAN.write_to(&mut out)?;
                    out.write_all(display_name.as_bytes())?;
                    out.write_all(RESET.as_bytes())?;
                } else if is_executable(meta.mode) {
                    GREEN.write_to(&mut out)?;
                    out.write_all(display_name.as_bytes())?;
                    out.write_all(RESET.as_bytes())?;
                } else {
                    out.write_all(display_name.as_bytes())?;
                }
            } else {
                out.write_all(display_name.as_bytes())?;
            }

            if (meta.mode & (S_IFMT as u32)) == (S_IFLNK as u32) {
                out.write_all(b" -> ")?;
                if let Some(target) = &meta.symlink_target {
                    out.write_all(escape_name(target).as_bytes())?;
                }
            }

            out.write_all(b"\n")?;
        }
    }
    Ok(())
}

/// Print files one per line (like `ls -1`).
///
/// Simple format with each filename on its own line, optionally with inode numbers
/// and color coding. Uses buffered output for performance.
///
/// # Arguments
///
/// * `items` - File information to display
/// * `arena` - Arena containing filename bytes
/// * `use_color` - Whether to colorize output
/// * `show_inode` - Whether to display inode numbers
pub fn print_one_per_line(
    items: &[FileInfo],
    arena: &[u8],
    use_color: bool,
    show_inode: bool,
) -> io::Result<()> {
    let stdout = io::stdout();
    let capacity =
        calculate_output_buffer_size(items, arena, false).max(determine_buffer_size(items.len()));
    let mut out = BufWriter::with_capacity(capacity, stdout.lock());
    match render_one_per_line_to(items, arena, use_color, show_inode, &mut out) {
        Ok(()) => out.flush(),
        Err(err) => Err(err),
    }
}

/// Render one-entry-per-line output to a caller-provided writer without buffering.
pub fn render_one_per_line_to<W: Write + ?Sized>(
    items: &[FileInfo],
    arena: &[u8],
    use_color: bool,
    show_inode: bool,
    mut out: &mut W,
) -> io::Result<()> {
    for item in items {
        let entry = item.entry;
        let name_bytes = entry_name(item, arena)?;
        let display_name = escape_name(name_bytes);

        if show_inode {
            if let Some(meta) = &item.metadata {
                write!(out, "{} ", meta.ino)?;
            } else {
                write!(out, "? ")?;
            }
        }

        if use_color {
            if entry.is_dir() {
                BLUE.write_to(&mut out)?;
                out.write_all(display_name.as_bytes())?;
                out.write_all(RESET.as_bytes())?;
            } else if let Some(meta) = &item.metadata {
                if (meta.mode & (S_IFMT as u32)) == (S_IFLNK as u32) {
                    CYAN.write_to(&mut out)?;
                    out.write_all(display_name.as_bytes())?;
                    out.write_all(RESET.as_bytes())?;
                } else if is_executable(meta.mode) {
                    GREEN.write_to(&mut out)?;
                    out.write_all(display_name.as_bytes())?;
                    out.write_all(RESET.as_bytes())?;
                } else {
                    out.write_all(display_name.as_bytes())?;
                }
            } else {
                out.write_all(display_name.as_bytes())?;
            }
        } else {
            out.write_all(display_name.as_bytes())?;
        }
        out.write_all(b"\n")?;
    }
    Ok(())
}
