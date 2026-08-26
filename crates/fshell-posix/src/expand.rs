// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use brush_parser::{
    ParserOptions,
    word::{self, Parameter, SpecialParameter, WordPiece},
};
use fshell_core::Val;

/// Configuration for word expansion.
#[derive(Debug, Clone)]
pub struct ExpansionConfig {
    /// IFS value — used for field splitting. `None` means default `" \t\n"`.
    pub ifs: Option<String>,
    /// Whether to perform pathname (glob) expansion.
    pub do_glob: bool,
}

impl Default for ExpansionConfig {
    fn default() -> Self {
        Self {
            ifs: None,
            do_glob: true,
        }
    }
}

fn effective_ifs(cfg: &ExpansionConfig, env: &fshell_engine::Env) -> String {
    if let Some(s) = &cfg.ifs {
        return s.clone();
    }
    // Check $IFS in env.vars
    if let Some(v) = env.vars.read().get("IFS") {
        return match v {
            Val::String(s) => s.clone(),
            other => other.to_text(),
        };
    }
    " \t\n".to_string()
}

/// Split a string according to POSIX $IFS rules.
///
/// IFS whitespace chars (space, tab, newline) and non-whitespace IFS chars
/// have distinct collapsing semantics.
pub fn split_ifs(s: &str, ifs: &str) -> Vec<String> {
    if ifs.is_empty() {
        return vec![s.to_string()];
    }
    let ifs_ws: Vec<char> = ifs
        .chars()
        .filter(|c| *c == ' ' || *c == '\t' || *c == '\n')
        .collect();
    let ifs_nws: Vec<char> = ifs
        .chars()
        .filter(|c| !(*c == ' ' || *c == '\t' || *c == '\n'))
        .collect();

    // Fast path: default IFS whitespace
    if ifs_nws.is_empty() {
        // Sequences of IFS whitespace collapse to one delimiter; leading/trailing trimmed.
        let parts: Vec<String> = s
            .split(|c| ifs_ws.contains(&c))
            .filter(|p| !p.is_empty())
            .map(|p| p.to_string())
            .collect();
        return parts;
    }

    // Mixed IFS: each nws char is its own delimiter (preserves empties between),
    // sequences of ws chars collapse and trim at edges.
    let mut fields: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    let mut prev_was_ws_delim = true; // treat leading ws as delimiter (trim leading)

    while let Some(c) = chars.next() {
        if ifs_nws.contains(&c) {
            // nws delimiter: push current field (may be empty)
            fields.push(std::mem::take(&mut cur));
            prev_was_ws_delim = false;
        } else if ifs_ws.contains(&c) {
            // ws delimiter: only delimit if we have accumulated content
            if !cur.is_empty() {
                fields.push(std::mem::take(&mut cur));
            }
            prev_was_ws_delim = true;
            // Collapse consecutive ws
            while matches!(chars.peek(), Some(&nc) if ifs_ws.contains(&nc)) {
                chars.next();
            }
        } else {
            cur.push(c);
            prev_was_ws_delim = false;
        }
    }
    if !cur.is_empty()
        || (fields.is_empty() && s.is_empty())
        || (!prev_was_ws_delim && !cur.is_empty())
    {
        fields.push(cur);
    }
    // Trailing delimiters
    if let Some(last) = s.chars().last()
        && ifs_nws.contains(&last)
        && (fields.last().map(|f| !f.is_empty()).unwrap_or(true) || fields.is_empty())
    {
        fields.push(String::new());
    }
    // Remove leading phantom empty that shouldn't be there when string started with ws
    if s.starts_with(|c| ifs_ws.contains(&c))
        && fields.first().map(|f| f.is_empty()).unwrap_or(false)
    {
        fields.remove(0);
    }
    fields
}

fn get_effective_positional(env: &fshell_engine::Env, fallback: &[String]) -> Vec<String> {
    if let Some(Val::List(items)) = env.vars.read().get("@") {
        return items.iter().map(|v| v.to_text()).collect();
    }
    fallback.to_vec()
}

