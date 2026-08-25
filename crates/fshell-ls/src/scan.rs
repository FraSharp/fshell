// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Directory scanning: read entries, collect metadata, sort.
//!
//! Adapted from rrls main.rs as a library API. No process::exit() calls.

#![allow(clippy::unnecessary_cast)]
use crate::args::{Config, GitStatus, SortMode};
use crate::file::{Entry, FileInfo, Metadata};
use crate::utils::is_directory;
use fshell_git::repo::Repository;
use fshell_git::status::Status as FgStatus;

use libc::{S_IFDIR, S_IFMT, closedir, dirfd, readdir};
use std::collections::HashMap;
use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const INITIAL_ARENA_CAPACITY: usize = 8 * 1024;
const INITIAL_ENTRIES_CAPACITY: usize = 512;

/// How long a computed `git status` snapshot stays valid before it is
/// recomputed. `ls` is frequently called repeatedly in the same repo (scripts,
/// completions, adjacent directories); re-running the index read + worktree
/// scan on every call is pure waste. A 400 ms TTL matches the FTUI hinter's
/// `PATH_CACHE_TTL_MS` convention — imperceptibly stale in practice, and never
/// longer than the gap between a user editing a file and re-running `ls`.
const GIT_STATUS_CACHE_TTL: Duration = Duration::from_millis(400);

