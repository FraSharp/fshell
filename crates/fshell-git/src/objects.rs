// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use fshell_hash::FxHashMap;

use crate::repo::{Error, Repository};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectType {
    Commit,
    Tree,
    Blob,
    Tag,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeEntry {
    pub oid: [u8; 20],
    pub mode: u32,
}

impl Repository {
    /// Read an object by SHA-1, including objects stored in packs and delta chains.
    pub fn read_object(&self, oid: &[u8; 20]) -> Result<ParsedObject, Error> {
        let object = self
            .inner
            .find_object(*oid)
            .map_err(|error| Error::InvalidObject(error.to_string()))?;
        let typ = match object.kind {
            gix::objs::Kind::Commit => ObjectType::Commit,
            gix::objs::Kind::Tree => ObjectType::Tree,
            gix::objs::Kind::Blob => ObjectType::Blob,
            gix::objs::Kind::Tag => ObjectType::Tag,
        };
        let mut object = object;
        let data = std::mem::take(&mut object.data);
        let size = data.len();
        Ok(ParsedObject { typ, size, data })
    }

    /// Read and decode a commit object.
    pub fn read_commit(&self, oid: &[u8; 20]) -> Result<CommitInfo, Error> {
        let commit = self
            .inner
            .find_commit(*oid)
            .map_err(|error| Error::InvalidObject(error.to_string()))?;
        let decoded = commit
            .decode()
            .map_err(|error| Error::InvalidObject(error.to_string()))?;
        let tree = parse_hex_oid(decoded.tree.as_ref())?;
        let parents = decoded
            .parents
            .iter()
            .map(|parent| parse_hex_oid(parent.as_ref()))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(CommitInfo {
            tree,
            parents,
            author: String::from_utf8_lossy(decoded.author.as_ref()).into_owned(),
            message: String::from_utf8_lossy(decoded.message.as_ref())
                .trim_end()
                .to_owned(),
        })
    }

    /// Read a commit tree into a path-indexed map, preserving Unix path bytes.
    pub fn read_tree_entries(
        &self,
        root_oid: &[u8; 20],
    ) -> Result<FxHashMap<PathBuf, TreeEntry>, Error> {
        enum Work {
            Enter([u8; 20], PathBuf),
            Exit([u8; 20]),
        }

        let mut entries = FxHashMap::default();
        let mut ancestors = Vec::new();
        let mut pending = vec![Work::Enter(*root_oid, PathBuf::new())];

        while let Some(work) = pending.pop() {
            let (oid, prefix) = match work {
                Work::Exit(oid) => {
                    if ancestors.pop() != Some(oid) {
                        return Err(Error::InvalidObject("invalid tree traversal state".into()));
                    }
                    continue;
                }
                Work::Enter(oid, prefix) => (oid, prefix),
            };
            if ancestors.len() >= 1024 || ancestors.contains(&oid) {
                return Err(Error::InvalidObject(
                    "cyclic or excessively deep tree".into(),
                ));
            }
            ancestors.push(oid);
            pending.push(Work::Exit(oid));

            let tree = self
                .inner
                .find_tree(oid)
                .map_err(|error| Error::InvalidObject(error.to_string()))?;
            let decoded = tree
                .decode()
                .map_err(|error| Error::InvalidObject(error.to_string()))?;
            let mut child_trees = Vec::new();

            for entry in decoded.entries {
                let path = prefix.join(OsStr::from_bytes(entry.filename.as_ref()));
                let child_oid = oid_to_array(entry.oid.as_bytes())?;
                let mode = u32::from(entry.mode.value());

                if entry.mode.is_tree() {
                    child_trees.push((child_oid, path));
                } else if entries
                    .insert(
                        path,
                        TreeEntry {
                            oid: child_oid,
                            mode,
                        },
                    )
                    .is_some()
                {
                    return Err(Error::InvalidObject("tree contains duplicate path".into()));
                }
            }

            pending.extend(
                child_trees
                    .into_iter()
                    .rev()
                    .map(|(oid, path)| Work::Enter(oid, path)),
            );
        }

        Ok(entries)
    }
}

fn parse_hex_oid(raw: &[u8]) -> Result<[u8; 20], Error> {
    let text = std::str::from_utf8(raw)
        .map_err(|_| Error::InvalidObject("object id is not ASCII".into()))?;
    let bytes = hex::decode(text)
        .map_err(|_| Error::InvalidObject("object id is not hexadecimal".into()))?;
    oid_to_array(&bytes)
}

fn oid_to_array(raw: &[u8]) -> Result<[u8; 20], Error> {
    raw.try_into()
        .map_err(|_| Error::InvalidObject("expected a 20-byte SHA-1 object id".into()))
}
