// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_hash::FxHashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::ignore::IgnoreRules;
use crate::index::Index;
use crate::repo::Repository;
use fshell_hash::FxHashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Clean,
    Modified,
    Added,
    Renamed,
    Deleted,
    TypeChange,
    Ignored,
    Conflicted,
    Untracked,
}

impl Repository {
    pub fn status(&self) -> Result<FxHashMap<PathBuf, Status>, crate::repo::Error> {
        let index = Index::parse(self.git_dir())
            .map_err(|e| crate::repo::Error::InvalidIndex(e.to_string()))?;

        let head_path = self.git_dir().join("HEAD");
        let has_head = head_path.try_exists().map_err(crate::repo::Error::Io)?;
        let head_entries = if has_head {
            match self.head() {
                Ok(head) => {
                    let commit = self.read_commit(&head.oid)?;
                    self.read_tree_entries(&commit.tree)?
                }
                Err(crate::repo::Error::InvalidRef(message))
                    if message.starts_with("ref not found:") =>
                {
                    FxHashMap::default()
                }
                Err(error) => return Err(error),
            }
        } else {
            FxHashMap::default()
        };

        let root_ignore = self.collect_ignore_rules(self.work_dir());
        let mut map = FxHashMap::default();

        // Exact object-id matches are unambiguous rename candidates. Ambiguous
        // duplicate-content pairs remain additions/deletions rather than being
        // assigned an arbitrary source path.
        let mut removed_by_oid: FxHashMap<([u8; 20], u32), Vec<PathBuf>> = FxHashMap::default();
        let mut added_by_oid: FxHashMap<([u8; 20], u32), Vec<PathBuf>> = FxHashMap::default();
        let indexed_paths: FxHashSet<PathBuf> =
            index.iter().map(|entry| entry.path.clone()).collect();
        for (path, head_entry) in &head_entries {
            if !indexed_paths.contains(path) {
                removed_by_oid
                    .entry((head_entry.oid, head_entry.mode & 0o170000))
                    .or_default()
                    .push(path.clone());
            }
        }
        for entry in index.iter().filter(|entry| entry.stage == 0) {
            if !head_entries.contains_key(&entry.path) {
                added_by_oid
                    .entry((entry.sha1, entry.mode & 0o170000))
                    .or_default()
                    .push(entry.path.clone());
            }
        }
        for (oid, old_paths) in &removed_by_oid {
            if old_paths.len() == 1
                && let Some(new_paths) = added_by_oid.get(oid)
                && new_paths.len() == 1
            {
                map.insert(new_paths[0].clone(), Status::Renamed);
                map.insert(old_paths[0].clone(), Status::Deleted);
            }
        }

        for entry in index.iter() {
            if entry.stage != 0 {
                map.insert(entry.path.clone(), Status::Conflicted);
                continue;
            }

            let work_path = self.work_dir().join(&entry.path);

            match fs::symlink_metadata(&work_path) {
                Ok(meta) => {
                    if meta.is_dir() {
                        map.insert(entry.path.clone(), Status::TypeChange);
                        continue;
                    }

                    let mode = if meta.is_symlink() {
                        0o120000
                    } else {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            if meta.permissions().mode() & 0o111 != 0 {
                                0o100755
                            } else {
                                0o100644
                            }
                        }
                        #[cfg(not(unix))]
                        {
                            0o100644
                        }
                    };
                    if mode != entry.mode {
                        map.insert(entry.path.clone(), Status::TypeChange);
                        continue;
                    }

                    let staged_status = if map.get(&entry.path) == Some(&Status::Renamed) {
                        Some(Status::Renamed)
                    } else {
                        match head_entries.get(&entry.path) {
                            Some(head_entry)
                                if head_entry.oid != entry.sha1
                                    || head_entry.mode != entry.mode =>
                            {
                                Some(Status::Modified)
                            }
                            Some(_) => None,
                            None => Some(Status::Added),
                        }
                    };

                    if !metadata_matches_index(&meta, entry) {
                        map.insert(entry.path.clone(), Status::Modified);
                    } else if let Some(status) = staged_status {
                        map.insert(entry.path.clone(), status);
                    } else {
                        map.insert(entry.path.clone(), Status::Clean);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    map.insert(entry.path.clone(), Status::Deleted);
                }
                Err(error) => return Err(crate::repo::Error::Io(error)),
            }
        }

        self.scan_untracked(self.work_dir(), &index, &root_ignore, &mut map)?;

        Ok(map)
    }

