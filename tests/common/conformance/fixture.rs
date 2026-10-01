//! Deterministic, reproducible sandbox for conformance runs.
//!
//! Every invocation — reference shell or fsh — runs in the *same* absolute
//! directory with the *same* environment, reset to a pristine tree immediately
//! before it starts. Two properties fall out of that:
//!
//! * `$HOME`, `$PWD` and glob results are identical across shells, so a
//!   differential comparison is meaningful even though the fixture lives in a
//!   randomly named temporary directory.
//! * file-mutating cases (`> out`, `>> out`) cannot contaminate the next
//!   shell's run, because the tree is rebuilt between invocations.
//!
//! Nothing from the developer's machine leaks in: the environment is cleared
//! and rebuilt from [`Fixture::base_env`], the tree is built from source, and
//! `TMPDIR` points inside the fixture.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tempfile::TempDir;

use super::case::Case;

/// Files created in every fixture, as `(relative path, contents)`.
const FILES: &[(&str, &str)] = &[
    ("a.txt", "alpha\n"),
    ("b.txt", "bravo\n"),
    ("c.txt", "charlie\n"),
    ("a.rs", "fn a() {}\n"),
    ("b.rs", "fn b() {}\n"),
    ("space file.txt", "spaced\n"),
    ("empty", ""),
    ("dir/nested.txt", "nested text\n"),
    ("dir/nested.rs", "fn nested() {}\n"),
];

/// Directories created in every fixture, in addition to parents of [`FILES`].
const DIRS: &[&str] = &["home", "space dir", "target/debug"];

/// `CACHEDIR.TAG` contents, as written by Cargo, so directory-walking tools can
/// be observed skipping `target/`.
const CACHEDIR_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n";

/// Fixture-relative file the engine trace is appended to.
///
/// Written by fsh when `FSH_ENGINE_TRACE` is set; the suite uses it to assert
/// *which engine ran and why*, rather than inferring dispatch from output.
///
/// It lives in the config directory rather than the working directory on
/// purpose: fsh writes the first trace line as it starts, so a case that globs
/// `*` would otherwise see the instrument itself and start testing it.
pub const ENGINE_TRACE_FILE: &str = ".config/engine-trace";

/// A pristine fixture tree that can be reset between invocations.
pub struct Fixture {
    root: TempDir,
    helper_dir: PathBuf,
    fsh_binary: PathBuf,
    /// Extra sandbox files this case's tree carries.
    tree: &'static [(&'static str, &'static str)],
    /// Sandbox tools this case provides on `PATH`.
    stubs: &'static [(&'static str, &'static str)],
}

impl Fixture {
    /// Create a fixture tree for `case` and populate it.
    pub fn new(case: &Case) -> std::io::Result<Self> {
        let root = tempfile::Builder::new()
            .prefix(&temp_prefix(case.name))
            .tempdir()?;

        let fixture = Self {
            root,
            helper_dir: helper_dir(),
            fsh_binary: PathBuf::from(env!("CARGO_BIN_EXE_fsh")),
            tree: case.tree,
            stubs: case.stubs,
        };
        fixture.reset()?;
        Ok(fixture)
    }

