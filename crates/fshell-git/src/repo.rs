// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Repository {
    pub(crate) git_dir: PathBuf,
    pub(crate) work_dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a git repository: .git not found")]
    NotFound,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid index: {0}")]
    InvalidIndex(String),
    #[error("invalid ref: {0}")]
    InvalidRef(String),
    #[error("invalid object: {0}")]
    InvalidObject(String),
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    #[error("zlib error: {0}")]
    Zlib(String),
    #[error("pack object corrupted at offset {0}")]
    CorruptedPackEntry(usize),
}

impl Repository {
    pub fn discover(path: &Path) -> Result<Self, Error> {
        let mut current = path.to_path_buf();
        loop {
            let git_path = current.join(".git");
            if git_path.is_dir() {
                return Ok(Repository {
                    git_dir: git_path,
                    work_dir: current,
                });
            }
            if git_path.is_file() {
                let content = std::fs::read_to_string(&git_path)?;
                if let Some(first_line) = content.lines().next()
                    && let Some(gitdir) = first_line.strip_prefix("gitdir: ")
                {
                    let git_dir = PathBuf::from(gitdir.trim());
                    let git_dir = if git_dir.is_absolute() {
                        git_dir
                    } else {
                        current.join(git_dir)
                    };
                    return Ok(Repository {
                        git_dir,
                        work_dir: current,
                    });
                }
            }
            if !current.pop() {
                return Err(Error::NotFound);
            }
        }
    }

    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discover_in_current_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        let repo = Repository::discover(tmp.path()).unwrap();
        assert_eq!(repo.git_dir(), git_dir);
        assert_eq!(repo.work_dir(), tmp.path());
    }

    #[test]
    fn discover_in_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        let sub = tmp.path().join("a/b/c");
        fs::create_dir_all(&sub).unwrap();
        let repo = Repository::discover(&sub).unwrap();
        assert_eq!(repo.work_dir(), tmp.path());
    }

    #[test]
    fn not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let err = Repository::discover(tmp.path()).unwrap_err();
        assert!(matches!(err, Error::NotFound));
    }

    #[test]
    fn worktree_git_file() {
        let tmp = tempfile::tempdir().unwrap();
        let real_git = tmp.path().join("real-git");
        fs::create_dir_all(&real_git).unwrap();
        let worktree = tmp.path().join("worktree");
        fs::create_dir_all(&worktree).unwrap();
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}", real_git.display()),
        )
        .unwrap();
        let repo = Repository::discover(&worktree).unwrap();
        assert_eq!(repo.git_dir(), real_git);
        assert_eq!(repo.work_dir(), &worktree);
    }

    #[test]
    fn worktree_git_file_multiline() {
        let tmp = tempfile::tempdir().unwrap();
        let real_git = tmp.path().join("real-git");
        fs::create_dir_all(&real_git).unwrap();
        let worktree = tmp.path().join("worktree");
        fs::create_dir_all(&worktree).unwrap();
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\ncommondir: ../..", real_git.display()),
        )
        .unwrap();
        let repo = Repository::discover(&worktree).unwrap();
        assert_eq!(repo.git_dir(), real_git);
        assert_eq!(repo.work_dir(), &worktree);
    }

    #[test]
    fn worktree_git_file_relative_path() {
        let tmp = tempfile::tempdir().unwrap();
        let real_git = tmp.path().join(".git/worktrees/feature");
        fs::create_dir_all(&real_git).unwrap();
        let worktree = tmp.path().join("worktree");
        fs::create_dir_all(&worktree).unwrap();
        fs::write(worktree.join(".git"), "gitdir: ../.git/worktrees/feature").unwrap();
        let repo = Repository::discover(&worktree).unwrap();
        assert_eq!(
            fs::canonicalize(repo.git_dir()).unwrap(),
            fs::canonicalize(&real_git).unwrap()
        );
        assert_eq!(repo.work_dir(), &worktree);
    }
}
