// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::args::GitStatus;
use std::ops::Range;

#[derive(Clone)]
pub struct Metadata {
    pub mode: u32,
    pub nlink: u64,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub mtime: i64,
    pub blocks: i64,
    pub ino: u64,
    pub symlink_target: Option<Vec<u8>>,
    pub git_status: GitStatus,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct Entry {
    start: usize,
    len: usize,
    is_dir: bool,
}

impl Entry {
    #[inline]
    pub fn new(start: usize, len: usize, is_dir: bool) -> Self {
        Entry { start, len, is_dir }
    }

    #[inline]
    pub fn start(self) -> usize {
        self.start
    }

    #[inline]
    pub fn len(self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn is_dir(self) -> bool {
        self.is_dir
    }

    #[inline]
    pub(crate) fn set_is_dir(&mut self, is_dir: bool) {
        self.is_dir = is_dir;
    }

    /// Return the entry's filename range when it is valid for an arena.
    ///
    /// `Entry` is part of the public API, so callers may construct a
    /// `FileInfo` independently of `list_dir`. Consumers must not assume that
    /// its offsets are valid merely because they are represented as `usize`.
    #[inline]
    pub fn range(self, arena_len: usize) -> Option<Range<usize>> {
        let end = self.start.checked_add(self.len)?;
        (end <= arena_len).then_some(self.start..end)
    }
}

#[derive(Clone)]
pub struct FileInfo {
    pub entry: Entry,
    pub metadata: Option<Metadata>,
}
