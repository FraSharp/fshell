// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::fs;
use std::path::Path;

use crate::repo::Repository;

#[derive(Debug, Clone)]
pub struct RefEntry {
    pub name: String,
    pub oid: [u8; 20],
}

/// Parse a 40-char hex string to a 20-byte SHA1.
#[allow(clippy::result_unit_err)]
pub fn parse_oid_str(s: &str) -> Result<[u8; 20], ()> {
    let s = s.trim();
    if s.len() != 40 {
        return Err(());
    }
    let mut oid = [0u8; 20];
    for i in 0..20 {
        oid[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| ())?;
    }
    Ok(oid)
}

impl Repository {
    /// Resolve a ref name to a SHA1. Handles loose refs, packed-refs,
    /// and symbolic refs recursively.
    pub fn resolve_ref(&self, name: &str) -> Result<[u8; 20], crate::repo::Error> {
        // Try loose ref first
        let loose = self.git_dir().join(name);
        if let Ok(content) = fs::read_to_string(&loose) {
            let content = content.trim_end();
            if content.starts_with("ref: ")
                && let Some(next) = content.strip_prefix("ref: ")
            {
                return self.resolve_ref(next);
            }
            return parse_oid_str(content)
                .map_err(|_| crate::repo::Error::InvalidRef(format!("invalid oid in {name}")));
        }

        // Try packed-refs
        self.resolve_packed_ref(name)
    }

    fn resolve_packed_ref(&self, name: &str) -> Result<[u8; 20], crate::repo::Error> {
        let packed_path = self.git_dir().join("packed-refs");
        if let Ok(content) = fs::read_to_string(&packed_path) {
            for line in content.lines() {
                let line = line.trim();
                if line.starts_with('#') || line.is_empty() {
                    continue;
                }
                if let Some((oid_str, ref_name)) = line.split_once(' ')
                    && ref_name == name
                {
                    return parse_oid_str(oid_str).map_err(|_| {
                        crate::repo::Error::InvalidRef(format!("invalid oid in packed ref {name}"))
                    });
                }
            }
        }
        Err(crate::repo::Error::InvalidRef(format!(
            "ref not found: {name}"
        )))
    }

    /// List all refs under a prefix (e.g., "refs/heads/").
    pub fn list_refs(&self, prefix: &str) -> Vec<RefEntry> {
        let mut refs = Vec::new();

        // Loose refs
        let dir = self.git_dir().join(prefix);
        if dir.is_dir() {
            self.walk_refs_dir(&dir, &mut refs);
        }

        // Packed refs
        let packed_path = self.git_dir().join("packed-refs");
        if let Ok(content) = fs::read_to_string(&packed_path) {
            for line in content.lines() {
                let line = line.trim();
                if line.starts_with('#') || line.is_empty() {
                    continue;
                }
                if let Some((oid_str, ref_name)) = line.split_once(' ')
                    && ref_name.starts_with(prefix)
                    && let Ok(oid) = parse_oid_str(oid_str)
                    && !refs.iter().any(|r| r.name == ref_name)
                {
                    refs.push(RefEntry {
                        name: ref_name.to_string(),
                        oid,
                    });
                }
            }
        }

        refs
    }

