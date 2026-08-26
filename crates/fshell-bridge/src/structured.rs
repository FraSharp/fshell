// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::Val;

/// Outcome of parsing a single line of stdout.
#[derive(Debug, PartialEq)]
pub enum ParseResult {
    /// Emit this Val on the pipeline as Data.
    Data(Val),
    /// This line was a header — schema saved in ParseState, do not emit.
    Header,
    /// No parser matched — caller falls through to Val::String.
    Fallthrough,
}

/// Per-command parsing state accumulated across lines of a single command's output.
#[derive(Debug, Default)]
pub struct ParseState {
    pub ps_columns: Option<Vec<String>>,
    pub df_columns: Option<Vec<String>>,
}
// Parser functions
/// Parse rg/grep `PATH:LINE:CONTENT` output.
///
/// Uses `splitn(3, ':')` from the left so that content containing colons
/// (e.g. `fn foo(a: usize)`) lands entirely in the content field.
fn parse_grep_line(line: &str, _state: &mut ParseState) -> ParseResult {
    let trimmed = line.trim();
    let mut parts = trimmed.splitn(3, ':');
    let filepath = match parts.next() {
        Some(s) if !s.is_empty() => s,
        _ => return ParseResult::Fallthrough,
    };
    let line_num = match parts.next() {
        Some(s) => s,
        _ => return ParseResult::Fallthrough,
    };
    let content = match parts.next() {
        Some(s) => s,
        _ => return ParseResult::Fallthrough,
    };

    let line_int: i64 = match line_num.parse() {
        Ok(n) => n,
        Err(_) => return ParseResult::Fallthrough,
    };

    // Require that the filepath contains at least one separator char to
    // avoid treating "time:10:value" as a grep match.
    if !filepath.contains('/') && !filepath.contains('\\') && !filepath.contains('.') {
        return ParseResult::Fallthrough;
    }

    let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    m.insert(ustr::ustr("file"), Val::String(filepath.to_string()));
    m.insert(ustr::ustr("line"), Val::Int(line_int));
    m.insert(ustr::ustr("content"), Val::String(content.to_string()));
    ParseResult::Data(Val::Map(m))
}

/// Parse `ls -l` output.
fn parse_ls_line(line: &str, _state: &mut ParseState) -> ParseResult {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return ParseResult::Fallthrough;
    }

    // Handle "total N" header line
    if trimmed.starts_with("total ") {
        return ParseResult::Header;
    }

    // Must start with a known file type character
    let first = match trimmed.chars().next() {
        Some(c) => c,
        None => return ParseResult::Fallthrough,
    };
    if !matches!(first, '-' | 'd' | 'l' | 'c' | 'b' | 's' | 'p') {
        return ParseResult::Fallthrough;
    }

    // Check the first 10 chars look like a mode string
    if trimmed.len() < 10
        || !trimmed[1..10].chars().all(|c| {
            matches!(
                c,
                '-' | 'r' | 'w' | 'x' | 's' | 'S' | 't' | 'T' | 'l' | '+' | '@' | '.'
            )
        })
    {
        return ParseResult::Fallthrough;
    }

    let mode = &trimmed[..10];

    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() < 9 {
        return ParseResult::Fallthrough;
    }

    let nlink: i64 = match parts[1].parse() {
        Ok(n) => n,
        Err(_) => return ParseResult::Fallthrough,
    };
    let owner = parts[2].to_string();
    let group = parts[3].to_string();
    let size: i64 = match parts[4].parse() {
        Ok(n) => n,
        Err(_) => return ParseResult::Fallthrough,
    };
    let month = parts[5].to_string();
    let day: i64 = match parts[6].parse() {
        Ok(n) => n,
        Err(_) => return ParseResult::Fallthrough,
    };
    let time_or_year = parts[7].to_string();

    // Name is whatever comes after the 8th whitespace-delimited token
    let name = parts[8..].join(" ");

    let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    m.insert(ustr::ustr("mode"), Val::String(mode.to_string()));
    m.insert(ustr::ustr("nlink"), Val::Int(nlink));
    m.insert(ustr::ustr("owner"), Val::String(owner));
    m.insert(ustr::ustr("group"), Val::String(group));
    m.insert(ustr::ustr("size"), Val::Int(size));
    m.insert(ustr::ustr("month"), Val::String(month));
    m.insert(ustr::ustr("day"), Val::Int(day));
    m.insert(ustr::ustr("time_or_year"), Val::String(time_or_year));
    m.insert(ustr::ustr("name"), Val::String(name));
    ParseResult::Data(Val::Map(m))
}

