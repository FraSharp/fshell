// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use walkdir::WalkDir;

#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, MetadataExt};

#[derive(Debug, Default, Clone)]
pub struct GlobQualifiers {
    pub follow_symlinks: bool,
    pub file_types: Vec<char>, // '.', '/', '@', '*', '=', 'p', '%'
    pub size_filter: Option<(char, char, u64)>, // (op: '+', '-', '=', unit: 'b','k','m','g', value)
    pub mtime_filter: Option<(char, char, u64)>, // (op: '+', '-', '=', unit: 'd','h','m','w', value)
    pub sort_by: Option<String>,                 // "mtime", "size", "name"
    pub sort_asc: bool,
    pub range: Option<(usize, usize)>, // 1-based start and end indices
}

pub fn parse_glob_qualifiers(s: &str) -> Option<GlobQualifiers> {
    if s.is_empty() {
        return None;
    }
    let mut qual = GlobQualifiers::default();
    let mut chars = s.chars().peekable();

    // Check for follow symlinks '-'
    if chars.peek() == Some(&'-') {
        qual.follow_symlinks = true;
        chars.next();
    }

    while let Some(&c) = chars.peek() {
        match c {
            '.' | '/' | '@' | '*' | '=' | 'p' | '%' => {
                qual.file_types.push(c);
                chars.next();
            }
            'm' | 'c' | 'a' => {
                // Time qualifier: e.g. m-1, mh-3, m+5
                let _ = c;
                chars.next();
                let mut unit = 'd'; // default days
                if let Some(&u) = chars.peek() {
                    let is_unit = u == 'h' || u == 'm' || u == 'w' || u == 's';
                    if is_unit {
                        unit = u;
                        chars.next();
                    }
                }
                // Expect op: '+' or '-' or digit
                let op = match chars.peek() {
                    Some(&'+') => {
                        chars.next();
                        '+'
                    }
                    Some(&'-') => {
                        chars.next();
                        '-'
                    }
                    _ => '=',
                };
                // Read numeric value
                let mut val_str = String::new();
                while let Some(&digit) = chars.peek() {
                    if digit.is_ascii_digit() {
                        val_str.push(digit);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if let Ok(val) = val_str.parse::<u64>() {
                    qual.mtime_filter = Some((op, unit, val));
                } else {
                    return None;
                }
            }
            'L' => {
                // Size qualifier: e.g. Lk+100, Lm-5, L+10
                chars.next();
                let mut unit = 'b'; // default bytes
                if let Some(&u) = chars.peek() {
                    let is_unit = u == 'k' || u == 'm' || u == 'g';
                    if is_unit {
                        unit = u;
                        chars.next();
                    }
                }
                let op = match chars.peek() {
                    Some(&'+') => {
                        chars.next();
                        '+'
                    }
                    Some(&'-') => {
                        chars.next();
                        '-'
                    }
                    _ => '=',
                };
                let mut val_str = String::new();
                while let Some(&digit) = chars.peek() {
                    if digit.is_ascii_digit() {
                        val_str.push(digit);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if let Ok(val) = val_str.parse::<u64>() {
                    qual.size_filter = Some((op, unit, val));
                } else {
                    return None;
                }
            }
            'o' => {
                // Sort qualifier: e.g. om, os, on
                chars.next();
                let sort_char = chars.next()?;
                match sort_char {
                    'm' => {
                        qual.sort_by = Some("mtime".to_string());
                        qual.sort_asc = false;
                    }
                    'M' => {
                        qual.sort_by = Some("mtime".to_string());
                        qual.sort_asc = true;
                    }
                    's' => {
                        qual.sort_by = Some("size".to_string());
                        qual.sort_asc = false;
                    }
                    'S' => {
                        qual.sort_by = Some("size".to_string());
                        qual.sort_asc = true;
                    }
                    'n' => {
                        qual.sort_by = Some("name".to_string());
                        qual.sort_asc = true;
                    }
                    'N' => {
                        qual.sort_by = Some("name".to_string());
                        qual.sort_asc = false;
                    }
                    _ => return None,
                }
            }
            '[' => {
                // Range qualifier: [N,M]
                chars.next();
                let mut range_str = String::new();
                while let Some(&rc) = chars.peek() {
                    if rc == ']' {
                        chars.next();
                        break;
                    }
                    range_str.push(rc);
                    chars.next();
                }
                if let Some((start_str, end_str)) = range_str.split_once(',') {
                    if let (Ok(s), Ok(e)) = (
                        start_str.trim().parse::<usize>(),
                        end_str.trim().parse::<usize>(),
                    ) {
                        qual.range = Some((s, e));
                    } else {
                        return None;
                    }
                } else {
                    return None;
                }
            }
            _ => {
                return None;
            }
        }
    }

    Some(qual)
}

fn apply_qualifiers(matches: Vec<String>, qual: &GlobQualifiers) -> Vec<String> {
    let mut path_meta_list = Vec::new();

    for path_str in matches {
        let path = std::path::Path::new(&path_str);

        let metadata_res = if qual.follow_symlinks {
            std::fs::metadata(path)
        } else {
            std::fs::symlink_metadata(path)
        };

        let metadata = match metadata_res {
            Ok(m) => m,
            Err(_) => continue,
        };

        // 1. File Type Filter
        if !qual.file_types.is_empty() {
            let mut matched_type = false;

            for &t in &qual.file_types {
                match t {
                    '.' if metadata.is_file() => {
                        matched_type = true;
                    }
                    '/' if metadata.is_dir() => {
                        matched_type = true;
                    }
                    '@' if metadata.file_type().is_symlink() => {
                        matched_type = true;
                    }
                    #[cfg(unix)]
                    '*' if metadata.is_file() && (metadata.mode() & 0o111) != 0 => {
                        matched_type = true;
                    }
                    #[cfg(not(unix))]
                    '*' if metadata.is_file() => {
                        matched_type = true;
                    }
                    #[cfg(unix)]
                    '=' if metadata.file_type().is_socket() => {
                        matched_type = true;
                    }
                    #[cfg(unix)]
                    'p' if metadata.file_type().is_fifo() => {
                        matched_type = true;
                    }
                    #[cfg(unix)]
                    '%' if metadata.file_type().is_block_device()
                        || metadata.file_type().is_char_device() =>
                    {
                        matched_type = true;
                    }
                    _ => {}
                }
            }
            if !matched_type {
                continue;
            }
        }

        // 2. Size Filter
        if let Some((op, unit, val)) = &qual.size_filter {
            let bytes = metadata.len();
            let unit_multiplier = match unit {
                'k' => 1024,
                'm' => 1024 * 1024,
                'g' => 1024 * 1024 * 1024,
                _ => 1,
            };
            let filter_bytes = val * unit_multiplier;
            let matched_size = match op {
                '+' => bytes > filter_bytes,
                '-' => bytes < filter_bytes,
                _ => bytes == filter_bytes,
            };
            if !matched_size {
                continue;
            }
        }

        // 3. Time Filter
        if let Some((op, unit, val)) = &qual.mtime_filter {
            let mtime_sec = if let Ok(modified) = metadata.modified() {
                if let Ok(dur) = modified.duration_since(std::time::UNIX_EPOCH) {
                    dur.as_secs() as i64
                } else {
                    0
                }
            } else {
                0
            };
            let now_sec = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let age_sec = now_sec - mtime_sec;

            let unit_sec = match unit {
                'h' => 3600,
                'm' => 60,
                'w' => 7 * 24 * 3600,
                's' => 1,
                _ => 24 * 3600, // default days
            };
            let filter_sec = (*val as i64) * unit_sec;
            let matched_time = match op {
                '+' => age_sec > filter_sec,
                '-' => age_sec < filter_sec,
                _ => age_sec == filter_sec,
            };
            if !matched_time {
                continue;
            }
        }

        path_meta_list.push((path_str, metadata));
    }

    // 4. Sort
    if let Some(sort_field) = &qual.sort_by {
        match sort_field.as_str() {
            "mtime" => {
                path_meta_list.sort_by(|a, b| {
                    let a_sec =
                        a.1.modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                    let b_sec =
                        b.1.modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                    let ord = a_sec.cmp(&b_sec);
                    if qual.sort_asc { ord } else { ord.reverse() }
                });
            }
            "size" => {
                path_meta_list.sort_by(|a, b| {
                    let ord = a.1.len().cmp(&b.1.len());
                    if qual.sort_asc { ord } else { ord.reverse() }
                });
            }
            "name" => {
                path_meta_list.sort_by(|a, b| {
                    let ord = a.0.cmp(&b.0);
                    if qual.sort_asc { ord } else { ord.reverse() }
                });
            }
            _ => {}
        }
    }

    let mut result: Vec<String> = path_meta_list.into_iter().map(|item| item.0).collect();

    // 5. Range
    if let Some((start, end)) = qual.range {
        if start > 0 && start <= result.len() {
            let start_idx = start - 1;
            let end_idx = std::cmp::min(end, result.len());
            if start_idx < end_idx {
                result = result[start_idx..end_idx].to_vec();
            } else {
                result.clear();
            }
        } else {
            result.clear();
        }
    }

    result
}

pub fn expand_glob(pattern: &str) -> Vec<String> {
    expand_glob_with_options(pattern, false, false)
}

pub fn expand_glob_with_options(pattern: &str, nullglob: bool, nocaseglob: bool) -> Vec<String> {
    // Extended globs: prefer the DP matcher (linear, no regex backtracking);
    // its alternates are matched literally so "a.b" is literal, not wildcard.
    if let Some(ext_glob) = crate::extended_glob::ExtendedGlob::parse(pattern) {
        let base = determine_base_dir(pattern);
        let mut results: Vec<String> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&base) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                if ext_glob.matches(&name) {
                    results.push(entry.path().to_string_lossy().to_string());
                }
            }
        }
        results.sort();
        return results;
    }

    let mut clean_pattern = pattern.to_string();
    let mut qualifiers = None;

    if pattern.ends_with(')') {
        let paren_start_opt = pattern.rfind('(');
        if let Some(paren_start) = paren_start_opt {
            let qual_content = &pattern[paren_start + 1..pattern.len() - 1];
            if let Some(qual) = parse_glob_qualifiers(qual_content) {
                clean_pattern = pattern[..paren_start].to_string();
                qualifiers = Some(qual);
            }
        }
    }

    let raw_matches = if !clean_pattern.contains('*')
        && !clean_pattern.contains('?')
        && !clean_pattern.contains('[')
    {
        if qualifiers.is_some() {
            let path = std::path::Path::new(&clean_pattern);
            if path.exists() {
                vec![clean_pattern.clone()]
            } else {
                vec![]
            }
        } else {
            return vec![clean_pattern];
        }
    } else {
        let glob = match globset::GlobBuilder::new(&clean_pattern)
            .case_insensitive(nocaseglob)
            .build()
        {
            Ok(g) => g.compile_matcher(),
            Err(_) => return vec![pattern.to_string()],
        };

        let is_recursive = clean_pattern.contains("**");
        let base = determine_base_dir(&clean_pattern);

        let mut raw: Vec<String> = Vec::new();

        if is_recursive {
            for entry in WalkDir::new(&base)
                .min_depth(1)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                let path = entry.path();
                if let Some(path_str) = path.to_str() {
                    let clean = path_str.strip_prefix("./").unwrap_or(path_str);
                    if glob.is_match(clean) || glob.is_match(path_str) {
                        raw.push(clean.to_string());
                    }
                } else {
                    let display = path.to_string_lossy();
                    let display_ref: &str = display.as_ref();
                    let clean = display_ref.strip_prefix("./").unwrap_or(display_ref);
                    if glob.is_match(clean) || glob.is_match(display_ref) {
                        raw.push(clean.to_string());
                    }
                }
            }
        } else if let Ok(dir) = std::fs::read_dir(&base) {
            for entry in dir.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let full = if base == "." {
                    name.clone()
                } else {
                    format!("{}/{}", base.trim_end_matches('/'), name)
                };
                if glob.is_match(&full) || glob.is_match(&name) {
                    raw.push(full);
                }
            }
        }
        raw
    };

    let mut matches = if let Some(ref q) = qualifiers {
        apply_qualifiers(raw_matches, q)
    } else {
        raw_matches
    };

    if matches.is_empty() {
        if nullglob {
            vec![]
        } else {
            vec![pattern.to_string()]
        }
    } else {
        let should_sort = if let Some(ref q) = qualifiers {
            q.sort_by.is_none()
        } else {
            true
        };
        if should_sort {
            matches.sort();
        }
        matches
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_glob_no_glob() {
        assert_eq!(expand_glob("hello"), vec!["hello"]);
    }

    #[test]
    fn test_expand_glob_flat() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "").unwrap();
        std::fs::write(tmp.path().join("b.rs"), "").unwrap();
        std::fs::write(tmp.path().join("c.txt"), "").unwrap();
        let pattern = format!("{}/*.rs", tmp.path().display());
        let mut result = expand_glob(&pattern);
        result.sort();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_expand_glob_recursive() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("src/lex")).unwrap();
        std::fs::write(tmp.path().join("src/a.rs"), "").unwrap();
        std::fs::write(tmp.path().join("src/lex/parser.rs"), "").unwrap();
        std::fs::write(tmp.path().join("src/lex/tok.rs"), "").unwrap();
        std::fs::write(tmp.path().join("src/lex/readme.txt"), "").unwrap();
        let pattern = format!("{}/**/*.rs", tmp.path().display());
        let mut result = expand_glob(&pattern);
        result.sort();
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_expand_glob_no_matches() {
        assert_eq!(
            expand_glob("*.nonexistent12345"),
            vec!["*.nonexistent12345"]
        );
    }

    #[test]
    fn test_expand_braces_comma() {
        let result = expand_braces("{a,b,c}");
        assert_eq!(result, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_expand_braces_prefix() {
        let result = expand_braces("file.{txt,bak,log}");
        assert_eq!(result, vec!["file.txt", "file.bak", "file.log"]);
    }

    #[test]
    fn test_expand_braces_nested() {
        let result = expand_braces("{a,{b,c}}");
        assert_eq!(result, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_expand_braces_no_brace() {
        let result = expand_braces("hello");
        assert_eq!(result, vec!["hello"]);
    }

    #[test]
    fn test_expand_braces_empty_content() {
        let result = expand_braces("{}");
        assert_eq!(result, vec!["{}"]);
    }

    #[test]
    fn test_brace_range_numeric() {
        let result = expand_braces("{1..5}");
        assert_eq!(result, vec!["1", "2", "3", "4", "5"]);
    }

    #[test]
    fn test_brace_range_alpha() {
        let result = expand_braces("{a..e}");
        assert_eq!(result, vec!["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn test_brace_range_reverse() {
        let result = expand_braces("{5..1}");
        assert_eq!(result, vec!["5", "4", "3", "2", "1"]);
    }

    #[test]
    fn test_expand_braces_multiple_braces() {
        let result = expand_braces("a{b,c}d{e,f}");
        assert_eq!(result, vec!["abde", "abdf", "acde", "acdf"]);
    }

    #[test]
    fn test_expand_braces_with_path() {
        let result = expand_braces("src/{main,util}.rs");
        assert_eq!(result, vec!["src/main.rs", "src/util.rs"]);
    }

    #[test]
    fn test_parse_glob_qualifiers() {
        // Test type qualifiers
        let q = parse_glob_qualifiers(".").unwrap();
        assert_eq!(q.file_types, vec!['.']);
        assert!(!q.follow_symlinks);

        let q = parse_glob_qualifiers("-/").unwrap();
        assert_eq!(q.file_types, vec!['/']);
        assert!(q.follow_symlinks);

        // Test size qualifiers
        let q = parse_glob_qualifiers("Lk+100").unwrap();
        assert_eq!(q.size_filter, Some(('+', 'k', 100)));

        let q = parse_glob_qualifiers("Lm-5").unwrap();
        assert_eq!(q.size_filter, Some(('-', 'm', 5)));

        let q = parse_glob_qualifiers("L10").unwrap();
        assert_eq!(q.size_filter, Some(('=', 'b', 10)));

        // Test time qualifiers
        let q = parse_glob_qualifiers("mh-3").unwrap();
        assert_eq!(q.mtime_filter, Some(('-', 'h', 3)));

        let q = parse_glob_qualifiers("m+5").unwrap();
        assert_eq!(q.mtime_filter, Some(('+', 'd', 5)));

        // Test sort qualifiers
        let q = parse_glob_qualifiers("om").unwrap();
        assert_eq!(q.sort_by.as_deref(), Some("mtime"));
        assert!(!q.sort_asc);

        let q = parse_glob_qualifiers("oS").unwrap();
        assert_eq!(q.sort_by.as_deref(), Some("size"));
        assert!(q.sort_asc);

        // Test range
        let q = parse_glob_qualifiers("[1,3]").unwrap();
        assert_eq!(q.range, Some((1, 3)));

        // Invalid qualifiers
        assert!(parse_glob_qualifiers("xyz").is_none());
    }

    #[test]
    fn test_expand_glob_qualifiers_filtering() {
        let tmp = tempfile::tempdir().unwrap();
        let path_a = tmp.path().join("a.txt");
        let path_b = tmp.path().join("b.txt");
        let path_dir = tmp.path().join("subdir");

        std::fs::write(&path_a, "short").unwrap(); // 5 bytes
        std::fs::write(&path_b, "much longer content").unwrap(); // 19 bytes
        std::fs::create_dir(&path_dir).unwrap();

        // 1. Filter by file type '.' (files)
        let pattern = format!("{}/*(.)", tmp.path().display());
        let matches = expand_glob(&pattern);
        assert_eq!(matches.len(), 2);
        assert!(matches.contains(&path_a.to_string_lossy().to_string()));
        assert!(matches.contains(&path_b.to_string_lossy().to_string()));

        // 2. Filter by file type '/' (directories)
        let pattern = format!("{}/*(/)", tmp.path().display());
        let matches = expand_glob(&pattern);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0], path_dir.to_string_lossy().to_string());

        // 3. Filter by size '+' (greater than)
        let pattern = format!("{}/*(.L+10)", tmp.path().display());
        let matches = expand_glob(&pattern);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0], path_b.to_string_lossy().to_string());

        // 4. Filter by size '-' (less than)
        let pattern = format!("{}/*(.L-10)", tmp.path().display());
        let matches = expand_glob(&pattern);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0], path_a.to_string_lossy().to_string());

        // 5. Sort by size descending (os)
        let pattern = format!("{}/*(.os)", tmp.path().display());
        let matches = expand_glob(&pattern);
        assert_eq!(matches.len(), 2);
        // "b.txt" (19 bytes) should be first, "a.txt" (5 bytes) second
        assert_eq!(matches[0], path_b.to_string_lossy().to_string());
        assert_eq!(matches[1], path_a.to_string_lossy().to_string());

        // 6. Sort by size ascending (oS)
        let pattern = format!("{}/*(.oS)", tmp.path().display());
        let matches = expand_glob(&pattern);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0], path_a.to_string_lossy().to_string());
        assert_eq!(matches[1], path_b.to_string_lossy().to_string());

        // 7. Range [1,1] with sort size descending
        let pattern = format!("{}/*(.os[1,1])", tmp.path().display());
        let matches = expand_glob(&pattern);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0], path_b.to_string_lossy().to_string());
    }
}

