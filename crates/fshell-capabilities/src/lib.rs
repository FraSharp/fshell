// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::panic))]
use fshell_core::ResourceHandle;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Registry tracking active capability tokens and strict enforcement tier.
///
/// Capability checks follow a **deny-then-allow** policy:
/// 1. If the action matches any entry in `denied`, it's rejected immediately.
/// 2. Otherwise, if it matches any entry in `held`, it's allowed.
/// 3. Otherwise, it's rejected.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct CapsRegistry {
    pub held: HashSet<ResourceHandle>,
    #[serde(default)]
    pub denied: HashSet<ResourceHandle>,
    pub strict_mode: bool,
}

impl CapsRegistry {
    pub fn new() -> Self {
        CapsRegistry {
            held: HashSet::new(),
            denied: HashSet::new(),
            strict_mode: false,
        }
    }

    /// Create an empty registry for utility/multicall mode.
    ///
    /// Grants nothing; permissiveness in multicall mode comes from the engine's
    /// trusted-bypass, not from this registry holding any capabilities.
    pub fn new_permissive() -> Self {
        Self::new()
    }

    /// Create registry with default interactive credentials derived from $PWD context.
    pub fn new_with_defaults(pwd: PathBuf) -> Self {
        let mut registry = Self::new();
        registry.grant(ResourceHandle::ReadDir(pwd.clone()));
        registry.grant(ResourceHandle::WriteDir(pwd.clone()));
        registry.grant(ResourceHandle::ReadFile(pwd.clone()));
        registry.grant(ResourceHandle::WriteFile(pwd));
        registry.grant(ResourceHandle::ProcessSpawn); // Spawning external binaries permitted by default in interactive

        // Load persistent capability grants from isolated or system paths
        let caps_path = if let Ok(dir) = std::env::var("FSH_CONFIG_DIR") {
            Some(PathBuf::from(dir).join("caps.json"))
        } else {
            let home = std::env::var("FSH_HOME")
                .ok()
                .or_else(|| std::env::var("HOME").ok())
                .or_else(|| std::env::var("USERPROFILE").ok());
            home.map(|h| PathBuf::from(h).join(".config/fsh/caps.json"))
        };

        if let Some(path) = caps_path
            && let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(saved) = serde_json::from_str::<HashSet<ResourceHandle>>(&content)
        {
            for h in saved {
                registry.grant(h);
            }
        }

        registry
    }

    /// Parse and extract a list of capabilities from a local profile in YAML format.
    pub fn load_profile_from_yaml(
        content: &str,
        base_dir: &Path,
        profile_name: &str,
    ) -> Result<Vec<ResourceHandle>, String> {
        let yaml_val: serde_yaml::Value =
            serde_yaml::from_str(content).map_err(|e| format!("Invalid YAML profile: {}", e))?;

        let profiles = yaml_val
            .get("profiles")
            .ok_or_else(|| "Missing 'profiles' key in caps.yaml".to_string())?;

        let profile = profiles
            .get(profile_name)
            .ok_or_else(|| format!("Profile '{}' not found in caps.yaml", profile_name))?;

        let cap_list = profile
            .as_sequence()
            .ok_or_else(|| format!("Profile '{}' must be a list of capabilities", profile_name))?;

        let mut handles = Vec::new();
        for item in cap_list {
            if let Some(s) = item.as_str() {
                if let Some(handle) = parse_cap_string(s, base_dir) {
                    handles.push(handle);
                } else {
                    return Err(format!("Unknown capability format: '{}'", s));
                }
            } else {
                return Err("Capabilities must be strings".to_string());
            }
        }
        Ok(handles)
    }

    /// Explicitly grant a capability token.
    pub fn grant(&mut self, handle: ResourceHandle) {
        self.held.insert(handle);
    }

    /// Explicitly deny a capability token (takes priority over grants).
    pub fn deny(&mut self, handle: ResourceHandle) {
        self.denied.insert(handle);
    }

    /// Explicitly revoke a capability token.
    pub fn revoke(&mut self, handle: &ResourceHandle) -> bool {
        self.held.remove(handle)
    }

    /// Remove a capability from the deny list.
    pub fn allow(&mut self, handle: &ResourceHandle) -> bool {
        self.denied.remove(handle)
    }