/// Parse `ps aux` / `ps -ef` tabular output.
///
/// Header detection: first line matching `USER ` or `UID ` or `PID `.
/// Column names are normalized to lowercase with `%` and `-` replaced.
fn parse_ps_line(line: &str, state: &mut ParseState) -> ParseResult {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return ParseResult::Fallthrough;
    }

    // Detect ps header by looking for known column starts
    let is_header = trimmed.starts_with("USER ")
        || trimmed.starts_with("UID ")
        || trimmed.starts_with("PID ")
        || trimmed.starts_with("%CPU");

    if is_header {
        let cols: Vec<String> = trimmed
            .split_whitespace()
            .map(|c| c.to_lowercase().replace('%', "").replace('-', "_"))
            .collect();
        state.ps_columns = Some(cols);
        return ParseResult::Header;
    }

    let columns = match &state.ps_columns {
        Some(c) => c,
        None => return ParseResult::Fallthrough,
    };

    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() < columns.len() {
        return ParseResult::Fallthrough;
    }

    let num_known = columns.len() - 1;
    let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    for (i, col) in columns.iter().enumerate() {
        let val = if i < num_known {
            Val::String(parts[i].to_string())
        } else {
            // Last column: everything remaining (command may contain spaces)
            Val::String(parts[num_known..].join(" "))
        };
        m.insert(ustr::ustr(col), val);
    }
    ParseResult::Data(Val::Map(m))
}

/// Parse `df -h` output.
///
/// Uses a hardcoded column schema because the header "Mounted on" contains
/// a space and would be split incorrectly by split_whitespace.
fn parse_df_line(line: &str, state: &mut ParseState) -> ParseResult {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return ParseResult::Fallthrough;
    }

    // Detect df -h header — use hardcoded schema
    if trimmed.starts_with("Filesystem") {
        state.df_columns = Some(vec![
            "filesystem".into(),
            "size".into(),
            "used".into(),
            "avail".into(),
            "capacity".into(),
            "mounted_on".into(),
        ]);
        return ParseResult::Header;
    }

    let columns = match &state.df_columns {
        Some(c) => c,
        None => return ParseResult::Fallthrough,
    };

    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() < columns.len() {
        return ParseResult::Fallthrough;
    }

    let last_idx = columns.len() - 1;
    let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    for (i, col) in columns.iter().enumerate() {
        let val = if i < last_idx {
            Val::String(parts[i].to_string())
        } else {
            // Mount point: everything after the 5th field
            Val::String(parts[last_idx..].join(" "))
        };
        m.insert(ustr::ustr(col), val);
    }
    ParseResult::Data(Val::Map(m))
}

/// Parse `mount` output — pattern-based, no header.
///
/// Format: `<device> on <mount_point> (<fstype>, <options>)`
fn parse_mount_line(line: &str, _state: &mut ParseState) -> ParseResult {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return ParseResult::Fallthrough;
    }

    if let Some(on_pos) = trimmed.find(" on ") {
        let device = &trimmed[..on_pos];
        let rest = &trimmed[on_pos + 4..];

        if let Some(paren_pos) = rest.find(" (") {
            let mount_point = &rest[..paren_pos];
            let paren_content = &rest[paren_pos + 2..];

            if let Some(close_pos) = paren_content.rfind(')') {
                let options_part = &paren_content[..close_pos];
                // First comma-separated segment is the fstype
                let fstype = match options_part.find(", ") {
                    Some(pos) => options_part[..pos].to_string(),
                    None => options_part.to_string(),
                };

                let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
                m.insert(ustr::ustr("device"), Val::String(device.to_string()));
                m.insert(
                    ustr::ustr("mount_point"),
                    Val::String(mount_point.to_string()),
                );
                m.insert(ustr::ustr("fstype"), Val::String(fstype));
                return ParseResult::Data(Val::Map(m));
            }
        }
    }
    ParseResult::Fallthrough
}

