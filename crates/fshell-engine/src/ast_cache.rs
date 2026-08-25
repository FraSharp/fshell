// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::Stmt;
use fshell_hash::FxHashMap;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::SystemTime;

pub struct AstCache {
    path_mtimes: FxHashMap<PathBuf, (SystemTime, [u8; 32])>,
    ast_map: FxHashMap<[u8; 32], Vec<Stmt>>,
    hash_to_path: FxHashMap<[u8; 32], PathBuf>,
    lru_keys: VecDeque<[u8; 32]>,
    max_size: usize,
}

impl AstCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            path_mtimes: FxHashMap::default(),
            ast_map: FxHashMap::default(),
            hash_to_path: FxHashMap::default(),
            lru_keys: VecDeque::new(),
            max_size,
        }
    }

    pub fn get_by_path(&mut self, path: &PathBuf, current_mtime: SystemTime) -> Option<Vec<Stmt>> {
        if let Some(&(cached_mtime, hash)) = self.path_mtimes.get(path)
            && cached_mtime == current_mtime
            && let Some(stmts) = self.ast_map.get(&hash)
        {
            // Update LRU: move hash to the back of lru_keys
            if let Some(pos) = self.lru_keys.iter().position(|&h| h == hash) {
                self.lru_keys.remove(pos);
            }
            self.lru_keys.push_back(hash);
            return Some(stmts.clone());
        }
        None
    }

    pub fn insert(&mut self, path: PathBuf, mtime: SystemTime, hash: [u8; 32], stmts: Vec<Stmt>) {
        self.path_mtimes.insert(path.clone(), (mtime, hash));
        self.ast_map.insert(hash, stmts);
        self.hash_to_path.insert(hash, path);

        // Update LRU
        if let Some(pos) = self.lru_keys.iter().position(|&h| h == hash) {
            self.lru_keys.remove(pos);
        }
        self.lru_keys.push_back(hash);

        // Eviction
        while self.ast_map.len() > self.max_size {
            if let Some(oldest_hash) = self.lru_keys.pop_front() {
                self.ast_map.remove(&oldest_hash);
                if let Some(oldest_path) = self.hash_to_path.remove(&oldest_hash) {
                    self.path_mtimes.remove(&oldest_path);
                }
            } else {
                break;
            }
        }
    }

    pub fn clear(&mut self) {
        self.path_mtimes.clear();
        self.ast_map.clear();
        self.hash_to_path.clear();
        self.lru_keys.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_ast_cache_basic() {
        let mut cache = AstCache::new(2);
        let path = PathBuf::from("test.fsh");
        let t1 = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let hash = [1u8; 32];
        let stmts = vec![];

        cache.insert(path.clone(), t1, hash, stmts.clone());

        // Cache hit
        assert!(cache.get_by_path(&path, t1).is_some());

        // Miss due to changed mtime
        let t2 = SystemTime::UNIX_EPOCH + Duration::from_secs(20);
        assert!(cache.get_by_path(&path, t2).is_none());

        // Cache eviction: max size 2
        let p2 = PathBuf::from("test2.fsh");
        let p3 = PathBuf::from("test3.fsh");
        let h2 = [2u8; 32];
        let h3 = [3u8; 32];

        cache.insert(p2.clone(), t1, h2, vec![]);
        cache.insert(p3.clone(), t1, h3, vec![]);

        // "test.fsh" (oldest) should be evicted
        assert!(cache.get_by_path(&path, t1).is_none());
        assert!(cache.get_by_path(&p2, t1).is_some());
        assert!(cache.get_by_path(&p3, t1).is_some());
    }
}
