// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::args::GitStatus;

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
#[allow(clippy::len_without_is_empty)]
pub struct Entry {
    pub start: u32,
    pub len_and_type: u32,
}

impl Entry {
    #[inline]
    pub fn new(start: usize, len: usize, is_dir: bool) -> Self {
        Entry {
            start: start as u32,
            len_and_type: (len as u32) | if is_dir { 1 << 31 } else { 0 },
        }
    }

    #[inline]
    pub fn start(self) -> usize {
        self.start as usize
    }

    #[inline]
    pub fn len(self) -> usize {
        (self.len_and_type & 0x7FFF_FFFF) as usize
    }

    #[inline]
    pub fn is_dir(self) -> bool {
        (self.len_and_type & (1 << 31)) != 0
    }
}

#[derive(Clone)]
pub struct FileInfo {
    pub entry: Entry,
    pub metadata: Option<Metadata>,
}
