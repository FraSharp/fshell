// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Clone)]
pub struct Repository {
    pub(crate) inner: Rc<gix::Repository>,
    pub(crate) git_dir: PathBuf,
    pub(crate) work_dir: PathBuf,
}

impl std::fmt::Debug for Repository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Repository")
            .field("git_dir", &self.git_dir)
            .field("work_dir", &self.work_dir)
            .finish_non_exhaustive()
    }
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
    #[error("git backend error: {0}")]
    Backend(String),
    #[error("unsupported Git feature: {0}")]
    UnsupportedFeature(String),
    #[error("unsupported git object hash: {0}")]
    UnsupportedObjectHash(String),
}

impl Repository {
    pub fn discover(path: &Path) -> Result<Self, Error> {
        let inner = gix::discover(path).map_err(|error| {
            if !has_worktree_git_marker(path) {
                Error::NotFound
            } else {
                Error::Backend(error.to_string())
            }
        })?;
        if inner.object_hash() != gix::hash::Kind::Sha1 {
            return Err(Error::UnsupportedObjectHash(
                inner.object_hash().to_string(),
            ));
        }
        let work_dir = inner
            .workdir()
            .ok_or_else(|| Error::Backend("bare repositories are unsupported".into()))?
            .to_path_buf();
        let git_dir = inner.git_dir().to_path_buf();

        Ok(Repository {
            inner: Rc::new(inner),
            git_dir,
            work_dir,
        })
    }

    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }
}

fn has_worktree_git_marker(path: &Path) -> bool {
    path.ancestors()
        .any(|ancestor| ancestor.join(".git").exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{commit_all, git, init_repo};
    use std::fs;

    #[test]
    fn discover_in_current_dir() {
        let tmp = tempfile::tempdir().expect("test fixture operation should succeed");
        init_repo(tmp.path());
        let git_dir = tmp.path().join(".git");
        let repo = Repository::discover(tmp.path()).expect("test fixture operation should succeed");
        assert_eq!(repo.git_dir(), git_dir);
        assert_eq!(repo.work_dir(), tmp.path());
    }

    #[test]
    fn discover_in_subdir() {
        let tmp = tempfile::tempdir().expect("test fixture operation should succeed");
        init_repo(tmp.path());
        let sub = tmp.path().join("a/b/c");
        fs::create_dir_all(&sub).expect("test fixture operation should succeed");
        let repo = Repository::discover(&sub).expect("test fixture operation should succeed");
        assert_eq!(repo.work_dir(), tmp.path());
    }

    #[test]
    fn not_found() {
        let tmp = tempfile::tempdir().expect("test fixture operation should succeed");
        let err = Repository::discover(tmp.path()).expect_err("expected an error");
        assert!(matches!(err, Error::NotFound));
    }

    #[test]
    fn worktree_git_file() {
        let tmp = tempfile::tempdir().expect("test fixture operation should succeed");
        init_repo(tmp.path());
        fs::write(tmp.path().join("tracked"), "data")
            .expect("test fixture operation should succeed");
        commit_all(tmp.path(), "initial");
        let worktree = tmp.path().join("feature");
        git(
            tmp.path(),
            &[
                "worktree",
                "add",
                "-b",
                "feature",
                worktree
                    .to_str()
                    .expect("test fixture operation should succeed"),
            ],
        );
        let repo = Repository::discover(&worktree).expect("test fixture operation should succeed");
        assert_eq!(
            repo.git_dir(),
            fs::canonicalize(tmp.path().join(".git/worktrees/feature"))
                .expect("test fixture operation should succeed")
        );
        assert_eq!(repo.work_dir(), &worktree);
    }
}
