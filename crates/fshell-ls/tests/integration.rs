// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use fshell_ls::{Config, SortMode, list_dir};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir() -> PathBuf {
    let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("fshell_ls_test_{}_{}", std::process::id(), n));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn create_file(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, "content").unwrap();
    path
}

fn create_dir(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::create_dir(&path).unwrap();
    path
}

fn extract_name<'a>(entry: &fshell_ls::FileInfo, arena: &'a [u8]) -> &'a str {
    let start = entry.entry.start();
    let end = start + entry.entry.len();
    std::str::from_utf8(&arena[start..end]).unwrap_or("?")
}

fn base_config(dir: PathBuf) -> Config {
    Config {
        path: dir,
        show_all: false,
        list_dirs: false,
        long_listing: false,
        one_per_line: false,
        human_readable: false,
        raw: false,
        show_inode: false,
        sort_mode: SortMode::Name,
        reverse_sort: false,
        use_color: false,
        tree: false,
        tree_depth: None,
        group_directories_first: false,
        show_icons: false,
        git: false,
        dereference: false,
        recursive: false,
        verbose: false,
    }
}

#[test]
fn test_list_dir_returns_entries() {
    let dir = temp_dir();
    create_file(&dir, "a.txt");
    create_file(&dir, "b.txt");
    create_dir(&dir, "sub");

    let config = base_config(dir.clone());
    let result = list_dir(&config).unwrap();
    assert_eq!(result.entries.len(), 3);

    let names: Vec<&str> = result
        .entries
        .iter()
        .map(|e| extract_name(e, &result.arena))
        .collect();
    assert!(names.contains(&"a.txt"));
    assert!(names.contains(&"b.txt"));
    assert!(names.contains(&"sub"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_list_dir_empty_directory() {
    let dir = temp_dir();

    let config = base_config(dir.clone());
    let result = list_dir(&config).unwrap();
    assert!(result.entries.is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_list_dir_nonexistent_path() {
    let config = base_config(PathBuf::from("/nonexistent_path_fshell_test_xyz"));
    let result = list_dir(&config);
    assert!(result.is_err());
}

#[test]
fn test_list_dir_sort_by_name() {
    let dir = temp_dir();
    create_file(&dir, "z.txt");
    create_file(&dir, "a.txt");
    create_file(&dir, "m.txt");

    let mut config = base_config(dir.clone());
    config.sort_mode = SortMode::Name;
    let result = list_dir(&config).unwrap();
    let names: Vec<&str> = result
        .entries
        .iter()
        .map(|e| extract_name(e, &result.arena))
        .collect();
    assert_eq!(names, vec!["a.txt", "m.txt", "z.txt"]);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_list_dir_sort_by_size() {
    let dir = temp_dir();
    fs::write(create_file(&dir, "small.txt"), "a").unwrap();
    fs::write(create_file(&dir, "large.txt"), "very large content here").unwrap();
    fs::write(create_file(&dir, "medium.txt"), "medium").unwrap();

    let mut config = base_config(dir.clone());
    config.sort_mode = SortMode::Size;
    let result = list_dir(&config).unwrap();
    let sizes: Vec<u64> = result
        .entries
        .iter()
        .map(|e| e.metadata.as_ref().unwrap().size)
        .collect();
    for i in 1..sizes.len() {
        assert!(
            sizes[i - 1] >= sizes[i],
            "entries not sorted by size descending"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_entry_found_by_name() {
    let dir = temp_dir();
    create_file(&dir, "test.txt");

    let config = base_config(dir.clone());
    let result = list_dir(&config).unwrap();
    let entry = result
        .entries
        .iter()
        .find(|e| extract_name(e, &result.arena) == "test.txt")
        .unwrap();
    assert!(!entry.entry.is_dir());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_hidden_files_excluded_by_default() {
    let dir = temp_dir();
    create_file(&dir, "visible.txt");
    create_file(&dir, ".hidden.txt");

    let config = base_config(dir.clone());
    let result = list_dir(&config).unwrap();
    let names: Vec<&str> = result
        .entries
        .iter()
        .map(|e| extract_name(e, &result.arena))
        .collect();
    assert!(names.contains(&"visible.txt"));
    assert!(!names.contains(&".hidden.txt"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_show_all_includes_hidden() {
    let dir = temp_dir();
    create_file(&dir, "visible.txt");
    create_file(&dir, ".hidden.txt");

    let mut config = base_config(dir.clone());
    config.show_all = true;
    let result = list_dir(&config).unwrap();
    let names: Vec<&str> = result
        .entries
        .iter()
        .map(|e| extract_name(e, &result.arena))
        .collect();
    assert!(names.contains(&"visible.txt"));
    assert!(names.contains(&".hidden.txt"));

    let _ = fs::remove_dir_all(&dir);
}