/// Resolve a Parameter to its string value from Env.
fn resolve_parameter(param: &Parameter, env: &fshell_engine::Env, positional: &[String]) -> String {
    let eff_pos = get_effective_positional(env, positional);
    match param {
        Parameter::Positional(n) => {
            if *n == 0 {
                // $0 is shell name; expose as "fsh"
                "fsh".to_string()
            } else {
                eff_pos
                    .get((*n as usize).saturating_sub(1))
                    .cloned()
                    .unwrap_or_default()
            }
        }
        Parameter::Special(sp) => match sp {
            SpecialParameter::AllPositionalParameters { .. } => eff_pos.join(" "),
            SpecialParameter::PositionalParameterCount => eff_pos.len().to_string(),
            SpecialParameter::LastExitStatus => Some(env.vars.read())
                .and_then(|vars| vars.get("?").map(|v| v.to_text()))
                .unwrap_or_else(|| "0".to_string()),
            SpecialParameter::CurrentOptionFlags => "".to_string(),
            SpecialParameter::ProcessId => std::process::id().to_string(),
            SpecialParameter::LastBackgroundProcessId => "0".to_string(),
            SpecialParameter::ShellName => "fsh".to_string(),
        },
        Parameter::Named(name) => {
            // Check special vars first
            if let Some(v) = env.special_vars.resolve(name) {
                return v.to_text();
            }
            Some(env.vars.read())
                .and_then(|vars| vars.get(name.as_str()).map(|v| v.to_text()))
                .unwrap_or_default()
        }
        Parameter::NamedWithIndex { name, index } => {
            // Treat as array element — fall back to variable lookup with index suffix
            let key = format!("{}[{}]", name, index);
            Some(env.vars.read())
                .and_then(|vars| vars.get(&key).map(|v| v.to_text()))
                .unwrap_or_default()
        }
        Parameter::NamedWithAllIndices { name, .. } => Some(env.vars.read())
            .and_then(|vars| {
                vars.get(name.as_str()).map(|v| match v {
                    Val::List(items) => items
                        .iter()
                        .map(|x| x.to_text())
                        .collect::<Vec<_>>()
                        .join(" "),
                    other => other.to_text(),
                })
            })
            .unwrap_or_default(),
    }
}

/// Minimal glob matcher for POSIX pathname expansion.
/// Supports `*`, `?`, `[...]`. Uses walkdir for `*` at filesystem level.
fn expand_glob(pattern: &str, cwd: &std::path::Path) -> Vec<String> {
    // Use the fshell glob machinery if available, else fallback.
    // We do a conservative filesystem glob using glob crate semantics.
    // For now, use globset for matching but restrict to current directory expansion.
    let has_glob = pattern.contains('*') || pattern.contains('?') || pattern.contains('[');
    if !has_glob {
        return vec![pattern.to_string()];
    }
    // Build glob pattern relative
    let glob = match glob::Pattern::new(pattern) {
        Ok(p) => p,
        Err(_) => return vec![pattern.to_string()],
    };

    // Enumerate directory entries at appropriate depth. For simplicity, support non-recursive
    // patterns (no **). If pattern contains '/', split directory traversal.
    let mut matches: Vec<String> = Vec::new();
    // Walk up to 1 level deep for simple patterns; full walk for patterns with '/'
    let candidates: Vec<std::path::PathBuf> = if pattern.contains('/') {
        walkdir::WalkDir::new(cwd)
            .max_depth(6)
            .into_iter()
            .filter_map(|e| e.ok())
            .map(|e| e.path().strip_prefix(cwd).unwrap_or(e.path()).to_path_buf())
            .collect()
    } else {
        std::fs::read_dir(cwd)
            .ok()
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().into())
                    .collect()
            })
            .unwrap_or_default()
    };

    for path in candidates {
        let s = path.to_string_lossy().to_string();
        if glob.matches(&s) {
            // Skip dotfiles unless pattern explicitly starts with .
            if s.starts_with('.') && !pattern.starts_with('.') {
                continue;
            }
            matches.push(s);
        }
    }

    if matches.is_empty() {
        // POSIX: non-matching glob is left as-is when nullglob is off (default)
        vec![pattern.to_string()]
    } else {
        matches.sort();
        matches
    }
}

