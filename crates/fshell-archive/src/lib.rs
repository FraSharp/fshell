// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Archive decoding is kept separate from the shell's capability policy. The caller
//! supplies an already-authorized, open archive and a canonical, authorized output
//! directory. libarchive only reads from the supplied fd; all writes use cap-std
//! directory handles, never libarchive's ambient-authority disk writer.

mod ffi;

use cap_std::ambient_authority;
use cap_std::fs::{Dir, DirBuilder, DirBuilderExt, OpenOptions, Permissions};
use fshell_hash::FxHashSet;
use std::ffi::{CStr, OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::ptr::NonNull;
use std::time::{Duration, SystemTime};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExtractError {
    #[error("archive read failed: {0}")]
    Archive(String),
    #[error("unsafe archive entry: {0}")]
    Unsafe(String),
    #[error("extraction limit exceeded: {0}")]
    Limit(String),
    #[error("destination conflict: {0}")]
    Conflict(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Explicit resource budget for archives whose expanded size is not known ahead of time.
/// The limit is checked on actual decoded bytes, not an untrusted archive header.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_entries: usize,
    pub max_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_entries: 100_000,
            max_bytes: 16 * 1024 * 1024 * 1024,
        }
    }
}

const MAX_PATH_BYTES: usize = 4096;
const MAX_DEPTH: usize = 128;
const BUFFER_SIZE: usize = 64 * 1024;

struct ArchiveReader<'a> {
    handle: NonNull<ffi::archive>,
    // The C reader borrows the file descriptor but never closes it.
    _file: &'a File,
}