pub fn determine_base_dir(pattern: &str) -> String {
    for (i, c) in pattern.char_indices() {
        if c == '*' || c == '?' || c == '[' {
            let prefix = &pattern[..i];
            if let Some(pos) = prefix.rfind('/') {
                let base = &pattern[..pos];
                return if base.is_empty() {
                    ".".to_string()
                } else {
                    base.to_string()
                };
            }
            return ".".to_string();
        }
    }
    ".".to_string()
}

/// Expand brace patterns like `{a,b,c}` and `{1..5}` into multiple strings.
pub fn expand_braces(s: &str) -> Vec<String> {
    // Find the first unquoted/un-nested '{' and matching '}' with content
    let Some(brace_start) = find_brace_start(s) else {
        return vec![s.to_string()];
    };
    // Find matching '}' accounting for nesting
    let Some(brace_end) = find_matching_brace(s, brace_start) else {
        return vec![s.to_string()];
    };

    let prefix = &s[..brace_start];
    let content = &s[brace_start + 1..brace_end];
    let suffix = &s[brace_end + 1..];

    let parts: Vec<&str> = split_brace_content(content);

    // Range pattern like {1..5} or {a..e}
    if parts.len() == 1 && is_range_pattern(content) {
        let ranged = expand_brace_range(content);
        let mut results = Vec::with_capacity(ranged.len());
        for r in &ranged {
            let candidate = format!("{}{}{}", prefix, r, suffix);
            if contains_brace(&candidate) {
                results.extend(expand_braces(&candidate));
            } else {
                results.push(candidate);
            }
        }
        return results;
    }

    if parts.len() <= 1 {
        return vec![s.to_string()];
    }

    let mut results = Vec::new();
    for part in &parts {
        let candidate = format!("{}{}{}", prefix, part, suffix);
        // Check if the result itself contains brace patterns
        if contains_brace(&candidate) {
            results.extend(expand_braces(&candidate));
        } else {
            results.push(candidate);
        }
    }
    results
}