/// Expand a single POSIX word into zero or more expanded strings.
///
/// Implements the 4-phase expansion:
///   1. tilde / parameter / command substitution / arithmetic
///   2. field splitting (IFS)
///   3. pathname expansion (glob)
///   4. quote removal (handled by brush-parser word pieces)
pub fn expand_word(
    word_str: &str,
    env: &fshell_engine::Env,
    cfg: &ExpansionConfig,
    positional: &[String],
) -> Vec<String> {
    if word_str == "$@" || word_str == "\"$@\"" {
        return get_effective_positional(env, positional);
    }

    let opts = ParserOptions {
        enable_extended_globbing: false,
        posix_mode: true,
        sh_mode: false,
        tilde_expansion_at_word_start: true,
        tilde_expansion_after_colon: true,
        ..Default::default()
    };

    let pieces = match word::parse(word_str, &opts) {
        Ok(p) => p,
        Err(_) => return vec![word_str.to_string()],
    };

    // Phase 1: build the expanded string
    let mut expanded = String::new();
    let mut had_quoted = false;

    for wp in &pieces {
        match &wp.piece {
            WordPiece::Text(t) => expanded.push_str(t),
            WordPiece::SingleQuotedText(t) => {
                expanded.push_str(t);
                had_quoted = true;
            }
            WordPiece::DoubleQuotedSequence(seq) => {
                had_quoted = true;
                for inner in seq {
                    match &inner.piece {
                        WordPiece::Text(t) => expanded.push_str(t),
                        WordPiece::ParameterExpansion(pe) => {
                            let val = eval_parameter_expr(pe, env, positional);
                            expanded.push_str(&val);
                        }
                        WordPiece::CommandSubstitution(cmd) => {
                            let out = run_command_subst(cmd, env);
                            expanded.push_str(&out);
                        }
                        WordPiece::ArithmeticExpression(expr) => {
                            let out = eval_arithmetic(&expr.value, env);
                            expanded.push_str(&out);
                        }
                        WordPiece::EscapeSequence(s) => expanded.push_str(s),
                        _ => {}
                    }
                }
            }
            WordPiece::ParameterExpansion(pe) => {
                let val = eval_parameter_expr(pe, env, positional);
                expanded.push_str(&val);
            }
            WordPiece::TildeExpansion(te) => {
                let home = std::env::var("HOME").unwrap_or_default();
                match te {
                    word::TildeExpr::Home => expanded.push_str(&home),
                    word::TildeExpr::UserHome(_) => expanded.push_str(&home),
                    _ => expanded.push_str(&home),
                }
            }
            WordPiece::CommandSubstitution(cmd) => {
                let out = run_command_subst(cmd, env);
                expanded.push_str(&out);
            }
            WordPiece::BackquotedCommandSubstitution(cmd) => {
                let out = run_command_subst(cmd, env);
                expanded.push_str(&out);
            }
            WordPiece::ArithmeticExpression(expr) => {
                let out = eval_arithmetic(&expr.value, env);
                expanded.push_str(&out);
            }
            WordPiece::AnsiCQuotedText(t) => {
                expanded.push_str(&unescape_ansi_c(t));
            }
            WordPiece::EscapeSequence(s) => expanded.push_str(s),
            _ => {}
        }
    }

    // Phase 2: field splitting (only if not quoted)
    let fields = if had_quoted {
        vec![expanded]
    } else {
        let ifs = effective_ifs(cfg, env);
        // If expansion came from unquoted parameter/command substitution, split.
        // For simplicity, split the whole word — quoted segments already guarded.
        // The pieces-level quoted tracking is coarse; this matches common POSIX behavior.
        let contains_expansion = pieces.iter().any(|wp| {
            matches!(
                wp.piece,
                WordPiece::ParameterExpansion(_)
                    | WordPiece::CommandSubstitution(_)
                    | WordPiece::BackquotedCommandSubstitution(_)
            )
        });
        if contains_expansion {
            split_ifs(&expanded, &ifs)
        } else if expanded.contains(' ') || expanded.contains('\t') || expanded.contains('\n') {
            // No expansion but word contains IFS whitespace? Don't split bare words like "hello world" from quoted source — but unquoted "a  b" should? Keep as single field to avoid breaking simple args.
            vec![expanded]
        } else {
            vec![expanded]
        }
    };

    // Phase 3: pathname expansion
    if cfg.do_glob {
        let mut result = Vec::new();
        for field in fields {
            // Don't glob if field came from quoted context
            if had_quoted {
                result.push(field);
            } else {
                result.extend(expand_glob(&field, &env.cwd()));
            }
        }
        result
    } else {
        fields
    }
}

