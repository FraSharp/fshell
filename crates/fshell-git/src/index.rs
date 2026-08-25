// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub path: PathBuf,
    pub sha1: [u8; 20],
    pub mode: u32,
    pub size: u32,
    pub ctime_secs: i64,
    pub mtime_secs: i64,
    pub flags: u16,
    pub stage: u8, // 0-3, from flags bits 12-13
}

#[derive(Debug)]
pub struct Index {
    version: u32,
    entries: Vec<IndexEntry>,
    path_lookup: HashMap<PathBuf, usize>,
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
}

impl Index {
    pub fn parse(git_dir: &Path) -> Result<Self, IndexError> {
        let path = git_dir.join("index");
        let data = fs::read(&path)?;
        Self::parse_bytes(&data)
    }

    pub fn parse_bytes(data: &[u8]) -> Result<Self, IndexError> {
        if data.len() < 12 {
            return Err(IndexError::TruncatedEntry(0));
        }

        let sig = &data[0..4];
        if sig != b"DIRC" {
            return Err(IndexError::InvalidSignature);
        }
        let version = u32::from_be_bytes(
            data[4..8]
                .try_into()
                .map_err(|_| IndexError::CorruptedEntry(4))?,
        );
        let num_entries = u32::from_be_bytes(
            data[8..12]
                .try_into()
                .map_err(|_| IndexError::CorruptedEntry(8))?,
        );

        if version != 2 && version != 3 && version != 4 {
            return Err(IndexError::UnsupportedVersion(version));
        }

        let mut entries = Vec::with_capacity(num_entries as usize);
        let mut offset = 12;

        for _ in 0..num_entries {
            if offset + 62 > data.len() {
                return Err(IndexError::TruncatedEntry(offset));
            }

            let ctime_secs = u32::from_be_bytes(
                data[offset..offset + 4]
                    .try_into()
                    .map_err(|_| IndexError::CorruptedEntry(offset))?,
            ) as i64;
            let _ctime_nanos = u32::from_be_bytes(
                data[offset + 4..offset + 8]
                    .try_into()
                    .map_err(|_| IndexError::CorruptedEntry(offset + 4))?,
            );
            let mtime_secs = u32::from_be_bytes(
                data[offset + 8..offset + 12]
                    .try_into()
                    .map_err(|_| IndexError::CorruptedEntry(offset + 8))?,
            ) as i64;
            let _mtime_nanos = u32::from_be_bytes(
                data[offset + 12..offset + 16]
                    .try_into()
                    .map_err(|_| IndexError::CorruptedEntry(offset + 12))?,
            );
            let _dev = u32::from_be_bytes(
                data[offset + 16..offset + 20]
                    .try_into()
                    .map_err(|_| IndexError::CorruptedEntry(offset + 16))?,
            );
            let _ino = u32::from_be_bytes(
                data[offset + 20..offset + 24]
                    .try_into()
                    .map_err(|_| IndexError::CorruptedEntry(offset + 20))?,
            );
            let mode = u32::from_be_bytes(
                data[offset + 24..offset + 28]
                    .try_into()
                    .map_err(|_| IndexError::CorruptedEntry(offset + 24))?,
            );
            let _uid = u32::from_be_bytes(
                data[offset + 28..offset + 32]
                    .try_into()
                    .map_err(|_| IndexError::CorruptedEntry(offset + 28))?,
            );
            let _gid = u32::from_be_bytes(
                data[offset + 32..offset + 36]
                    .try_into()
                    .map_err(|_| IndexError::CorruptedEntry(offset + 32))?,
            );
            let size = u32::from_be_bytes(
                data[offset + 36..offset + 40]
                    .try_into()
                    .map_err(|_| IndexError::CorruptedEntry(offset + 36))?,
            );

            let mut sha1 = [0u8; 20];
            sha1.copy_from_slice(&data[offset + 40..offset + 60]);
            let flags = u16::from_be_bytes(
                data[offset + 60..offset + 62]
                    .try_into()
                    .map_err(|_| IndexError::CorruptedEntry(offset + 60))?,
            );

            // Stage is bits 12-13 of flags (0-3)
            let stage = ((flags >> 12) & 0x3) as u8;

            let path_start = offset + 62;
            let path_end = data[path_start..]
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(data.len() - path_start);
            let path = PathBuf::from(
                String::from_utf8_lossy(&data[path_start..path_start + path_end]).to_string(),
            );

            let entry_len = 62 + path_end + 1;
            let padded_len = (entry_len + 7) & !7;
            offset += padded_len;

            entries.push(IndexEntry {
                path,
                sha1,
                mode,
                size,
                ctime_secs,
                mtime_secs,
                flags,
                stage,
            });
        }

        let mut path_lookup = HashMap::with_capacity(num_entries as usize);
        for (i, entry) in entries.iter().enumerate() {
            path_lookup.insert(entry.path.clone(), i);
        }

        Ok(Index {
            version,
            entries,
            path_lookup,
        })
    }

