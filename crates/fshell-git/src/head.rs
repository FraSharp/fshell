// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::repo::{Error, Repository};

#[derive(Debug, Clone)]
pub struct HeadInfo {
    pub branch: Option<String>,
    pub oid: [u8; 20],
    pub detached: bool,
}

impl Repository {
    pub fn head(&self) -> Result<HeadInfo, Error> {
        let head = self
            .inner
            .head()
            .map_err(|error| Error::InvalidRef(error.to_string()))?;
        let id = head
            .id()
            .ok_or_else(|| Error::InvalidRef("HEAD points to an unborn branch".into()))?
            .detach();
        let oid = id
            .as_bytes()
            .try_into()
            .map_err(|_| Error::UnsupportedObjectHash(id.kind().to_string()))?;
        let branch = head.referent_name().and_then(|name| {
            name.as_bstr()
                .strip_prefix(b"refs/heads/")
                .map(String::from_utf8_lossy)
                .map(|name| name.into_owned())
        });

        Ok(HeadInfo {
            branch,
            oid,
            detached: head.is_detached(),
        })
    }
}