/// Parse `git status --porcelain` output.
fn parse_git_status_line(line: &str, _state: &mut ParseState) -> ParseResult {
    let trimmed = line.trim_end();
    if trimmed.len() < 4 || !is_git_status_prefix(&trimmed[..2]) {
        return ParseResult::Fallthrough;
    }

    let status = &trimmed[..2];
    let rest = trimmed[2..].trim();

    // Detect renames: "R  old -> new"
    if status.starts_with('R')
        && let Some(arrow_pos) = rest.find(" -> ")
    {
        let old_path = rest[..arrow_pos].to_string();
        let new_path = rest[arrow_pos + 4..].to_string();
        let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        m.insert(ustr::ustr("status"), Val::String(status.to_string()));
        m.insert(ustr::ustr("old_path"), Val::String(old_path));
        m.insert(ustr::ustr("new_path"), Val::String(new_path));
        return ParseResult::Data(Val::Map(m));
    }

    let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    m.insert(ustr::ustr("status"), Val::String(status.to_string()));
    m.insert(ustr::ustr("path"), Val::String(rest.to_string()));
    ParseResult::Data(Val::Map(m))
}

/// Check if the first two chars look like git status porcelain codes.
fn is_git_status_prefix(s: &str) -> bool {
    let valid = |c: char| matches!(c, ' ' | 'M' | 'A' | 'D' | 'R' | 'C' | 'U' | '?' | '!');
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(a), Some(b)) => valid(a) && valid(b),
        _ => false,
    }
}

/// Pre-execution flag injection: add flags for known tools that support JSON output.
/// Never overrides user-specified flags. Returns the flag to inject, if any.
///
/// `rg` is deliberately not injected: its plain PATH:LINE:CONTENT output is parsed
/// natively below, and --json produces NDJSON this bridge cannot parse yet.
pub fn maybe_inject_flag(cmd: &str, args: &[Val]) -> Option<&'static str> {
    let has_flag = |flag: &str| {
        args.iter()
            .any(|a| matches!(a, Val::String(s) if s == flag))
    };
    match cmd {
        "fd" if !has_flag("--json") => Some("--json"),
        "git"
            if args
                .iter()
                .any(|a| matches!(a, Val::String(s) if s == "status"))
                && !has_flag("--porcelain")
                && !has_flag("--short")
                && !has_flag("-s") =>
        {
            Some("--porcelain")
        }
        _ => None,
    }
}

/// Returns true if the command is a recognized structured output provider.
pub fn is_known_structured_command(cmd: &str, args: &[String]) -> bool {
    match cmd {
        "rg" | "grep" | "ps" | "df" | "mount" => true,
        "ls" => args
            .iter()
            .any(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('l')),
        "git" => {
            let is_status = args.iter().any(|a| a == "status");
            let is_porcelain = args
                .iter()
                .any(|a| a == "--porcelain" || a == "--short" || a == "-s");
            let is_log = args.iter().any(|a| a == "log") && args.iter().any(|a| a == "--oneline");
            (is_status && is_porcelain) || is_log
        }
        _ => false,
    }
}

