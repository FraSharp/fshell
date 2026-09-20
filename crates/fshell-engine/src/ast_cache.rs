// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::Stmt;
use fshell_hash::FxHashMap;
use std::collections::VecDeque;
use std::path::PathBuf;

struct CacheEntry {
    content_hash: [u8; 32],
    stmts: Vec<Stmt>,
}

pub struct AstCache {
    entries: FxHashMap<PathBuf, CacheEntry>,
    lru_keys: VecDeque<PathBuf>,
    max_size: usize,
}

impl AstCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            entries: FxHashMap::default(),
            lru_keys: VecDeque::new(),
            max_size,
        }
    }

    pub fn get_by_path(&mut self, path: &PathBuf, content_hash: [u8; 32]) -> Option<Vec<Stmt>> {
        if let Some(entry) = self.entries.get(path)
            && entry.content_hash == content_hash
        {
            let stmts = entry.stmts.clone();
            if let Some(pos) = self.lru_keys.iter().position(|key| key == path) {
                self.lru_keys.remove(pos);
            }
            self.lru_keys.push_back(path.clone());
            return Some(stmts);
        }
        None
    }

    pub fn insert(&mut self, path: PathBuf, content_hash: [u8; 32], stmts: Vec<Stmt>) {
        self.entries.insert(
            path.clone(),
            CacheEntry {
                content_hash,
                stmts,
            },
        );

        // Update LRU
        if let Some(pos) = self.lru_keys.iter().position(|key| key == &path) {
            self.lru_keys.remove(pos);
        }
        self.lru_keys.push_back(path);

        // Eviction
        while self.entries.len() > self.max_size {
            if let Some(oldest_path) = self.lru_keys.pop_front() {
                self.entries.remove(&oldest_path);
            } else {
                break;
            }
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.lru_keys.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ast_cache_basic() {
        let mut cache = AstCache::new(2);
        let path = PathBuf::from("test.fsh");
        let hash = [1u8; 32];
        let stmts = vec![];

        cache.insert(path.clone(), hash, stmts.clone());

        // Cache hit
        assert!(cache.get_by_path(&path, hash).is_some());

        // Miss due to changed content
        assert!(cache.get_by_path(&path, [2u8; 32]).is_none());

        // Cache eviction: max size 2
        let p2 = PathBuf::from("test2.fsh");
        let p3 = PathBuf::from("test3.fsh");
        let h2 = [2u8; 32];
        let h3 = [3u8; 32];

        cache.insert(p2.clone(), h2, vec![]);
        cache.insert(p3.clone(), h3, vec![]);

        // "test.fsh" (oldest) should be evicted
        assert!(cache.get_by_path(&path, hash).is_none());
        assert!(cache.get_by_path(&p2, h2).is_some());
        assert!(cache.get_by_path(&p3, h3).is_some());
    }

    #[test]
    fn test_same_content_hash_on_distinct_paths_has_independent_entries() {
        let mut cache = AstCache::new(2);
        let first = PathBuf::from("first.fsh");
        let second = PathBuf::from("second.fsh");
        let hash = [7u8; 32];

        cache.insert(first.clone(), hash, vec![]);
        cache.insert(second.clone(), hash, vec![]);

        assert!(cache.get_by_path(&first, hash).is_some());
        assert!(cache.get_by_path(&second, hash).is_some());
    }

    #[test]
    fn test_replacing_path_does_not_leave_stale_lru_entry() {
        let mut cache = AstCache::new(1);
        let path = PathBuf::from("changed.fsh");

        cache.insert(path.clone(), [1u8; 32], vec![]);
        cache.insert(path.clone(), [2u8; 32], vec![]);
        cache.insert(PathBuf::from("other.fsh"), [3u8; 32], vec![]);

        assert!(cache.get_by_path(&path, [2u8; 32]).is_none());
        assert!(
            cache
                .get_by_path(&PathBuf::from("other.fsh"), [3u8; 32])
                .is_some()
        );
    }
}
