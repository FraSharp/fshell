// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_hash::FxHashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ignore::IgnoreRules;
use crate::index::Index;
use crate::repo::Repository;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Clean,
    Modified,
    Added,
    Deleted,
    TypeChange,
    Ignored,
    Conflicted,
}

impl Repository {
    pub fn status(&self) -> Result<FxHashMap<PathBuf, Status>, crate::repo::Error> {
        let index = Index::parse(self.git_dir())
            .map_err(|e| crate::repo::Error::InvalidIndex(e.to_string()))?;

        let root_ignore = self.collect_ignore_rules(self.work_dir());
        let mut map = FxHashMap::default();

        for entry in index.iter() {
            if entry.stage != 0 {
                map.insert(entry.path.clone(), Status::Conflicted);
                continue;
            }

            let work_path = self.work_dir().join(&entry.path);

            if root_ignore.is_ignored(&entry.path, false) {
                map.insert(entry.path.clone(), Status::Ignored);
                continue;
            }

            match fs::symlink_metadata(&work_path) {
                Ok(meta) => {
                    if meta.is_dir() {
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

                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    let size = meta.len() as u32;

                    if mtime != entry.mtime_secs || size != entry.size {
                        map.insert(entry.path.clone(), Status::Modified);
                    } else {
                        map.insert(entry.path.clone(), Status::Clean);
                    }
                }
                Err(_) => {
                    map.insert(entry.path.clone(), Status::Deleted);
                }
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
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                let name_str = name.to_string_lossy();

                if name_str == ".git" {
                    continue;
                }

                let relative = path.strip_prefix(self.work_dir()).unwrap_or(&path);

                let is_dir = path.is_dir();

                if ignore.is_ignored(relative, is_dir) {
                    map.insert(relative.to_path_buf(), Status::Ignored);
                    continue;
                }

                if is_dir {
                    let nested_ignore = self.collect_ignore_rules(&path);
                    self.scan_untracked(&path, index, &nested_ignore, map)?;
                } else if index.get(relative).is_none() && !map.contains_key(relative) {
                    map.insert(relative.to_path_buf(), Status::Added);
                }
            }
        }
        Ok(())
    }

    pub fn file_status(&self, path: &Path) -> Result<Status, crate::repo::Error> {
        let index = Index::parse(self.git_dir())
            .map_err(|e| crate::repo::Error::InvalidIndex(e.to_string()))?;

        let relative = path.strip_prefix(self.work_dir()).unwrap_or(path);

        let ignore = self.collect_ignore_rules(path);
        if ignore.is_ignored(relative, path.is_dir()) {
            return Ok(Status::Ignored);
        }

        if let Some(entry) = index.get(relative) {
            if entry.stage != 0 {
                return Ok(Status::Conflicted);
            }

            match fs::symlink_metadata(path) {
                Ok(meta) => {
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    let size = meta.len() as u32;

                    if mtime != entry.mtime_secs || size != entry.size {
                        Ok(Status::Modified)
                    } else {
                        Ok(Status::Clean)
                    }
                }
                Err(_) => Ok(Status::Deleted),
            }
        } else {
            Ok(Status::Added)
        }
    }
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
            buf.extend_from_slice(&0u32.to_be_bytes()); // ctime_secs
            buf.extend_from_slice(&0u32.to_be_bytes()); // ctime_nanos
            buf.extend_from_slice(&(*mtime as u32).to_be_bytes()); // mtime_secs
            buf.extend_from_slice(&0u32.to_be_bytes()); // mtime_nanos
            buf.extend_from_slice(&0u32.to_be_bytes());
            buf.extend_from_slice(&0u32.to_be_bytes());
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
        assert_eq!(status.get(Path::new("hello.txt")), Some(&Status::Clean));
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
        assert_eq!(status.get(Path::new("new.txt")), Some(&Status::Added));
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
            Status::Clean
        );
    }
}