/// Main dispatch: route stdout lines to the correct parser based on command name.
/// Returns ParseResult — caller handles Data, Header, or Fallthrough.
pub fn parse_line(cmd: &str, args: &[String], line: &str, state: &mut ParseState) -> ParseResult {
    match cmd {
        "rg" | "grep" => parse_grep_line(line, state),
        "ls" => {
            // Only parse ls -l style output; a long-format flag must look like a
            // flag (leading '-'), so file arguments like `ls src/lib.rs` don't
            // trigger the parser.
            let is_long = args
                .iter()
                .any(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('l'));
            if is_long {
                parse_ls_line(line, state)
            } else {
                ParseResult::Fallthrough
            }
        }
        "ps" => parse_ps_line(line, state),
        "df" => parse_df_line(line, state),
        "mount" => parse_mount_line(line, state),
        "git" => {
            let is_status = args.iter().any(|a| a == "status");
            let is_porcelain = args
                .iter()
                .any(|a| a == "--porcelain" || a == "--short" || a == "-s");
            let is_log = args.iter().any(|a| a == "log") && args.iter().any(|a| a == "--oneline");
            if is_status && is_porcelain {
                parse_git_status_line(line, state)
            } else if is_log {
                parse_git_log_line(line, state)
            } else {
                ParseResult::Fallthrough
            }
        }
        _ => ParseResult::Fallthrough,
    }
}

/// Parse `git log --oneline` output.
fn parse_git_log_line(line: &str, _state: &mut ParseState) -> ParseResult {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.len() < 8 {
        return ParseResult::Fallthrough;
    }

    // Find the first space — everything before is the commit hash
    if let Some(space_pos) = trimmed.find(' ') {
        let commit = &trimmed[..space_pos];
        let message = trimmed[space_pos + 1..].trim();

        // Verify commit hash looks valid (hex chars)
        if commit.len() >= 7 && commit.chars().all(|c| c.is_ascii_hexdigit()) {
            let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            m.insert(ustr::ustr("commit"), Val::String(commit.to_string()));
            m.insert(ustr::ustr("message"), Val::String(message.to_string()));
            return ParseResult::Data(Val::Map(m));
        }
    }
    ParseResult::Fallthrough
}
// Tests
#[cfg(test)]
mod tests {
    use super::*;
    use fshell_core::FxIndexMap;
    use ustr::ustr;

    fn assert_data_map(result: ParseResult) -> FxIndexMap<ustr::Ustr, Val> {
        match result {
            ParseResult::Data(Val::Map(map)) => map,
            other => panic!("Expected Data(Map), got {:?}", other),
        }
    }

    // Grep parser tests
    #[test]
    fn test_parse_grep_line_basic() {
        let line = "src/main.rs:42:fn main() {}";
        let mut state = ParseState::default();
        let map = assert_data_map(parse_grep_line(line, &mut state));
        assert_eq!(
            map.get(&ustr("file")),
            Some(&Val::String("src/main.rs".into()))
        );
        assert_eq!(map.get(&ustr("line")), Some(&Val::Int(42)));
        assert_eq!(
            map.get(&ustr("content")),
            Some(&Val::String("fn main() {}".into()))
        );
    }

    #[test]
    fn test_parse_grep_line_context_line() {
        // Lines with "-" instead of ":" are context/continuation lines
        let line = "src/main.rs-45-    // comment";
        let mut state = ParseState::default();
        assert_eq!(parse_grep_line(line, &mut state), ParseResult::Fallthrough);
    }

    #[test]
    fn test_parse_grep_line_no_match() {
        let line = "just a plain string without colons";
        let mut state = ParseState::default();
        assert_eq!(parse_grep_line(line, &mut state), ParseResult::Fallthrough);
    }

    #[test]
    fn test_parse_grep_line_content_with_colons() {
        // splitn(3, ':') from the left: content after 2nd colon is whole remainder
        let line = "file.rs:10:fn foo(a: usize, b: usize)";
        let mut state = ParseState::default();
        let map = assert_data_map(parse_grep_line(line, &mut state));
        assert_eq!(map.get(&ustr("file")), Some(&Val::String("file.rs".into())));
        assert_eq!(map.get(&ustr("line")), Some(&Val::Int(10)));
        assert_eq!(
            map.get(&ustr("content")),
            Some(&Val::String("fn foo(a: usize, b: usize)".into()))
        );
    }

