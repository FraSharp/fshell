// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![allow(clippy::unnecessary_cast)]
use fshell_hash::FxHashMap;
use libc::{
    S_IFBLK, S_IFCHR, S_IFDIR, S_IFIFO, S_IFLNK, S_IFMT, S_IFSOCK, S_IRGRP, S_IROTH, S_IRUSR,
    S_ISGID, S_ISUID, S_ISVTX, S_IWGRP, S_IWOTH, S_IWUSR, S_IXGRP, S_IXOTH, S_IXUSR, fstatat,
    getgrgid_r, getpwuid_r, group, passwd,
};
use std::borrow::Cow;
use std::ffi::CStr;
use std::io::Write;

use crate::file::FileInfo;

pub const LARGE_BUFFER_SIZE: usize = 1024 * 1024;

/// Produce a terminal-safe, loss-tolerant display form of filesystem bytes.
/// Control characters are escaped so names cannot add lines, move the cursor,
/// or inject terminal escape sequences into listings.
pub fn escape_name(bytes: &[u8]) -> String {
    escape_name_cow(bytes).into_owned()
}

/// Return a terminal-safe display form, borrowing names that need no changes.
///
/// This avoids allocating for the common case of valid UTF-8 filenames without
/// control characters or backslashes. Invalid UTF-8 and escaped names are owned.
pub fn escape_name_cow(bytes: &[u8]) -> Cow<'_, str> {
    if let Ok(name) = std::str::from_utf8(bytes)
        && !name.chars().any(needs_escaping)
    {
        return Cow::Borrowed(name);
    }

    Cow::Owned(escape_lossy_name(bytes))
}

fn needs_escaping(ch: char) -> bool {
    matches!(ch, '\\' | '\n' | '\r' | '\t') || ch.is_control()
}

fn escape_lossy_name(bytes: &[u8]) -> String {
    let lossy_name = String::from_utf8_lossy(bytes);
    let mut escaped = String::with_capacity(lossy_name.len());
    for ch in lossy_name.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => escaped.push_str(&format!("\\u{{{:x}}}", ch as u32)),
            ch => escaped.push(ch),
        }
    }
    escaped
}

/// Convert file mode bits to Unix permission string (e.g., "drwxr-xr-x").
///
/// Formats the mode as a 10-character string with file type and permission bits.
/// Uses the provided buffer to avoid allocations.
///
/// # Arguments
///
/// * `mode` - File mode from stat
/// * `buf` - Buffer to write into (must be at least 10 bytes)
///
/// # Returns
///
/// A string slice view of the formatted permissions in `buf`.
pub fn get_mode_string(mode: u32, buf: &mut [u8]) -> &str {
    let chars = [
        match mode & (S_IFMT as u32) {
            m if m == (S_IFDIR as u32) => b'd',
            m if m == (S_IFLNK as u32) => b'l',
            m if m == (S_IFIFO as u32) => b'p',
            m if m == (S_IFSOCK as u32) => b's',
            m if m == (S_IFCHR as u32) => b'c',
            m if m == (S_IFBLK as u32) => b'b',
            _ => b'-',
        },
        if mode & (S_IRUSR as u32) != 0 {
            b'r'
        } else {
            b'-'
        },
        if mode & (S_IWUSR as u32) != 0 {
            b'w'
        } else {
            b'-'
        },
        if mode & (S_ISUID as u32) != 0 {
            if mode & (S_IXUSR as u32) != 0 {
                b's'
            } else {
                b'S'
            }
        } else if mode & (S_IXUSR as u32) != 0 {
            b'x'
        } else {
            b'-'
        },
        if mode & (S_IRGRP as u32) != 0 {
            b'r'
        } else {
            b'-'
        },
        if mode & (S_IWGRP as u32) != 0 {
            b'w'
        } else {
            b'-'
        },
        if mode & (S_ISGID as u32) != 0 {
            if mode & (S_IXGRP as u32) != 0 {
                b's'
            } else {
                b'S'
            }
        } else if mode & (S_IXGRP as u32) != 0 {
            b'x'
        } else {
            b'-'
        },
        if mode & (S_IROTH as u32) != 0 {
            b'r'
        } else {
            b'-'
        },
        if mode & (S_IWOTH as u32) != 0 {
            b'w'
        } else {
            b'-'
        },
        if mode & (S_ISVTX as u32) != 0 {
            if mode & (S_IXOTH as u32) != 0 {
                b't'
            } else {
                b'T'
            }
        } else if mode & (S_IXOTH as u32) != 0 {
            b'x'
        } else {
            b'-'
        },
    ];

    buf[..10].copy_from_slice(&chars);
    std::str::from_utf8(&buf[..10]).expect("mode string is always ASCII")
}

