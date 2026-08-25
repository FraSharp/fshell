use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use tempfile::TempDir;

static PROCESS_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

fn get_process_mutex() -> &'static Mutex<()> {
    PROCESS_MUTEX.get_or_init(|| Mutex::new(()))
}

/// Global lock guard to serialize tests that mutate process-wide global state
/// (such as `std::env::set_current_dir` or `std::env::set_var`).
pub struct ProcessLockGuard<'a> {
    _guard: MutexGuard<'a, ()>,
}

impl<'a> ProcessLockGuard<'a> {
    pub fn acquire() -> ProcessLockGuard<'static> {
        let guard = get_process_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        ProcessLockGuard { _guard: guard }
    }
}

/// RAII Guard that captures the current working directory, switches to a new one,
/// and guarantees the original directory is restored when dropped.
pub struct CwdGuard {
    _lock: ProcessLockGuard<'static>,
    original_cwd: PathBuf,
    current_cwd: PathBuf,
    _temp_dir: Option<TempDir>,
}

impl CwdGuard {
    /// Create a new temporary directory and switch the current working directory to it.
    pub fn new_temp() -> Self {
        let lock = ProcessLockGuard::acquire();
        let original_cwd = std::env::current_dir()
            .and_then(|p| p.canonicalize())
            .unwrap_or_else(|_| PathBuf::from("/"));
        let temp = TempDir::new().expect("failed to create temporary directory for test");
        let current_cwd = temp
            .path()
            .canonicalize()
            .unwrap_or_else(|_| temp.path().to_path_buf());
        std::env::set_current_dir(&current_cwd).expect("failed to set current dir to tempdir");

        Self {
            _lock: lock,
            original_cwd,
            current_cwd,
            _temp_dir: Some(temp),
        }
    }

    /// Switch the working directory to an existing path, restoring on drop.
    pub fn switch_to<P: AsRef<Path>>(path: P) -> Self {
        let lock = ProcessLockGuard::acquire();
        let original_cwd = std::env::current_dir()
            .and_then(|p| p.canonicalize())
            .unwrap_or_else(|_| PathBuf::from("/"));
        let target = path
            .as_ref()
            .canonicalize()
            .unwrap_or_else(|_| path.as_ref().to_path_buf());
        std::env::set_current_dir(&target).expect("failed to switch current dir");

        Self {
            _lock: lock,
            original_cwd,
            current_cwd: target,
            _temp_dir: None,
        }
    }

    /// The active directory path under this guard.
    pub fn path(&self) -> &Path {
        &self.current_cwd
    }

    /// Create a file with contents relative to the guard's directory.
    pub fn create_file<P: AsRef<Path>>(
        &self,
        rel_path: P,
        content: &str,
    ) -> std::io::Result<PathBuf> {
        let full_path = self.current_cwd.join(rel_path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&full_path, content)?;
        Ok(full_path)
    }

    /// Create a subdirectory relative to the guard's directory.
    pub fn create_dir<P: AsRef<Path>>(&self, rel_path: P) -> std::io::Result<PathBuf> {
        let full_path = self.current_cwd.join(rel_path);
        std::fs::create_dir_all(&full_path)?;
        Ok(full_path)
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original_cwd);
    }
}

/// RAII Guard that manages changes to environment variables, restoring them on drop.
pub struct EnvVarGuard {
    _lock: ProcessLockGuard<'static>,
    original_vars: HashMap<String, Option<String>>,
}

impl EnvVarGuard {
    pub fn new() -> Self {
        let lock = ProcessLockGuard::acquire();
        Self {
            _lock: lock,
            original_vars: HashMap::new(),
        }
    }

    /// Set an environment variable, recording its previous state.
    pub fn set(&mut self, key: &str, value: &str) {
        if !self.original_vars.contains_key(key) {
            let original = std::env::var(key).ok();
            self.original_vars.insert(key.to_string(), original);
        }
        fshell_core::set_var(key, value);
    }

    /// Remove an environment variable, recording its previous state.
    pub fn remove(&mut self, key: &str) {
        if !self.original_vars.contains_key(key) {
            let original = std::env::var(key).ok();
            self.original_vars.insert(key.to_string(), original);
        }
        fshell_core::remove_var(key);
    }
}

impl Default for EnvVarGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        for (key, original_val) in &self.original_vars {
            match original_val {
                Some(val) => fshell_core::set_var(key, val),
                None => fshell_core::remove_var(key),
            }
        }
    }
}
