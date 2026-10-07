// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::repo::Repository;
use gix::bstr::ByteSlice;

#[derive(Debug, Clone, Default)]
pub struct IgnoreRules {
    search: gix::ignore::Search,
}

impl IgnoreRules {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Parse patterns using Git's ignore parser and matcher.
    pub fn parse(content: &str) -> Self {
        let patterns = content.lines().map(OsString::from);
        Self {
            search: gix::ignore::Search::from_overrides(
                patterns,
                gix::ignore::search::Ignore::default(),
            ),
        }
    }

    pub fn extend(&mut self, other: IgnoreRules) {
        self.search.patterns.extend(other.search.patterns);
    }

    pub fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        self.search
            .pattern_matching_relative_path(
                path.as_os_str().as_bytes().as_bstr(),
                Some(is_dir),
                gix::ignore::glob::pattern::Case::Sensitive,
            )
            .is_some_and(|matched| !matched.pattern.is_negative())
    }
}

impl Repository {
    pub fn load_ignore_rules(&self, directory: &Path) -> IgnoreRules {
        let gitignore = directory.join(".gitignore");
        let Ok(content) = fs::read(&gitignore) else {
            return IgnoreRules::empty();
        };

        let mut rules = IgnoreRules::empty();
        rules.search.add_patterns_buffer(
            &content,
            gitignore,
            Some(self.work_dir()),
            gix::ignore::search::Ignore::default(),
        );
        rules
    }

    pub fn collect_ignore_rules(&self, path: &Path) -> IgnoreRules {
        let absolute_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.work_dir().join(path)
        };
        let directory = if absolute_path.is_dir() {
            absolute_path.as_path()
        } else {
            absolute_path.parent().unwrap_or(self.work_dir())
        };

        let mut directories: Vec<PathBuf> = directory
            .ancestors()
            .take_while(|ancestor| ancestor.starts_with(self.work_dir()))
            .map(Path::to_path_buf)
            .collect();
        directories.reverse();

        let mut rules = IgnoreRules::empty();
        for directory in directories {
            rules.extend(self.load_ignore_rules(&directory));
        }
        rules
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::init_repo;

    #[test]
    fn gitignore_patterns_follow_git_matching_rules() {
        let rules = IgnoreRules::parse("*.log\n!important.log\n/cache/**/tmp\n");
        assert!(rules.is_ignored(Path::new("debug.log"), false));
        assert!(!rules.is_ignored(Path::new("important.log"), false));
        assert!(rules.is_ignored(Path::new("cache/a/b/tmp"), false));
        assert!(rules.is_ignored(Path::new("nested/debug.log"), false));
    }

    #[test]
    fn collected_rules_keep_root_and_nested_gitignore_scopes() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        init_repo(directory.path());
        fs::write(directory.path().join(".gitignore"), "*.log\n")
            .expect("root ignore file should be written");
        let nested = directory.path().join("nested");
        fs::create_dir(&nested).expect("nested directory should be created");
        fs::write(nested.join(".gitignore"), "!keep.log\n")
            .expect("nested ignore file should be written");

        let repo = Repository::discover(directory.path()).expect("repository should be discovered");
        let rules = repo.collect_ignore_rules(&nested);
        assert!(rules.is_ignored(Path::new("nested/drop.log"), false));
        assert!(!rules.is_ignored(Path::new("nested/keep.log"), false));
    }
}