/// Format Unix timestamp as human-readable date/time string.
///
/// For files modified within 6 months: "Mon DD HH:MM"
/// For older files: "Mon DD  YYYY"
///
/// # Arguments
///
/// * `mtime` - Modification time (seconds since Unix epoch)
/// * `now` - Current time (seconds since Unix epoch)
/// * `buf` - Buffer to write into (must be at least 64 bytes)
///
/// # Returns
///
/// A string slice view of the formatted time in `buf`.
pub fn format_time_with_now(mtime: i64, now: i64, buf: &mut [u8]) -> &str {
    let six_months = 6 * 30 * 24 * 60 * 60;
    let is_recent = mtime > now - six_months && mtime < now + six_months;

    let mut tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&mtime, &mut tm) };

    let fmt: &[u8] = if is_recent {
        b"%b %e %H:%M\0"
    } else {
        b"%b %e  %Y\0"
    };

    let len = unsafe {
        libc::strftime(
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            fmt.as_ptr() as *const libc::c_char,
            &tm,
        )
    };
    std::str::from_utf8(&buf[..len]).expect("strftime output is always ASCII")
}

/// Get username for a given UID, with caching.
///
/// Looks up the username via thread-safe `getpwuid_r` and caches results to avoid
/// repeated syscalls. Falls back to numeric UID string if lookup fails.
///
/// # Arguments
///
/// * `uid` - User ID to look up
/// * `cache` - Cache of previously looked-up UIDs
///
/// # Returns
///
/// Username or numeric UID as a string slice (borrowed from cache).
pub fn get_user_name(uid: u32, cache: &mut FxHashMap<u32, String>) -> &str {
    cache.entry(uid).or_insert_with(|| {
        let mut pwd = unsafe { std::mem::zeroed::<passwd>() };
        let mut buf = [0 as libc::c_char; 4096];
        let mut result: *mut passwd = std::ptr::null_mut();
        let ret = unsafe {
            getpwuid_r(
                uid,
                &mut pwd,
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut result,
            )
        };
        if ret == 0 && !result.is_null() {
            unsafe {
                CStr::from_ptr((*result).pw_name)
                    .to_string_lossy()
                    .into_owned()
            }
        } else {
            uid.to_string()
        }
    })
}

/// Get group name for a given GID, with caching.
///
/// Looks up the group name via thread-safe `getgrgid_r` and caches results to avoid
/// repeated syscalls. Falls back to numeric GID string if lookup fails.
///
/// # Arguments
///
/// * `gid` - Group ID to look up
/// * `cache` - Cache of previously looked-up GIDs
///
/// # Returns
///
/// Group name or numeric GID as a string slice (borrowed from cache).
pub fn get_group_name(gid: u32, cache: &mut FxHashMap<u32, String>) -> &str {
    cache.entry(gid).or_insert_with(|| {
        let mut grp = unsafe { std::mem::zeroed::<group>() };
        let mut buf = [0 as libc::c_char; 4096];
        let mut result: *mut group = std::ptr::null_mut();
        let ret = unsafe {
            getgrgid_r(
                gid,
                &mut grp,
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut result,
            )
        };
        if ret == 0 && !result.is_null() {
            unsafe {
                CStr::from_ptr((*result).gr_name)
                    .to_string_lossy()
                    .into_owned()
            }
        } else {
            gid.to_string()
        }
    })
}

/// Format file size as string, optionally human-readable.
///
/// In human-readable mode, uses units: B, K, M, G, T, P (base 1024).
/// Writes into provided buffer to avoid allocations.
///
/// # Arguments
///
/// * `size` - File size in bytes
/// * `human` - Whether to use human-readable format
/// * `buf` - Buffer to write into (must be at least 64 bytes)
///
/// # Returns
///
/// A string slice view of the formatted size in `buf`.
pub fn format_size(size: u64, human: bool, buf: &mut [u8]) -> &str {
    let total_len = buf.len();
    let mut output_buf = &mut buf[..];
    if !human {
        write!(output_buf, "{}", size).expect("write to buffer should not fail");
    } else {
        const UNITS: &[&str] = &["B", "K", "M", "G", "T", "P"];
        let mut s = size as f64;
        let mut unit_idx = 0;

        while s >= 1024.0 && unit_idx < UNITS.len() - 1 {
            s /= 1024.0;
            unit_idx += 1;
        }

        if unit_idx == 0 {
            write!(output_buf, "{}{}", s as u64, UNITS[unit_idx])
                .expect("write to buffer should not fail");
        } else {
            write!(output_buf, "{:.1}{}", s, UNITS[unit_idx])
                .expect("write to buffer should not fail");
        }
    }
    let len = total_len - output_buf.len();
    std::str::from_utf8(&buf[..len]).expect("formatted size is always ASCII")
}

