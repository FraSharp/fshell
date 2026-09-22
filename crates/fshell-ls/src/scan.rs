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
use fshell_hash::FxHashMap;

use libc::{
    O_CLOEXEC, O_DIRECTORY, O_NOFOLLOW, O_RDONLY, S_IFDIR, S_IFMT, close, dirfd, fdopendir, fstat,
    open, readdir,
};
use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

const INITIAL_ARENA_CAPACITY: usize = 8 * 1024;
const INITIAL_ENTRIES_CAPACITY: usize = 512;

/// Result of scanning a directory.
pub struct ListResult {
    pub entries: Vec<FileInfo>,
    pub arena: Vec<u8>,
    pub root_identity: RootIdentity,
}

/// Identity of the filesystem object from which a listing was collected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootIdentity {
    pub device: u64,
    pub inode: u64,
    pub is_dir: bool,
}

/// Repository status snapshots reused across scans in one recursive listing.
///
/// Keep one cache for a top-level operation. Separate repositories encountered
/// below the root receive separate snapshots.
#[derive(Default)]
pub struct GitStatusCache {
    snapshots: FxHashMap<std::path::PathBuf, GitStatusSnapshot>,
}

struct GitStatusSnapshot {
    workdir: std::path::PathBuf,
    statuses: FxHashMap<std::path::PathBuf, FgStatus>,
}

impl GitStatusCache {
    fn snapshot_for(
        &mut self,
        path: &Path,
        dereference: bool,
    ) -> io::Result<Option<&GitStatusSnapshot>> {
        let canonical_path = canonical_git_path(path, dereference)?;
        let repo = match Repository::discover(&canonical_path) {
            Ok(repo) => repo,
            Err(fshell_git::repo::Error::NotFound) => return Ok(None),
            Err(err) => return Err(io::Error::other(err)),
        };
        let workdir = repo.work_dir().to_path_buf();

        if !self.snapshots.contains_key(&workdir) {
            let statuses = repo.status().map_err(io::Error::other)?;
            self.snapshots.insert(
                workdir.clone(),
                GitStatusSnapshot {
                    workdir: workdir.clone(),
                    statuses,
                },
            );
        }

        Ok(self.snapshots.get(&workdir))
    }
}

struct DirGuard(*mut libc::DIR);

impl Drop for DirGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the guard owns this valid DIR pointer.
            unsafe { libc::closedir(self.0) };
        }
    }
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
    let mut git_status_cache = GitStatusCache::default();
    list_dir_with_git_status_cache(config, &mut git_status_cache)
}