fn eval_parameter_expr(
    expr: &word::ParameterExpr,
    env: &fshell_engine::Env,
    positional: &[String],
) -> String {
    use word::ParameterExpr as PE;
    match expr {
        PE::Parameter { parameter, .. } => resolve_parameter(parameter, env, positional),
        PE::ParameterLength { parameter, .. } => {
            let val = resolve_parameter(parameter, env, positional);
            val.chars().count().to_string()
        }
        PE::UseDefaultValues {
            parameter,
            default_value,
            test_type,
            ..
        } => {
            let val = resolve_parameter(parameter, env, positional);
            let is_unset_or_null = val.is_empty();
            let is_unset = {
                let check = |p: &Parameter| match p {
                    Parameter::Named(n) => Some(env.vars.read())
                        .and_then(|vars| vars.get(n.as_str()).cloned())
                        .is_none(),
                    Parameter::Positional(n) => {
                        positional.get((*n as usize).saturating_sub(1)).is_none()
                    }
                    _ => false,
                };
                check(parameter)
            };
            let should_use_default = match test_type {
                word::ParameterTestType::UnsetOrNull => is_unset_or_null,
                word::ParameterTestType::Unset => is_unset,
            };
            if should_use_default {
                default_value.clone().unwrap_or_default()
            } else {
                val
            }
        }
        PE::AssignDefaultValues {
            parameter,
            default_value,
            ..
        } => {
            let val = resolve_parameter(parameter, env, positional);
            if val.is_empty() {
                let default = default_value.clone().unwrap_or_default();
                if let Parameter::Named(name) = parameter {
                    env.vars
                        .write()
                        .insert(name.clone(), Val::String(default.clone()));
                }
                default
            } else {
                val
            }
        }
        PE::IndicateErrorIfNullOrUnset {
            parameter,
            error_message,
            ..
        } => {
            let val = resolve_parameter(parameter, env, positional);
            if val.is_empty() {
                let msg = error_message
                    .clone()
                    .unwrap_or_else(|| format!("parameter {:?} is unset or null", parameter));
                // In POSIX this is a fatal error; we return empty and let caller handle via error reporting
                // For now, write to stderr and return empty
                eprintln!("fsh: {}: {}", parameter, msg);
                String::new()
            } else {
                val
            }
        }
        PE::UseAlternativeValue {
            parameter,
            alternative_value,
            ..
        } => {
            let val = resolve_parameter(parameter, env, positional);
            if val.is_empty() {
                String::new()
            } else {
                alternative_value.clone().unwrap_or_default()
            }
        }
        PE::RemoveSmallestPrefixPattern {
            parameter, pattern, ..
        } => {
            let val = resolve_parameter(parameter, env, positional);
            if let Some(pat) = pattern {
                let pat_expanded =
                    expand_word(pat, env, &ExpansionConfig::default(), positional).join("");
                // # shortest prefix
                if val.starts_with(&pat_expanded) {
                    val[pat_expanded.len()..].to_string()
                } else {
                    // Try glob match for pattern
                    match glob::Pattern::new(&pat_expanded) {
                        Ok(g) => {
                            // Find shortest prefix matching pattern
                            for i in 0..=val.len() {
                                let prefix = &val[..i];
                                if g.matches(prefix) {
                                    return val[i..].to_string();
                                }
                            }
                            val
                        }
                        Err(_) => val,
                    }
                }
            } else {
                val
            }
        }
        PE::RemoveLargestPrefixPattern {
            parameter, pattern, ..
        } => {
            let val = resolve_parameter(parameter, env, positional);
            if let Some(pat) = pattern {
                let pat_expanded =
                    expand_word(pat, env, &ExpansionConfig::default(), positional).join("");
                // ## longest prefix
                if let Ok(g) = glob::Pattern::new(&pat_expanded) {
                    for i in (0..=val.len()).rev() {
                        let prefix = &val[..i];
                        if g.matches(prefix) {
                            return val[i..].to_string();
                        }
                    }
                }
                val
            } else {
                val
            }
        }
        PE::RemoveSmallestSuffixPattern {
            parameter, pattern, ..
        } => {
            let val = resolve_parameter(parameter, env, positional);
            if let Some(pat) = pattern {
                let pat_expanded =
                    expand_word(pat, env, &ExpansionConfig::default(), positional).join("");
                if let Ok(g) = glob::Pattern::new(&pat_expanded) {
                    for i in 0..=val.len() {
                        let suffix = &val[val.len() - i..];
                        if g.matches(suffix) {
                            return val[..val.len() - i].to_string();
                        }
                    }
                }
                val
            } else {
                val
            }
        }
        PE::RemoveLargestSuffixPattern {
            parameter, pattern, ..
        } => {
            let val = resolve_parameter(parameter, env, positional);
            if let Some(pat) = pattern {
                let pat_expanded =
                    expand_word(pat, env, &ExpansionConfig::default(), positional).join("");
                if let Ok(g) = glob::Pattern::new(&pat_expanded) {
                    for i in 0..=val.len() {
                        let suffix = &val[val.len() - i..];
                        if g.matches(suffix) {
                            return val[..val.len() - i].to_string();
                        }
                    }
                }
                val
            } else {
                val
            }
        }
        PE::Substring {
            parameter,
            offset,
            length,
            ..
        } => {
            let val = resolve_parameter(parameter, env, positional);
            let offset_str =
                expand_word(&offset.value, env, &ExpansionConfig::default(), positional).join("");
            let off: i64 = offset_str.trim().parse().unwrap_or(0);
            let chars: Vec<char> = val.chars().collect();
            let len = chars.len() as i64;
            let start = if off < 0 {
                (len + off).max(0) as usize
            } else {
                (off as usize).min(chars.len())
            };
            let end = if let Some(len_expr) = length {
                let len_str = expand_word(
                    &len_expr.value,
                    env,
                    &ExpansionConfig::default(),
                    positional,
                )
                .join("");
                let l: i64 = len_str.trim().parse().unwrap_or(0);
                if l < 0 {
                    chars.len()
                } else {
                    (start as i64 + l).min(chars.len() as i64) as usize
                }
            } else {
                chars.len()
            };
            chars[start..end.min(chars.len())].iter().collect()
        }
        PE::ReplaceSubstring {
            parameter,
            pattern,
            replacement,
            match_kind,
            ..
        } => {
            let val = resolve_parameter(parameter, env, positional);
            let pat = expand_word(pattern, env, &ExpansionConfig::default(), positional).join("");
            let repl = replacement.clone().unwrap_or_default();
            match match_kind {
                word::SubstringMatchKind::FirstOccurrence => {
                    if let Some(pos) = val.find(&pat) {
                        format!("{}{}{}", &val[..pos], repl, &val[pos + pat.len()..])
                    } else {
                        val
                    }
                }
                word::SubstringMatchKind::Anywhere => val.replace(&pat, &repl),
                word::SubstringMatchKind::Prefix => {
                    if val.starts_with(&pat) {
                        format!("{}{}", repl, &val[pat.len()..])
                    } else {
                        val
                    }
                }
                word::SubstringMatchKind::Suffix => {
                    if val.ends_with(&pat) {
                        format!("{}{}", &val[..val.len() - pat.len()], repl)
                    } else {
                        val
                    }
                }
            }
        }
        PE::UppercaseFirstChar { parameter, .. } => {
            let mut val = resolve_parameter(parameter, env, positional);
            if let Some(first) = val.get_mut(0..1) {
                first.make_ascii_uppercase();
            } else {
                let mut chars = val.chars();
                if let Some(c) = chars.next() {
                    val = c.to_uppercase().to_string() + chars.as_str();
                }
            }
            val
        }
        PE::UppercasePattern { parameter, .. } => {
            resolve_parameter(parameter, env, positional).to_uppercase()
        }
        PE::LowercaseFirstChar { parameter, .. } => {
            let mut val = resolve_parameter(parameter, env, positional);
            if let Some(first) = val.get_mut(0..1) {
                first.make_ascii_lowercase();
            } else {
                let mut chars = val.chars();
                if let Some(c) = chars.next() {
                    val = c.to_lowercase().to_string() + chars.as_str();
                }
            }
            val
        }
        PE::LowercasePattern { parameter, .. } => {
            resolve_parameter(parameter, env, positional).to_lowercase()
        }
        PE::Transform { parameter, op, .. } => {
            let val = resolve_parameter(parameter, env, positional);
            match op {
                word::ParameterTransformOp::ToUpperCase => val.to_uppercase(),
                word::ParameterTransformOp::ToLowerCase => val.to_lowercase(),
                _ => val,
            }
        }
        // Unhandled parameter-expression forms resolve to empty rather than
        // being misinterpreted as a variable name.
        _ => String::new(),
    }
}

