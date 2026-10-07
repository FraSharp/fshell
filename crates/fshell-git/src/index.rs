// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_hash::FxHashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub path: PathBuf,
    pub sha1: [u8; 20],
    pub mode: u32,
    pub size: u32,
    pub ctime_secs: i64,
    pub ctime_nanos: u32,
    pub mtime_secs: i64,
    pub mtime_nanos: u32,
    pub dev: u32,
    pub ino: u32,
    pub flags: u16,
    pub stage: u8,
}

#[derive(Debug)]
pub struct Index {
    version: u32,
    sparse: bool,
    entries: Vec<IndexEntry>,
    path_lookup: FxHashMap<PathBuf, usize>,
}

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid signature: expected DIRC")]
    InvalidSignature,
    #[error("unsupported version: {0}")]
    UnsupportedVersion(u32),
    #[error("truncated entry at offset {0}")]
    TruncatedEntry(usize),
    #[error("index entry corrupted at offset {0}")]
    CorruptedEntry(usize),
    #[error("invalid index: {0}")]
    Backend(String),
}

impl Index {
    pub fn parse(git_dir: &Path) -> Result<Self, IndexError> {
        let path = git_dir.join("index");
        let index = gix::index::File::at(
            &path,
            gix::hash::Kind::Sha1,
            false,
            gix::index::decode::Options::default(),
        )
        .map_err(|error| IndexError::Backend(error.to_string()))?;
        Self::from_state(&index)
    }

    /// Decode an index with Gitoxide's index reader, including v4 paths and extensions.
    pub fn parse_bytes(data: &[u8]) -> Result<Self, IndexError> {
        let (state, _) = gix::index::State::from_bytes(
            data,
            filetime::FileTime::from_unix_time(0, 0),
            gix::hash::Kind::Sha1,
            gix::index::decode::Options {
                thread_limit: Some(1),
                ..Default::default()
            },
        )
        .map_err(|error| IndexError::Backend(error.to_string()))?;
        let checksum_start = data
            .len()
            .checked_sub(gix::hash::Kind::Sha1.len_in_bytes())
            .ok_or(IndexError::TruncatedEntry(data.len()))?;
        let stored_checksum = &data[checksum_start..];
        let mut hasher = gix::hash::hasher(gix::hash::Kind::Sha1);
        hasher.update(&data[..checksum_start]);
        let actual = hasher
            .try_finalize()
            .map_err(|error| IndexError::Backend(error.to_string()))?;
        if actual.as_bytes() != stored_checksum {
            return Err(IndexError::Backend("index checksum mismatch".into()));
        }
        Self::from_state(&state)
    }

    fn from_state(state: &gix::index::State) -> Result<Self, IndexError> {
        if state.object_hash() != gix::hash::Kind::Sha1 {
            return Err(IndexError::Backend(format!(
                "unsupported object hash: {}",
                state.object_hash()
            )));
        }

        let mut entries = Vec::with_capacity(state.entries().len());
        let mut path_lookup = FxHashMap::default();
        for entry in state.entries() {
            let path = PathBuf::from(OsStr::from_bytes(entry.path(state).as_ref()));
            let oid =
                entry.id.as_bytes().try_into().map_err(|_| {
                    IndexError::Backend("expected a 20-byte SHA-1 object id".into())
                })?;
            let stat = entry.stat;
            let index_entry = IndexEntry {
                path: path.clone(),
                sha1: oid,
                mode: entry.mode.bits(),
                size: stat.size,
                ctime_secs: i64::from(stat.ctime.secs),
                ctime_nanos: stat.ctime.nsecs,
                mtime_secs: i64::from(stat.mtime.secs),
                mtime_nanos: stat.mtime.nsecs,
                dev: stat.dev,
                ino: stat.ino,
                flags: entry.flags.to_storage().bits(),
                stage: entry.stage_raw() as u8,
            };
            path_lookup.insert(path, entries.len());
            entries.push(index_entry);
        }

        Ok(Index {
            version: state.version() as u32,
            sparse: state.is_sparse(),
            entries,
            path_lookup,
        })
    }

    pub fn get(&self, path: &Path) -> Option<&IndexEntry> {
        self.path_lookup.get(path).and_then(|&index| {
            let entry = &self.entries[index];
            (entry.stage == 0).then_some(entry)
        })
    }

    pub fn get_all(&self, path: &Path) -> Vec<&IndexEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.path == path)
            .collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = &IndexEntry> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    /// Whether this is a sparse index whose directory entries summarize trees.
    pub fn is_sparse(&self) -> bool {
        self.sparse
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{git, init_repo};
    use std::fs;

    #[test]
    fn parses_git_index_and_rejects_truncation() {
        let directory = tempfile::tempdir().expect("test fixture operation should succeed");
        init_repo(directory.path());
        fs::write(directory.path().join("tracked.txt"), "content")
            .expect("test fixture operation should succeed");
        git(directory.path(), &["add", "tracked.txt"]);

        let index_path = directory.path().join(".git/index");
        let bytes = fs::read(&index_path).expect("test fixture operation should succeed");
        let index = Index::parse_bytes(&bytes).expect("test fixture operation should succeed");
        let entry = index
            .get(Path::new("tracked.txt"))
            .expect("test fixture operation should succeed");
        assert_eq!(entry.mode, 0o100644);
        assert_eq!(entry.stage, 0);
        assert_eq!(entry.size, 7);

        assert!(Index::parse_bytes(&bytes[..bytes.len() - 1]).is_err());

        let mut corrupt = bytes;
        let last_byte = corrupt
            .last_mut()
            .expect("Git index includes a trailing checksum");
        *last_byte ^= 0xff;
        assert!(Index::parse_bytes(&corrupt).is_err());
    }

    #[test]
    fn parses_index_file_and_rejects_a_bad_checksum() {
        let directory = tempfile::tempdir().expect("test fixture operation should succeed");
        init_repo(directory.path());
        fs::write(directory.path().join("tracked.txt"), "content")
            .expect("test fixture operation should succeed");
        git(directory.path(), &["add", "tracked.txt"]);

        let index_path = directory.path().join(".git/index");
        let mut bytes = fs::read(&index_path).expect("Git index should be readable");
        assert!(Index::parse(directory.path().join(".git").as_path()).is_ok());
        let last_byte = bytes
            .last_mut()
            .expect("Git index should include its trailing checksum");
        *last_byte ^= 0xff;
        fs::write(index_path, bytes).expect("corrupt Git index should be writable");

        assert!(Index::parse(directory.path().join(".git").as_path()).is_err());
    }
}