    /// Delete everything inside the fixture and rebuild it from scratch.
    pub fn reset(&self) -> std::io::Result<()> {
        let root = self.root.path();

        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                fs::remove_dir_all(&path)?;
            } else {
                fs::remove_file(&path)?;
            }
        }

        for dir in DIRS {
            fs::create_dir_all(root.join(dir))?;
        }
        for (relative, contents) in FILES {
            let path = root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, contents)?;
        }
        fs::write(root.join("target/CACHEDIR.TAG"), CACHEDIR_TAG)?;

        // The case's own sandbox: the tree a workflow expects to find, and the
        // tools it drives, both rebuilt from source on every invocation so one
        // run cannot leave anything behind for the next.
        for (relative, contents) in self.tree {
            let path = root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, contents)?;
        }
        if !self.stubs.is_empty() {
            let bin = root.join("bin");
            fs::create_dir_all(&bin)?;
            for (name, script) in self.stubs {
                let path = bin.join(name);
                fs::write(&path, script)?;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
            }
        }

        // Scratch space for child shells (POSIX subshells write temp files).
        fs::create_dir_all(root.join(".tmp"))?;
        fs::create_dir_all(root.join(".config"))?;

        Ok(())
    }

    /// Fixture root, used as the working directory for every invocation.
    pub fn root(&self) -> &Path {
        self.root.path()
    }

    /// Absolute path of a fixture-relative entry.
    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }

    /// Absolute path of `$HOME` inside the fixture.
    pub fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }

    /// Path to the built `fsh` binary under test.
    pub fn fsh_binary(&self) -> &Path {
        &self.fsh_binary
    }

    /// Directory holding the conformance helper binaries (`argvdump`, `emit`).
    pub fn helper_dir(&self) -> &Path {
        &self.helper_dir
    }

    /// The controlled environment every invocation receives.
    ///
    /// The child environment is *cleared* before these are applied, so nothing
    /// from the developer's machine — aliases, rc files, locale, git config,
    /// a real `$HOME` — can influence a result.
    pub fn base_env(&self) -> BTreeMap<String, String> {
        let root = self.root.path();
        let mut env = BTreeMap::new();

        // Sandbox identity.
        env.insert("HOME".to_string(), path_string(&self.home()));
        env.insert("PWD".to_string(), path_string(root));
        env.insert("TMPDIR".to_string(), path_string(&root.join(".tmp")));
        env.insert("SHELL".to_string(), "/bin/sh".to_string());

        // Locale pinned so collation and byte handling are stable.
        env.insert("LANG".to_string(), "C".to_string());
        env.insert("LC_ALL".to_string(), "C".to_string());

        // Variables the corpus expands. `IFS` is deliberately absent: unset
        // means the default, which is what the cases assume.
        env.insert("FOO".to_string(), "hello".to_string());
        env.insert("BAR".to_string(), "a b".to_string());
        env.insert("EMPTY".to_string(), String::new());
        env.insert("NUMBER".to_string(), "42".to_string());
        env.insert("COLON".to_string(), "one:two:three".to_string());

        // `emit`/`argvdump` first so the corpus can call them by name, then the
        // system directories holding the real utilities the corpus uses. A case
        // with stubs gets those first of all: they stand in for the tooling a
        // workflow drives, and must shadow the real thing.
        let path = if self.stubs.is_empty() {
            format!("{}:/usr/bin:/bin", path_string(&self.helper_dir))
        } else {
            format!(
                "{}:{}:/usr/bin:/bin",
                path_string(&root.join("bin")),
                path_string(&self.helper_dir)
            )
        };
        env.insert("PATH".to_string(), path);

        // fsh-specific isolation: no colour, no "did you mean", isolated state,
        // and a known binary for child shells (POSIX subshells re-exec `fsh`).
        env.insert("NO_COLOR".to_string(), "1".to_string());
        env.insert("FSH_TEST_ENV".to_string(), "1".to_string());
        env.insert(
            "FSH_CONFIG_DIR".to_string(),
            path_string(&root.join(".config")),
        );
        env.insert(
            "FSH_Z_DB_PATH".to_string(),
            path_string(&root.join(".frecency.json")),
        );
        env.insert("FSH_BINARY_PATH".to_string(), path_string(&self.fsh_binary));
        // Routing decisions land here, one `engine=… reason=…` line each. The
        // runner reads it to assert dispatch directly; the reference shells
        // ignore the variable entirely.
        env.insert(
            "FSH_ENGINE_TRACE".to_string(),
            path_string(&root.join(ENGINE_TRACE_FILE)),
        );

        env
    }

    /// Read `relative` and return its contents, or `None` when absent.
    pub fn read_file(&self, relative: &str) -> Option<String> {
        fs::read_to_string(self.path(relative)).ok()
    }
}

/// Directory containing the conformance helper binaries.
///
/// The helpers are examples, not bins, so `cargo install` ships only `fsh`.
/// A plain `cargo test` builds examples; a filtered run (`cargo test --test
/// …`) does not, so build them on demand when they are missing.
pub fn helper_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = PathBuf::from(env!("CARGO_BIN_EXE_fsh"))
            .parent()
            .map(|parent| parent.join("examples"))
            .expect("fsh binary has no parent directory");
        if dir.join("argvdump").exists() {
            dir
        } else {
            build_helper_examples()
        }
    })
    .clone()
}

/// Build the helper examples, returning the directory holding `argvdump`.
fn build_helper_examples() -> PathBuf {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = std::process::Command::new(cargo)
        .args(["build", "--examples", "--message-format=json"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("could not run cargo to build the conformance helper examples");
    assert!(
        output.status.success(),
        "`cargo build --examples` failed while preparing the conformance helpers:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter_map(|line| serde_json::from_slice::<serde_json::Value>(line).ok())
        .find_map(|message| {
            let target = message.get("target")?;
            if target.get("name")?.as_str()? != "argvdump" {
                return None;
            }
            let executable = message.get("executable")?.as_str()?;
            Path::new(executable).parent().map(Path::to_path_buf)
        })
        .expect("cargo did not report an `argvdump` example artifact")
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Build a filesystem-safe temp-directory prefix from a case name.
///
/// Case names are namespaced with `/` (`word/empty-argument`), which a temp
/// prefix cannot contain, so those characters are folded to `-`.
fn temp_prefix(case_name: &str) -> String {
    let sanitized: String = case_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    format!("fsh-conformance-{sanitized}-")
}
