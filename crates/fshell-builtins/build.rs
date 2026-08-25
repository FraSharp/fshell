// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

fn main() {
    let manifest_dir = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"),
    );

    let workspace_root = match manifest_dir.parent().and_then(|p| p.parent()) {
        Some(root) => root,
        None => return,
    };

    let cargo_toml = workspace_root.join("Cargo.toml");
    if !cargo_toml.exists() {
        return;
    }

    let ws = workspace_root.to_string_lossy();
    println!("cargo:rustc-env=FSHELL_WORKSPACE_ROOT={ws}");
    println!("cargo:rerun-if-changed={}", cargo_toml.display());

    let content = match std::fs::read_to_string(&cargo_toml) {
        Ok(c) => c,
        Err(_) => return,
    };

    for line in content.lines() {
        if let Some(url) = line
            .trim()
            .strip_prefix("repository = \"")
            .and_then(|val| val.strip_suffix('"'))
        {
            println!("cargo:rustc-env=FSHELL_REPO_URL={url}");
            break;
        }
    }
}
