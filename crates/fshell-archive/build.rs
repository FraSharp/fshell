// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::env;
use std::io;
use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        // Homebrew's libarchive is keg-only. Preserve any caller-supplied
        // pkg-config search paths rather than replacing them.
        let mut paths: Vec<PathBuf> = env::var_os("PKG_CONFIG_PATH")
            .map(|p| env::split_paths(&p).collect())
            .unwrap_or_default();
        for prefix in ["/opt/homebrew", "/usr/local"] {
            let path = Path::new(prefix).join("opt/libarchive/lib/pkgconfig");
            if path.join("libarchive.pc").exists() && !paths.contains(&path) {
                paths.push(path);
            }
        }
        if !paths.is_empty() {
            let path = env::join_paths(paths).expect("invalid PKG_CONFIG_PATH");
            // SAFETY: This single-threaded build script sets its own child
            // environment before invoking pkg-config; it is not the shell's
            // runtime environment.
            unsafe { env::set_var("PKG_CONFIG_PATH", path) };
        }
    }

    // Use the maintained, security-patched libarchive supplied by the build
    // environment. pkg-config's Libs.private lists codec dependencies but does
    // not always supply their keg-only search paths, so resolve and link the
    // non-system codecs statically and explicitly.
    let library = pkg_config::Config::new()
        .atleast_version("3.6")
        .statik(true)
        .cargo_metadata(false)
        .probe("libarchive")
        .expect("libarchive development headers and static library are required");
    let mut paths = library.link_paths.clone();
    for package in ["liblzma", "libzstd", "liblz4", "libb2"] {
        let codec = pkg_config::Config::new()
            .cargo_metadata(false)
            .probe(package)
            .map_err(|e| {
                io::Error::other(format!("{package} development files are required: {e}"))
            })?;
        for path in codec.link_paths {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    for path in &paths {
        println!("cargo:rustc-link-search=native={}", path.display());
    }
    for framework_path in &library.framework_paths {
        println!(
            "cargo:rustc-link-search=framework={}",
            framework_path.display()
        );
    }
    // Static pkg-config dependency expansion can report a native archive more
    // than once. rustc rejects repeated -l entries when one uses the
    // `+whole-archive` modifier, so emit each such archive exactly once.
    let mut whole_archives = std::collections::HashSet::new();
    for lib in &library.libs {
        let native = matches!(lib.as_str(), "archive" | "lzma" | "zstd" | "lz4" | "b2");
        if native {
            if !whole_archives.insert(lib.as_str()) {
                continue;
            }
            let archive = format!("lib{lib}.a");
            if !paths.iter().any(|path| path.join(&archive).exists()) {
                return Err(io::Error::other(format!(
                    "static {archive} is required to build a portable fsh binary"
                ))
                .into());
            }
            // The decoder's C objects reference functions in the codec
            // archives. Include all native objects in our rlib so transitive
            // users (including unrelated integration-test targets) link the
            // same decoder and codec set, independent of archive scan order.
            println!("cargo:rustc-link-lib=static:+whole-archive={lib}");
        } else {
            // macOS system libraries (z, bz2, expat, iconv) and platform
            // frameworks are supplied by the OS. Linux release validation
            // checks that optional codec libraries did not remain dynamic.
            println!("cargo:rustc-link-lib={lib}");
        }
    }
    for framework in &library.frameworks {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
    // System upgrades must trigger relinking even with unchanged Rust source.
    for path in &paths {
        for lib in &library.libs {
            let archive = path.join(format!("lib{lib}.a"));
            if archive.exists() {
                println!("cargo:rerun-if-changed={}", archive.display());
            }
        }
    }
    let mut bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .allowlist_function("archive_.*")
        .allowlist_type("archive.*")
        .allowlist_var("ARCHIVE_.*")
        .allowlist_var("AE_.*")
        .layout_tests(false)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()));
    for path in &library.include_paths {
        bindings = bindings.clang_arg(format!("-I{}", path.display()));
    }
    bindings
        .generate()
        .expect("libarchive bindings could not be generated")
        .write_to_file(
            PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"))
                .join("bindings.rs"),
        )
        .expect("generated libarchive bindings could not be written");
    Ok(())
}