    fn scan_untracked(
        &self,
        dir: &Path,
        index: &Index,
        ignore: &IgnoreRules,
        map: &mut FxHashMap<PathBuf, Status>,
    ) -> Result<(), crate::repo::Error> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name();

            if name == ".git" {
                continue;
            }

            let relative = path.strip_prefix(self.work_dir()).unwrap_or(&path);
            let file_type = entry.file_type()?;
            let is_dir = file_type.is_dir();

            if ignore.is_ignored(relative, is_dir) {
                map.insert(relative.to_path_buf(), Status::Ignored);
                continue;
            }

            if is_dir {
                if path.join(".git").try_exists()? {
                    if index.get(relative).is_none() && !map.contains_key(relative) {
                        map.insert(relative.to_path_buf(), Status::Untracked);
                    }
                    continue;
                }
                let nested_ignore = if path.join(".gitignore").is_file() {
                    let mut rules = ignore.clone();
                    let local_rules = self.load_ignore_rules(&path);
                    rules.extend(local_rules);
                    rules
                } else {
                    ignore.clone()
                };
                self.scan_untracked(&path, index, &nested_ignore, map)?;
            } else if index.get(relative).is_none() && !map.contains_key(relative) {
                map.insert(relative.to_path_buf(), Status::Untracked);
            }
        }
        Ok(())
    }

    pub fn file_status(&self, path: &Path) -> Result<Status, crate::repo::Error> {
        let relative = path.strip_prefix(self.work_dir()).unwrap_or(path);
        let statuses = self.status()?;
        Ok(statuses.get(relative).copied().unwrap_or(Status::Untracked))
    }
}

