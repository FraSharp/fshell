//! Reference-shell discovery.
//!
//! POSIX cases are checked against `bash --posix` (primary) and `dash`
//! (secondary) when it is installed. A missing `dash` — the normal state on
//! macOS — downgrades the case to a single-oracle comparison; it never aborts
//! the suite. Discovery searches the *developer's* `PATH` (plus the usual
//! system directories) and invokes each shell by absolute path, because the
//! invocation environment is cleared and therefore has no usable `PATH` of its
//! own.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// A usable reference shell.
#[derive(Debug, Clone)]
pub struct ReferenceShell {
    /// Human-readable name used in reports.
    pub name: &'static str,
    /// Absolute path to the executable.
    pub program: PathBuf,
    /// Arguments inserted before `-c <script>`.
    pub prefix: Vec<String>,
}

impl ReferenceShell {
    /// Full argument vector for running `script`, including the `-c` script.
    pub fn command_args(&self, script: &str) -> Vec<String> {
        let mut args = self.prefix.clone();
        args.push("-c".to_string());
        args.push(script.to_string());
        args
    }
}

/// POSIX reference shells that are actually installed, primary first.
///
/// Computed once per test process because the search hits the filesystem.
pub fn posix_references() -> &'static [ReferenceShell] {
    static REFERENCES: OnceLock<Vec<ReferenceShell>> = OnceLock::new();
    REFERENCES.get_or_init(|| {
        let mut references = Vec::new();

        // `--noprofile --norc` keep the reference shell from sourcing anything;
        // the environment is cleared anyway, but bash also honours `BASH_ENV`.
        if let Some(program) = find_program("bash") {
            references.push(ReferenceShell {
                name: "bash --posix",
                program,
                prefix: vec!["--noprofile".into(), "--norc".into(), "--posix".into()],
            });
        }

        if let Some(program) = find_program("dash") {
            references.push(ReferenceShell {
                name: "dash",
                program,
                prefix: Vec::new(),
            });
        }

        references
    })
}

/// Plain `bash`, *without* `--posix`.
///
/// Some bash extensions are rejected by bash's own POSIX mode — process
/// substitution in particular — so they need a bash oracle that is not asked to
/// pretend to be POSIX. See [`Oracle::Bash`](super::case::Oracle::Bash).
pub fn bash_reference() -> Option<ReferenceShell> {
    static BASH: OnceLock<Option<PathBuf>> = OnceLock::new();
    BASH.get_or_init(|| find_program("bash"))
        .as_ref()
        .map(|program| ReferenceShell {
            name: "bash",
            program: program.clone(),
            prefix: vec!["--noprofile".into(), "--norc".into()],
        })
}

/// Locate an executable by name, searching `PATH` then the usual system dirs.
fn find_program(name: &str) -> Option<PathBuf> {
    let mut directories: Vec<PathBuf> = Vec::new();

    if let Some(path) = std::env::var_os("PATH") {
        directories.extend(std::env::split_paths(&path));
    }
    for fallback in ["/bin", "/usr/bin", "/usr/local/bin", "/opt/homebrew/bin"] {
        directories.push(PathBuf::from(fallback));
    }

    directories
        .into_iter()
        .map(|directory| directory.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}