    /// Check whether an action is denied (deny list takes priority).
    pub fn check_with_deny(&self, action: &ResourceHandle) -> Result<(), String> {
        if self.is_denied(action) {
            return Err(format!("Capability denied: {:?}", action));
        }
        match action {
            ResourceHandle::ReadDir(p) => {
                if self.check_read_dir(p) {
                    Ok(())
                } else {
                    Err(format!("Capability not held: ReadDir({:?})", p))
                }
            }
            ResourceHandle::WriteDir(p) => {
                if self.check_write_dir(p) {
                    Ok(())
                } else {
                    Err(format!("Capability not held: WriteDir({:?})", p))
                }
            }
            ResourceHandle::ReadFile(p) => {
                if self.check_read_file(p) {
                    Ok(())
                } else {
                    Err(format!("Capability not held: ReadFile({:?})", p))
                }
            }
            ResourceHandle::WriteFile(p) => {
                if self.check_write_file(p) {
                    Ok(())
                } else {
                    Err(format!("Capability not held: WriteFile({:?})", p))
                }
            }
            ResourceHandle::NetworkSocket(host) => {
                if self.check_network(host) {
                    Ok(())
                } else {
                    Err(format!("Capability not held: NetworkSocket({})", host))
                }
            }
            ResourceHandle::NetworkAll => {
                if self.check_network("") {
                    Ok(())
                } else {
                    Err("Capability not held: NetworkAll".to_string())
                }
            }
            ResourceHandle::ReadEnv(var) => {
                if self.check_env_read(var) {
                    Ok(())
                } else {
                    Err(format!("Capability not held: ReadEnv({})", var))
                }
            }
            ResourceHandle::WriteEnv(var) => {
                if self.check_env_write(var) {
                    Ok(())
                } else {
                    Err(format!("Capability not held: WriteEnv({})", var))
                }
            }
            ResourceHandle::ProcessSpawn => {
                if self.check_process_spawn("") {
                    Ok(())
                } else {
                    Err("Capability not held: ProcessSpawn".to_string())
                }
            }
            ResourceHandle::ProcessSpawnPath(cmd) => {
                if self.check_process_spawn(cmd) {
                    Ok(())
                } else {
                    Err(format!("Capability not held: ProcessSpawnPath({})", cmd))
                }
            }
        }
    }

    fn is_denied_path(&self, path: &Path) -> bool {
        self.denied.iter().any(|h| match h {
            ResourceHandle::ReadDir(p)
            | ResourceHandle::WriteDir(p)
            | ResourceHandle::ReadFile(p)
            | ResourceHandle::WriteFile(p) => match_path_pattern(p, path),
            _ => false,
        })
    }

    fn is_denied_read(&self, path: &Path) -> bool {
        self.denied.iter().any(|h| match h {
            ResourceHandle::ReadDir(p) | ResourceHandle::ReadFile(p) => match_path_pattern(p, path),
            _ => false,
        })
    }

    fn is_denied_write(&self, path: &Path) -> bool {
        self.denied.iter().any(|h| match h {
            ResourceHandle::WriteDir(p) | ResourceHandle::WriteFile(p) => {
                match_path_pattern(p, path)
            }
            _ => false,
        })
    }

    pub fn is_denied(&self, handle: &ResourceHandle) -> bool {
        match handle {
            ResourceHandle::ReadDir(p)
            | ResourceHandle::WriteDir(p)
            | ResourceHandle::ReadFile(p)
            | ResourceHandle::WriteFile(p) => self.is_denied_path(p),
            ResourceHandle::NetworkSocket(host) => self.denied.iter().any(|h| match h {
                ResourceHandle::NetworkAll => true,
                ResourceHandle::NetworkSocket(s) => match_network_host(s, host),
                _ => false,
            }),
            ResourceHandle::NetworkAll => self.denied.contains(&ResourceHandle::NetworkAll),
            ResourceHandle::ReadEnv(var) => self.denied.iter().any(|h| match h {
                ResourceHandle::ReadEnv(s) => glob_match_str(s, var),
                _ => false,
            }),
            ResourceHandle::WriteEnv(var) => self.denied.iter().any(|h| match h {
                ResourceHandle::WriteEnv(s) => glob_match_str(s, var),
                _ => false,
            }),
            ResourceHandle::ProcessSpawn => {
                self.denied.contains(&ResourceHandle::ProcessSpawn)
                    || self.denied.iter().any(|h| match h {
                        ResourceHandle::ProcessSpawnPath(pattern) => glob_match_str(pattern, ""),
                        _ => false,
                    })
            }
            ResourceHandle::ProcessSpawnPath(cmd) => {
                self.denied.contains(&ResourceHandle::ProcessSpawn)
                    || self.denied.iter().any(|h| match h {
                        ResourceHandle::ProcessSpawnPath(pattern) => glob_match_str(pattern, cmd),
                        _ => false,
                    })
            }
        }
    }

    /// Verify directory read permissions.
    pub fn check_read_dir(&self, path: &Path) -> bool {
        if self.is_denied_read(path) {
            return false;
        }
        self.held.iter().any(|h| match h {
            ResourceHandle::ReadDir(p) | ResourceHandle::WriteDir(p) => match_path_pattern(p, path),
            ResourceHandle::ReadFile(p) | ResourceHandle::WriteFile(p) => {
                match_path_pattern(p, path)
            }
            _ => false,
        })
    }

    /// Verify directory write permissions.
    pub fn check_write_dir(&self, path: &Path) -> bool {
        if self.is_denied_write(path) {
            return false;
        }
        self.held.iter().any(|h| match h {
            ResourceHandle::WriteDir(p) => match_path_pattern(p, path),
            ResourceHandle::WriteFile(p) => match_path_pattern(p, path),
            _ => false,
        })
    }

