// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::args::Config;
use crate::colors::{BLUE, CYAN, GREEN, RESET};
use crate::platform::get_dirent_name;
use crate::utils::escape_name;
use libc::{
    O_DIRECTORY, O_RDONLY, S_IFDIR, S_IFLNK, S_IFMT, S_IXUSR, close, closedir, dirfd, dup,
    fdopendir, fstat, fstatat, open, openat, readdir,
};
use std::ffi::{CString, OsStr};
use std::io::{self, BufWriter, Write};
use std::os::unix::ffi::OsStrExt;

/// RAII guard that closes a file descriptor on drop.
struct FdGuard(i32);

impl Drop for FdGuard {
    fn drop(&mut self) {
        if self.0 >= 0 {
            unsafe { close(self.0) };
        }
    }
}

/// RAII guard that closes a DIR* stream on drop.
struct DirGuard(*mut libc::DIR);

impl Drop for DirGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { closedir(self.0) };
        }
    }
}

/// Print directory contents in tree format.
///
/// Recursively displays directory structure with tree-like formatting using
/// box-drawing characters. Respects depth limits and color settings from config.
///
/// # Arguments
///
/// * `config` - Configuration including path, depth limit, color settings, etc.
///
/// # Errors
///
/// Returns an error if the root path cannot be opened or read.
pub fn print_tree<F>(config: &Config, check_read_dir: F) -> io::Result<()>
where
    F: Fn(&std::path::Path) -> bool,
{
    let stdout = io::stdout();
    let mut out = BufWriter::with_capacity(64 * 1024, stdout.lock());
    let result = render_tree(config, &mut out, check_read_dir);
    match result {
        Ok(()) => out.flush(),
        Err(err) => Err(err),
    }
}

/// Renders the tree to a generic writer.
pub fn render_tree<W, F>(config: &Config, mut out: W, check_read_dir: F) -> io::Result<()>
where
    W: Write,
    F: Fn(&std::path::Path) -> bool,
{
    if !check_read_dir(&config.path) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("permission denied: '{}'", config.path.display()),
        ));
    }
    let result = crate::scan::list_dir(config)?;
    render_tree_with_result(config, &result, &mut out, check_read_dir)
}

/// Print a previously scanned root listing as a tree, scanning only descendants.
pub fn print_tree_with_result<F>(
    config: &Config,
    result: &crate::scan::ListResult,
    check_read_dir: F,
) -> io::Result<()>
where
    F: Fn(&std::path::Path) -> bool,
{
    let stdout = io::stdout();
    let mut out = BufWriter::with_capacity(64 * 1024, stdout.lock());
    let result = render_tree_with_result(config, result, &mut out, check_read_dir);
    match result {
        Ok(()) => out.flush(),
        Err(err) => Err(err),
    }
}

pub fn render_tree_with_result<W, F>(
    config: &Config,
    result: &crate::scan::ListResult,
    mut out: W,
    check_read_dir: F,
) -> io::Result<()>
where
    W: Write,
    F: Fn(&std::path::Path) -> bool,
{
    if !check_read_dir(&config.path) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("permission denied: '{}'", config.path.display()),
        ));
    }

    // Default tree depth is bounded (20 levels) to avoid runaway recursion on
    // pathological/deep structures; users can override with `ls tree --depth N`.
    let max_depth = config.tree_depth.unwrap_or(20);
    if max_depth > 128 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "tree depth exceeds the safe maximum of 128",
        ));
    }
    let c_path = CString::new(config.path.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid path (contains null byte)",
        )
    })?;
    // SAFETY: c_path is a valid null-terminated C string
    let mut expected_root = std::mem::MaybeUninit::<libc::stat>::uninit();
    let expected_res = unsafe {
        fstatat(
            libc::AT_FDCWD,
            c_path.as_ptr(),
            expected_root.as_mut_ptr(),
            if config.dereference {
                0
            } else {
                libc::AT_SYMLINK_NOFOLLOW
            },
        )
    };
    if expected_res != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstatat succeeded, initializing expected_root.
    let expected_root = unsafe { expected_root.assume_init() };
    let root_identity = result.root_identity;
    if expected_root.st_dev as u64 != root_identity.device
        || expected_root.st_ino as u64 != root_identity.inode
        || ((expected_root.st_mode & S_IFMT) == S_IFDIR) != root_identity.is_dir
    {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "tree root changed after its directory listing was collected",
        ));
    }

    let root_name = escape_name(config.path.as_os_str().as_bytes());
    out.write_all(root_name.as_bytes())?;
    out.write_all(b"\n")?;

    if !root_identity.is_dir || config.list_dirs || max_depth == 0 {
        return Ok(());
    }

    let fd = unsafe {
        open(
            c_path.as_ptr(),
            O_RDONLY
                | O_DIRECTORY
                | libc::O_CLOEXEC
                | if config.dereference {
                    0
                } else {
                    libc::O_NOFOLLOW
                },
        )
    };
    if fd < 0 {
        let err = io::Error::last_os_error();
        return Err(io::Error::new(
            err.kind(),
            format!("cannot open '{}': {}", config.path.display(), err),
        ));
    }
    let fd = FdGuard(fd);

    let mut opened_root = std::mem::MaybeUninit::<libc::stat>::uninit();
    let root_stat_res = unsafe { fstat(fd.0, opened_root.as_mut_ptr()) };
    if root_stat_res != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstat succeeded, initializing opened_root.
    let opened_root = unsafe { opened_root.assume_init() };
    if opened_root.st_dev as u64 != root_identity.device
        || opened_root.st_ino as u64 != root_identity.inode
    {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "tree root changed while it was being opened",
        ));
    }

    let mut prefix = String::new();
    visit_dir_iterative(
        fd,
        &config.path,
        &mut prefix,
        &mut out,
        config,
        max_depth,
        Some(result),
        &check_read_dir,
    )
}

