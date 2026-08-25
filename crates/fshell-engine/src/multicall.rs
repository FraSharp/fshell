// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::fs;
use std::path::{Path, PathBuf};

pub const SUPPORTED_UTILITIES: &[&str] = &["ls"];

pub struct DeployResult {
    pub bin_dir: PathBuf,
    pub in_path: bool,
}

pub fn deploy_utility_symlinks() -> Result<DeployResult, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
    let local_bin = PathBuf::from(&home).join(".local/bin");
    let fshell_bin = PathBuf::from(&home).join(".fshell/bin");

    let (dir, in_path) = select_target_dir(&local_bin, &fshell_bin);
    deploy_to(&dir)?;

    Ok(DeployResult {
        bin_dir: dir,
        in_path,
    })
}

fn select_target_dir(local_bin: &Path, fshell_bin: &Path) -> (PathBuf, bool) {
    for p in [fshell_bin, local_bin] {
        if p.join("ls").exists() {
            return (p.to_path_buf(), is_dir_on_path(p));
        }
    }
    if is_dir_on_path(local_bin) {
        return (local_bin.to_path_buf(), true);
    }
    if let Some(writable) = find_writable_path_dir() {
        return (writable, true);
    }
    (local_bin.to_path_buf(), is_dir_on_path(local_bin))
}

fn deploy_to(bin_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(bin_dir).map_err(|e| format!("Failed to create {:?}: {}", bin_dir, e))?;
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Failed to get current exe: {}", e))?;
    for utility in SUPPORTED_UTILITIES {
        let link = bin_dir.join(utility);
        let _ = fs::remove_file(&link);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&current_exe, &link)
            .map_err(|e| format!("Failed to symlink {}: {}", utility, e))?;
    }
    Ok(())
}

pub fn is_dir_on_path(dir: &Path) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d == dir))
        .unwrap_or(false)
}

fn find_writable_path_dir() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find(|d| d.is_dir() && is_writable(d))
}

fn is_writable(dir: &Path) -> bool {
    let test = dir.join(".fsh_write_test");
    let ok = fs::write(&test, "").is_ok();
    let _ = fs::remove_file(&test);
    ok
}
