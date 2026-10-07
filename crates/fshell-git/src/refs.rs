// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::repo::{Error, Repository};
use gix::bstr::ByteSlice;

#[derive(Debug, Clone)]
pub struct RefEntry {
    pub name: String,
    pub oid: [u8; 20],
}

/// Parse a 40-character hexadecimal SHA-1 into its byte representation.
#[allow(clippy::result_unit_err)]
pub fn parse_oid_str(value: &str) -> Result<[u8; 20], ()> {
    let value = value.trim();
    if value.len() != 40 {
        return Err(());
    }

    hex::decode(value)
        .map_err(|_| ())?
        .try_into()
        .map_err(|_| ())
}

impl Repository {
    /// Resolve a reference name, following symbolic references and packed refs.
    pub fn resolve_ref(&self, name: &str) -> Result<[u8; 20], Error> {
        let mut reference = self
            .inner
            .find_reference(name)
            .map_err(|error| Error::InvalidRef(format!("{name}: {error}")))?;
        let id = reference
            .follow_to_object()
            .map_err(|error| Error::InvalidRef(format!("{name}: {error}")))?
            .detach();
        id.as_bytes()
            .try_into()
            .map_err(|_| Error::UnsupportedObjectHash(id.kind().to_string()))
    }

    /// List refs under a prefix, returning an empty list if the backend cannot enumerate them.
    ///
    /// Use [`try_list_refs`](Self::try_list_refs) when the caller needs to handle errors.
    pub fn list_refs(&self, prefix: &str) -> Vec<RefEntry> {
        self.try_list_refs(prefix).unwrap_or_default()
    }

    /// List all refs under a prefix such as `refs/heads/`, preserving backend errors.
    pub fn try_list_refs(&self, prefix: &str) -> Result<Vec<RefEntry>, Error> {
        let platform = self
            .inner
            .references()
            .map_err(|error| Error::Backend(error.to_string()))?;
        let references = platform
            .prefixed(prefix.as_bytes())
            .map_err(|error| Error::Backend(error.to_string()))?;

        references
            .map(|reference| {
                let mut reference = reference.map_err(|error| Error::Backend(error.to_string()))?;
                let id = reference
                    .follow_to_object()
                    .map_err(|error| Error::Backend(error.to_string()))?
                    .detach();
                let oid = id
                    .as_bytes()
                    .try_into()
                    .map_err(|_| Error::UnsupportedObjectHash(id.kind().to_string()))?;
                Ok(RefEntry {
                    name: reference.name().as_bstr().to_str_lossy().into_owned(),
                    oid,
                })
            })
            .collect()
    }
}
