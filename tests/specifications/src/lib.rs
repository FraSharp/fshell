//! Specifications of fshell's current implementation boundaries.
//!
//! Tests use production constructors and entrypoints. Each target names the
//! contract family it exercises; assertions retain typed outcomes and errors.

use std::io;
use std::path::PathBuf;

pub const SHELL_EXECUTABLE_VARIABLE: &str = "FSHELL_TEST_EXECUTABLE";

/// The orchestration layer provides the exact Cargo-selected shell artifact.
/// Never guess a target directory or silently skip executable specifications.
pub fn shell_executable() -> io::Result<PathBuf> {
    let path = std::env::var_os(SHELL_EXECUTABLE_VARIABLE)
        .map(PathBuf::from)
        .ok_or_else(|| {
            io::Error::other(
                "FSHELL_TEST_EXECUTABLE is required; use cargo run -p fshell-test-runner",
            )
        })?;
    if !path.is_absolute() {
        return Err(io::Error::other(
            "FSHELL_TEST_EXECUTABLE must be an absolute artifact path",
        ));
    }
    Ok(path)
}
