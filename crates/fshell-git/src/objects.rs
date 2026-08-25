// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::fs;
use std::io::Read;
use std::path::Path;

use flate2::read::ZlibDecoder;

use crate::repo::{Error, Repository};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectType {
    Commit,
    Tree,
    Blob,
    Tag,
}

impl ObjectType {
    fn from_str(s: &str) -> Result<Self, Error> {
        match s {
            "commit" => Ok(ObjectType::Commit),
            "tree" => Ok(ObjectType::Tree),
            "blob" => Ok(ObjectType::Blob),
            "tag" => Ok(ObjectType::Tag),
            _ => Err(Error::InvalidObject(format!("unknown type: {s}"))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParsedObject {
    pub typ: ObjectType,
    pub size: usize,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub tree: [u8; 20],
    pub parents: Vec<[u8; 20]>,
    pub author: String,
    pub message: String,
}

impl Repository {
    /// Read an object by SHA1. Checks loose objects first, then packfiles.
    pub fn read_object(&self, oid: &[u8; 20]) -> Result<ParsedObject, Error> {
        // Try loose object
        if let Ok(obj) = self.read_loose_object(oid) {
            return Ok(obj);
        }

        // Try packfiles
        self.read_pack_object(oid)
    }

    /// Read a commit object and parse its header.
    pub fn read_commit(&self, oid: &[u8; 20]) -> Result<CommitInfo, Error> {
        let obj = self.read_object(oid)?;
        if obj.typ != ObjectType::Commit {
            return Err(Error::InvalidObject(format!(
                "expected commit, got {:?}",
                obj.typ
            )));
        }

        let content = String::from_utf8_lossy(&obj.data);
        let mut tree = [0u8; 20];
        let mut parents = Vec::new();
        let mut author = String::new();
        let mut message = String::new();
        let mut in_body = false;

        for line in content.lines() {
            if in_body {
                message.push_str(line);
                message.push('\n');
                continue;
            }

            if line.is_empty() {
                in_body = true;
                continue;
            }

            if let Some(hash_hex) = line.strip_prefix("tree ") {
                let hash_hex = hash_hex.trim();
                if hash_hex.len() == 40 {
                    for i in 0..20 {
                        tree[i] = u8::from_str_radix(&hash_hex[i * 2..i * 2 + 2], 16)
                            .map_err(|_| Error::InvalidObject("invalid tree hash".into()))?;
                    }
                }
            } else if let Some(hash_hex) = line.strip_prefix("parent ") {
                let hash_hex = hash_hex.trim();
                if hash_hex.len() == 40 {
                    let mut parent = [0u8; 20];
                    for i in 0..20 {
                        parent[i] = u8::from_str_radix(&hash_hex[i * 2..i * 2 + 2], 16)
                            .map_err(|_| Error::InvalidObject("invalid parent hash".into()))?;
                    }
                    parents.push(parent);
                }
            } else if let Some(author_str) = line.strip_prefix("author ") {
                author = author_str.to_string();
            }
        }

        Ok(CommitInfo {
            tree,
            parents,
            author,
            message: message.trim_end().to_string(),
        })
    }

    fn read_loose_object(&self, oid: &[u8; 20]) -> Result<ParsedObject, Error> {
        let hex = hex::encode(oid);
        let dir = &hex[..2];
        let file = &hex[2..];
        let path = self.git_dir().join("objects").join(dir).join(file);

        let compressed = fs::read(&path)?;
        let mut decoder = ZlibDecoder::new(&compressed[..]);
        let mut decompressed = Vec::new();
        decoder
            .read_to_end(&mut decompressed)
            .map_err(|e| Error::Zlib(e.to_string()))?;

        // Parse header: "<type> <size>\0<data>"
        let null_pos = decompressed
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| Error::InvalidObject("missing null byte in object header".into()))?;
        let header = String::from_utf8_lossy(&decompressed[..null_pos]);
        let (type_str, size_str) = header
            .split_once(' ')
            .ok_or_else(|| Error::InvalidObject("invalid object header".into()))?;

        let typ = ObjectType::from_str(type_str)?;
        let size: usize = size_str
            .parse()
            .map_err(|_| Error::InvalidObject("invalid size in header".into()))?;

        let data = decompressed[null_pos + 1..].to_vec();
        if data.len() != size {
            return Err(Error::InvalidObject(format!(
                "size mismatch: header says {size}, got {}",
                data.len()
            )));
        }

        Ok(ParsedObject { typ, size, data })
    }

    fn read_pack_object(&self, oid: &[u8; 20]) -> Result<ParsedObject, Error> {
        let pack_dir = self.git_dir().join("objects").join("pack");
        if !pack_dir.is_dir() {
            return Err(Error::InvalidObject(format!(
                "object not found: {}",
                hex::encode(oid)
            )));
        }

        for entry in fs::read_dir(&pack_dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("idx")
                && let Ok(obj) = self.read_from_pack(&path, oid)
            {
                return Ok(obj);
            }
        }

        Err(Error::InvalidObject(format!(
            "object not found: {}",
            hex::encode(oid)
        )))
    }

    fn read_from_pack(&self, idx_path: &Path, oid: &[u8; 20]) -> Result<ParsedObject, Error> {
        let data = fs::read(idx_path)?;
        if data.len() < 1028 {
            return Err(Error::InvalidObject("pack index too small".into()));
        }

        // Version 2: magic 0xFF744F63, then version
        let magic = &data[0..4];
        if magic != b"\xfftOc" {
            return Err(Error::InvalidObject("not a v2 pack index".into()));
        }
        let version = u32::from_be_bytes(
            data[4..8]
                .try_into()
                .map_err(|_| Error::CorruptedPackEntry(4))?,
        );
        if version != 2 {
            return Err(Error::InvalidObject(format!(
                "unsupported pack index version: {version}"
            )));
        }

        // Fan-out table: 256 x u32 starting at offset 8
        let fanout = |i: usize| -> u32 {
            u32::from_be_bytes(
                data[8 + i * 4..12 + i * 4]
                    .try_into()
                    .expect("fanout table entry is always 4 bytes"),
            )
        };

        let first_byte = oid[0] as usize;
        let start = if first_byte == 0 {
            0
        } else {
            fanout(first_byte - 1) as usize
        };
        let end = fanout(first_byte) as usize;

        // Binary search over sorted hashes
        // Hashes start at offset 8 + 1024 = 1032
        let hash_start = 1032;
        let mut lo = start;
        let mut hi = end;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let hash_offset = hash_start + mid * 20;
            let mid_hash = &data[hash_offset..hash_offset + 20];
            match mid_hash.cmp(oid.as_slice()) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => {
                    // Found it — read offset
                    let num_objects = fanout(255) as usize;
                    let offset_start = hash_start + num_objects * 20 + num_objects * 4; // skip hashes + CRCs
                    let offset_val = u32::from_be_bytes(
                        data[offset_start + mid * 4..offset_start + mid * 4 + 4]
                            .try_into()
                            .map_err(|_| Error::CorruptedPackEntry(offset_start + mid * 4))?,
                    );

                    let pack_offset = if offset_val & 0x80000000 != 0 {
                        // 64-bit offset table
                        let idx64 = (offset_val & 0x7FFFFFFF) as usize;
                        let table_start = offset_start + num_objects * 4;
                        u64::from_be_bytes(
                            data[table_start + idx64 * 8..table_start + idx64 * 8 + 8]
                                .try_into()
                                .map_err(|_| Error::CorruptedPackEntry(table_start + idx64 * 8))?,
                        ) as usize
                    } else {
                        offset_val as usize
                    };

                    return self.read_pack_entry_at(idx_path, pack_offset);
                }
            }
        }

        Err(Error::InvalidObject("not found in pack".into()))
    }

    fn read_pack_entry_at(&self, idx_path: &Path, offset: usize) -> Result<ParsedObject, Error> {
        let pack_path = idx_path.with_extension("pack");
        let file = fs::File::open(&pack_path)?;
        let pack_data = unsafe { memmap2::Mmap::map(&file)? };

        if offset >= pack_data.len() {
            return Err(Error::InvalidObject("pack offset out of bounds".into()));
        }

        let mut pos = offset;
        let mut byte = pack_data[pos];
        pos += 1;
        let mut size: u64 = (byte & 0x0f) as u64;
        let mut shift = 4;
        while byte & 0x80 != 0 {
            byte = pack_data[pos];
            pos += 1;
            size |= ((byte & 0x7f) as u64) << shift;
            shift += 7;
        }

        let obj_type = match (size >> 60) as u8 {
            1 => ObjectType::Commit,
            2 => ObjectType::Tree,
            3 => ObjectType::Blob,
            4 => ObjectType::Tag,
            _ => {
                return Err(Error::InvalidObject(
                    "delta objects not yet supported".into(),
                ));
            }
        };
        let size = size & 0x0FFFFFFFFFFFFFFF;

        let mut decoder = ZlibDecoder::new(&pack_data[pos..]);
        let mut decompressed = Vec::new();
        decoder
            .read_to_end(&mut decompressed)
            .map_err(|e| Error::Zlib(e.to_string()))?;

        Ok(ParsedObject {
            typ: obj_type,
            size: size as usize,
            data: decompressed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn create_loose_object(git_dir: &Path, oid_hex: &str, obj_type: &str, content: &[u8]) {
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
    fn read_loose_blob() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();

        let oid_hex = "abc123def456789012345678901234567890abcd";
        let mut oid = [0u8; 20];
        for i in 0..20 {
            oid[i] = u8::from_str_radix(&oid_hex[i * 2..i * 2 + 2], 16).unwrap();
        }

        create_loose_object(&git_dir, oid_hex, "blob", b"hello world");

        let repo = Repository {
            git_dir,
            work_dir: tmp.path().to_path_buf(),
        };
        let obj = repo.read_object(&oid).unwrap();
        assert_eq!(obj.typ, ObjectType::Blob);
        assert_eq!(obj.data, b"hello world");
    }

    #[test]
    fn read_loose_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();

        let oid_hex = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        let mut oid = [0u8; 20];
        for i in 0..20 {
            oid[i] = u8::from_str_radix(&oid_hex[i * 2..i * 2 + 2], 16).unwrap();
        }

        let commit_content = b"tree 93bf6ec9945e4c490227048b31659adc2f953c16\nparent fe319d5fe11b9ce068f5095782c9b5c3a69caeb3\nauthor Test <test@test.com> 1636585191 -0800\n\nInitial commit\n";
        create_loose_object(&git_dir, oid_hex, "commit", commit_content);

        let repo = Repository {
            git_dir,
            work_dir: tmp.path().to_path_buf(),
        };
        let commit = repo.read_commit(&oid).unwrap();
        assert_eq!(commit.parents.len(), 1);
        assert_eq!(
            hex::encode(commit.parents[0]),
            "fe319d5fe11b9ce068f5095782c9b5c3a69caeb3"
        );
        assert_eq!(commit.message, "Initial commit");
    }

    #[test]
    fn object_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();

        let oid = [0u8; 20];
        let repo = Repository {
            git_dir,
            work_dir: tmp.path().to_path_buf(),
        };
        assert!(repo.read_object(&oid).is_err());
    }
}
