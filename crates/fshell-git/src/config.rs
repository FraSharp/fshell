// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::repo::{Error, Repository};
use std::collections::HashMap;
use std::fs;

#[derive(Debug, Clone)]
pub struct Config {
    sections: HashMap<String, HashMap<String, String>>,
}

impl Config {
    pub fn parse(content: &str) -> Result<Self, Error> {
        let mut sections = HashMap::new();
        let mut current_section = String::new();

        for line in content.lines() {
            let line = line.trim();

            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }

            if line.starts_with('[') {
                if let Some(end) = line.find(']') {
                    let header = &line[1..end];
                    current_section = if let Some(sub_start) = header.find('"') {
                        if let Some(sub_end) = header.rfind('"') {
                            let main = header[..sub_start].trim();
                            let sub = &header[sub_start + 1..sub_end];
                            format!("{main}.{sub}")
                        } else {
                            header.to_string()
                        }
                    } else {
                        header.to_string()
                    };
                    sections
                        .entry(current_section.clone())
                        .or_insert_with(HashMap::new);
                }
                continue;
            }

            if let Some(eq_pos) = line.find('=') {
                let key = line[..eq_pos].trim().to_string();
                let value = line[eq_pos + 1..].trim().to_string();
                let value = if (value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\''))
                {
                    &value[1..value.len() - 1]
                } else {
                    &value
                };
                if let Some(section) = sections.get_mut(&current_section) {
                    section.insert(key, value.to_string());
                }
            }
        }

        Ok(Config { sections })
    }

    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.sections
            .get(section)
            .and_then(|s| s.get(key))
            .map(|s| s.as_str())
    }
}

impl Repository {
    pub fn config(&self) -> Result<Config, Error> {
        let config_path = self.git_dir().join("config");
        let content = match fs::read_to_string(&config_path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Config {
                    sections: HashMap::new(),
                });
            }
            Err(e) => return Err(e.into()),
        };
        Config::parse(&content)
    }

    pub fn find_upstream(&self, branch: &str) -> Result<Option<(String, [u8; 20])>, Error> {
        let config = self.config()?;

        let section = format!("branch.\"{branch}\"");
        let remote = config.get(&section, "remote");
        let merge = config.get(&section, "merge");

        if let (Some(remote), Some(merge)) = (remote, merge) {
            let short_merge = merge.strip_prefix("refs/heads/").unwrap_or(merge);
            let ref_name = format!("{remote}/{short_merge}");
            match self.resolve_ref(&ref_name) {
                Ok(oid) => Ok(Some((remote.to_string(), oid))),
                Err(_) => Ok(None),
            }
        } else {
            let ref_name = format!("refs/remotes/origin/{branch}");
            match self.resolve_ref(&ref_name) {
                Ok(oid) => Ok(Some(("origin".to_string(), oid))),
                Err(_) => Ok(None),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_config() {
        let content = r#"[core]
    autocrlf = true
    repositoryformatversion = 0
[branch "main"]
    remote = origin
    merge = refs/heads/main
"#;
        let config = Config::parse(content).unwrap();
        assert_eq!(config.get("core", "autocrlf"), Some("true"));
        assert_eq!(config.get("branch.main", "remote"), Some("origin"));
        assert_eq!(config.get("branch.main", "merge"), Some("refs/heads/main"));
    }

    #[test]
    fn parse_quoted_values() {
        let content = "[user]\n    name = \"John Doe\"\n    email = 'john@example.com'\n";
        let config = Config::parse(content).unwrap();
        assert_eq!(config.get("user", "name"), Some("John Doe"));
        assert_eq!(config.get("user", "email"), Some("john@example.com"));
    }

    #[test]
    fn parse_comments() {
        let content = "# comment\n[core]\n; another comment\n    bare = false\n";
        let config = Config::parse(content).unwrap();
        assert_eq!(config.get("core", "bare"), Some("false"));
    }

    #[test]
    fn missing_section() {
        let content = "[core]\n    bare = false\n";
        let config = Config::parse(content).unwrap();
        assert_eq!(config.get("nonexistent", "key"), None);
    }

    #[test]
    fn find_upstream_from_config() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        fs::create_dir_all(git_dir.join("refs/remotes/origin")).unwrap();
        fs::write(
            git_dir.join("config"),
            "[branch \"main\"]\n    remote = origin\n    merge = refs/heads/main\n",
        )
        .unwrap();
        fs::write(
            git_dir.join("refs/remotes/origin/main"),
            "abc123def456789012345678901234567890abcd\n",
        )
        .unwrap();

        let repo = Repository::discover(tmp.path()).unwrap();
        let upstream = repo.find_upstream("main").unwrap().unwrap();
        assert_eq!(upstream.0, "origin");
        assert_eq!(
            hex::encode(upstream.1),
            "abc123def456789012345678901234567890abcd"
        );
    }

    #[test]
    fn find_upstream_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        fs::create_dir_all(git_dir.join("refs/remotes/origin")).unwrap();
        fs::write(
            git_dir.join("refs/remotes/origin/feature"),
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef\n",
        )
        .unwrap();

        let repo = Repository::discover(tmp.path()).unwrap();
        let upstream = repo.find_upstream("feature").unwrap().unwrap();
        assert_eq!(upstream.0, "origin");
    }
}