fn find_brace_start(s: &str) -> Option<usize> {
    s.find('{')
}

fn find_matching_brace(s: &str, start: usize) -> Option<usize> {
    let mut depth = 0u32;
    let mut in_single = false;
    let mut in_double = false;
    for (i, c) in s[start..].char_indices() {
        if in_single {
            if c == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            if c == '"' {
                in_double = false;
            }
            continue;
        }
        match c {
            '\'' => in_single = true,
            '"' => in_double = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(start + i);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_brace_content(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0u32;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

fn is_range_pattern(s: &str) -> bool {
    if let Some((left, right)) = s.split_once("..") {
        if right.is_empty() || left.is_empty() {
            return false;
        }
        // Check left and right parts don't contain commas (to avoid confusion)
        if left.contains(',') || right.contains(',') {
            return false;
        }
        true
    } else {
        false
    }
}

fn contains_brace(s: &str) -> bool {
    s.contains('{')
}

/// Generate brace-expanded range values. Handles `{1..5}` and `{a..e}`.
pub fn expand_brace_range(s: &str) -> Vec<String> {
    let Some((left, right)) = s.split_once("..") else {
        return vec![s.to_string()];
    };

    // Numeric range
    if let (Ok(l), Ok(r)) = (left.parse::<i64>(), right.parse::<i64>()) {
        let step = if l <= r { 1 } else { -1 };
        let mut range = Vec::new();
        let mut i = l;
        loop {
            range.push(i.to_string());
            if i == r {
                break;
            }
            i += step;
        }
        return range;
    }

    // Alphabetic range (single char)
    if let (Some(l), Some(r)) = (left.chars().next(), right.chars().next())
        && l.is_ascii_alphabetic()
        && r.is_ascii_alphabetic()
    {
        let step: i8 = if l as u8 <= r as u8 { 1 } else { -1 };
        let mut range = Vec::new();
        let mut b = l as u8;
        loop {
            range.push((b as char).to_string());
            if b == r as u8 {
                break;
            }
            b = b.wrapping_add(step as u8);
        }
        return range;
    }

    vec![s.to_string()]
}