    /// Verify file read/write permissions.
    pub fn check_read_write_file(&self, path: &Path) -> bool {
        if self.is_denied_path(path) {
            return false;
        }
        self.held.iter().any(|h| match h {
            ResourceHandle::ReadFile(p) => match_path_pattern(p, path),
            ResourceHandle::WriteFile(p) => match_path_pattern(p, path),
            ResourceHandle::ReadDir(p) if match_path_pattern(p, path) => true,
            ResourceHandle::WriteDir(p) if match_path_pattern(p, path) => true,
            _ => false,
        })
    }

    /// Verify network socket target connectivity.
    pub fn check_network(&self, host: &str) -> bool {
        self.check_network_denied(host) && self.check_network_allowed(host)
    }

    fn check_network_allowed(&self, host: &str) -> bool {
        self.held.iter().any(|h| match h {
            ResourceHandle::NetworkAll => true,
            ResourceHandle::NetworkSocket(s) => match_network_host(s, host),
            _ => false,
        })
    }

    fn check_network_denied(&self, host: &str) -> bool {
        !(self.denied.contains(&ResourceHandle::NetworkAll)
            || self.denied.iter().any(|h| match h {
                ResourceHandle::NetworkSocket(s) => match_network_host(s, host),
                _ => false,
            }))
    }

    /// Verify environment variable read permissions (supports glob patterns).
    pub fn check_env_read(&self, var: &str) -> bool {
        if self.denied.iter().any(|h| match h {
            ResourceHandle::ReadEnv(s) => glob_match_str(s, var),
            _ => false,
        }) {
            return false;
        }
        self.held.iter().any(|h| match h {
            ResourceHandle::ReadEnv(s) => glob_match_str(s, var),
            _ => false,
        })
    }

    /// Verify environment variable write permissions (supports glob patterns).
    pub fn check_env_write(&self, var: &str) -> bool {
        if self.denied.iter().any(|h| match h {
            ResourceHandle::WriteEnv(s) => glob_match_str(s, var),
            _ => false,
        }) {
            return false;
        }
        self.held.iter().any(|h| match h {
            ResourceHandle::WriteEnv(s) => glob_match_str(s, var),
            _ => false,
        })
    }

    /// Verify file read permission (requires ReadFile or ReadDir on ancestor).
    pub fn check_read_file(&self, path: &Path) -> bool {
        if self.is_denied_read(path) {
            return false;
        }
        self.held.iter().any(|h| match h {
            ResourceHandle::ReadFile(p) => match_path_pattern(p, path),
            ResourceHandle::ReadDir(p) => match_path_pattern(p, path),
            _ => false,
        })
    }

    /// Verify file write permission (requires WriteFile or WriteDir on ancestor).
    pub fn check_write_file(&self, path: &Path) -> bool {
        if self.is_denied_write(path) {
            return false;
        }
        self.held.iter().any(|h| match h {
            ResourceHandle::WriteFile(p) => match_path_pattern(p, path),
            ResourceHandle::WriteDir(p) => match_path_pattern(p, path),
            _ => false,
        })
    }

    /// Verify process spawn permission.
    pub fn check_process_spawn(&self, cmd: &str) -> bool {
        if self.denied.contains(&ResourceHandle::ProcessSpawn)
            || self.denied.iter().any(|h| match h {
                ResourceHandle::ProcessSpawnPath(pattern) => glob_match_str(pattern, cmd),
                _ => false,
            })
        {
            return false;
        }
        self.held.iter().any(|h| match h {
            ResourceHandle::ProcessSpawn => true,
            ResourceHandle::ProcessSpawnPath(pattern) => glob_match_str(pattern, cmd),
            _ => false,
        })
    }
}

fn glob_match_str(pattern: &str, target: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
        if let Ok(glob) = globset::Glob::new(pattern) {
            glob.compile_matcher().is_match(target)
        } else {
            false
        }
    } else {
        pattern == target
    }
}

/// Match a network host against a pattern.
/// Non-glob patterns also match subdomains (e.g., `github.com` matches `api.github.com`).
fn match_network_host(pattern: &str, host: &str) -> bool {
    if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
        if let Ok(glob) = globset::Glob::new(pattern) {
            glob.compile_matcher().is_match(host)
        } else {
            false
        }
    } else {
        pattern == host || host.ends_with(&format!(".{}", pattern))
    }
}

fn match_path_pattern(pattern_path: &Path, target_path: &Path) -> bool {
    let p_str = pattern_path.to_string_lossy().to_string();
    if p_str.contains('*') || p_str.contains('?') || p_str.contains('[') {
        if let Ok(glob) = globset::Glob::new(&p_str) {
            glob.compile_matcher().is_match(target_path)
        } else {
            false
        }
    } else {
        target_path.starts_with(pattern_path)
    }
}

