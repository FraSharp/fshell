// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::repo::{Error, Repository};

#[derive(Debug, Clone)]
pub struct Branch {
    pub name: String,
    pub head: bool,
    pub oid: [u8; 20],
    pub ahead: u32,
    pub behind: u32,
}

impl Repository {
    pub fn branches(&self) -> Result<Vec<Branch>, Error> {
        let head = self.head()?;
        let refs = self.try_list_refs("refs/heads/")?;
        let mut branches: Vec<Branch> = refs
            .into_iter()
            .map(|reference| {
                let name = reference
                    .name
                    .strip_prefix("refs/heads/")
                    .unwrap_or(&reference.name)
                    .to_owned();
                Branch {
                    head: head.branch.as_deref() == Some(&name),
                    name,
                    oid: reference.oid,
                    ahead: 0,
                    behind: 0,
                }
            })
            .collect();

        for branch in &mut branches {
            if !branch.head {
                (branch.ahead, branch.behind) = self.ahead_behind_oids(&head.oid, &branch.oid)?;
            }
        }

        Ok(branches)
    }

    pub fn ahead_behind(&self) -> Result<(u32, u32), Error> {
        let head = self.head()?;
        if head.detached {
            return Ok((0, 0));
        }

        let branch_name = head
            .branch
            .as_deref()
            .ok_or_else(|| Error::InvalidRef("HEAD does not name a local branch".into()))?;

        match self.find_upstream(branch_name)? {
            Some((_, upstream_oid)) => self.ahead_behind_oids(&head.oid, &upstream_oid),
            None => Ok((0, 0)),
        }
    }

    fn ahead_behind_oids(&self, local: &[u8; 20], remote: &[u8; 20]) -> Result<(u32, u32), Error> {
        if local == remote {
            return Ok((0, 0));
        }

        let ahead = self.count_reachable_only(local, remote)?;
        let behind = self.count_reachable_only(remote, local)?;
        Ok((ahead, behind))
    }

    fn count_reachable_only(&self, include: &[u8; 20], exclude: &[u8; 20]) -> Result<u32, Error> {
        let walk = self
            .inner
            .rev_walk([*include])
            .with_hidden([*exclude])
            .all()
            .map_err(|error| Error::Backend(error.to_string()))?;
        let mut count = 0u32;
        for commit in walk {
            commit.map_err(|error| Error::Backend(error.to_string()))?;
            count = count
                .checked_add(1)
                .ok_or_else(|| Error::Backend("commit distance exceeds u32".into()))?;
        }
        Ok(count)
    }
}
