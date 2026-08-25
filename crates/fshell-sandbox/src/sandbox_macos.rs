// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::profile::{SandboxMode, SandboxProfile};
use std::ffi::CString;
use std::path::Path;

unsafe extern "C" {
    fn sandbox_init(
        profile: *const libc::c_char,
        flags: u64,
        errorbuf: *mut *mut libc::c_char,
    ) -> libc::c_int;
    fn sandbox_free_error(errorbuf: *mut libc::c_char);
}

/// Apply macOS SBPL sandbox via sandbox_init inside the pre_exec closure.
pub fn apply_sbpl_sandbox(profile: &SandboxProfile, cwd: &Path) -> Result<(), String> {
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
    let sbpl_str = generate_sbpl_profile(profile, cwd);
    let c_str =
        CString::new(sbpl_str).map_err(|e| format!("Failed to create CString for SBPL: {e}"))?;

    let mut error_buf: *mut libc::c_char = std::ptr::null_mut();
    let rc = unsafe { sandbox_init(c_str.as_ptr(), 0, &mut error_buf) };
    if rc != 0 {
        let err_msg = if !error_buf.is_null() {
            let s = unsafe {
                std::ffi::CStr::from_ptr(error_buf)
                    .to_string_lossy()
                    .into_owned()
            };
            unsafe { sandbox_free_error(error_buf) };
            s
        } else {
            "sandbox_init returned error".to_string()
        };
        return Err(format!("macOS SBPL sandbox_init failed: {err_msg}"));
    }
    Ok(())
}

pub fn generate_sbpl_profile(profile: &SandboxProfile, cwd: &Path) -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/shared".into());
    let mut sbpl = String::from("(version 1)\n(allow default)\n");

    // 1. Allow writes to safe execution areas: /tmp, /var/tmp, /var/folders, /dev, cwd, and custom allow paths
    sbpl.push_str(
        "(allow file-write*\n\
         (subpath \"/tmp\")\n\
         (subpath \"/private/tmp\")\n\
         (subpath \"/var/tmp\")\n\
         (subpath \"/private/var/tmp\")\n\
         (subpath \"/var/folders\")\n\
         (subpath \"/private/var/folders\")\n\
         (subpath \"/dev/null\")\n\
         (subpath \"/dev/zero\")\n\
         (subpath \"/dev/urandom\")\n\
         (subpath \"/dev/random\")\n\
         (subpath \"/dev/tty\")\n\
         (subpath \"/dev/pts\")\n\
         (subpath \"/dev/dtracehelper\")\n",
    );

    let emit_path = |target_sbpl: &mut String, p: &Path| {
        let p_str = p.to_string_lossy();
        if !p_str.is_empty() {
            target_sbpl.push_str(&format!("(subpath \"{}\")\n", escape_sbpl(&p_str)));
        }
        if let Ok(canon) = std::fs::canonicalize(p) {
            let canon_str = canon.to_string_lossy();
            if canon_str != p_str && !canon_str.is_empty() {
                target_sbpl.push_str(&format!("(subpath \"{}\")\n", escape_sbpl(&canon_str)));
            }
        }
    };

    emit_path(&mut sbpl, cwd);

    for allow_path in &profile.allow_write_paths {
        emit_path(&mut sbpl, allow_path);
    }
    sbpl.push_str(")\n");

    // 2. Deny writes to sensitive system directories and explicitly denied paths (overriding allows)
    sbpl.push_str(
        "(deny file-write*\n\
         (subpath \"/etc\")\n\
         (subpath \"/usr\")\n\
         (subpath \"/bin\")\n\
         (subpath \"/sbin\")\n\
         (subpath \"/System\")\n\
         (subpath \"/private/etc\")\n\
         (subpath \"/private/var/db\")\n",
    );
    sbpl.push_str(&format!("(subpath \"{home}/.ssh\")\n"));
    sbpl.push_str(&format!("(subpath \"{home}/.gnupg\")\n"));
    sbpl.push_str(&format!("(subpath \"{home}/.aws\")\n"));
    sbpl.push_str(&format!("(subpath \"{home}/.config/fsh\")\n"));

    for deny_path in &profile.deny_write_paths {
        emit_path(&mut sbpl, deny_path);
    }
    sbpl.push_str(")\n");

    // Network isolation if requested
    if !profile.allow_network || profile.mode == SandboxMode::Isolated {
        sbpl.push_str(
            "(deny network-outbound)\n\
             (deny network-inbound)\n\
             (deny network-bind)\n",
        );
    }

    sbpl
}

fn escape_sbpl(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_generate_sbpl_default() {
        let profile = SandboxProfile::new(SandboxMode::ReadOnlySystem);
        let sbpl = generate_sbpl_profile(&profile, Path::new("/Users/test/myproject"));
        assert!(sbpl.contains("(version 1)"));
        assert!(sbpl.contains("(deny file-write*"));
        assert!(sbpl.contains("/etc"));
        assert!(sbpl.contains("/System"));
        assert!(sbpl.contains("(allow file-write*"));
        assert!(sbpl.contains("/Users/test/myproject"));
        assert!(!sbpl.contains("deny network-outbound"));
    }

    #[test]
    fn test_generate_sbpl_isolated() {
        let profile = SandboxProfile::new(SandboxMode::Isolated);
        let sbpl = generate_sbpl_profile(&profile, Path::new("/tmp"));
        assert!(sbpl.contains("deny network-outbound"));
    }

    #[test]
    fn test_generate_sbpl_custom_paths() {
        let profile = SandboxProfile::new(SandboxMode::ReadOnlySystem)
            .allow_write(PathBuf::from("/custom/allowed"))
            .deny_write(PathBuf::from("/custom/blocked"));
        let sbpl = generate_sbpl_profile(&profile, Path::new("/tmp"));
        assert!(sbpl.contains("/custom/allowed"));
        assert!(sbpl.contains("/custom/blocked"));
    }
}