pub fn init() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_caps_defaults() {
        let pwd = PathBuf::from("/home/user/fshell");
        let reg = CapsRegistry::new_with_defaults(pwd);

        assert!(reg.check_read_dir(Path::new("/home/user/fshell/src")));
        assert!(!reg.check_read_dir(Path::new("/etc")));
        assert!(reg.check_process_spawn("test_cmd"));
    }

    #[test]
    fn test_caps_network() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::NetworkSocket("api.github.com".to_string()));

        assert!(reg.check_network("api.github.com"));
        assert!(!reg.check_network("google.com"));
    }

    // new()

    #[test]
    fn test_new_empty_registry() {
        let reg = CapsRegistry::new();
        assert!(reg.held.is_empty());
        assert!(!reg.check_read_dir(Path::new("/any")));
        assert!(!reg.check_write_dir(Path::new("/any")));
        assert!(!reg.check_read_write_file(Path::new("/any")));
        assert!(!reg.check_network("anything"));
        assert!(!reg.check_env_read("ANY"));
        assert!(!reg.check_env_write("ANY"));
        assert!(!reg.check_process_spawn("test_cmd"));
    }

    // grant()

    #[test]
    fn test_grant_read_dir() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadDir(PathBuf::from("/tmp")));
        assert!(reg.check_read_dir(Path::new("/tmp")));
    }

    #[test]
    fn test_grant_write_dir() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::WriteDir(PathBuf::from("/tmp")));
        assert!(reg.check_write_dir(Path::new("/tmp")));
    }

    #[test]
    fn test_grant_read_file() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadFile(PathBuf::from("/tmp")));
        assert!(reg.check_read_file(Path::new("/tmp")));
    }

    #[test]
    fn test_grant_write_file() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::WriteFile(PathBuf::from("/tmp")));
        assert!(reg.check_write_file(Path::new("/tmp")));
    }

    #[test]
    fn test_grant_network_socket() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::NetworkSocket("example.com".to_string()));
        assert!(reg.check_network("example.com"));
    }

    #[test]
    fn test_grant_network_all() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::NetworkAll);
        assert!(reg.check_network("anything.example.com"));
    }

    #[test]
    fn test_grant_env_read() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadEnv("PATH".to_string()));
        assert!(reg.check_env_read("PATH"));
    }

    #[test]
    fn test_grant_env_write() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::WriteEnv("HOME".to_string()));
        assert!(reg.check_env_write("HOME"));
    }

    #[test]
    fn test_grant_process_spawn() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ProcessSpawn);
        assert!(reg.check_process_spawn("test_cmd"));
    }

    // revoke()

    #[test]
    fn test_revoke_read_dir() {
        let mut reg = CapsRegistry::new();
        let h = ResourceHandle::ReadDir(PathBuf::from("/tmp"));
        reg.grant(h.clone());
        assert!(reg.check_read_dir(Path::new("/tmp")));
        assert!(reg.revoke(&h));
        assert!(!reg.check_read_dir(Path::new("/tmp")));
    }

    #[test]
    fn test_revoke_network_all() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::NetworkAll);
        assert!(reg.check_network("any"));
        assert!(reg.revoke(&ResourceHandle::NetworkAll));
        assert!(!reg.check_network("any"));
    }

    #[test]
    fn test_revoke_non_existent_returns_false() {
        let mut reg = CapsRegistry::new();
        let h = ResourceHandle::ReadDir(PathBuf::from("/nope"));
        assert!(!reg.revoke(&h));
    }

    #[test]
    fn test_revoke_does_not_affect_other_handles() {
        let mut reg = CapsRegistry::new();
        let r = ResourceHandle::ReadDir(PathBuf::from("/a"));
        let w = ResourceHandle::WriteDir(PathBuf::from("/b"));
        reg.grant(r.clone());
        reg.grant(w.clone());
        reg.revoke(&r);
        assert!(!reg.check_read_dir(Path::new("/a")));
        assert!(reg.check_write_dir(Path::new("/b")));
    }

    // new_with_defaults()

    #[test]
    fn test_new_with_defaults_grants_file_permissions() {
        let pwd = PathBuf::from("/home/user/project");
        let reg = CapsRegistry::new_with_defaults(pwd.clone());
        // 5 permissions: ReadDir, WriteDir, ReadFile, WriteFile, ProcessSpawn
        assert_eq!(reg.held.len(), 5);
        assert!(reg.check_read_dir(&pwd));
        assert!(reg.check_write_dir(&pwd));
        assert!(reg.check_read_write_file(&pwd));
        assert!(reg.check_process_spawn("test_cmd"));
    }

    #[test]
    fn test_new_with_defaults_subdir_allowed() {
        let reg = CapsRegistry::new_with_defaults(PathBuf::from("/home/user"));
        assert!(reg.check_read_dir(Path::new("/home/user/projects")));
        assert!(reg.check_write_dir(Path::new("/home/user/projects")));
        assert!(reg.check_read_write_file(Path::new("/home/user/projects")));
    }

    #[test]
    fn test_new_with_defaults_outside_pwd_denied() {
        let reg = CapsRegistry::new_with_defaults(PathBuf::from("/home/user"));
        assert!(!reg.check_read_dir(Path::new("/etc")));
        assert!(!reg.check_write_dir(Path::new("/etc")));
        assert!(!reg.check_read_write_file(Path::new("/etc")));
    }

    // check_read_dir()

    #[test]
    fn test_check_read_dir_subdirectory_allowed() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadDir(PathBuf::from("/base")));
        assert!(reg.check_read_dir(Path::new("/base/sub/dir")));
    }

    #[test]
    fn test_check_read_dir_parent_denied() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadDir(PathBuf::from("/base/sub")));
        assert!(!reg.check_read_dir(Path::new("/base")));
    }

    #[test]
    fn test_check_read_dir_different_dir_denied() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadDir(PathBuf::from("/base")));
        assert!(!reg.check_read_dir(Path::new("/other")));
    }

    #[test]
    fn test_check_read_dir_with_write_dir() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::WriteDir(PathBuf::from("/base")));
        assert!(reg.check_read_dir(Path::new("/base/sub")));
    }

    #[test]
    fn test_check_read_dir_with_read_file() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadFile(PathBuf::from("/base")));
        assert!(reg.check_read_dir(Path::new("/base/sub")));
    }

    // check_write_dir()

    #[test]
    fn test_check_write_dir_subdirectory_allowed() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::WriteDir(PathBuf::from("/base")));
        assert!(reg.check_write_dir(Path::new("/base/sub/dir")));
    }

    #[test]
    fn test_check_write_dir_parent_denied() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::WriteDir(PathBuf::from("/base/sub")));
        assert!(!reg.check_write_dir(Path::new("/base")));
    }

    #[test]
    fn test_check_write_dir_with_read_dir_only_denied() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadDir(PathBuf::from("/base")));
        assert!(!reg.check_write_dir(Path::new("/base")));
    }

    #[test]
    fn test_check_write_dir_with_write_file() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::WriteFile(PathBuf::from("/base")));
        assert!(reg.check_write_dir(Path::new("/base/sub")));
    }

    // check_read_write_file()

    #[test]
    fn test_check_read_write_file_subdirectory_allowed() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadFile(PathBuf::from("/base")));
        assert!(reg.check_read_write_file(Path::new("/base/sub/file.txt")));
    }

    #[test]
    fn test_check_read_write_file_with_read_dir_only() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadDir(PathBuf::from("/base")));
        assert!(reg.check_read_write_file(Path::new("/base/sub/file.txt")));
    }

    #[test]
    fn test_check_read_write_file_with_write_dir_only() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::WriteDir(PathBuf::from("/base")));
        assert!(reg.check_read_write_file(Path::new("/base/sub/file.txt")));
    }

    #[test]
    fn test_check_read_write_file_parent_denied() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadFile(PathBuf::from("/base/sub")));
        assert!(!reg.check_read_write_file(Path::new("/base")));
    }

    #[test]
    fn test_check_read_write_file_different_dir_denied() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::WriteFile(PathBuf::from("/base")));
        assert!(!reg.check_read_write_file(Path::new("/other")));
    }

    // check_network()

    #[test]
    fn test_check_network_all_matches_everything() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::NetworkAll);
        assert!(reg.check_network(""));
        assert!(reg.check_network("any.host.com"));
        assert!(reg.check_network("127.0.0.1"));
    }

    #[test]
    fn test_check_network_socket_exact_match() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::NetworkSocket("api.github.com".to_string()));
        assert!(reg.check_network("api.github.com"));
    }

    #[test]
    fn test_check_network_socket_substring_match() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::NetworkSocket("github.com".to_string()));
        assert!(reg.check_network("api.github.com"));
        assert!(reg.check_network("git.github.com"));
    }

    #[test]
    fn test_check_network_no_match() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::NetworkSocket("api.github.com".to_string()));
        assert!(!reg.check_network("gitlab.com"));
        assert!(!reg.check_network("google.com"));
    }

    #[test]
    fn test_check_network_no_grants_denies_all() {
        let reg = CapsRegistry::new();
        assert!(!reg.check_network("anything"));
    }

    // check_env_read()

    #[test]
    fn test_check_env_read_exact_match() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadEnv("PATH".to_string()));
        assert!(reg.check_env_read("PATH"));
    }

    #[test]
    fn test_check_env_read_wildcard_matches_all() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadEnv("*".to_string()));
        assert!(reg.check_env_read("PATH"));
        assert!(reg.check_env_read("HOME"));
        assert!(reg.check_env_read("USER"));
    }

    #[test]
    fn test_check_env_read_no_match() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadEnv("PATH".to_string()));
        assert!(!reg.check_env_read("HOME"));
    }

    #[test]
    fn test_check_env_read_empty_registry() {
        let reg = CapsRegistry::new();
        assert!(!reg.check_env_read("ANYTHING"));
    }

    // check_env_write()

    #[test]
    fn test_check_env_write_exact_match() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::WriteEnv("HOME".to_string()));
        assert!(reg.check_env_write("HOME"));
    }

    #[test]
    fn test_check_env_write_wildcard_matches_all() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::WriteEnv("*".to_string()));
        assert!(reg.check_env_write("PATH"));
        assert!(reg.check_env_write("HOME"));
        assert!(reg.check_env_write("USER"));
    }

    #[test]
    fn test_check_env_write_no_match() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::WriteEnv("HOME".to_string()));
        assert!(!reg.check_env_write("PATH"));
    }

    #[test]
    fn test_check_env_write_empty_registry() {
        let reg = CapsRegistry::new();
        assert!(!reg.check_env_write("ANYTHING"));
    }

    // check_process_spawn()

    #[test]
    fn test_check_process_spawn_with_permission() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ProcessSpawn);
        assert!(reg.check_process_spawn("test_cmd"));
    }

    #[test]
    fn test_check_process_spawn_without_permission() {
        let reg = CapsRegistry::new();
        assert!(!reg.check_process_spawn("test_cmd"));
    }

    #[test]
    fn test_check_process_spawn_after_revoke() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ProcessSpawn);
        assert!(reg.check_process_spawn("test_cmd"));
        reg.revoke(&ResourceHandle::ProcessSpawn);
        assert!(!reg.check_process_spawn("test_cmd"));
    }

    // Clone & Debug

    #[test]
    fn test_caps_registry_clone() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadDir(PathBuf::from("/tmp")));
        reg.grant(ResourceHandle::ProcessSpawn);
        let cloned = reg.clone();
        assert_eq!(reg, cloned);
        assert!(cloned.check_read_dir(Path::new("/tmp")));
        assert!(cloned.check_process_spawn("test_cmd"));
    }

    #[test]
    fn test_caps_registry_debug() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadDir(PathBuf::from("/tmp")));
        let debug_str = format!("{:?}", reg);
        assert!(debug_str.contains("CapsRegistry"));
        assert!(debug_str.contains("ReadDir"));
    }

    // deny()
    #[test]
    fn test_deny_read_dir() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadDir(PathBuf::from("/tmp")));
        assert!(reg.check_read_dir(Path::new("/tmp/file.txt")));
        reg.deny(ResourceHandle::ReadDir(PathBuf::from("/tmp")));
        assert!(!reg.check_read_dir(Path::new("/tmp/file.txt")));
    }

    #[test]
    fn test_deny_block_allowed_with_different_prefix() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadDir(PathBuf::from("/tmp")));
        reg.deny(ResourceHandle::ReadDir(PathBuf::from("/tmp/secret")));
        assert!(reg.check_read_dir(Path::new("/tmp/public")));
        assert!(!reg.check_read_dir(Path::new("/tmp/secret/data.txt")));
    }

    #[test]
    fn test_deny_removed_by_allow() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadDir(PathBuf::from("/tmp")));
        reg.deny(ResourceHandle::ReadDir(PathBuf::from("/tmp/secret")));
        assert!(!reg.check_read_dir(Path::new("/tmp/secret")));
        assert!(reg.allow(&ResourceHandle::ReadDir(PathBuf::from("/tmp/secret"))));
        assert!(reg.check_read_dir(Path::new("/tmp/secret")));
    }

    #[test]
    fn test_deny_process_spawn() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ProcessSpawn);
        assert!(reg.check_process_spawn("ls"));
        reg.deny(ResourceHandle::ProcessSpawn);
        assert!(!reg.check_process_spawn("ls"));
    }

    #[test]
    fn test_deny_process_spawn_path() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ProcessSpawnPath("/usr/bin/*".to_string()));
        assert!(reg.check_process_spawn("/usr/bin/ls"));
        reg.deny(ResourceHandle::ProcessSpawnPath("/usr/bin/rm".to_string()));
        assert!(reg.check_process_spawn("/usr/bin/ls"));
        assert!(!reg.check_process_spawn("/usr/bin/rm"));
    }

    #[test]
    fn test_deny_network_all() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::NetworkAll);
        assert!(reg.check_network("example.com"));
        reg.deny(ResourceHandle::NetworkAll);
        assert!(!reg.check_network("example.com"));
    }

    #[test]
    fn test_deny_network_specific_host() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::NetworkAll);
        reg.deny(ResourceHandle::NetworkSocket("evil.com".to_string()));
        assert!(reg.check_network("example.com"));
        assert!(!reg.check_network("evil.com"));
    }

    #[test]
    fn test_deny_env_read_glob() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadEnv("*".to_string()));
        reg.deny(ResourceHandle::ReadEnv("SECRET_*".to_string()));
        assert!(reg.check_env_read("PATH"));
        assert!(reg.check_env_read("HOME"));
        assert!(!reg.check_env_read("SECRET_KEY"));
        assert!(!reg.check_env_read("SECRET_TOKEN"));
    }

    #[test]
    fn test_deny_env_write_glob() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::WriteEnv("*".to_string()));
        reg.deny(ResourceHandle::WriteEnv("PATH".to_string()));
        assert!(reg.check_env_write("HOME"));
        assert!(!reg.check_env_write("PATH"));
    }

    // check_with_deny()
    #[test]
    fn test_check_with_deny_allowed() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadDir(PathBuf::from("/tmp")));
        assert_eq!(
            reg.check_with_deny(&ResourceHandle::ReadDir(PathBuf::from("/tmp"))),
            Ok(())
        );
    }

    #[test]
    fn test_check_with_deny_denied() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadDir(PathBuf::from("/tmp")));
        reg.deny(ResourceHandle::ReadDir(PathBuf::from("/tmp")));
        let result = reg.check_with_deny(&ResourceHandle::ReadDir(PathBuf::from("/tmp")));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("denied"));
    }

    #[test]
    fn test_check_with_deny_not_held() {
        let reg = CapsRegistry::new();
        let result = reg.check_with_deny(&ResourceHandle::ReadDir(PathBuf::from("/tmp")));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not held"));
    }

    #[test]
    fn test_check_with_deny_process_spawn() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ProcessSpawn);
        reg.deny(ResourceHandle::ProcessSpawn);
        assert!(reg.check_with_deny(&ResourceHandle::ProcessSpawn).is_err());
    }

    // Policy composition (allow + deny)
    #[test]
    fn test_policy_composition_allow_dir_deny_file() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadDir(PathBuf::from("/data")));
        reg.deny(ResourceHandle::ReadFile(PathBuf::from("/data/secrets.txt")));
        assert!(reg.check_read_dir(Path::new("/data")));
        assert!(reg.check_read_file(Path::new("/data/readme.txt")));
        assert!(!reg.check_read_file(Path::new("/data/secrets.txt")));
    }

    #[test]
    fn test_policy_composition_allow_all_network_deny_evil() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::NetworkAll);
        reg.deny(ResourceHandle::NetworkSocket("*.evil.com".to_string()));
        assert!(reg.check_network("good.com"));
        assert!(!reg.check_network("sub.evil.com"));
        // evil.com does not match *.evil.com (globset * requires at least one char before literal),
        // deny the exact name too
        assert!(reg.check_network("evil.com"));
        reg.deny(ResourceHandle::NetworkSocket("evil.com".to_string()));
        assert!(!reg.check_network("evil.com"));
    }

    #[test]
    fn test_policy_composition_glob_env_read() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadEnv("*".to_string()));
        reg.deny(ResourceHandle::ReadEnv("AWS_*".to_string()));
        assert!(reg.check_env_read("PATH"));
        assert!(!reg.check_env_read("AWS_SECRET_KEY"));
        assert!(!reg.check_env_read("AWS_ACCESS_KEY_ID"));
    }

    #[test]
    fn test_glob_env_read_pattern() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadEnv("SECRET_*".to_string()));
        assert!(reg.check_env_read("SECRET_KEY"));
        assert!(reg.check_env_read("SECRET_TOKEN"));
        assert!(!reg.check_env_read("PATH"));
        assert!(!reg.check_env_read("PUBLIC_KEY"));
    }

    #[test]
    fn test_glob_env_write_pattern() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::WriteEnv("*_PATH".to_string()));
        assert!(reg.check_env_write("MY_PATH"));
        assert!(reg.check_env_write("SECRET_PATH"));
        assert!(!reg.check_env_write("PATH_VAR"));
    }

    #[test]
    fn test_serde_denied_field_default() {
        let json = r#"{"held":[],"strict_mode":false}"#;
        let reg: CapsRegistry = serde_json::from_str(json).unwrap();
        assert!(reg.denied.is_empty());
        assert!(reg.held.is_empty());
        assert!(!reg.strict_mode);
    }

    #[test]
    fn test_serde_denied_roundtrip() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadDir(PathBuf::from("/tmp")));
        reg.deny(ResourceHandle::ReadDir(PathBuf::from("/tmp/secret")));
        let json = serde_json::to_string(&reg).unwrap();
        let back: CapsRegistry = serde_json::from_str(&json).unwrap();
        assert_eq!(reg, back);
        assert!(!back.check_read_dir(Path::new("/tmp/secret")));
        assert!(back.check_read_dir(Path::new("/tmp/public")));
    }

    #[test]
    fn test_denied_not_auto_granted_by_new_with_defaults() {
        let mut reg = CapsRegistry::new_with_defaults(PathBuf::from("/home/user"));
        // Manually add a deny that overlaps with defaults
        reg.deny(ResourceHandle::ReadDir(PathBuf::from("/home/user")));
        // Deny should block the default grant
        assert!(!reg.check_read_dir(Path::new("/home/user")));
    }

    #[test]
    fn test_deny_parent_block_child_read() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ReadDir(PathBuf::from("/project")));
        reg.deny(ResourceHandle::ReadDir(PathBuf::from("/project/sub")));
        assert!(reg.check_read_dir(Path::new("/project/other")));
        assert!(reg.check_read_file(Path::new("/project/other/file.rs")));
        assert!(!reg.check_read_dir(Path::new("/project/sub")));
        assert!(!reg.check_read_file(Path::new("/project/sub/file.rs")));
    }

    #[test]
    fn test_deny_write_file_glob() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::WriteDir(PathBuf::from("/tmp")));
        reg.deny(ResourceHandle::WriteFile(
            PathBuf::from("/tmp/*.exe").to_path_buf(),
        ));
        assert!(reg.check_write_file(Path::new("/tmp/readme.txt")));
        // The glob pattern in deny should be handled by match_path_pattern which uses globset
        assert!(!reg.check_write_file(Path::new("/tmp/virus.exe")));
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("FSH_HOME")
        .ok()
        .or_else(|| std::env::var("HOME").ok())
        .or_else(|| std::env::var("USERPROFILE").ok())
        .map(PathBuf::from)
}