/// Scan a directory while reusing Git status snapshots from a shared cache.
///
/// Use this for recursive scans within one top-level operation. The cache
/// discovers the nearest repository for each path and computes each worktree's
/// status only once.
pub fn list_dir_with_git_status_cache(
    config: &Config,
    git_status_cache: &mut GitStatusCache,
) -> io::Result<ListResult> {
    let mut arena: Vec<u8> = Vec::with_capacity(INITIAL_ARENA_CAPACITY);
    let mut entries = Vec::with_capacity(INITIAL_ENTRIES_CAPACITY);

    let c_path = CString::new(config.path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains null byte"))?;

    let mut path_stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let stat_res = if config.dereference {
        // SAFETY: c_path is a valid null-terminated path and path_stat is a valid output buffer.
        unsafe { libc::stat(c_path.as_ptr(), path_stat.as_mut_ptr()) }
    } else {
        // SAFETY: c_path is a valid null-terminated path and path_stat is a valid output buffer.
        unsafe { libc::lstat(c_path.as_ptr(), path_stat.as_mut_ptr()) }
    };
    if stat_res != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: stat/lstat returned success, so path_stat is initialized.
    let path_stat = unsafe { path_stat.assume_init() };
    let path_is_dir = (path_stat.st_mode & S_IFMT) == S_IFDIR;
    let list_as_single_file = config.list_dirs || !path_is_dir;

    let (mut entries, dir_guard, dir_fd, root_identity) = if list_as_single_file {
        let name_bytes = config.path.as_os_str().as_bytes();
        let start = arena.len();
        arena.extend_from_slice(name_bytes);
        arena.push(0);

        entries.push(FileInfo {
            entry: Entry::new(start, name_bytes.len(), path_is_dir),
            metadata: None,
        });
        (
            entries,
            None,
            libc::AT_FDCWD,
            RootIdentity {
                device: path_stat.st_dev as u64,
                inode: path_stat.st_ino as u64,
                is_dir: path_is_dir,
            },
        )
    } else {
        let mut open_flags = O_RDONLY | O_DIRECTORY | O_CLOEXEC;
        if !config.dereference {
            open_flags |= O_NOFOLLOW;
        }
        // SAFETY: c_path is a valid null-terminated path.
        let fd = unsafe { open(c_path.as_ptr(), open_flags) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }

        let mut opened_stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: fd was returned by open and opened_stat is a valid output buffer.
        let opened_stat_res = unsafe { fstat(fd, opened_stat.as_mut_ptr()) };
        if opened_stat_res != 0 {
            let err = io::Error::last_os_error();
            // SAFETY: fd is owned by this branch and has not been transferred.
            unsafe { close(fd) };
            return Err(err);
        }
        // SAFETY: fstat returned success, so opened_stat is initialized.
        let opened_stat = unsafe { opened_stat.assume_init() };
        if opened_stat.st_dev != path_stat.st_dev || opened_stat.st_ino != path_stat.st_ino {
            // SAFETY: fd is owned by this branch and has not been transferred.
            unsafe { close(fd) };
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "directory changed while it was being opened",
            ));
        }

        // SAFETY: fd is a valid directory descriptor; ownership transfers to DIR on success.
        let dir = unsafe { fdopendir(fd) };
        if dir.is_null() {
            let err = io::Error::last_os_error();
            // SAFETY: fdopendir failed, so fd remains owned by this branch.
            unsafe { close(fd) };
            return Err(err);
        }
        // SAFETY: dir is a valid non-null DIR pointer.
        let dir_fd = unsafe { dirfd(dir) };
        if dir_fd < 0 {
            let err = io::Error::last_os_error();
            // SAFETY: dir owns the descriptor and is valid.
            unsafe { libc::closedir(dir) };
            return Err(err);
        }
        let guard = DirGuard(dir);
        let entries = read_directory_entries(dir, dir_fd, config, &mut arena)?;
        (
            entries,
            Some(guard),
            dir_fd,
            RootIdentity {
                device: opened_stat.st_dev as u64,
                inode: opened_stat.st_ino as u64,
                is_dir: true,
            },
        )
    };

    // Collect metadata
    let flags = determine_metadata_needs(config);

    let num_items = entries.len();
    let metadata_result = if !flags.any() {
        Ok(())
    } else if num_items > 100 {
        use rayon::prelude::*;
        entries
            .par_iter_mut()
            .try_for_each(|item| collect_metadata(item, &arena, dir_fd, flags, config.dereference))
    } else {
        let mut result = Ok(());
        for item in &mut entries {
            if let Err(err) = collect_metadata(item, &arena, dir_fd, flags, config.dereference) {
                result = Err(err);
                break;
            }
        }
        result
    };
    drop(dir_guard);
    metadata_result?;

    // Git status
    if config.git {
        if let Some(snapshot) = git_status_cache.snapshot_for(&config.path, config.dereference)? {
            apply_git_status(
                &mut entries,
                &arena,
                &config.path,
                list_as_single_file,
                config.dereference,
                snapshot,
            )?;
        }
    }

    // Sort
    sort_entries(&mut entries, &arena, config);

    Ok(ListResult {
        entries,
        arena,
        root_identity,
    })
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
        crate::platform::clear_errno();
        // SAFETY: dir is a valid non-null DIR pointer.
        let entry_ptr = unsafe { readdir(dir) };
        if entry_ptr.is_null() {
            let errno = crate::platform::current_errno();
            if errno != 0 {
                return Err(io::Error::from_raw_os_error(errno));
            }
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

        let is_dir = is_directory(entry, dir_fd, config.dereference)?;
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
        need_mode: config.long_listing
            || config.sort_mode != SortMode::Name
            || verbose
            || config.git,
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
) -> io::Result<()> {
    if !flags.any() {
        return Ok(());
    }

    let range = item.entry.range(arena.len()).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "file entry points outside the filename arena",
        )
    })?;
    if arena.get(range.end).copied() != Some(0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file entry is not terminated in the filename arena",
        ));
    }
    // SAFETY: range.start is within arena and the entry's trailing byte was
    // checked above to be the C-string terminator.
    let name_ptr = unsafe { arena.as_ptr().add(range.start) as *const libc::c_char };
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

    if res != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstatat returned 0 (success), meaning stat_buf is initialized.
    let stat = unsafe { stat_buf.assume_init() };

    let is_dir = (stat.st_mode & S_IFMT) == S_IFDIR;
    item.entry.set_is_dir(is_dir);

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
        if link_len < 0 {
            return Err(io::Error::last_os_error());
        }
        Some(link_buf[..link_len as usize].to_vec())
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
            u64::try_from(stat.st_size).unwrap_or_default()
        } else {
            0
        },
        mtime: if flags.need_mtime { stat.st_mtime } else { 0 },
        blocks: if flags.need_blocks { stat.st_blocks } else { 0 },
        ino: if flags.need_ino { stat.st_ino } else { 0 },
        symlink_target,
        git_status: GitStatus::Clean,
    });
    Ok(())
}