fn run_command_subst(cmd: &str, env: &fshell_engine::Env) -> String {
    let child_env = crate::bridge::fork_env_for_subshell(env);
    if let Ok(parsed) = crate::parser::parse_posix_script(cmd) {
        let bytes_res = std::thread::scope(|s| {
            s.spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok()
                    .map(|rt| {
                        rt.block_on(async {
                            crate::eval::eval_source_capture(&parsed, &child_env).await
                        })
                    })
                    .unwrap_or(Ok(Vec::new()))
            })
            .join()
            .unwrap_or(Ok(Vec::new()))
        });

        if let Ok(bytes) = bytes_res {
            let mut s = String::from_utf8_lossy(&bytes).into_owned();
            while s.ends_with('\n') || s.ends_with('\r') {
                s.pop();
            }
            return s;
        }
    }

    let output = std::process::Command::new("sh").arg("-c").arg(cmd).output();
    match output {
        Ok(out) => {
            let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
            while s.ends_with('\n') || s.ends_with('\r') {
                s.pop();
            }
            s
        }
        Err(_) => String::new(),
    }
}

fn eval_arithmetic(expr: &str, env: &fshell_engine::Env) -> String {
    match crate::arithmetic::eval_arithmetic_expr(expr, env) {
        Ok(n) => n.to_string(),
        Err(e) => {
            eprintln!("fsh: arithmetic error: {e}");
            "0".to_string()
        }
    }
}