/// Resolve the path part of an `fs.read:`/`fs.write:` capability string,
/// expanding `~` and `~/...` against the home directory.
fn resolve_cap_path(rest: &str, base_dir: &Path) -> PathBuf {
    if rest == "~" {
        return home_dir().unwrap_or_else(|| base_dir.join(rest));
    }
    if let Some(sub) = rest.strip_prefix("~/")
        && let Some(h) = home_dir()
    {
        return h.join(sub);
    }
    base_dir.join(rest)
}

pub fn parse_cap_string(s: &str, base_dir: &Path) -> Option<ResourceHandle> {
    if s == "process.spawn" {
        return Some(ResourceHandle::ProcessSpawn);
    }
    if let Some(rest) = s.strip_prefix("process.spawn:") {
        return Some(ResourceHandle::ProcessSpawnPath(rest.to_string()));
    }
    if s == "net.*" {
        return Some(ResourceHandle::NetworkAll);
    }
    if let Some(rest) = s.strip_prefix("net.") {
        return Some(ResourceHandle::NetworkSocket(rest.to_string()));
    }
    if let Some(rest) = s.strip_prefix("env.read:") {
        return Some(ResourceHandle::ReadEnv(rest.to_string()));
    }
    if let Some(rest) = s.strip_prefix("env.write:") {
        return Some(ResourceHandle::WriteEnv(rest.to_string()));
    }
    if let Some(rest) = s.strip_prefix("fs.read:") {
        return Some(ResourceHandle::ReadDir(resolve_cap_path(rest, base_dir)));
    }
    if let Some(rest) = s.strip_prefix("fs.write:") {
        return Some(ResourceHandle::WriteDir(resolve_cap_path(rest, base_dir)));
    }
    if s == "fs.read" {
        return Some(ResourceHandle::ReadDir(base_dir.to_path_buf()));
    }
    if s == "fs.write" {
        return Some(ResourceHandle::WriteDir(base_dir.to_path_buf()));
    }
    None
}