    #[test]
    fn test_parse_grep_line_no_path_separator() {
        // Without a path separator, we reject to avoid false positives
        let line = "time:10:value";
        let mut state = ParseState::default();
        assert_eq!(parse_grep_line(line, &mut state), ParseResult::Fallthrough);
    }

    // ls -l parser tests
    #[test]
    fn test_parse_ls_line_long() {
        let line = "-rw-r--r--  1 ariel  staff  233 May 28 14:23 src/main.rs";
        let mut state = ParseState::default();
        let map = assert_data_map(parse_ls_line(line, &mut state));
        assert_eq!(
            map.get(&ustr("mode")),
            Some(&Val::String("-rw-r--r--".into()))
        );
        assert_eq!(map.get(&ustr("nlink")), Some(&Val::Int(1)));
        assert_eq!(map.get(&ustr("owner")), Some(&Val::String("ariel".into())));
        assert_eq!(map.get(&ustr("group")), Some(&Val::String("staff".into())));
        assert_eq!(map.get(&ustr("size")), Some(&Val::Int(233)));
        assert_eq!(map.get(&ustr("month")), Some(&Val::String("May".into())));
        assert_eq!(map.get(&ustr("day")), Some(&Val::Int(28)));
        assert_eq!(
            map.get(&ustr("time_or_year")),
            Some(&Val::String("14:23".into()))
        );
        assert_eq!(
            map.get(&ustr("name")),
            Some(&Val::String("src/main.rs".into()))
        );
    }

    #[test]
    fn test_parse_ls_line_total_line() {
        let line = "total 42";
        let mut state = ParseState::default();
        assert_eq!(parse_ls_line(line, &mut state), ParseResult::Header);
    }

    #[test]
    fn test_parse_ls_line_symlink() {
        let line = "lrwxr-xr-x  1 ariel  staff  11 May 28 14:23 link -> target";
        let mut state = ParseState::default();
        let map = assert_data_map(parse_ls_line(line, &mut state));
        assert_eq!(
            map.get(&ustr("mode")),
            Some(&Val::String("lrwxr-xr-x".into()))
        );
        assert_eq!(
            map.get(&ustr("name")),
            Some(&Val::String("link -> target".into()))
        );
    }

    #[test]
    fn test_parse_ls_line_fallthrough() {
        let line = "some random text";
        let mut state = ParseState::default();
        assert_eq!(parse_ls_line(line, &mut state), ParseResult::Fallthrough);
    }

    // ps parser tests
    #[test]
    fn test_parse_ps_line_header() {
        let line =
            "USER             PID  %CPU %MEM      VSZ    RSS   TT  STAT STARTED      TIME COMMAND";
        let mut state = ParseState::default();
        assert_eq!(parse_ps_line(line, &mut state), ParseResult::Header);
        assert_eq!(
            state.ps_columns.as_deref(),
            Some(
                &[
                    "user".to_string(),
                    "pid".to_string(),
                    "cpu".to_string(),
                    "mem".to_string(),
                    "vsz".to_string(),
                    "rss".to_string(),
                    "tt".to_string(),
                    "stat".to_string(),
                    "started".to_string(),
                    "time".to_string(),
                    "command".to_string(),
                ][..]
            )
        );
    }

    #[test]
    fn test_parse_ps_line_data() {
        let line = "ariel          12345   0.0  0.1  4268000  12345 s001  S+   10:00AM   0:00.01 /usr/bin/some command";
        let mut state = ParseState {
            ps_columns: Some(vec![
                "user".into(),
                "pid".into(),
                "cpu".into(),
                "mem".into(),
                "vsz".into(),
                "rss".into(),
                "tt".into(),
                "stat".into(),
                "started".into(),
                "time".into(),
                "command".into(),
            ]),
            ..Default::default()
        };
        let map = assert_data_map(parse_ps_line(line, &mut state));
        assert_eq!(map.get(&ustr("pid")), Some(&Val::String("12345".into())));
    }

