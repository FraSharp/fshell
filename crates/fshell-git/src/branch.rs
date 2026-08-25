// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_hash::FxHashMap;
use std::collections::VecDeque;

use crate::repo::Repository;

#[derive(Debug, Clone)]
pub struct Branch {
    pub name: String,
    pub head: bool,
    pub oid: [u8; 20],
    pub ahead: u32,
    pub behind: u32,
}

impl Repository {
    pub fn branches(&self) -> Result<Vec<Branch>, crate::repo::Error> {
        let head = self.head()?;
        let refs = self.list_refs("refs/heads/");

        let mut branches: Vec<Branch> = refs
            .into_iter()
            .map(|r| {
                let name = r
                    .name
                    .strip_prefix("refs/heads/")
                    .unwrap_or(&r.name)
                    .to_string();
                let is_head = head.branch.as_deref() == Some(&name);
                Branch {
                    name,
                    head: is_head,
                    oid: r.oid,
                    ahead: 0,
                    behind: 0,
                }
            })
            .collect();

        for branch in &mut branches {
            if !branch.head {
                let (ahead, behind) = self.ahead_behind_oids(&head.oid, &branch.oid)?;
                branch.ahead = ahead;
                branch.behind = behind;
            }
        }

        Ok(branches)
    }

    pub fn ahead_behind(&self) -> Result<(u32, u32), crate::repo::Error> {
        let head = self.head()?;
        if head.detached {
            return Ok((0, 0));
        }

        let branch_name = head
            .branch
            .as_ref()
            .ok_or(crate::repo::Error::InvalidRef("not on a branch".into()))?;

        match self.find_upstream(branch_name)? {
            Some((_, upstream_oid)) => self.ahead_behind_oids(&upstream_oid, &head.oid),
            None => Ok((0, 0)),
        }
    }

    fn ahead_behind_oids(
        &self,
        local: &[u8; 20],
        remote: &[u8; 20],
    ) -> Result<(u32, u32), crate::repo::Error> {
        if local == remote {
            return Ok((0, 0));
        }

        let local_set = self.reachable_commits(local)?;
        let remote_set = self.reachable_commits(remote)?;

        let ahead = local_set
            .keys()
            .filter(|k| !remote_set.contains_key(*k))
            .count() as u32;
        let behind = remote_set
            .keys()
            .filter(|k| !local_set.contains_key(*k))
            .count() as u32;

        Ok((ahead, behind))
    }

    fn reachable_commits(
        &self,
        start: &[u8; 20],
    ) -> Result<FxHashMap<[u8; 20], ()>, crate::repo::Error> {
        let mut visited = FxHashMap::default();
        let mut queue = VecDeque::new();

        queue.push_back(*start);
        visited.insert(*start, ());

        while let Some(oid) = queue.pop_front() {
            match self.read_commit(&oid) {
                Ok(commit) => {
                    for parent in &commit.parents {
                        if !visited.contains_key(parent) {
                            visited.insert(*parent, ());
                            queue.push_back(*parent);
                        }
                    }
                }
                Err(_) => {
                    // Object not found (maybe shallow clone or packfile delta)
                }
            }
        }

        Ok(visited)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn create_loose_object(
        git_dir: &std::path::Path,
        oid_hex: &str,
        obj_type: &str,
        content: &[u8],
    ) {
        let dir = git_dir.join("objects").join(&oid_hex[..2]);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(&oid_hex[2..]);

        use flate2::Compression;
        use flate2::write::ZlibEncoder;
        use std::io::Write;

        let header = format!("{} {}\0", obj_type, content.len());
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(header.as_bytes()).unwrap();
        encoder.write_all(content).unwrap();
        let compressed = encoder.finish().unwrap();
        fs::write(&path, compressed).unwrap();
    }

    #[test]
    fn branches_list() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();

        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(
            git_dir.join("refs/heads/main"),
            "abc123def456789012345678901234567890abcd\n",
        )
        .unwrap();
        fs::write(
            git_dir.join("refs/heads/feature"),
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef\n",
        )
        .unwrap();

        let repo = Repository::discover(tmp.path()).unwrap();
        let branches = repo.branches().unwrap();
        assert_eq!(branches.len(), 2);

        let main = branches.iter().find(|b| b.name == "main").unwrap();
        assert!(main.head);

        let feature = branches.iter().find(|b| b.name == "feature").unwrap();
        assert!(!feature.head);
    }

    #[test]
    fn ahead_behind_same() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        fs::create_dir_all(git_dir.join("refs/remotes/origin")).unwrap();

        let oid = "abc123def456789012345678901234567890abcd";
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(git_dir.join("refs/heads/main"), format!("{oid}\n")).unwrap();
        fs::write(git_dir.join("refs/remotes/origin/main"), format!("{oid}\n")).unwrap();

        fs::write(
            git_dir.join("config"),
            "[branch \"main\"]\n    remote = origin\n    merge = refs/heads/main\n",
        )
        .unwrap();

        let repo = Repository::discover(tmp.path()).unwrap();
        let (ahead, behind) = repo.ahead_behind().unwrap();
        assert_eq!(ahead, 0);
        assert_eq!(behind, 0);
    }

    #[test]
    fn ahead_behind_with_commits() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        fs::create_dir_all(git_dir.join("refs/remotes/origin")).unwrap();

        let c1 = "1111111111111111111111111111111111111111";
        let c2 = "2222222222222222222222222222222222222222";
        let c3 = "3333333333333333333333333333333333333333";
        let c4 = "4444444444444444444444444444444444444444";

        create_loose_object(
            &git_dir,
            c1,
            "commit",
            b"tree 0000000000000000000000000000000000000000\n\ninit\n",
        );
        create_loose_object(
            &git_dir,
            c2,
            "commit",
            format!("tree 0000000000000000000000000000000000000000\nparent {c1}\n\nsecond\n")
                .as_bytes(),
        );
        create_loose_object(
            &git_dir,
            c3,
            "commit",
            format!("tree 0000000000000000000000000000000000000000\nparent {c2}\n\nthird\n")
                .as_bytes(),
        );
        create_loose_object(
            &git_dir,
            c4,
            "commit",
            format!("tree 0000000000000000000000000000000000000000\nparent {c2}\n\nfourth\n")
                .as_bytes(),
        );

        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(git_dir.join("refs/heads/main"), format!("{c3}\n")).unwrap();
        fs::write(git_dir.join("refs/remotes/origin/main"), format!("{c4}\n")).unwrap();
        fs::write(
            git_dir.join("config"),
            "[branch \"main\"]\n    remote = origin\n    merge = refs/heads/main\n",
        )
        .unwrap();

        let repo = Repository::discover(tmp.path()).unwrap();
        let (ahead, behind) = repo.ahead_behind().unwrap();
        assert_eq!(ahead, 1);
        assert_eq!(behind, 1);
    }

    #[test]
    fn ahead_behind_detached() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();

        fs::write(
            git_dir.join("HEAD"),
            "abc123def456789012345678901234567890abcd\n",
        )
        .unwrap();

        let repo = Repository::discover(tmp.path()).unwrap();
        let (ahead, behind) = repo.ahead_behind().unwrap();
        assert_eq!(ahead, 0);
        assert_eq!(behind, 0);
    }
}
