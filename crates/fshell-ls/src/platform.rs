// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Platform-specific dirent handling
//!
//! BSD systems (macOS, FreeBSD, NetBSD, OpenBSD) use `d_namlen` for name length.
//! Linux and other systems use null-terminated strings.

#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
))]
/// Get the name bytes from a dirent entry (BSD variant)
pub fn get_dirent_name(entry: &libc::dirent) -> &[u8] {
    // SAFETY: entry.d_name is a valid char array and d_namlen contains the valid name length.
    unsafe {
        std::slice::from_raw_parts(entry.d_name.as_ptr() as *const u8, entry.d_namlen as usize)
    }
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
)))]
/// Get the name bytes from a dirent entry (Linux/POSIX variant)
pub fn get_dirent_name(entry: &libc::dirent) -> &[u8] {
    // SAFETY: entry.d_name is a valid null-terminated C string representing the directory entry name.
    unsafe { std::ffi::CStr::from_ptr(entry.d_name.as_ptr()).to_bytes() }
}