fn metadata_matches_index(meta: &fs::Metadata, entry: &crate::index::IndexEntry) -> bool {
    meta.len() as u32 == entry.size
        && meta.mtime() == entry.mtime_secs
        && meta.mtime_nsec() as u32 == entry.mtime_nanos
        && meta.ctime() == entry.ctime_secs
        && meta.ctime_nsec() as u32 == entry.ctime_nanos
        && meta.dev() as u32 == entry.dev
        && meta.ino() as u32 == entry.ino
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::Repository;
    use std::fs;

    fn setup_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".git/refs/heads")).unwrap();
        tmp
    }

    fn write_test_index(git_dir: &Path, entries: &[(&str, u32, u32, i64)]) {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"DIRC");
        buf.extend_from_slice(&3u32.to_be_bytes());
        buf.extend_from_slice(&(entries.len() as u32).to_be_bytes());

        for (path, mode, size, mtime) in entries {
            let metadata = fs::symlink_metadata(git_dir.parent().unwrap().join(path)).ok();
            buf.extend_from_slice(
                &(metadata.as_ref().map(MetadataExt::ctime).unwrap_or(0) as u32).to_be_bytes(),
            );
            buf.extend_from_slice(
                &(metadata.as_ref().map(MetadataExt::ctime_nsec).unwrap_or(0) as u32).to_be_bytes(),
            );
            let indexed_mtime = if *mtime == 0 {
                0
            } else {
                metadata.as_ref().map(MetadataExt::mtime).unwrap_or(*mtime)
            };
            buf.extend_from_slice(&(indexed_mtime as u32).to_be_bytes());
            buf.extend_from_slice(
                &(metadata.as_ref().map(MetadataExt::mtime_nsec).unwrap_or(0) as u32).to_be_bytes(),
            );
            buf.extend_from_slice(
                &(metadata.as_ref().map(MetadataExt::dev).unwrap_or(0) as u32).to_be_bytes(),
            );
            buf.extend_from_slice(
                &(metadata.as_ref().map(MetadataExt::ino).unwrap_or(0) as u32).to_be_bytes(),
            );
            buf.extend_from_slice(&mode.to_be_bytes());
            buf.extend_from_slice(&0u32.to_be_bytes());
            buf.extend_from_slice(&0u32.to_be_bytes());
            buf.extend_from_slice(&size.to_be_bytes());
            buf.extend_from_slice(&[0u8; 20]);
            buf.extend_from_slice(&0u16.to_be_bytes());
            buf.extend_from_slice(path.as_bytes());
            buf.push(0);
            let entry_len = 62 + path.len() + 1;
            let padded = (entry_len + 7) & !7;
            buf.extend(std::iter::repeat(0u8).take(padded - entry_len));
        }

        fs::write(git_dir.join("index"), &buf).unwrap();
    }

    #[test]
    fn clean_file() {
        let tmp = setup_repo();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        fs::write(tmp.path().join("hello.txt"), "hello").unwrap();
        write_test_index(
            tmp.path().join(".git").as_path(),
            &[("hello.txt", 0o100644, 5, now)],
        );
        let repo = Repository::discover(tmp.path()).unwrap();
        let status = repo.status().unwrap();
        assert_eq!(status.get(Path::new("hello.txt")), Some(&Status::Added));
    }

    #[test]
    fn modified_file() {
        let tmp = setup_repo();
        fs::write(tmp.path().join("hello.txt"), "modified content").unwrap();
        write_test_index(
            tmp.path().join(".git").as_path(),
            &[("hello.txt", 0o100644, 5, 0)],
        );
        let repo = Repository::discover(tmp.path()).unwrap();
        let status = repo.status().unwrap();
        assert_eq!(status.get(Path::new("hello.txt")), Some(&Status::Modified));
    }

    #[test]
    fn deleted_file() {
        let tmp = setup_repo();
        write_test_index(
            tmp.path().join(".git").as_path(),
            &[("deleted.txt", 0o100644, 100, 0)],
        );
        let repo = Repository::discover(tmp.path()).unwrap();
        let status = repo.status().unwrap();
        assert_eq!(status.get(Path::new("deleted.txt")), Some(&Status::Deleted));
    }

    #[test]
    fn added_file() {
        let tmp = setup_repo();
        fs::write(tmp.path().join("new.txt"), "new content").unwrap();
        write_test_index(tmp.path().join(".git").as_path(), &[]);
        let repo = Repository::discover(tmp.path()).unwrap();
        let status = repo.status().unwrap();
        assert_eq!(status.get(Path::new("new.txt")), Some(&Status::Untracked));
    }

    #[test]
    fn ignored_file() {
        let tmp = setup_repo();
        fs::write(tmp.path().join("debug.log"), "log").unwrap();
        fs::write(tmp.path().join(".gitignore"), "*.log\n").unwrap();
        write_test_index(tmp.path().join(".git").as_path(), &[]);
        let repo = Repository::discover(tmp.path()).unwrap();
        let status = repo.status().unwrap();
        assert_eq!(status.get(Path::new("debug.log")), Some(&Status::Ignored));
    }

    #[test]
    fn type_change() {
        let tmp = setup_repo();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("target", tmp.path().join("link")).unwrap();
        }
        write_test_index(
            tmp.path().join(".git").as_path(),
            &[("link", 0o100644, 0, 0)],
        );
        let repo = Repository::discover(tmp.path()).unwrap();
        let status = repo.status().unwrap();
        assert_eq!(status.get(Path::new("link")), Some(&Status::TypeChange));
    }

    #[test]
    fn file_status_method() {
        let tmp = setup_repo();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        fs::write(tmp.path().join("test.txt"), "hello").unwrap();
        write_test_index(
            tmp.path().join(".git").as_path(),
            &[("test.txt", 0o100644, 5, now)],
        );
        let repo = Repository::discover(tmp.path()).unwrap();
        assert_eq!(
            repo.file_status(tmp.path().join("test.txt").as_path())
                .unwrap(),
            Status::Added
        );
    }
}