/// Determine optimal output buffer size based on entry count.
///
/// Uses tiered buffer sizes to balance memory usage with syscall overhead.
/// Larger directories get proportionally larger buffers.
///
/// # Arguments
///
/// * `entries_count` - Number of directory entries
///
/// # Returns
///
/// Recommended buffer size in bytes.
#[inline]
pub fn determine_buffer_size(entries_count: usize) -> usize {
    match entries_count {
        0..=100 => 8 * 1024,          // 8KB for small directories
        101..=1000 => 32 * 1024,      // 32KB for medium directories
        1001..=10000 => 128 * 1024,   // 128KB for larger directories
        10001..=100000 => 512 * 1024, // 512KB for very large directories
        _ => LARGE_BUFFER_SIZE,       // 1MB for huge directories
    }
}

/// Calculate estimated output buffer size for given entries and format.
///
/// Estimates buffer size based on display format and entry metadata to
/// minimize reallocations during output formatting.
///
/// # Arguments
///
/// * `entries` - File entries to be displayed
/// * `_arena` - Arena containing filenames (currently unused in estimation)
/// * `long_format` - Whether using long listing format
///
/// # Returns
///
/// Estimated buffer size in bytes.
#[inline]
pub fn calculate_output_buffer_size(
    entries: &[FileInfo],
    _arena: &[u8],
    long_format: bool,
) -> usize {
    estimate_output_buffer_size(entries.len(), long_format)
}

fn estimate_output_buffer_size(count: usize, long_format: bool) -> usize {
    let estimated = if long_format {
        // Long format: mode(11) + nlink(5) + user(20) + group(20) + size(20) + time(20) + inode(15) + git(2) + filename(255) = ~368
        count.saturating_mul(512)
    } else {
        // Column format: filename(255) + inode(15) + icon(2) + spacing(4) = ~276
        count.saturating_mul(256)
    };
    estimated.clamp(8 * 1024, LARGE_BUFFER_SIZE)
}

/// Check if a directory entry represents a directory.
///
/// First checks the `d_type` field for efficiency. For unknown types or
/// symlinks (when resolving), falls back to `fstatat`.
///
/// # Arguments
///
/// * `entry` - Directory entry from `readdir`
/// * `dir_fd` - File descriptor of parent directory
/// * `resolve_symlinks` - Whether to resolve symlinks to check their target type
///
/// # Returns
///
/// `true` if the entry is a directory (or a symlink pointing to one if resolving).
#[inline]
pub fn is_directory(
    entry: &libc::dirent,
    dir_fd: i32,
    resolve_symlinks: bool,
) -> std::io::Result<bool> {
    let d_type = entry.d_type;

    if d_type == libc::DT_DIR {
        return Ok(true);
    }

    if d_type == libc::DT_LNK && !resolve_symlinks {
        return Ok(false);
    }

    if d_type != libc::DT_LNK && d_type != libc::DT_UNKNOWN {
        return Ok(false);
    }

    let mut stat_buf = std::mem::MaybeUninit::<libc::stat>::uninit();
    let flags = if resolve_symlinks {
        0
    } else {
        libc::AT_SYMLINK_NOFOLLOW
    };
    // SAFETY: dir_fd is valid, entry.d_name is null-terminated, stat_buf is valid.
    let res = unsafe { fstatat(dir_fd, entry.d_name.as_ptr(), stat_buf.as_mut_ptr(), flags) };

    if res == 0 {
        // SAFETY: fstatat succeeded, stat_buf is initialized.
        let stat_buf = unsafe { stat_buf.assume_init() };
        return Ok((stat_buf.st_mode & libc::S_IFMT) == libc::S_IFDIR);
    }

    let err = std::io::Error::last_os_error();
    if d_type == libc::DT_UNKNOWN {
        Err(err)
    } else {
        // A dangling symlink is still a non-directory entry when following it.
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_utf8_names_are_borrowed() {
        assert!(matches!(
            escape_name_cow("plain-name_π".as_bytes()),
            Cow::Borrowed(_)
        ));
        assert_eq!(escape_name(b"plain-name"), "plain-name");
    }

    #[test]
    fn unsafe_and_invalid_names_keep_their_escaped_display_form() {
        assert_eq!(escape_name_cow(b"a\\b\nc\x1b"), "a\\\\b\\nc\\u{1b}");
        assert_eq!(escape_name_cow(b"bad\xffname"), "bad\u{fffd}name");
        assert!(matches!(escape_name_cow(b"bad\xffname"), Cow::Owned(_)));
    }

    #[test]
    fn output_buffer_estimate_has_a_bounded_capacity() {
        assert_eq!(
            estimate_output_buffer_size(10_000_000, false),
            LARGE_BUFFER_SIZE
        );
        assert_eq!(
            estimate_output_buffer_size(10_000_000, true),
            LARGE_BUFFER_SIZE
        );
    }

    #[test]
    fn empty_listing_still_gets_a_useful_buffer_estimate() {
        assert_eq!(estimate_output_buffer_size(0, false), 8 * 1024);
        assert_eq!(estimate_output_buffer_size(0, true), 8 * 1024);
    }
}