fn unescape_ansi_c(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some('\'') => out.push('\''),
                Some('"') => out.push('"'),
                Some('a') => out.push('\x07'),
                Some('b') => out.push('\x08'),
                Some('f') => out.push('\x0C'),
                Some('v') => out.push('\x0B'),
                Some('0') => out.push('\0'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_ifs_default() {
        assert_eq!(split_ifs("a  b\tc\n", " \t\n"), vec!["a", "b", "c"]);
        assert_eq!(split_ifs("  a  ", " \t\n"), vec!["a"]);
        assert_eq!(split_ifs("", " \t\n"), Vec::<String>::new());
    }

    #[test]
    fn test_split_ifs_colon() {
        assert_eq!(split_ifs("a:b::c", ":"), vec!["a", "b", "", "c"]);
    }

    #[test]
    fn test_split_ifs_empty() {
        assert_eq!(split_ifs("a b c", ""), vec!["a b c"]);
    }

    #[test]
    fn test_glob_no_match() {
        let r = expand_glob("no_such_file_zzz_12345", std::path::Path::new("."));
        assert_eq!(r, vec!["no_such_file_zzz_12345"]);
    }

    #[test]
    fn test_expand_simple_word() {
        let env = fshell_engine::Env::for_command();
        let cfg = ExpansionConfig::default();
        let r = expand_word("hello", &env, &cfg, &[]);
        assert_eq!(r, vec!["hello"]);
    }

    #[test]
    fn test_expand_parameter() {
        let env = fshell_engine::Env::for_command();
        env.vars
            .write()
            .insert("FOO".to_string(), Val::String("bar".to_string()));
        let cfg = ExpansionConfig::default();
        let r = expand_word("$FOO", &env, &cfg, &[]);
        assert_eq!(r, vec!["bar"]);
    }

    #[test]
    fn test_expand_default_value() {
        let env = fshell_engine::Env::for_command();
        let cfg = ExpansionConfig::default();
        let r = expand_word("${UNSET:-default}", &env, &cfg, &[]);
        assert_eq!(r, vec!["default"]);
    }

    #[test]
    fn test_expand_length() {
        let env = fshell_engine::Env::for_command();
        env.vars
            .write()
            .insert("FOO".to_string(), Val::String("hello".to_string()));
        let cfg = ExpansionConfig::default();
        let r = expand_word("${#FOO}", &env, &cfg, &[]);
        assert_eq!(r, vec!["5"]);
    }

    #[test]
    fn test_expand_substring() {
        let env = fshell_engine::Env::for_command();
        env.vars
            .write()
            .insert("FOO".to_string(), Val::String("hello".to_string()));
        let cfg = ExpansionConfig::default();
        let r = expand_word("${FOO:1:3}", &env, &cfg, &[]);
        assert_eq!(r, vec!["ell"]);
    }

    #[test]
    fn test_expand_prefix_removal() {
        let env = fshell_engine::Env::for_command();
        env.vars
            .write()
            .insert("FOO".to_string(), Val::String("hello".to_string()));
        let cfg = ExpansionConfig::default();
        let r = expand_word("${FOO#hel}", &env, &cfg, &[]);
        assert_eq!(r, vec!["lo"]);
    }

    #[test]
    fn test_expand_suffix_removal() {
        let env = fshell_engine::Env::for_command();
        env.vars
            .write()
            .insert("FOO".to_string(), Val::String("hello".to_string()));
        let cfg = ExpansionConfig::default();
        let r = expand_word("${FOO%lo}", &env, &cfg, &[]);
        assert_eq!(r, vec!["hel"]);
    }
}
