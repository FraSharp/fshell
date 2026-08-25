// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::profile::{SandboxMode, SandboxProfile};
use std::ffi::CString;
use std::path::Path;

const LANDLOCK_RULE_PATH_BENEATH: i32 = 1;
const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1 << 0;

const LANDLOCK_ACCESS_FS_WRITE_FILE: u64 = 1 << 1;
const LANDLOCK_ACCESS_FS_REMOVE_DIR: u64 = 1 << 4;
const LANDLOCK_ACCESS_FS_REMOVE_FILE: u64 = 1 << 5;
const LANDLOCK_ACCESS_FS_MAKE_CHAR: u64 = 1 << 6;
const LANDLOCK_ACCESS_FS_MAKE_DIR: u64 = 1 << 7;
const LANDLOCK_ACCESS_FS_MAKE_REG: u64 = 1 << 8;
const LANDLOCK_ACCESS_FS_MAKE_SYM: u64 = 1 << 9;
const LANDLOCK_ACCESS_FS_MAKE_FIFO: u64 = 1 << 10;
const LANDLOCK_ACCESS_FS_MAKE_SOCK: u64 = 1 << 11;
const LANDLOCK_ACCESS_FS_TRUNCATE: u64 = 1 << 13;
const LANDLOCK_ACCESS_FS_RENAME: u64 = 1 << 14;

const LANDLOCK_ACCESS_NET_BIND_TCP: u64 = 1 << 0;
const LANDLOCK_ACCESS_NET_CONNECT_TCP: u64 = 1 << 1;

#[repr(C)]
struct landlock_ruleset_attr {
    handled_access_fs: u64,
    handled_access_net: u64,
}

#[repr(C)]
struct landlock_path_beneath_attr {
    allowed_access: u64,
    parent_fd: i32,
}

/// Apply Landlock sandbox before execve inside the pre_exec closure.
pub fn apply_landlock_sandbox(profile: &SandboxProfile, cwd: &Path) -> Result<(), String> {
    // 1. Check Landlock ABI version
    let abi = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<landlock_ruleset_attr>(),
            0,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };

    // If kernel doesn't support Landlock (< 5.13 or disabled), apply PR_SET_NO_NEW_PRIVS as fallback
    if abi < 1 {
        unsafe {
            libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
        }
        return Ok(());
    }

    let mut handled_fs = LANDLOCK_ACCESS_FS_WRITE_FILE
        | LANDLOCK_ACCESS_FS_REMOVE_FILE
        | LANDLOCK_ACCESS_FS_REMOVE_DIR
        | LANDLOCK_ACCESS_FS_MAKE_CHAR
        | LANDLOCK_ACCESS_FS_MAKE_DIR
        | LANDLOCK_ACCESS_FS_MAKE_REG
        | LANDLOCK_ACCESS_FS_MAKE_SYM
        | LANDLOCK_ACCESS_FS_MAKE_FIFO
        | LANDLOCK_ACCESS_FS_MAKE_SOCK
        | LANDLOCK_ACCESS_FS_RENAME;

    if abi >= 3 {
        handled_fs |= LANDLOCK_ACCESS_FS_TRUNCATE;
    }

    let mut handled_net = 0;
    if abi >= 4 && (!profile.allow_network || profile.mode == SandboxMode::Isolated) {
        handled_net = LANDLOCK_ACCESS_NET_BIND_TCP | LANDLOCK_ACCESS_NET_CONNECT_TCP;
    }

    let attr = landlock_ruleset_attr {
        handled_access_fs: handled_fs,
        handled_access_net: handled_net,
    };

    let ruleset_fd = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &attr as *const _,
            std::mem::size_of::<landlock_ruleset_attr>(),
            0,
        )
    };
    if ruleset_fd < 0 {
        // Fallback to PR_SET_NO_NEW_PRIVS
        unsafe {
            libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
        }
        return Ok(());
    }

    // Monitor mode: log the effective profile before enforcing
    if profile.mode == SandboxMode::Monitor {
        eprintln!(
            "[fshell sandbox:monitor] cwd={} allow_write={:?} deny_write={:?} network={}",
            cwd.display(),
            profile.allow_write_paths,
            profile.deny_write_paths,
            if profile.allow_network {
                "allow"
            } else {
                "deny"
            }
        );
    }

    // Paths allowed for write mutations — deny list overrides allow list
    let is_denied = |path: &Path| {
        profile
            .deny_write_paths
            .iter()
            .any(|deny| path.starts_with(deny) || deny.starts_with(path))
    };
    let mut allowed_paths = Vec::new();
    if !is_denied(Path::new("/tmp")) {
        if let Ok(c) = CString::new("/tmp") {
            allowed_paths.push(c);
        }
    }
    if !is_denied(Path::new("/var/tmp")) {
        if let Ok(c) = CString::new("/var/tmp") {
            allowed_paths.push(c);
        }
    }

    if !is_denied(cwd) {
        if let Ok(c_cwd) = CString::new(cwd.to_string_lossy().into_owned()) {
            allowed_paths.push(c_cwd);
        }
    }

    for p in &profile.allow_write_paths {
        if is_denied(p) {
            continue;
        }
        if let Ok(c_p) = CString::new(p.to_string_lossy().into_owned()) {
            allowed_paths.push(c_p);
        }
    }

    for path_c in &allowed_paths {
        let fd = unsafe { libc::open(path_c.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
        if fd < 0 {
            continue;
        }

        let path_attr = landlock_path_beneath_attr {
            allowed_access: handled_fs,
            parent_fd: fd,
        };

        unsafe {
            libc::syscall(
                libc::SYS_landlock_add_rule,
                ruleset_fd,
                LANDLOCK_RULE_PATH_BENEATH,
                &path_attr as *const _,
                0,
            );
            libc::close(fd);
        }
    }

    unsafe {
        libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
        let ret = libc::syscall(libc::SYS_landlock_restrict_self, ruleset_fd, 0);
        libc::close(ruleset_fd as i32);

        if ret < 0 {
            return Err("landlock_restrict_self failed".into());
        }
    }
    Ok(())
}