impl ArchiveReader<'_> {
    fn new<'a>(file: &'a File, signature: &[u8]) -> Result<ArchiveReader<'a>, ExtractError> {
        // SAFETY: A non-null archive_read_new result is owned exclusively by this
        // wrapper and freed on every path, including failed initialization.
        let handle = NonNull::new(unsafe { ffi::archive_read_new() })
            .ok_or_else(|| ExtractError::Archive("cannot allocate archive reader".into()))?;
        let reader = ArchiveReader {
            handle,
            _file: file,
        };
        let ptr = reader.handle.as_ptr();
        // Register only in-process filters. Some libarchive filters (lrzip,
        // grzip, lzop and unavailable codec fallbacks) execute external programs.
        // A codec that returns WARN has registered a program fallback, so do not
        // open the archive after that result. No ProcessSpawn permission is needed.
        let filter = if signature.starts_with(&[0x1f, 0x8b]) {
            Some(unsafe { ffi::archive_read_support_filter_gzip(ptr) })
        } else if signature.starts_with(b"BZh") {
            Some(unsafe { ffi::archive_read_support_filter_bzip2(ptr) })
        } else if signature.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) {
            Some(unsafe { ffi::archive_read_support_filter_xz(ptr) })
        } else if signature.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
            Some(unsafe { ffi::archive_read_support_filter_zstd(ptr) })
        } else if signature.starts_with(&[0x04, 0x22, 0x4d, 0x18]) {
            Some(unsafe { ffi::archive_read_support_filter_lz4(ptr) })
        } else if signature.starts_with(&[0x1f, 0x9d]) {
            Some(unsafe { ffi::archive_read_support_filter_compress(ptr) })
        } else if signature.starts_with(b"LZIP") {
            Some(unsafe { ffi::archive_read_support_filter_lzip(ptr) })
        } else if signature.starts_with(&[0xed, 0xab, 0xee, 0xdb]) {
            Some(unsafe { ffi::archive_read_support_filter_rpm(ptr) })
        } else {
            None
        };
        if let Some(status) = filter {
            reader.check(status)?;
        }
        // These are archive containers, not format_raw (which accepts *any*
        // bytes) or mtree (which can read arbitrary host files via contents=).
        // Filter registration only selects decoding; libarchive still detects the
        // container format from its contents, not its extension.
        for register in [
            ffi::archive_read_support_format_tar,
            ffi::archive_read_support_format_zip,
            ffi::archive_read_support_format_7zip,
            ffi::archive_read_support_format_rar,
            ffi::archive_read_support_format_rar5,
            ffi::archive_read_support_format_cpio,
            ffi::archive_read_support_format_ar,
            ffi::archive_read_support_format_cab,
            ffi::archive_read_support_format_lha,
            ffi::archive_read_support_format_iso9660,
            ffi::archive_read_support_format_xar,
            ffi::archive_read_support_format_warc,
        ] {
            // SAFETY: The reader is not yet open and `ptr` remains valid.
            reader.check(unsafe { register(ptr) })?;
        }
        if filter.is_some() {
            // Raw means a *recognized compressed stream* with no archive inside.
            // Never register it for arbitrary input (it would turn junk into a
            // successful extraction of a file called `data`).
            reader.check(unsafe { ffi::archive_read_support_format_raw(ptr) })?;
        }
        // SAFETY: file stays open until the reader is closed. libarchive's
        // archive_read_open_fd supplies its own seek and close callbacks; its
        // close callback frees only its buffer, not the caller's file descriptor.
        reader.check(unsafe { ffi::archive_read_open_fd(ptr, file.as_raw_fd(), BUFFER_SIZE) })?;
        Ok(reader)
    }

    fn check(&self, code: i32) -> Result<(), ExtractError> {
        if code == ffi::ARCHIVE_OK as i32 {
            Ok(())
        } else {
            // SAFETY: the reader owns this live archive pointer. libarchive
            // owns the error string until the next library call; copy it now.
            let msg = unsafe { ffi::archive_error_string(self.handle.as_ptr()) };
            let detail = if msg.is_null() {
                format!("libarchive returned status {code}")
            } else {
                unsafe { CStr::from_ptr(msg) }
                    .to_string_lossy()
                    .into_owned()
            };
            Err(ExtractError::Archive(detail))
        }
    }

    fn next(&mut self) -> Result<Option<Entry>, ExtractError> {
        let mut entry = std::ptr::null_mut();
        // SAFETY: libarchive owns the entry until the next header call. All
        // metadata is copied into Rust-owned values before returning.
        let code = unsafe { ffi::archive_read_next_header(self.handle.as_ptr(), &mut entry) };
        if code == ffi::ARCHIVE_EOF as i32 {
            return Ok(None);
        }
        self.check(code)?;
        if entry.is_null() {
            return Err(ExtractError::Archive("entry without a header".into()));
        }
        let path = entry_path(unsafe { ffi::archive_entry_pathname(entry) })?;
        let symlink = optional_entry_path(unsafe { ffi::archive_entry_symlink(entry) });
        let hardlink = optional_entry_path(unsafe { ffi::archive_entry_hardlink(entry) });
        let kind = unsafe { ffi::archive_entry_filetype(entry) } as u32;
        let mode = unsafe { ffi::archive_entry_perm(entry) } as u32;
        let size = unsafe { ffi::archive_entry_size(entry) };
        let mtime = if unsafe { ffi::archive_entry_mtime_is_set(entry) } != 0 {
            let sec = unsafe { ffi::archive_entry_mtime(entry) };
            let nanos = unsafe { ffi::archive_entry_mtime_nsec(entry) };
            if sec >= 0 && (0..1_000_000_000).contains(&nanos) {
                SystemTime::UNIX_EPOCH.checked_add(Duration::new(sec as u64, nanos as u32))
            } else {
                None
            }
        } else {
            None
        };
        Ok(Some(Entry {
            path,
            symlink,
            hardlink,
            kind,
            mode,
            size,
            mtime,
        }))
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, ExtractError> {
        // SAFETY: `buf` is writable for its entire length, and the reader owns
        // the live archive pointer for the duration of this call.
        let n = unsafe {
            ffi::archive_read_data(
                self.handle.as_ptr(),
                buf.as_mut_ptr().cast::<std::ffi::c_void>(),
                buf.len(),
            )
        };
        if n < 0 {
            self.check(n as i32)?;
            return Err(ExtractError::Archive("cannot read entry data".into()));
        }
        Ok(n as usize)
    }

    fn finish(self) -> Result<(), ExtractError> {
        // SAFETY: this is the only owner of the live reader. Drop still frees
        // it even when the close operation reports a checksum/decoder error.
        self.check(unsafe { ffi::archive_read_close(self.handle.as_ptr()) })
    }
}

impl Drop for ArchiveReader<'_> {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this wrapper. `archive_read_free`
        // closes it if necessary and releases all native allocations.
        unsafe { ffi::archive_read_free(self.handle.as_ptr()) };
    }
}

struct Entry {
    path: PathBuf,
    symlink: Option<PathBuf>,
    hardlink: Option<PathBuf>,
    kind: u32,
    mode: u32,
    size: i64,
    mtime: Option<SystemTime>,
}

fn entry_path(ptr: *const std::ffi::c_char) -> Result<PathBuf, ExtractError> {
    if ptr.is_null() {
        return Err(ExtractError::Unsafe("entry has no pathname".into()));
    }
    // SAFETY: libarchive owns this NUL-terminated name until the next header.
    let bytes = unsafe { CStr::from_ptr(ptr) }.to_bytes();
    Ok(PathBuf::from(OsStr::from_bytes(bytes)))
}