    #[test]
    fn test_parse_ps_line_no_state_yet() {
        let line = "ariel          12345   0.0  0.1  4268000  12345 s001  S+   10:00AM   0:00.01 /usr/bin/some command";
        let mut state = ParseState::default();
        assert_eq!(parse_ps_line(line, &mut state), ParseResult::Fallthrough);
    }

    // df parser tests
    #[test]
    fn test_parse_df_line_header() {
        let line = "Filesystem      Size   Used  Avail Capacity Mounted on";
        let mut state = ParseState::default();
        let result = parse_df_line(line, &mut state);
        assert_eq!(result, ParseResult::Header);
        assert!(state.df_columns.is_some());
    }

    #[test]
    fn test_parse_df_line_data() {
        let line = "/dev/disk1s1  233G  120G   113G    52% /";
        let mut state = ParseState {
            df_columns: Some(vec![
                "filesystem".into(),
                "size".into(),
                "used".into(),
                "avail".into(),
                "capacity".into(),
                "mounted_on".into(),
            ]),
            ..Default::default()
        };
        let map = assert_data_map(parse_df_line(line, &mut state));
        assert_eq!(
            map.get(&ustr("filesystem")),
            Some(&Val::String("/dev/disk1s1".into()))
        );
        assert_eq!(map.get(&ustr("size")), Some(&Val::String("233G".into())));
        assert_eq!(map.get(&ustr("mounted_on")), Some(&Val::String("/".into())));
    }

    // mount parser tests
    #[test]
    fn test_parse_mount_line_standard() {
        let line = "/dev/disk1s1 on / (apfs, local, journaled)";
        let mut state = ParseState::default();
        let map = assert_data_map(parse_mount_line(line, &mut state));
        assert_eq!(
            map.get(&ustr("device")),
            Some(&Val::String("/dev/disk1s1".into()))
        );
        assert_eq!(
            map.get(&ustr("mount_point")),
            Some(&Val::String("/".into()))
        );
        assert_eq!(map.get(&ustr("fstype")), Some(&Val::String("apfs".into())));
    }

    #[test]
    fn test_parse_mount_line_no_match() {
        let line = "some random text";
        let mut state = ParseState::default();
        assert_eq!(parse_mount_line(line, &mut state), ParseResult::Fallthrough);
    }

    // git status parser tests
    #[test]
    fn test_parse_git_status_line_modified() {
        let line = " M src/main.rs";
        let mut state = ParseState::default();
        let map = assert_data_map(parse_git_status_line(line, &mut state));
        assert_eq!(map.get(&ustr("status")), Some(&Val::String(" M".into())));
        assert_eq!(
            map.get(&ustr("path")),
            Some(&Val::String("src/main.rs".into()))
        );
    }

    #[test]
    fn test_parse_git_status_line_untracked() {
        let line = "?? new_file.txt";
        let mut state = ParseState::default();
        let map = assert_data_map(parse_git_status_line(line, &mut state));
        assert_eq!(map.get(&ustr("status")), Some(&Val::String("??".into())));
        assert_eq!(
            map.get(&ustr("path")),
            Some(&Val::String("new_file.txt".into()))
        );
    }

    #[test]
    fn test_parse_git_status_line_rename() {
        let line = "R  old_name.rs -> new_name.rs";
        let mut state = ParseState::default();
        let map = assert_data_map(parse_git_status_line(line, &mut state));
        assert_eq!(map.get(&ustr("status")), Some(&Val::String("R ".into())));
        assert_eq!(
            map.get(&ustr("old_path")),
            Some(&Val::String("old_name.rs".into()))
        );
        assert_eq!(
            map.get(&ustr("new_path")),
            Some(&Val::String("new_name.rs".into()))
        );
    }

    #[test]
    fn test_parse_git_status_line_fallthrough() {
        let line = "some random text";
        let mut state = ParseState::default();
        assert_eq!(
            parse_git_status_line(line, &mut state),
            ParseResult::Fallthrough
        );
    }

