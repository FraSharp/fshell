// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::fs;
use std::path::Path;

use crate::repo::Repository;

#[derive(Debug, Clone)]
pub struct IgnoreRules {
    patterns: Vec<IgnorePattern>,
}

#[derive(Debug, Clone)]
struct IgnorePattern {
    pattern: String,
    negated: bool,
    dir_only: bool,
    anchored: bool,
}

impl IgnoreRules {
    pub fn empty() -> Self {
        IgnoreRules {
            patterns: Vec::new(),
        }
    }

    pub fn parse(content: &str) -> Self {
        let patterns = content
            .lines()
            .filter_map(|line| {
                let line = line.trim_end();
                if line.is_empty() || line.starts_with('#') {
                    return None;
                }

                let negated = line.starts_with('!');
                let line = if negated { &line[1..] } else { line };

                let anchored = line.contains('/') || line.contains('*');

                let line = line.strip_prefix('/').unwrap_or(line);

                let (line, dir_only) = if let Some(l) = line.strip_suffix('/') {
                    (l, true)
                } else {
                    (line, false)
                };

                Some(IgnorePattern {
                    pattern: line.to_string(),
                    negated,
                    dir_only,
                    anchored,
                })
            })
            .collect();

        IgnoreRules { patterns }
    }

    pub fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        let path_str = path.to_string_lossy();
        let mut ignored = false;

        for pattern in &self.patterns {
            if pattern.dir_only && !is_dir {
                continue;
            }

            let matched = if pattern.anchored {
                // For anchored patterns without globs, match as prefix
                // (e.g., "target" matches "target/debug/foo.o")
                if !pattern.pattern.contains('*') && !pattern.pattern.contains('?') {
                    path_str == pattern.pattern
                        || path_str.starts_with(&format!("{}/", pattern.pattern))
                } else {
                    glob_match(&pattern.pattern, &path_str)
                }
            } else {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                glob_match(&pattern.pattern, &name)
            };

            if matched {
                ignored = !pattern.negated;
            }
        }

        ignored
    }
}

fn glob_match(pattern: &str, text: &str) -> bool {
    glob_match_inner(pattern.as_bytes(), text.as_bytes())
}

fn glob_match_inner(pattern: &[u8], text: &[u8]) -> bool {
    let mut pi = 0;
    let mut ti = 0;
    let mut star_pi = None;
    let mut star_ti = 0;

    while ti < text.len() {
        if pi < pattern.len() && pattern[pi] == b'[' {
            let class_end = pattern[pi..]
                .iter()
                .position(|&b| b == b']')
                .map(|p| pi + p);
            if let Some(end) = class_end {
                let class = &pattern[pi + 1..end];
                let negated = class.starts_with(b"^") || class.starts_with(b"!");
                let class_content = if negated { &class[1..] } else { class };
                let matched = char_class_match(class_content, text[ti]);
                if matched != negated {
                    pi = end + 1;
                    ti += 1;
                    continue;
                }
            }
        } else if pi < pattern.len() && (pattern[pi] == text[ti] || pattern[pi] == b'?') {
            pi += 1;
            ti += 1;
            continue;
        } else if pi < pattern.len() && pattern[pi] == b'*' {
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
            continue;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
            continue;
        }

        return false;
    }

    while pi < pattern.len() && pattern[pi] == b'*' {
        pi += 1;
    }

    pi == pattern.len()
}

fn char_class_match(class: &[u8], c: u8) -> bool {
    let mut i = 0;
    while i < class.len() {
        if i + 2 < class.len() && class[i + 1] == b'-' {
            if c >= class[i] && c <= class[i + 2] {
                return true;
            }
            i += 3;
        } else {
            if class[i] == c {
                return true;
            }
            i += 1;
        }
    }
    false
}

impl Repository {
    pub fn load_ignore_rules(&self, dir: &Path) -> IgnoreRules {
        let gitignore = dir.join(".gitignore");
        if let Ok(content) = fs::read_to_string(&gitignore) {
            IgnoreRules::parse(&content)
        } else {
            IgnoreRules::empty()
        }
    }

