// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use parking_lot::Mutex;
use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::OnceLock;

/// Thread-safe wrapper around process environment variables.
///
/// Rust's `std::env::set_var` and `std::env::remove_var` are not thread-safe
/// (per Rust std docs: "This function is not thread-safe... calling it concurrently
/// with other environment operations is undefined behavior").
///
/// This wrapper uses a `Mutex` to synchronize all access to the process environment,
/// while also maintaining an in-memory cache for fast reads.
///
/// NOTE: Any external code or third-party crates calling `std::env::set_var` or
/// `std::env::remove_var` directly outside of this lock will still cause undefined behavior.
pub struct ThreadSafeEnv {
    /// In-memory cache of environment variables.
    /// Only updated through `set_var`/`remove_var` to stay in sync.
    cache: Mutex<HashMap<String, OsString>>,
}

impl ThreadSafeEnv {
    /// Get the global instance.
    pub fn global() -> &'static ThreadSafeEnv {
        static INSTANCE: OnceLock<ThreadSafeEnv> = OnceLock::new();
        INSTANCE.get_or_init(ThreadSafeEnv::new)
    }

    /// Create a new instance with the current process environment.
    fn new() -> Self {
        let mut cache = HashMap::new();
        for (k, v) in std::env::vars_os() {
            if let Ok(key) = k.into_string() {
                cache.insert(key, v);
            }
        }
        Self {
            cache: Mutex::new(cache),
        }
    }

    /// Get an environment variable.
    pub fn get(&self, key: &str) -> Option<OsString> {
        // Fast path: check cache first
        if let Some(v) = self.cache.lock().get(key) {
            return Some(v.clone());
        }
        // Fallback: read from process env (in case it was modified externally)
        std::env::var_os(key)
    }

    /// Set an environment variable.
    ///
    /// This is thread-safe and updates both the process environment and the cache.
    pub fn set_var(&self, key: &str, value: &str) {
        let mut cache = self.cache.lock();
        cache.insert(key.to_string(), OsString::from(value));
        // SAFETY: We hold the lock, so no concurrent access to std::env::set_var
        unsafe {
            std::env::set_var(key, value);
        }
    }

    /// Remove an environment variable.
    ///
    /// This is thread-safe and updates both the process environment and the cache.
    pub fn remove_var(&self, key: &str) {
        let mut cache = self.cache.lock();
        cache.remove(key);
        // SAFETY: We hold the lock, so no concurrent access to std::env::remove_var
        unsafe {
            std::env::remove_var(key);
        }
    }

    /// Get all environment variables as a vector of (key, value) pairs.
    pub fn vars(&self) -> Vec<(String, OsString)> {
        self.cache
            .lock()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Get all environment variables as a HashMap.
    pub fn vars_map(&self) -> HashMap<String, OsString> {
        self.cache.lock().clone()
    }
}

/// Convenience function to get an environment variable.
pub fn get_var(key: &str) -> Option<OsString> {
    ThreadSafeEnv::global().get(key)
}

/// Convenience function to set an environment variable.
pub fn set_var(key: &str, value: &str) {
    ThreadSafeEnv::global().set_var(key, value)
}

/// Convenience function to remove an environment variable.
pub fn remove_var(key: &str) {
    ThreadSafeEnv::global().remove_var(key)
}

/// Convenience function to get all environment variables.
pub fn vars() -> Vec<(String, OsString)> {
    ThreadSafeEnv::global().vars()
}

/// Get the current user's home directory from environment.
pub fn get_home_dir() -> std::path::PathBuf {
    if let Some(h) = get_var("HOME") {
        if let Ok(s) = h.into_string() {
            if !s.is_empty() {
                return std::path::PathBuf::from(s);
            }
        }
    }
    std::env::var("HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/"))
}

/// Expand leading `~` or `~/...` in path string.
pub fn expand_tilde(path: &str) -> std::path::PathBuf {
    if path == "~" || path == "~/" {
        return get_home_dir();
    } else if let Some(stripped) = path.strip_prefix("~/") {
        let mut home = get_home_dir();
        home.push(stripped);
        return home;
    }
    std::path::PathBuf::from(path)
}

/// Expand leading `~` or `~/...` returning a String.
pub fn expand_tilde_str(path: &str) -> String {
    expand_tilde(path).to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thread_safe_env() {
        let env = ThreadSafeEnv::new();

        // Test set/get
        env.set_var("TEST_KEY", "test_value");
        assert_eq!(env.get("TEST_KEY"), Some(OsString::from("test_value")));

        // Test remove
        env.remove_var("TEST_KEY");
        assert_eq!(env.get("TEST_KEY"), None);

        // Test overwrite
        env.set_var("TEST_KEY", "value1");
        env.set_var("TEST_KEY", "value2");
        assert_eq!(env.get("TEST_KEY"), Some(OsString::from("value2")));

        // Test vars
        let vars = env.vars();
        assert!(vars.iter().any(|(k, v)| k == "TEST_KEY" && v == "value2"));
    }

    #[test]
    fn test_global_instance() {
        set_var("GLOBAL_TEST_KEY", "global_value");
        assert_eq!(
            get_var("GLOBAL_TEST_KEY"),
            Some(OsString::from("global_value"))
        );
        remove_var("GLOBAL_TEST_KEY");
        assert_eq!(get_var("GLOBAL_TEST_KEY"), None);
    }
}
