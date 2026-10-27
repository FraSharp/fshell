// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Artifact-aware orchestration. Nextest owns discovery, scheduling and verdicts.

use clap::Parser;
use fshell_test_runner::{
    Arguments, Subcommand, Suite, build_arguments, native_target, nextest_arguments,
    shell_artifact, validate_library_listing, validate_package_metadata, validate_target,
};
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

fn main() -> ExitCode {
    match run(Arguments::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fshell-test-runner: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: Arguments) -> Result<(), Box<dyn std::error::Error>> {
    if arguments.full && matches!(arguments.suite, Suite::Support | Suite::Containment) {
        return Err(
            io::Error::other("--full is not applicable to mechanical support checks").into(),
        );
    }
    if arguments.suite == Suite::Containment && !cfg!(target_os = "linux") {
        return Err(
            io::Error::other("PID-namespace containment requires a native Linux host").into(),
        );
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let nextest = Command::new(&cargo)
        .args(["nextest", "--version"])
        .current_dir(&root)
        .stdin(Stdio::null())
        .output()?;
    if !nextest.status.success() {
        return Err(io::Error::other("cargo-nextest is required; install with: cargo install cargo-nextest --version 0.9.114 --locked").into());
    }
    io::stdout().write_all(&nextest.stdout)?;
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let version = Command::new(rustc)
        .arg("-vV")
        .stdin(Stdio::null())
        .output()?;
    if !version.status.success() {
        return Err(io::Error::other("could not determine the native compiler target").into());
    }
    let host = native_target(&version.stdout)?;
    let target = validate_target(arguments.target.as_deref(), &host)?;
    if !matches!(arguments.suite, Suite::Support | Suite::Containment) {
        let metadata = Command::new(&cargo)
            .args(["metadata", "--locked", "--no-deps", "--format-version=1"])
            .current_dir(&root)
            .stdin(Stdio::null())
            .output()?;
        io::stderr().write_all(&metadata.stderr)?;
        if !metadata.status.success() {
            return Err(
                io::Error::other(format!("Cargo metadata failed: {}", metadata.status)).into(),
            );
        }
        validate_package_metadata(&metadata.stdout)?;
    }
    // The listing guard runs before any build or execution: a curated family
    // that no longer exists must fail the selection, not silently shrink it.
    if matches!(arguments.suite, Suite::All | Suite::Library) {
        let listing = Command::new(&cargo)
            .args(nextest_arguments(
                &arguments,
                &target,
                &root,
                Subcommand::List,
            ))
            .current_dir(&root)
            .stdin(Stdio::null())
            .output()?;
        io::stderr().write_all(&listing.stderr)?;
        if !listing.status.success() {
            return Err(
                io::Error::other(format!("selection listing failed: {}", listing.status)).into(),
            );
        }
        let selection = validate_library_listing(&listing.stdout)?;
        eprintln!("library selection: {selection}");
    }
    let executable = if arguments.suite.needs_shell() {
        let output = Command::new(&cargo)
            .args(build_arguments(&arguments, &target))
            .current_dir(&root)
            .stdin(Stdio::null())
            .output()?;
        io::stderr().write_all(&output.stderr)?;
        if !output.status.success() {
            return Err(io::Error::other(format!("shell build failed: {}", output.status)).into());
        }
        let path = shell_artifact(&output.stdout)?;
        if !path.is_file() {
            return Err(io::Error::other(format!(
                "Cargo artifact does not exist: {}",
                path.display()
            ))
            .into());
        }
        println!("shell artifact: {}", path.display());
        Some(path)
    } else {
        None
    };
    let mut nextest = Command::new(&cargo);
    nextest.args(nextest_arguments(
        &arguments,
        &target,
        &root,
        Subcommand::Run,
    ));
    nextest.current_dir(&root).stdin(Stdio::null());
    // Artifact-free selections must not accept a caller-supplied executable.
    nextest.env_remove("FSHELL_TEST_EXECUTABLE");
    if let Some(path) = executable {
        nextest.env("FSHELL_TEST_EXECUTABLE", path);
    }
    let status = nextest.status()?;
    if !status.success() {
        return Err(io::Error::other(format!("specifications failed: {status}")).into());
    }
    Ok(())
}