    pub fn collect_ignore_rules(&self, path: &Path) -> IgnoreRules {
        let mut all_rules = IgnoreRules::empty();

        let root_ignore = self.load_ignore_rules(self.work_dir());
        all_rules.patterns.extend(root_ignore.patterns);

        if let Ok(rel) = path.strip_prefix(self.work_dir()) {
            let mut current = self.work_dir().to_path_buf();
            for component in rel.components() {
                current = current.join(component);
                let nested_ignore = self.load_ignore_rules(&current);
                all_rules.patterns.extend(nested_ignore.patterns);
            }
        }

        all_rules
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_patterns() {
        let rules = IgnoreRules::parse("*.log\nbuild/\n");
        assert!(rules.is_ignored(Path::new("debug.log"), false));
        assert!(rules.is_ignored(Path::new("build"), true));
        assert!(!rules.is_ignored(Path::new("main.rs"), false));
    }

    #[test]
    fn negation() {
        let rules = IgnoreRules::parse("*.log\n!important.log\n");
        assert!(rules.is_ignored(Path::new("debug.log"), false));
        assert!(!rules.is_ignored(Path::new("important.log"), false));
    }

    #[test]
    fn anchored_pattern() {
        let rules = IgnoreRules::parse("/build\n");
        assert!(rules.is_ignored(Path::new("build"), false));
        assert!(!rules.is_ignored(Path::new("sub/build"), false));
    }

    #[test]
    fn unanchored_pattern() {
        let rules = IgnoreRules::parse("build\n");
        assert!(rules.is_ignored(Path::new("build"), false));
        assert!(rules.is_ignored(Path::new("sub/build"), false));
    }

    #[test]
    fn dir_only_pattern() {
        let rules = IgnoreRules::parse("build/\n");
        assert!(rules.is_ignored(Path::new("build"), true));
        assert!(!rules.is_ignored(Path::new("build"), false));
    }

    #[test]
    fn glob_wildcard() {
        let rules = IgnoreRules::parse("*.o\n");
        assert!(rules.is_ignored(Path::new("main.o"), false));
        assert!(!rules.is_ignored(Path::new("main.rs"), false));
    }

    #[test]
    fn glob_question_mark() {
        let rules = IgnoreRules::parse("?.txt\n");
        assert!(rules.is_ignored(Path::new("a.txt"), false));
        assert!(!rules.is_ignored(Path::new("ab.txt"), false));
    }

    #[test]
    fn char_class() {
        let rules = IgnoreRules::parse("[abc].txt\n");
        assert!(rules.is_ignored(Path::new("a.txt"), false));
        assert!(rules.is_ignored(Path::new("b.txt"), false));
        assert!(!rules.is_ignored(Path::new("d.txt"), false));
    }

    #[test]
    fn char_class_range() {
        let rules = IgnoreRules::parse("[a-z].txt\n");
        assert!(rules.is_ignored(Path::new("a.txt"), false));
        assert!(rules.is_ignored(Path::new("z.txt"), false));
        assert!(!rules.is_ignored(Path::new("0.txt"), false));
    }

    #[test]
    fn comments_and_empty_lines() {
        let rules = IgnoreRules::parse("# comment\n\n*.log\n");
        assert!(rules.is_ignored(Path::new("debug.log"), false));
    }

    #[test]
    fn nested_gitignore() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::create_dir_all(tmp.path().join("sub")).unwrap();
        fs::write(tmp.path().join(".gitignore"), "*.log\n").unwrap();
        fs::write(tmp.path().join("sub/.gitignore"), "*.tmp\n").unwrap();

        let repo = Repository::discover(tmp.path()).unwrap();
        let rules = repo.collect_ignore_rules(&tmp.path().join("sub/file.tmp"));
        assert!(rules.is_ignored(Path::new("file.tmp"), false));
    }
}
