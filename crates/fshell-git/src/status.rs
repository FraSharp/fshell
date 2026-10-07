// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_hash::FxHashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::repo::{Error, Repository};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Clean,
    Modified,
    Added,
    Renamed,
    Deleted,
    TypeChange,
    Ignored,
    Conflicted,
    Untracked,
}

impl Repository {
    /// Return the combined HEAD/index/worktree status for paths in this repository.
    pub fn status(&self) -> Result<FxHashMap<PathBuf, Status>, Error> {
        let index = self
            .inner
            .index_or_empty()
            .map_err(|error| Error::InvalidIndex(error.to_string()))?;
        if index.is_sparse() {
            return Err(Error::UnsupportedFeature(
                "status for sparse indexes".into(),
            ));
        }
        let mut statuses = FxHashMap::default();

        for entry in index.entries() {
            let path = PathBuf::from(OsStr::from_bytes(entry.path(&index).as_ref()));
            set_status(
                &mut statuses,
                path,
                if entry.stage_raw() == 0 {
                    Status::Clean
                } else {
                    Status::Conflicted
                },
            );
        }

        let platform = self
            .inner
            .status(gix::progress::Discard)
            .map_err(|error| Error::Backend(error.to_string()))?
            .index(index.into())
            .untracked_files(gix::status::UntrackedFiles::Files)
            .dirwalk_options(|options| {
                options.emit_ignored(Some(gix::dir::walk::EmissionMode::Matching))
            });
        let changes = platform
            .into_iter(Vec::new())
            .map_err(|error| Error::Backend(error.to_string()))?;

        for change in changes {
            let change = change.map_err(|error| Error::Backend(error.to_string()))?;
            match change {
                gix::status::Item::TreeIndex(change) => {
                    map_tree_index_change(change, &mut statuses)
                }
                gix::status::Item::IndexWorktree(change) => {
                    map_index_worktree_change(change, &mut statuses)
                }
            }
        }

        Ok(statuses)
    }

    pub fn file_status(&self, path: &Path) -> Result<Status, Error> {
        let relative = path.strip_prefix(self.work_dir()).unwrap_or(path);
        let statuses = self.status()?;
        Ok(statuses.get(relative).copied().unwrap_or(Status::Untracked))
    }
}

fn map_tree_index_change(
    change: gix::diff::index::Change,
    statuses: &mut FxHashMap<PathBuf, Status>,
) {
    use gix::diff::index::Change;

    match change {
        Change::Addition { location, .. } => {
            set_status(statuses, path_from_git_bytes(&location), Status::Added);
        }
        Change::Deletion { location, .. } => {
            set_status(statuses, path_from_git_bytes(&location), Status::Deleted);
        }
        Change::Modification {
            location,
            previous_entry_mode,
            entry_mode,
            ..
        } => {
            let previous_kind = previous_entry_mode.bits() & 0o170000;
            let current_kind = entry_mode.bits() & 0o170000;
            let status = if previous_kind != current_kind {
                Status::TypeChange
            } else {
                Status::Modified
            };
            set_status(statuses, path_from_git_bytes(&location), status);
        }
        Change::Rewrite {
            source_location,
            location,
            copy,
            ..
        } => {
            let destination_status = if copy { Status::Added } else { Status::Renamed };
            set_status(statuses, path_from_git_bytes(&location), destination_status);
            if !copy {
                set_status(
                    statuses,
                    path_from_git_bytes(&source_location),
                    Status::Deleted,
                );
            }
        }
    }
}

fn map_index_worktree_change(
    change: gix::status::index_worktree::Item,
    statuses: &mut FxHashMap<PathBuf, Status>,
) {
    use gix::status::index_worktree::Item;

    match change {
        Item::Modification {
            rela_path, status, ..
        } => {
            let path = path_from_git_bytes(&rela_path);
            let status = match status {
                gix::status::plumbing::index_as_worktree::EntryStatus::Conflict { .. } => {
                    Some(Status::Conflicted)
                }
                gix::status::plumbing::index_as_worktree::EntryStatus::Change(change) => {
                    use gix::status::plumbing::index_as_worktree::Change;
                    Some(match change {
                        Change::Removed => Status::Deleted,
                        Change::Type { .. } => Status::TypeChange,
                        Change::Modification { .. } | Change::SubmoduleModification(_) => {
                            Status::Modified
                        }
                    })
                }
                gix::status::plumbing::index_as_worktree::EntryStatus::IntentToAdd => {
                    Some(Status::Added)
                }
                gix::status::plumbing::index_as_worktree::EntryStatus::NeedsUpdate(_) => None,
            };
            if let Some(status) = status {
                set_status(statuses, path, status);
            }
        }
        Item::DirectoryContents { entry, .. } => {
            let path = path_from_git_bytes(&entry.rela_path);
            let status = match entry.status {
                gix::dir::entry::Status::Ignored(_) => Some(Status::Ignored),
                gix::dir::entry::Status::Untracked => Some(Status::Untracked),
                _ => None,
            };
            if let Some(status) = status {
                set_status(statuses, path, status);
            }
        }
        Item::Rewrite {
            source,
            dirwalk_entry,
            copy,
            ..
        } => {
            let destination = path_from_git_bytes(&dirwalk_entry.rela_path);
            set_status(
                statuses,
                destination,
                if copy { Status::Added } else { Status::Renamed },
            );
            if let gix::status::index_worktree::RewriteSource::RewriteFromIndex {
                source_rela_path,
                ..
            } = source
                && !copy
            {
                set_status(
                    statuses,
                    path_from_git_bytes(&source_rela_path),
                    Status::Deleted,
                );
            }
        }
    }
}

fn path_from_git_bytes(path: &[u8]) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(path))
}

fn set_status(statuses: &mut FxHashMap<PathBuf, Status>, path: PathBuf, status: Status) {
    let Some(current) = statuses.get_mut(&path) else {
        statuses.insert(path, status);
        return;
    };

    if status_precedence(status) > status_precedence(*current) {
        *current = status;
    }
}

const fn status_precedence(status: Status) -> u8 {
    match status {
        Status::Clean => 0,
        Status::Ignored => 1,
        Status::Untracked => 2,
        Status::Added => 3,
        Status::Renamed => 4,
        Status::Deleted => 5,
        Status::Modified => 6,
        Status::TypeChange => 7,
        Status::Conflicted => 8,
    }
}