#[allow(clippy::too_many_arguments)]
fn visit_dir_iterative<F>(
    dir_fd: FdGuard,
    current_path: &std::path::Path,
    prefix: &mut String,
    out: &mut dyn Write,
    config: &Config,
    max_depth: usize,
    root_result: Option<&crate::scan::ListResult>,
    check_read_dir: &F,
) -> io::Result<()>
where
    F: Fn(&std::path::Path) -> bool,
{
    // SAFETY: dir_fd.0 is a valid file descriptor from open/openat
    let dir = unsafe { fdopendir(dir_fd.0) };

    if dir.is_null() {
        return Err(io::Error::last_os_error());
    }
    // fdopendir consumed the fd on success, forget our guard
    std::mem::forget(dir_fd);
    let _dir_guard = DirGuard(dir);

    // SAFETY: dir is a valid non-null DIR pointer
    let fd = unsafe { dirfd(dir) };
    // Duplicate fd so we can use it after closedir
    // SAFETY: fd is valid from dirfd
    let fd_dup = unsafe { dup(fd) };
    if fd_dup < 0 {
        return Err(io::Error::last_os_error());
    }
    let _fd_guard = FdGuard(fd_dup);

    let mut arena: Vec<u8> = Vec::new();
    let mut entries = Vec::new();

    if let Some(root_result) = root_result {
        for item in &root_result.entries {
            let Some(range) = item.entry.range(root_result.arena.len()) else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "tree entry points outside the filename arena",
                ));
            };
            let Some(name_bytes) = root_result.arena.get(range) else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "tree entry points outside the filename arena",
                ));
            };
            let start = arena.len();
            arena.extend_from_slice(name_bytes);
            arena.push(0);
            let d_type = if item.entry.is_dir() {
                libc::DT_DIR
            } else {
                item.metadata
                    .as_ref()
                    .map(|metadata| (metadata.mode as libc::mode_t) & S_IFMT)
                    .map(|mode| {
                        if mode == S_IFDIR {
                            libc::DT_DIR
                        } else if mode == S_IFLNK {
                            libc::DT_LNK
                        } else {
                            libc::DT_UNKNOWN
                        }
                    })
                    .unwrap_or(libc::DT_UNKNOWN)
            };
            entries.push((start, name_bytes.len(), d_type));
        }
    } else {
        loop {
            crate::platform::clear_errno();
            // SAFETY: dir is a valid DIR pointer.
            let entry_ptr = unsafe { readdir(dir) };
            if entry_ptr.is_null() {
                let errno = crate::platform::current_errno();
                if errno != 0 {
                    return Err(io::Error::from_raw_os_error(errno));
                }
                break;
            }
            // SAFETY: readdir returned non-null, pointer valid until next readdir/closedir.
            let entry = unsafe { &*entry_ptr };
            let name_bytes = get_dirent_name(entry);

            if name_bytes == b"." || name_bytes == b".." {
                continue;
            }
            if !config.show_all && name_bytes.starts_with(b".") {
                continue;
            }

            let start = arena.len();
            arena.extend_from_slice(name_bytes);
            arena.push(0);
            entries.push((start, name_bytes.len(), entry.d_type));
        }
    }

    entries.sort_by(|&(a_start, a_len, _), &(b_start, b_len, _)| {
        arena[a_start..a_start + a_len].cmp(&arena[b_start..b_start + b_len])
    });

    let count = entries.len();
    for (i, &(start, len, d_type)) in entries.iter().enumerate() {
        let is_last = i == count - 1;

        let name_bytes = &arena[start..start + len];
        let display_name = escape_name(name_bytes);
        // SAFETY: start + len is within the arena, and arena[start + len] is null
        let name_ptr = unsafe { arena.as_ptr().add(start) as *const libc::c_char };

        out.write_all(prefix.as_bytes())?;
        if is_last {
            out.write_all("└── ".as_bytes())?;
        } else {
            out.write_all("├── ".as_bytes())?;
        }

        let mut is_dir = d_type == libc::DT_DIR;
        let mut is_link = d_type == libc::DT_LNK;
        let mut is_exec = false;

        // Single fstatat path: for unknown d_type (need type detection) or
        // when use_color is on for non-dir/non-link entries (need executable check)
        if d_type == libc::DT_UNKNOWN
            || (config.dereference && d_type == libc::DT_LNK)
            || (config.use_color && !is_dir && !is_link)
        {
            let mut stat_buf = std::mem::MaybeUninit::<libc::stat>::uninit();
            // SAFETY: fd is a valid file descriptor, name_ptr points to a valid null-terminated C string in arena, and stat_buf is a valid MaybeUninit pointer.
            let res = unsafe {
                fstatat(
                    fd,
                    name_ptr,
                    stat_buf.as_mut_ptr(),
                    if config.dereference {
                        0
                    } else {
                        libc::AT_SYMLINK_NOFOLLOW
                    },
                )
            };
            if res == 0 {
                // SAFETY: fstatat returned 0 (success), meaning stat_buf is now initialized.
                let stat = unsafe { stat_buf.assume_init() };
                if d_type == libc::DT_UNKNOWN || config.dereference {
                    is_dir = (stat.st_mode & S_IFMT) == S_IFDIR;
                    is_link = (stat.st_mode & S_IFMT) == S_IFLNK;
                }
                if config.use_color {
                    is_exec = (stat.st_mode & S_IXUSR) != 0;
                }
            }
        }

        if config.use_color {
            if is_dir {
                BLUE.write_to(out)?;
                out.write_all(display_name.as_bytes())?;
                out.write_all(RESET.as_bytes())?;
            } else if is_link {
                CYAN.write_to(out)?;
                out.write_all(display_name.as_bytes())?;
                out.write_all(RESET.as_bytes())?;
            } else if is_exec {
                GREEN.write_to(out)?;
                out.write_all(display_name.as_bytes())?;
                out.write_all(RESET.as_bytes())?;
            } else {
                out.write_all(display_name.as_bytes())?;
            }
        } else {
            out.write_all(display_name.as_bytes())?;
        }

        out.write_all(b"\n")?;

        if is_dir && max_depth > 1 {
            let subdir_path = current_path.join(OsStr::from_bytes(name_bytes));
            if check_read_dir(&subdir_path) {
                // Capture expected dev+ino before openat (resist symlink-swap races)
                let mut expected_stat = std::mem::MaybeUninit::<libc::stat>::uninit();
                let stat_res = unsafe {
                    fstatat(
                        fd_dup,
                        name_ptr,
                        expected_stat.as_mut_ptr(),
                        if config.dereference {
                            0
                        } else {
                            libc::AT_SYMLINK_NOFOLLOW
                        },
                    )
                };

                // SAFETY: fd_dup is a valid file descriptor, and name_ptr points to a valid null-terminated C string in the arena.
                let sub_fd = unsafe {
                    openat(
                        fd_dup,
                        name_ptr,
                        O_RDONLY
                            | O_DIRECTORY
                            | libc::O_CLOEXEC
                            | if config.dereference {
                                0
                            } else {
                                libc::O_NOFOLLOW
                            },
                    )
                };
                if sub_fd >= 0 {
                    // Verify the opened fd corresponds to the same file as the checked path
                    let mut opened_stat = std::mem::MaybeUninit::<libc::stat>::uninit();
                    let opened_stat_res = unsafe { fstat(sub_fd, opened_stat.as_mut_ptr()) };

                    let is_same_file = if stat_res == 0 && opened_stat_res == 0 {
                        let expected = unsafe { expected_stat.assume_init() };
                        let opened = unsafe { opened_stat.assume_init() };
                        expected.st_dev == opened.st_dev && expected.st_ino == opened.st_ino
                    } else {
                        false
                    };

                    if is_same_file {
                        let original_len = prefix.len();
                        if is_last {
                            prefix.push_str("    ");
                        } else {
                            prefix.push_str("│   ");
                        }
                        visit_dir_iterative(
                            FdGuard(sub_fd),
                            &subdir_path,
                            prefix,
                            out,
                            config,
                            max_depth - 1,
                            None,
                            check_read_dir,
                        )?;
                        prefix.truncate(original_len);
                    } else {
                        // Mismatch or stat failure — close fd and skip
                        unsafe { close(sub_fd) };
                    }
                }
            }
        }
    }

    Ok(())
}