fn sort_entries(entries_data: &mut [FileInfo], arena: &[u8], config: &Config) {
    entries_data.sort_unstable_by(|a, b| {
        if config.group_directories_first {
            let a_is_dir = a.entry.is_dir();
            let b_is_dir = b.entry.is_dir();
            if a_is_dir != b_is_dir {
                return if config.reverse_sort {
                    a_is_dir.cmp(&b_is_dir)
                } else {
                    b_is_dir.cmp(&a_is_dir)
                };
            }
        }

        let primary_cmp = match config.sort_mode {
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

        let primary_cmp = if config.reverse_sort {
            primary_cmp.reverse()
        } else {
            primary_cmp
        };
        if primary_cmp != std::cmp::Ordering::Equal {
            primary_cmp
        } else {
            let name_a = &arena[a.entry.start()..a.entry.start() + a.entry.len()];
            let name_b = &arena[b.entry.start()..b.entry.start() + b.entry.len()];
            let name_cmp = name_a.cmp(name_b);
            if config.reverse_sort {
                name_cmp.reverse()
            } else {
                name_cmp
            }
        }
    });
}

fn canonical_git_path(path: &Path, dereference: bool) -> io::Result<std::path::PathBuf> {
    if !dereference && std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        let parent = path.parent().unwrap_or(Path::new(".")).canonicalize()?;
        let file_name = path.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "file path has no final component",
            )
        })?;
        Ok(parent.join(file_name))
    } else {
        path.canonicalize()
    }
}

fn apply_git_status(
    entries_data: &mut [FileInfo],
    arena: &[u8],
    path: &Path,
    single: bool,
    dereference: bool,
    snapshot: &GitStatusSnapshot,
) -> io::Result<()> {
    let canonical_path = canonical_git_path(path, dereference)?;
    let workdir = &snapshot.workdir;
    let statuses = &snapshot.statuses;

    let base_dir = if single {
        canonical_path.parent().unwrap_or(workdir)
    } else {
        &canonical_path
    };

    for item in entries_data {
        let Some(meta) = item.metadata.as_mut() else {
            continue;
        };
        let Some(range) = item.entry.range(arena.len()) else {
            continue;
        };
        let Some(name_bytes) = arena.get(range) else {
            continue;
        };
        let entry_path = if single {
            canonical_path.clone()
        } else {
            base_dir.join(std::ffi::OsStr::from_bytes(name_bytes))
        };
        let Ok(relative_path) = entry_path.strip_prefix(workdir) else {
            continue;
        };

        if let Some(status) = statuses.get(relative_path) {
            meta.git_status = match status {
                FgStatus::Added => GitStatus::New,
                FgStatus::Renamed => GitStatus::Renamed,
                FgStatus::Modified | FgStatus::TypeChange => GitStatus::Modified,
                FgStatus::Deleted => GitStatus::Deleted,
                FgStatus::Ignored => GitStatus::Ignored,
                FgStatus::Conflicted => GitStatus::Conflicted,
                FgStatus::Untracked => GitStatus::Untracked,
                FgStatus::Clean => GitStatus::Clean,
            };
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::SortMode;
    use std::fs;

    fn config(path: std::path::PathBuf) -> Config {
        Config {
            path,
            show_all: false,
            list_dirs: false,
            long_listing: false,
            one_per_line: false,
            human_readable: false,
            raw: false,
            show_inode: false,
            sort_mode: SortMode::Name,
            reverse_sort: false,
            use_color: false,
            tree: false,
            tree_depth: None,
            group_directories_first: false,
            show_icons: false,
            git: true,
            dereference: false,
            recursive: false,
            verbose: false,
        }
    }

    fn untracked_file(result: &ListResult, name: &[u8]) -> bool {
        result.entries.iter().any(|item| {
            let Some(range) = item.entry.range(result.arena.len()) else {
                return false;
            };
            result.arena.get(range) == Some(name)
                && item
                    .metadata
                    .as_ref()
                    .is_some_and(|metadata| metadata.git_status == GitStatus::Untracked)
        })
    }

    #[test]
    fn recursive_scans_reuse_git_status_per_worktree() {
        let temp = tempfile::tempdir().unwrap();
        let git_dir = temp.path().join(".git");
        let child_dir = temp.path().join("child");
        fs::create_dir_all(&git_dir).unwrap();
        fs::create_dir(&child_dir).unwrap();
        fs::write(temp.path().join("root.txt"), b"root").unwrap();
        fs::write(child_dir.join("nested.txt"), b"nested").unwrap();

        let mut index = b"DIRC".to_vec();
        index.extend_from_slice(&2u32.to_be_bytes());
        index.extend_from_slice(&0u32.to_be_bytes());
        let index_path = git_dir.join("index");
        fs::write(&index_path, index).unwrap();

        let mut cache = GitStatusCache::default();
        list_dir_with_git_status_cache(&config(temp.path().to_path_buf()), &mut cache).unwrap();

        // If the second scan recomputes repository status, it will fail to parse
        // the now-removed index instead of using the operation's cached snapshot.
        fs::remove_file(index_path).unwrap();
        let child_result = list_dir_with_git_status_cache(&config(child_dir), &mut cache).unwrap();
        assert!(untracked_file(&child_result, b"nested.txt"));
        assert_eq!(cache.snapshots.len(), 1);
    }
}