    fn walk_refs_dir(&self, dir: &Path, refs: &mut Vec<RefEntry>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    self.walk_refs_dir(&path, refs);
                } else if let Ok(content) = fs::read_to_string(&path) {
                    let content = content.trim().to_string();
                    if let Ok(oid) = parse_oid_str(&content) {
                        let relative = path
                            .strip_prefix(self.git_dir())
                            .expect("ref path is always under git_dir by construction")
                            .to_string_lossy()
                            .replace('\\', "/");
                        refs.push(RefEntry {
                            name: relative,
                            oid,
                        });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::repo::Repository;
    use std::fs;

    fn setup_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".git/refs/heads")).unwrap();
        fs::create_dir_all(tmp.path().join(".git/refs/remotes/origin")).unwrap();
        tmp
    }

    #[test]
    fn resolve_loose_ref() {
        let tmp = setup_repo();
        fs::write(
            tmp.path().join(".git/refs/heads/main"),
            "abc123def456789012345678901234567890abcd\n",
        )
        .unwrap();
        let repo = Repository::discover(tmp.path()).unwrap();
        let oid = repo.resolve_ref("refs/heads/main").unwrap();
        assert_eq!(hex::encode(oid), "abc123def456789012345678901234567890abcd");
    }

    #[test]
    fn resolve_packed_ref() {
        let tmp = setup_repo();
        let packed = "abc123def456789012345678901234567890abcd refs/heads/main\n";
        fs::write(tmp.path().join(".git/packed-refs"), packed).unwrap();
        let repo = Repository::discover(tmp.path()).unwrap();
        let oid = repo.resolve_ref("refs/heads/main").unwrap();
        assert_eq!(hex::encode(oid), "abc123def456789012345678901234567890abcd");
    }

    #[test]
    fn resolve_symbolic_ref() {
        let tmp = setup_repo();
        fs::write(tmp.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(
            tmp.path().join(".git/refs/heads/main"),
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef\n",
        )
        .unwrap();
        let repo = Repository::discover(tmp.path()).unwrap();
        let oid = repo.resolve_ref("HEAD").unwrap();
        assert_eq!(hex::encode(oid), "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef");
    }

    #[test]
    fn resolve_nested_symbolic() {
        let tmp = setup_repo();
        fs::write(
            tmp.path().join(".git/refs/heads/feature"),
            "ref: refs/heads/main\n",
        )
        .unwrap();
        fs::write(
            tmp.path().join(".git/refs/heads/main"),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
        )
        .unwrap();
        let repo = Repository::discover(tmp.path()).unwrap();
        let oid = repo.resolve_ref("refs/heads/feature").unwrap();
        assert_eq!(hex::encode(oid), "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    }

    #[test]
    fn list_loose_refs() {
        let tmp = setup_repo();
        fs::write(
            tmp.path().join(".git/refs/heads/main"),
            "abc123def456789012345678901234567890abcd\n",
        )
        .unwrap();
        fs::write(
            tmp.path().join(".git/refs/heads/feature"),
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef\n",
        )
        .unwrap();
        let repo = Repository::discover(tmp.path()).unwrap();
        let refs = repo.list_refs("refs/heads/");
        assert_eq!(refs.len(), 2);
        assert!(refs.iter().any(|r| r.name == "refs/heads/main"));
        assert!(refs.iter().any(|r| r.name == "refs/heads/feature"));
    }

    #[test]
    fn list_packed_refs() {
        let tmp = setup_repo();
        let packed = "# pack-refs with: peeled fully-peeled sorted\nabc123def456789012345678901234567890abcd refs/heads/main\n";
        fs::write(tmp.path().join(".git/packed-refs"), packed).unwrap();
        let repo = Repository::discover(tmp.path()).unwrap();
        let refs = repo.list_refs("refs/heads/");
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn loose_overrides_packed() {
        let tmp = setup_repo();
        let packed = "1111111111111111111111111111111111111111 refs/heads/main\n";
        fs::write(tmp.path().join(".git/packed-refs"), packed).unwrap();
        fs::write(
            tmp.path().join(".git/refs/heads/main"),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
        )
        .unwrap();
        let repo = Repository::discover(tmp.path()).unwrap();
        let refs = repo.list_refs("refs/heads/");
        let main = refs.iter().find(|r| r.name == "refs/heads/main").unwrap();
        assert_eq!(
            hex::encode(main.oid),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
    }

    #[test]
    fn ref_not_found() {
        let tmp = setup_repo();
        let repo = Repository::discover(tmp.path()).unwrap();
        assert!(repo.resolve_ref("refs/heads/nonexistent").is_err());
    }
}