fn optional_entry_path(ptr: *const std::ffi::c_char) -> Option<PathBuf> {
    if ptr.is_null() {
        None
    } else {
        // SAFETY: libarchive owns the name until the next header; copy it.
        Some(PathBuf::from(OsStr::from_bytes(
            unsafe { CStr::from_ptr(ptr) }.to_bytes(),
        )))
    }
}

fn normalize(path: &Path) -> Result<PathBuf, ExtractError> {
    if path.as_os_str().as_bytes().len() > MAX_PATH_BYTES {
        return Err(ExtractError::Limit(format!(
            "path longer than {MAX_PATH_BYTES} bytes"
        )));
    }
    let mut clean = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(s) => clean.push(s),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) | Component::ParentDir => {
                return Err(ExtractError::Unsafe(format!(
                    "path {:?} escapes destination",
                    path
                )));
            }
        }
    }
    if clean.components().count() > MAX_DEPTH {
        return Err(ExtractError::Limit(format!(
            "path exceeds {MAX_DEPTH} components"
        )));
    }
    Ok(clean)
}

fn checked_symlink_target(link: &Path, target: &Path) -> Result<PathBuf, ExtractError> {
    if target.as_os_str().as_bytes().len() > MAX_PATH_BYTES || target.is_absolute() {
        return Err(ExtractError::Unsafe(format!(
            "symlink {:?} has an outside target {:?}",
            link, target
        )));
    }
    let mut parts: Vec<OsString> = link
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .filter_map(|part| match part {
            Component::Normal(s) => Some(s.to_os_string()),
            _ => None,
        })
        .collect();
    for part in target.components() {
        match part {
            Component::Normal(s) => parts.push(s.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir if !parts.is_empty() => {
                parts.pop();
            }
            _ => {
                return Err(ExtractError::Unsafe(format!(
                    "symlink {:?} escapes destination via {:?}",
                    link, target
                )));
            }
        }
        if parts.len() > MAX_DEPTH {
            return Err(ExtractError::Limit("symlink target too deep".into()));
        }
    }
    Ok(parts.iter().collect())
}

fn account_bytes(total: &mut u64, n: usize, limit: u64) -> Result<(), ExtractError> {
    *total = total
        .checked_add(n as u64)
        .ok_or_else(|| ExtractError::Limit("decoded size overflow".into()))?;
    if *total > limit {
        return Err(ExtractError::Limit(format!(
            "decoded data exceeds {limit} bytes"
        )));
    }
    Ok(())
}

fn drain_entry(
    archive: &mut ArchiveReader<'_>,
    buffer: &mut [u8],
    total: &mut u64,
    limit: u64,
) -> Result<(), ExtractError> {
    loop {
        let n = archive.read(buffer)?;
        if n == 0 {
            return Ok(());
        }
        account_bytes(total, n, limit)?;
    }
}

fn raw_output_name(source_name: &OsStr) -> PathBuf {
    let bytes = source_name.as_bytes();
    for suffix in [
        b".gz".as_slice(),
        b".bz2",
        b".xz",
        b".zst",
        b".lz4",
        b".lz",
        b".Z",
    ] {
        if bytes.ends_with(suffix) && bytes.len() > suffix.len() {
            return PathBuf::from(OsString::from_vec(
                bytes[..bytes.len() - suffix.len()].to_vec(),
            ));
        }
    }
    let mut name = bytes.to_vec();
    name.extend_from_slice(b".unpacked");
    PathBuf::from(OsString::from_vec(name))
}

#[derive(Clone)]
struct DirectoryEntry {
    path: PathBuf,
    mode: Option<u32>,
    mtime: Option<SystemTime>,
}

struct SymlinkEntry {
    path: PathBuf,
    target: PathBuf,
    resolved: PathBuf,
}

/// Keep the private staging tree bound to the already-open destination fd,
/// including cleanup. Path-based tempdir creation could race a rename of the
/// destination between authorization and creation.
struct Staging {
    root: Dir,
    name: String,
    dir: Dir,
}

impl Staging {
    fn new(root: Dir) -> Result<Self, ExtractError> {
        for _ in 0..16 {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random)
                .map_err(|e| io::Error::other(format!("cannot generate staging name: {e}")))?;
            let name = format!(".fsh-extract-{:032x}", u128::from_be_bytes(random));
            let mut options = DirBuilder::new();
            options.mode(0o700);
            match root.create_dir_with(&name, &options) {
                Ok(()) => match root.open_dir(&name) {
                    Ok(dir) => return Ok(Self { root, name, dir }),
                    Err(e) => {
                        let _ = root.remove_dir(&name);
                        return Err(e.into());
                    }
                },
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(ExtractError::Io(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a private staging directory",
        )))
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = self.root.remove_dir_all(&self.name);
    }
}

/// Extract an authorized file into an authorized, already-existing directory.
///
/// The archive is decoded into a private staging directory first, so malformed
/// archives and limit failures leave the destination untouched. Publication
/// never replaces existing paths. Multiple top-level entries cannot be made
/// atomic as a group; a publication-time I/O failure may leave earlier entries.
/// Links are created only after ordinary files, and no decoder or archive
/// writer is ever given ambient filesystem write access.
pub fn extract(
    source: &mut File,
    source_name: &OsStr,
    destination: &Path,
    limits: Limits,
) -> Result<usize, ExtractError> {
    let staging = Staging::new(Dir::open_ambient_dir(destination, ambient_authority())?)?;
    let dest = &staging.root;
    let stage = &staging.dir;
    let mut signature = [0u8; 8];
    let n = source.read(&mut signature)?;
    source.seek(SeekFrom::Start(0))?;
    let mut archive = ArchiveReader::new(source, &signature[..n])?;

    let mut seen = FxHashSet::default();
    let mut dirs = Vec::<DirectoryEntry>::new();
    let mut directory_paths = FxHashSet::default();
    let mut regulars = FxHashSet::default();
    let mut files = Vec::<PathBuf>::new();
    let mut symlinks = Vec::<SymlinkEntry>::new();
    let mut hardlinks = Vec::<(PathBuf, PathBuf)>::new();
    let mut total = 0u64;
    let mut count = 0usize;
    let mut buffer = [0u8; BUFFER_SIZE];
    while let Some(entry) = archive.next()? {
        count = count
            .checked_add(1)
            .ok_or_else(|| ExtractError::Limit("entry count overflow".into()))?;
        if count > limits.max_entries {
            return Err(ExtractError::Limit(format!(
                "more than {} entries",
                limits.max_entries
            )));
        }
        let path = if unsafe { ffi::archive_format(archive.handle.as_ptr()) }
            == ffi::ARCHIVE_FORMAT_RAW as i32
        {
            raw_output_name(source_name)
        } else {
            entry.path
        };
        let path = normalize(&path)?;
        if path.as_os_str().is_empty() && entry.kind == 0o040000 {
            continue; // an explicit `./` root entry
        }
        if path.as_os_str().is_empty() || !seen.insert(path.clone()) {
            return Err(ExtractError::Unsafe(format!(
                "empty or duplicate entry {:?}",
                path
            )));
        }
        // All parents are created beneath the staging descriptor. No symlinks
        // exist in staging until after all entries have been read.
        let mut parent = path.parent();
        while let Some(p) = parent {
            if p.as_os_str().is_empty() {
                break;
            }
            directory_paths.insert(p.to_path_buf());
            parent = p.parent();
        }
        if let Some(p) = path.parent() {
            stage.create_dir_all(p)?;
        }
        if entry.size >= 0 && entry.size as u64 > limits.max_bytes.saturating_sub(total) {
            return Err(ExtractError::Limit(format!(
                "decoded data exceeds {} bytes",
                limits.max_bytes
            )));
        }
        let unused_data = entry.hardlink.is_some() || entry.kind != 0o100000;
        if let Some(target) = entry.hardlink {
            let target = normalize(&target)?;
            if target.as_os_str().is_empty() {
                return Err(ExtractError::Unsafe("hardlink to archive root".into()));
            }
            hardlinks.push((path, target));
        } else {
            match entry.kind {
                0o040000 => {
                    stage.create_dir_all(&path)?;
                    directory_paths.insert(path.clone());
                    dirs.push(DirectoryEntry {
                        path,
                        mode: Some(entry.mode),
                        mtime: entry.mtime,
                    });
                }
                0o100000 => {
                    let mut output =
                        stage.open_with(&path, OpenOptions::new().write(true).create_new(true))?;
                    loop {
                        let n = archive.read(&mut buffer)?;
                        if n == 0 {
                            break;
                        }
                        account_bytes(&mut total, n, limits.max_bytes)?;
                        output.write_all(&buffer[..n])?;
                    }
                    let mode = if entry.mode == 0 {
                        0o644
                    } else {
                        entry.mode & 0o777
                    };
                    output.set_permissions(Permissions::from_std(
                        std::fs::Permissions::from_mode(mode),
                    ))?;
                    if let Some(mtime) = entry.mtime {
                        output
                            .into_std()
                            .set_times(std::fs::FileTimes::new().set_modified(mtime))?;
                    }
                    regulars.insert(path.clone());
                    files.push(path);
                }
                0o120000 => {
                    let target = entry.symlink.ok_or_else(|| {
                        ExtractError::Unsafe(format!("symlink {:?} has no target", path))
                    })?;
                    let resolved = checked_symlink_target(&path, &target)?;
                    symlinks.push(SymlinkEntry {
                        path,
                        target,
                        resolved,
                    });
                }
                _ => {
                    return Err(ExtractError::Unsafe(format!(
                        "unsupported special entry {:?} (type {:o})",
                        path, entry.kind
                    )));
                }
            }
        }
        // A malformed archive can place payload bytes on a directory or
        // link. Account for them instead of letting the next header silently
        // skip an unbounded amount of decoded data.
        if unused_data {
            drain_entry(&mut archive, &mut buffer, &mut total, limits.max_bytes)?;
        }
    }
    archive.finish()?;
    // Hardlinks can refer forward, but only to ordinary files written by THIS
    // archive. Never link to an existing file in the destination or a symlink.
    for (path, target) in &hardlinks {
        if !regulars.contains(target) {
            return Err(ExtractError::Unsafe(format!(
                "hardlink {:?} refers outside extracted files: {:?}",
                path, target
            )));
        }
        stage.hard_link(target, stage, path)?;
        files.push(path.clone());
    }
    // Defer symbolic links until all other entries are present, so no payload
    // can traverse one while staging. A link that collides with an implicit
    // parent directory fails here before publishing any output.
    for link in &symlinks {
        if !regulars.contains(&link.resolved) && !directory_paths.contains(&link.resolved) {
            return Err(ExtractError::Unsafe(format!(
                "symlink {:?} does not target a file or directory in the archive: {:?}",
                link.path, link.target
            )));
        }
        stage.symlink(&link.target, &link.path)?;
    }
    let mut dirs_to_publish: Vec<_> = directory_paths.into_iter().collect();
    dirs_to_publish.sort_by_key(|p| p.components().count());
    for path in &dirs_to_publish {
        match dest.symlink_metadata(path) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(ExtractError::Conflict(format!(
                    "{:?} is not a directory",
                    path
                )));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    for path in files.iter().chain(symlinks.iter().map(|link| &link.path)) {
        match dest.symlink_metadata(path) {
            Ok(_) => return Err(ExtractError::Conflict(format!("{:?} already exists", path))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    let mut new_dirs = FxHashSet::default();
    for path in &dirs_to_publish {
        match dest.create_dir(path) {
            Ok(()) => {
                new_dirs.insert(path.clone());
            }
            Err(e)
                if e.kind() == io::ErrorKind::AlreadyExists
                    && dest.symlink_metadata(path)?.is_dir()
                    && !dest.symlink_metadata(path)?.file_type().is_symlink() => {}
            Err(e) => return Err(e.into()),
        }
    }
    for path in &files {
        stage.hard_link(path, dest, path)?; // creates exclusively; never overwrites
    }
    for link in &symlinks {
        dest.symlink(&link.target, &link.path)?; // creates exclusively; never overwrites
    }
    // Directory metadata is applied last, after populating children. Existing
    // destination directories retain their own permissions and timestamps.
    dirs.sort_by_key(|d| std::cmp::Reverse(d.path.components().count()));
    for dir in dirs {
        if !new_dirs.contains(&dir.path) {
            continue;
        }
        // `open_dir` may return an O_PATH descriptor on Linux. Such a handle
        // is useful for capability-relative lookup, but fchmod/futimens reject
        // it with EBADF. Open a readable descriptor and apply both metadata
        // changes through that stable handle instead.
        let handle = dest.open_with(&dir.path, OpenOptions::new().read(true))?;
        if !handle.metadata()?.is_dir() {
            return Err(ExtractError::Conflict(format!(
                "{:?} is not a directory",
                dir.path
            )));
        }
        if let Some(mode) = dir.mode {
            handle.set_permissions(Permissions::from_std(std::fs::Permissions::from_mode(
                mode & 0o777,
            )))?;
        }
        if let Some(mtime) = dir.mtime {
            handle
                .into_std()
                .set_times(std::fs::FileTimes::new().set_modified(mtime))?;
        }
    }
    Ok(count)
}
