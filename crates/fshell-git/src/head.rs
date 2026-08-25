// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::fs;

use crate::refs::parse_oid_str;
use crate::repo::{Error, Repository};

#[derive(Debug, Clone)]
pub struct HeadInfo {
    pub branch: Option<String>,
    pub oid: [u8; 20],
    pub detached: bool,
}

impl Repository {
    pub fn head(&self) -> Result<HeadInfo, Error> {
        let head_path = self.git_dir().join("HEAD");
        let content = fs::read_to_string(&head_path)?;
        let content = content.trim_end();

        if let Some(ref_name) = content.strip_prefix("ref: ") {
            let oid = self.resolve_ref(ref_name)?;
            let branch = ref_name.strip_prefix("refs/heads/").map(|s| s.to_string());
            Ok(HeadInfo {
                branch,
                oid,
                detached: false,
            })
        } else {
            let oid = parse_oid_str(content)
                .map_err(|_| Error::InvalidRef(format!("invalid HEAD: {content}")))?;
            Ok(HeadInfo {
                branch: None,
                oid,
                detached: true,
            })
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
        tmp
    }

    #[test]
    fn head_symbolic_ref() {
        let tmp = setup_repo();
        fs::write(tmp.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(
            tmp.path().join(".git/refs/heads/main"),
            "abc123def456789012345678901234567890abcd\n",
        )
        .unwrap();
        let repo = Repository::discover(tmp.path()).unwrap();
        let head = repo.head().unwrap();
        assert_eq!(head.branch.as_deref(), Some("main"));
        assert!(!head.detached);
        assert_eq!(
            hex::encode(head.oid),
            "abc123def456789012345678901234567890abcd"
        );
    }

    #[test]
    fn head_detached() {
        let tmp = setup_repo();
        fs::write(
            tmp.path().join(".git/HEAD"),
            "abc123def456789012345678901234567890abcd\n",
        )
        .unwrap();
        let repo = Repository::discover(tmp.path()).unwrap();
        let head = repo.head().unwrap();
        assert!(head.branch.is_none());
        assert!(head.detached);
    }

    #[test]
    fn head_nested_symbolic() {
        let tmp = setup_repo();
        fs::write(tmp.path().join(".git/HEAD"), "ref: refs/heads/feature\n").unwrap();
        fs::write(
            tmp.path().join(".git/refs/heads/feature"),
            "ref: refs/heads/main\n",
        )
        .unwrap();
        fs::write(
            tmp.path().join(".git/refs/heads/main"),
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef\n",
        )
        .unwrap();
        let repo = Repository::discover(tmp.path()).unwrap();
        let head = repo.head().unwrap();
        assert_eq!(
            hex::encode(head.oid),
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
        );
    }

    #[test]
    fn head_invalid() {
        let tmp = setup_repo();
        fs::write(tmp.path().join(".git/HEAD"), "not-a-ref\n").unwrap();
        let repo = Repository::discover(tmp.path()).unwrap();
        assert!(repo.head().is_err());
    }
}