#[cfg(test)]
mod parse_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_parse_wildcards_and_tilde() {
        let base = Path::new("/base");

        // Save current home env
        let old_home = fshell_core::get_var("FSH_HOME").map(|v| v.to_string_lossy().to_string());
        fshell_core::set_var("FSH_HOME", "/home/testuser");

        let cap1 = parse_cap_string("fs.read:~/projects/**", base).unwrap();
        assert_eq!(
            cap1,
            ResourceHandle::ReadDir(PathBuf::from("/home/testuser/projects/**"))
        );

        let cap2 = parse_cap_string("process.spawn:/usr/bin/*", base).unwrap();
        assert_eq!(
            cap2,
            ResourceHandle::ProcessSpawnPath("/usr/bin/*".to_string())
        );

        let cap3 = parse_cap_string("net.api.*.com", base).unwrap();
        assert_eq!(cap3, ResourceHandle::NetworkSocket("api.*.com".to_string()));

        // Restore env
        if let Some(h) = old_home {
            fshell_core::set_var("FSH_HOME", &h);
        } else {
            fshell_core::remove_var("FSH_HOME");
        }
    }

    #[test]
    fn test_glob_matching() {
        let mut reg = CapsRegistry::new();
        reg.grant(ResourceHandle::ProcessSpawnPath("/usr/bin/*".to_string()));
        reg.grant(ResourceHandle::ReadDir(PathBuf::from(
            "/home/testuser/projects/**",
        )));
        reg.grant(ResourceHandle::NetworkSocket("*.github.com".to_string()));

        assert!(reg.check_process_spawn("/usr/bin/ls"));
        assert!(reg.check_process_spawn("/usr/bin/git"));
        assert!(!reg.check_process_spawn("/bin/ls"));

        assert!(reg.check_read_file(Path::new("/home/testuser/projects/myproj/main.rs")));
        assert!(!reg.check_read_file(Path::new("/home/testuser/other/main.rs")));

        assert!(reg.check_network("api.github.com"));
        assert!(!reg.check_network("github.com")); // globset '*.github.com' does not match 'github.com'
        assert!(!reg.check_network("google.com"));
    }
}