    pub fn get(&self, path: &Path) -> Option<&IndexEntry> {
        self.path_lookup.get(path).and_then(|&i| {
            let entry = &self.entries[i];
            if entry.stage == 0 { Some(entry) } else { None }
        })
    }

    pub fn get_all(&self, path: &Path) -> Vec<&IndexEntry> {
        self.entries.iter().filter(|e| e.path == path).collect()
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_test_index(entries: &[(&str, [u8; 20], u32, u32)]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"DIRC");
        buf.extend_from_slice(&3u32.to_be_bytes());
        buf.extend_from_slice(&(entries.len() as u32).to_be_bytes());

        for (path, sha1, mode, size) in entries {
            buf.extend_from_slice(&0i64.to_be_bytes());
            buf.extend_from_slice(&0i64.to_be_bytes());
            buf.extend_from_slice(&0u32.to_be_bytes());
            buf.extend_from_slice(&0u32.to_be_bytes());
            buf.extend_from_slice(&mode.to_be_bytes());
            buf.extend_from_slice(&0u32.to_be_bytes());
            buf.extend_from_slice(&0u32.to_be_bytes());
            buf.extend_from_slice(&size.to_be_bytes());
            buf.extend_from_slice(sha1);
            buf.extend_from_slice(&0u16.to_be_bytes());
            buf.extend_from_slice(path.as_bytes());
            buf.push(0);
            let entry_len = 62 + path.len() + 1;
            let padded = (entry_len + 7) & !7;
            buf.extend(std::iter::repeat(0u8).take(padded - entry_len));
        }
        buf
    }

    #[test]
    fn parse_empty_index() {
        let data = build_test_index(&[]);
        let index = Index::parse_bytes(&data).unwrap();
        assert_eq!(index.len(), 0);
        assert_eq!(index.version(), 3);
    }

    #[test]
    fn parse_single_entry() {
        let sha1 = [1u8; 20];
        let data = build_test_index(&[("src/main.rs", sha1, 0o100644, 1024)]);
        let index = Index::parse_bytes(&data).unwrap();
        let entry = index.get(Path::new("src/main.rs")).unwrap();
        assert_eq!(entry.sha1, sha1);
        assert_eq!(entry.mode, 0o100644);
        assert_eq!(entry.size, 1024);
        assert_eq!(entry.stage, 0);
    }

    #[test]
    fn parse_multiple_entries() {
        let data = build_test_index(&[
            ("Cargo.toml", [2u8; 20], 0o100644, 512),
            ("src/lib.rs", [3u8; 20], 0o100644, 2048),
            ("README.md", [4u8; 20], 0o100644, 256),
        ]);
        let index = Index::parse_bytes(&data).unwrap();
        assert_eq!(index.len(), 3);
        assert!(index.get(Path::new("Cargo.toml")).is_some());
        assert!(index.get(Path::new("missing.txt")).is_none());
    }

    #[test]
    fn invalid_signature() {
        let mut data = build_test_index(&[]);
        data[0] = b'X';
        assert!(matches!(
            Index::parse_bytes(&data),
            Err(IndexError::InvalidSignature)
        ));
    }

    #[test]
    fn unsupported_version() {
        let mut data = build_test_index(&[]);
        data[7] = 5;
        assert!(matches!(
            Index::parse_bytes(&data),
            Err(IndexError::UnsupportedVersion(5))
        ));
    }

    #[test]
    fn iter_entries() {
        let data = build_test_index(&[
            ("a.txt", [1u8; 20], 0o100644, 10),
            ("b.txt", [2u8; 20], 0o100644, 20),
        ]);
        let index = Index::parse_bytes(&data).unwrap();
        assert_eq!(index.iter().count(), 2);
    }
}