/// Result of scanning a directory.
pub struct ListResult {
    pub entries: Vec<FileInfo>,
    pub arena: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
struct MetadataFlags {
    need_mode: bool,
    need_nlink: bool,
    need_uid_gid: bool,
    need_size: bool,
    need_mtime: bool,
    need_blocks: bool,
    need_ino: bool,
    need_symlink_target: bool,
}

impl MetadataFlags {
    fn any(&self) -> bool {
        self.need_mode
            || self.need_nlink
            || self.need_uid_gid
            || self.need_size
            || self.need_mtime
            || self.need_blocks
            || self.need_ino
            || self.need_symlink_target
    }
}

/// Scan a directory: read entries, collect metadata, apply git status, sort.
///
/// # Thread safety
///
/// Internally uses `readdir` which is not reentrant. This function must
/// not be called concurrently on the same directory from multiple
/// threads. Callers should ensure single-threaded use.
///
/// This is the main entry point used by fshell's builtin ls.
pub fn list_dir(config: &Config) -> io::Result<ListResult> {
    let mut arena: Vec<u8> = Vec::with_capacity(INITIAL_ARENA_CAPACITY);
    let mut entries = Vec::with_capacity(INITIAL_ENTRIES_CAPACITY);

    let c_path = CString::new(config.path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains null byte"))?;

    let mut dir: *mut libc::DIR = std::ptr::null_mut();

    let path_exists_but_not_dir = if config.dereference {
        std::fs::metadata(&config.path)
            .map(|m| !m.is_dir())
            .unwrap_or(false)
    } else {
        std::fs::symlink_metadata(&config.path)
            .map(|m| !m.is_dir())
            .unwrap_or(false)
    };
    let list_as_single_file = config.list_dirs || path_exists_but_not_dir;

    let dir_fd = if list_as_single_file {
        let name_bytes = config.path.as_os_str().as_bytes();
        let start = arena.len();
        arena.extend_from_slice(name_bytes);
        arena.push(0);

        let mut stat_buf = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: c_path is a valid null-terminated C string, and stat_buf points to a valid MaybeUninit stat struct.
        let res = if config.dereference {
            unsafe { libc::stat(c_path.as_ptr(), stat_buf.as_mut_ptr()) }
        } else {
            // SAFETY: c_path is a valid null-terminated C string, and stat_buf points to a valid MaybeUninit stat struct.
            unsafe { libc::lstat(c_path.as_ptr(), stat_buf.as_mut_ptr()) }
        };
        // SAFETY: stat/lstat call returned 0 (success), meaning stat_buf is initialized.
        let is_dir = res == 0 && unsafe { (stat_buf.assume_init().st_mode & S_IFMT) == S_IFDIR };

        entries.push(FileInfo {
            entry: Entry::new(start, name_bytes.len(), is_dir),
            metadata: None,
        });
        libc::AT_FDCWD
    } else {
        // SAFETY: c_path is a valid null-terminated C string representing a directory.
        dir = unsafe { libc::opendir(c_path.as_ptr()) };
        if dir.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: dir is a valid non-null DIR pointer.
        let fd = unsafe { dirfd(dir) };
        entries = read_directory_entries(dir, fd, config, &mut arena)?;
        fd
    };

    // Collect metadata
    let flags = determine_metadata_needs(config);

    let num_items = entries.len();
    if num_items > 100 {
        use rayon::prelude::*;
        entries.par_iter_mut().for_each(|item| {
            collect_metadata(item, &arena, dir_fd, flags, config.dereference);
        });
    } else {
        for item in entries.iter_mut() {
            collect_metadata(item, &arena, dir_fd, flags, config.dereference);
        }
    }

    if !dir.is_null() {
        // SAFETY: dir is a valid non-null DIR pointer.
        unsafe { closedir(dir) };
    }

    // Git status
    if config.git {
        apply_git_status(&mut entries, &arena, &config.path);
    }

    // Sort
    sort_entries(&mut entries, &arena, config);

    Ok(ListResult { entries, arena })
}

/// Read all entries from an open directory stream.
///
/// # Safety
///
/// `dir` must be a valid DIR* pointer. `readdir` is not reentrant — this
/// function must not be called concurrently on the same `dir` stream from
/// multiple threads. The caller (`list_dir`) ensures single-threaded use.
fn read_directory_entries(
    dir: *mut libc::DIR,
    dir_fd: i32,
    config: &Config,
    arena: &mut Vec<u8>,
) -> io::Result<Vec<FileInfo>> {
    let mut entries_data = Vec::with_capacity(INITIAL_ENTRIES_CAPACITY);

    loop {
        // SAFETY: dir is a valid non-null DIR pointer.
        let entry_ptr = unsafe { readdir(dir) };
        if entry_ptr.is_null() {
            break;
        }

        // SAFETY: readdir returned a non-null entry_ptr, valid until next readdir/closedir.
        let entry = unsafe { &*entry_ptr };
        let name_bytes = crate::platform::get_dirent_name(entry);

        if name_bytes == b"." || name_bytes == b".." {
            continue;
        }
        if !config.show_all && name_bytes.starts_with(b".") {
            continue;
        }

        let is_dir = is_directory(entry, dir_fd, false);
        let start = arena.len();
        arena.extend_from_slice(name_bytes);
        arena.push(0);
        entries_data.push(FileInfo {
            entry: Entry::new(start, name_bytes.len(), is_dir),
            metadata: None,
        });
    }

    Ok(entries_data)
}

fn determine_metadata_needs(config: &Config) -> MetadataFlags {
    let verbose = config.verbose;
    MetadataFlags {
        need_mode: config.long_listing || config.sort_mode != SortMode::Name || verbose,
        need_nlink: config.long_listing,
        need_uid_gid: config.long_listing,
        need_size: config.long_listing || config.sort_mode == SortMode::Size || verbose,
        need_mtime: config.long_listing || config.sort_mode == SortMode::Time || verbose,
        need_blocks: config.long_listing,
        need_ino: config.show_inode,
        need_symlink_target: config.long_listing,
    }
}

fn collect_metadata(
    item: &mut FileInfo,
    arena: &[u8],
    dir_fd: i32,
    flags: MetadataFlags,
    dereference: bool,
) {
    if !flags.any() {
        return;
    }

    // SAFETY: item.entry.start() is within the bounds of arena.
    let name_ptr = unsafe { arena.as_ptr().add(item.entry.start()) as *const libc::c_char };
    let mut stat_buf = std::mem::MaybeUninit::<libc::stat>::uninit();

    // SAFETY: dir_fd is a valid directory file descriptor (or AT_FDCWD), name_ptr is a valid C string from the arena, and stat_buf points to a valid MaybeUninit stat struct.
    let res = if dereference {
        unsafe { libc::fstatat(dir_fd, name_ptr, stat_buf.as_mut_ptr(), 0) }
    } else {
        // SAFETY: dir_fd is a valid directory file descriptor (or AT_FDCWD), name_ptr is a valid C string from the arena, and stat_buf points to a valid MaybeUninit stat struct.
        unsafe {
            libc::fstatat(
                dir_fd,
                name_ptr,
                stat_buf.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        }
    };

    if res == 0 {
        // SAFETY: fstatat returned 0 (success), meaning stat_buf is initialized.
        let stat = unsafe { stat_buf.assume_init() };

        let symlink_target = if flags.need_symlink_target
            && (stat.st_mode as u32 & S_IFMT as u32) == (libc::S_IFLNK as u32)
        {
            let mut link_buf = [0u8; 4096];
            // SAFETY: dir_fd is valid, name_ptr is a valid C string from the arena, and link_buf is a valid stack-allocated byte array.
            let link_len = unsafe {
                libc::readlinkat(
                    dir_fd,
                    name_ptr,
                    link_buf.as_mut_ptr() as *mut libc::c_char,
                    link_buf.len(),
                )
            };
            if link_len > 0 {
                Some(link_buf[..link_len as usize].to_vec())
            } else {
                None
            }
        } else {
            None
        };

        item.metadata = Some(Metadata {
            mode: if flags.need_mode {
                stat.st_mode as u32
            } else {
                0
            },
            nlink: if flags.need_nlink {
                stat.st_nlink as u64
            } else {
                0
            },
            uid: if flags.need_uid_gid { stat.st_uid } else { 0 },
            gid: if flags.need_uid_gid { stat.st_gid } else { 0 },
            size: if flags.need_size {
                stat.st_size as u64
            } else {
                0
            },
            mtime: if flags.need_mtime { stat.st_mtime } else { 0 },
            blocks: if flags.need_blocks { stat.st_blocks } else { 0 },
            ino: if flags.need_ino { stat.st_ino } else { 0 },
            symlink_target,
            git_status: GitStatus::Clean,
        });
    }
}

fn sort_entries(entries_data: &mut [FileInfo], arena: &[u8], config: &Config) {
    entries_data.sort_unstable_by(|a, b| {
        if config.group_directories_first {
            let a_is_dir = a.entry.is_dir();
            let b_is_dir = b.entry.is_dir();
            if a_is_dir != b_is_dir {
                return if config.reverse_sort {
                    b_is_dir.cmp(&a_is_dir)
                } else {
                    a_is_dir.cmp(&b_is_dir)
                };
            }
        }

        let cmp = match config.sort_mode {
            SortMode::Name => {
                let name_a = &arena[a.entry.start()..a.entry.start() + a.entry.len()];
                let name_b = &arena[b.entry.start()..b.entry.start() + b.entry.len()];
                name_a.cmp(name_b)
            }
            SortMode::Size => {
                let size_a = a.metadata.as_ref().map_or(0, |m| m.size);
                let size_b = b.metadata.as_ref().map_or(0, |m| m.size);
                size_b.cmp(&size_a)
            }
            SortMode::Time => {
                let time_a = a.metadata.as_ref().map_or(0, |m| m.mtime);
                let time_b = b.metadata.as_ref().map_or(0, |m| m.mtime);
                time_b.cmp(&time_a)
            }
        };

        if config.reverse_sort {
            cmp.reverse()
        } else {
            cmp
        }
    });
}

fn with_git_repo<F, R>(path: &Path, f: F) -> Option<R>
where
    F: FnOnce(&Repository) -> R,
{
    thread_local! {
        static CACHED: std::cell::RefCell<Option<(PathBuf, Repository)>> = const { std::cell::RefCell::new(None) };
    }

    CACHED.with(|cache| {
        let mut cache_borrow = cache.borrow_mut();
        if let Some((ref workdir, ref repo)) = *cache_borrow
            && path.starts_with(workdir)
        {
            return Some(f(repo));
        }

        // Cache miss / different repo
        if let Ok(repo) = Repository::discover(path) {
            let workdir_buf = repo.work_dir().to_path_buf();
            let res = f(&repo);
            *cache_borrow = Some((workdir_buf, repo));
            return Some(res);
        }
        None
    })
}

/// A cached `git status` snapshot for one repository root.
type StatusSnapshot = (PathBuf, Instant, Arc<HashMap<PathBuf, FgStatus>>);

fn apply_git_status(entries_data: &mut [FileInfo], arena: &[u8], path: &Path) {
    thread_local! {
        static STATUS_CACHE: std::cell::RefCell<Option<StatusSnapshot>> =
            const { std::cell::RefCell::new(None) };
    }

    let _ = with_git_repo(path, |repo| {
        let workdir = repo.work_dir();
        let rel_dir = path.strip_prefix(workdir).unwrap_or(Path::new(""));

        // `repo.status()` re-reads the index and re-scans the worktree; reuse
        // the last snapshot for this repo within the TTL instead.
        let statuses: Arc<HashMap<PathBuf, FgStatus>> = STATUS_CACHE.with(|cache| {
            let mut cache_borrow = cache.borrow_mut();
            if let Some((cached_workdir, at, snap)) = &*cache_borrow
                && cached_workdir.as_path() == workdir
                && at.elapsed() < GIT_STATUS_CACHE_TTL
            {
                return Arc::clone(snap);
            }
            let snap = match repo.status() {
                Ok(s) => Arc::new(s.into_iter().collect::<HashMap<PathBuf, FgStatus>>()),
                Err(_) => return Arc::new(HashMap::new()),
            };
            *cache_borrow = Some((workdir.to_path_buf(), Instant::now(), snap.clone()));
            snap
        });

        let mut status_map: HashMap<String, GitStatus> = HashMap::with_capacity(statuses.len());

        for (file_path, status) in statuses.iter() {
            let rel = if let Ok(r) = file_path.strip_prefix(workdir) {
                r
            } else {
                file_path.as_path()
            };
            let git_status = match status {
                FgStatus::Added => GitStatus::New,
                FgStatus::Modified => GitStatus::Modified,
                FgStatus::Deleted => GitStatus::Deleted,
                FgStatus::Ignored => GitStatus::Ignored,
                FgStatus::Conflicted => GitStatus::Conflicted,
                FgStatus::TypeChange => GitStatus::Modified,
                FgStatus::Clean => GitStatus::Clean,
            };
            status_map.insert(rel.to_string_lossy().to_string(), git_status);
        }

        for item in entries_data.iter_mut() {
            let start = item.entry.start();
            let len = item.entry.len();
            let name_bytes = &arena[start..start + len];

            if let Ok(name) = std::str::from_utf8(name_bytes) {
                let rel_path = if rel_dir.as_os_str().is_empty() {
                    name.to_owned()
                } else {
                    format!("{}/{}", rel_dir.to_string_lossy(), name)
                };
                if let Some(ref mut meta) = item.metadata
                    && let Some(git_status) = status_map.get(&rel_path)
                {
                    meta.git_status = *git_status;
                }
            }
        }
    });
}