    // flag injection tests — rg injection disabled until NDJSON parser is added
    #[test]
    fn test_maybe_inject_flag_rg_no_flags() {
        // rg injection is disabled — no flags should not trigger injection
        let args = vec![Val::String("search".into()), Val::String(".".into())];
        assert_eq!(maybe_inject_flag("rg", &args), None);
    }

    #[test]
    fn test_maybe_inject_flag_rg_already_json() {
        let args = vec![Val::String("--json".into()), Val::String("search".into())];
        assert_eq!(maybe_inject_flag("rg", &args), None);
    }

    #[test]
    fn test_maybe_inject_flag_unknown_tool() {
        let args = vec![Val::String("file.txt".into())];
        assert_eq!(maybe_inject_flag("cat", &args), None);
    }

    #[test]
    fn test_maybe_inject_flag_fd_no_flags() {
        // fd injection is still active
        let args = vec![Val::String(".".into())];
        assert_eq!(maybe_inject_flag("fd", &args), Some("--json"));
    }

    #[test]
    fn test_maybe_inject_flag_fd_already_json() {
        let args = vec![Val::String("--json".into()), Val::String(".".into())];
        assert_eq!(maybe_inject_flag("fd", &args), None);
    }

    #[test]
    fn test_maybe_inject_flag_git_status() {
        let args = vec![Val::String("status".into())];
        assert_eq!(maybe_inject_flag("git", &args), Some("--porcelain"));
    }

    #[test]
    fn test_maybe_inject_flag_git_status_already_porcelain() {
        let args = vec![
            Val::String("status".into()),
            Val::String("--porcelain".into()),
        ];
        assert_eq!(maybe_inject_flag("git", &args), None);
    }

    #[test]
    fn test_maybe_inject_flag_git_log() {
        let args = vec![Val::String("log".into())];
        assert_eq!(maybe_inject_flag("git", &args), None);
    }

    // git log --oneline tests
    #[test]
    fn test_parse_git_log_line_basic() {
        let line = "a1b2c3d fix: resolve login issue";
        let mut state = ParseState::default();
        let map = assert_data_map(parse_git_log_line(line, &mut state));
        assert_eq!(
            map.get(&ustr("commit")),
            Some(&Val::String("a1b2c3d".into()))
        );
        assert_eq!(
            map.get(&ustr("message")),
            Some(&Val::String("fix: resolve login issue".into()))
        );
    }

    #[test]
    fn test_parse_git_log_line_fallthrough() {
        let line = "not a log line";
        let mut state = ParseState::default();
        assert_eq!(
            parse_git_log_line(line, &mut state),
            ParseResult::Fallthrough
        );
    }

    // dispatch function tests
    #[test]
    fn test_parse_line_dispatch_rg() {
        let line = "src/main.rs:42:fn main() {}";
        let mut state = ParseState::default();
        let result = parse_line("rg", &[], line, &mut state);
        assert!(matches!(result, ParseResult::Data(Val::Map(_))));
    }

    #[test]
    fn test_parse_line_dispatch_unknown_fallthrough() {
        let line = "hello world";
        let mut state = ParseState::default();
        assert_eq!(
            parse_line("echo", &[], line, &mut state),
            ParseResult::Fallthrough
        );
    }

    #[test]
    fn test_parse_line_ls_without_l_flag() {
        let line = "-rw-r--r--  1 ariel  staff  233 May 28 14:23 file.rs";
        let mut state = ParseState::default();
        // Without -l flag, ls output should NOT be parsed
        assert_eq!(
            parse_line("ls", &[], line, &mut state),
            ParseResult::Fallthrough
        );
    }

    #[test]
    fn test_parse_line_ls_with_l_flag() {
        let line = "-rw-r--r--  1 ariel  staff  233 May 28 14:23 file.rs";
        let mut state = ParseState::default();
        let result = parse_line("ls", &["-l".to_string()], line, &mut state);
        assert!(matches!(result, ParseResult::Data(Val::Map(_))));
    }
}
