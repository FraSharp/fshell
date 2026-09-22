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

    // POSIX 2.6.5 Mixed IFS splitting:
    // IFS whitespace is ignored at the beginning and end of input.
    // Each non-whitespace IFS char, along with any adjacent IFS whitespace, delimits a field.
    let mut fields: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();

    // 1. Skip leading IFS whitespace
    while matches!(chars.peek(), Some(&c) if ifs_ws.contains(&c)) {
        chars.next();
    }

    let mut had_nws_delim = false;

    while let Some(c) = chars.next() {
        if ifs_nws.contains(&c) {
            // Non-whitespace IFS delimiter: push current field
            fields.push(std::mem::take(&mut cur));
            had_nws_delim = true;
            // Skip any following IFS whitespace adjacent to this delimiter
            while matches!(chars.peek(), Some(&nc) if ifs_ws.contains(&nc)) {
                chars.next();
            }
        } else if ifs_ws.contains(&c) {
            // IFS whitespace: skip additional whitespace
            while matches!(chars.peek(), Some(&nc) if ifs_ws.contains(&nc)) {
                chars.next();
            }
            // If immediately followed by a non-whitespace IFS delimiter,
            // that delimiter will handle the split; otherwise this whitespace is a delimiter.
            if matches!(chars.peek(), Some(&nc) if ifs_nws.contains(&nc)) {
                // let next iteration handle the nws delimiter
            } else if chars.peek().is_some() {
                // Not trailing whitespace -> delimits field
                fields.push(std::mem::take(&mut cur));
                had_nws_delim = false;
            }
        } else {
            cur.push(c);
            had_nws_delim = false;
        }
    }

    if !cur.is_empty() || had_nws_delim {
        fields.push(cur);
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
fn resolve_parameter_with_presence(
    param: &Parameter,
    env: &fshell_engine::Env,
    positional: &[String],
) -> (String, bool) {
    let eff_pos = get_effective_positional(env, positional);
    match param {
        Parameter::Positional(n) => {
            if *n == 0 {
                // $0 is shell name; expose as "fsh"
                ("fsh".to_string(), true)
            } else {
                let value = eff_pos
                    .get((*n as usize).saturating_sub(1))
                    .cloned()
                    .unwrap_or_default();
                let is_set = (*n as usize).saturating_sub(1) < eff_pos.len();
                (value, is_set)
            }
        }
        Parameter::Special(sp) => match sp {
            SpecialParameter::AllPositionalParameters { .. } => (eff_pos.join(" "), true),
            SpecialParameter::PositionalParameterCount => (eff_pos.len().to_string(), true),
            SpecialParameter::LastExitStatus => (env.exit_code().to_string(), true),
            SpecialParameter::CurrentOptionFlags => (String::new(), true),
            SpecialParameter::ProcessId => (std::process::id().to_string(), true),
            SpecialParameter::LastBackgroundProcessId => ("0".to_string(), true),
            SpecialParameter::ShellName => ("fsh".to_string(), true),
        },
        Parameter::Named(name) => {
            // Check special vars first
            if let Some(v) = env.special_vars.resolve(name) {
                return (v.to_text(), true);
            }
            if let Some(ref locals) = env.local_vars
                && let Some(val) = locals.get(name.as_str())
            {
                return (val.to_text(), true);
            }
            env.vars
                .read()
                .get(name.as_str())
                .map(|v| (v.to_text(), true))
                .unwrap_or_else(|| (String::new(), false))
        }
        Parameter::NamedWithIndex { name, index } => {
            // Treat as array element — fall back to variable lookup with index suffix
            let key = format!("{}[{}]", name, index);
            env.vars
                .read()
                .get(&key)
                .map(|v| (v.to_text(), true))
                .unwrap_or_else(|| (String::new(), false))
        }
        Parameter::NamedWithAllIndices { name, .. } => env
            .vars
            .read()
            .get(name.as_str())
            .map(|v| {
                let value = match v {
                    Val::List(items) => items
                        .iter()
                        .map(|x| x.to_text())
                        .collect::<Vec<_>>()
                        .join(" "),
                    other => other.to_text(),
                };
                (value, true)
            })
            .unwrap_or_else(|| (String::new(), false)),
    }
}

fn resolve_parameter(param: &Parameter, env: &fshell_engine::Env, positional: &[String]) -> String {
    resolve_parameter_with_presence(param, env, positional).0
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
) -> Result<Vec<String>, fshell_engine::EngineError> {
    expand_word_internal(word_str, env, cfg, positional, true, false)
}

/// Expand a shell assignment value. POSIX assignment values do not undergo
/// field splitting or pathname expansion, even when their expansions contain
/// IFS whitespace or glob characters.
pub(crate) fn expand_assignment_word(
    word_str: &str,
    env: &fshell_engine::Env,
    positional: &[String],
) -> Result<String, fshell_engine::EngineError> {
    let values = expand_word_internal(
        word_str,
        env,
        &ExpansionConfig {
            do_glob: false,
            ..Default::default()
        },
        positional,
        false,
        false,
    )?;
    Ok(values.join(" "))
}

/// Expand a shell word for a `case` pattern.  Quoted and escaped wildcard
/// characters are retained as literal glob syntax (for example `[*]`) so the
/// pattern matcher cannot accidentally turn them back into wildcards.
pub(crate) fn expand_word_as_pattern(
    word_str: &str,
    env: &fshell_engine::Env,
    cfg: &ExpansionConfig,
    positional: &[String],
) -> Result<Vec<String>, fshell_engine::EngineError> {
    expand_word_internal(word_str, env, cfg, positional, false, true)
}

fn expand_word_internal(
    word_str: &str,
    env: &fshell_engine::Env,
    cfg: &ExpansionConfig,
    positional: &[String],
    do_field_split: bool,
    preserve_pattern_quoting: bool,
) -> Result<Vec<String>, fshell_engine::EngineError> {
    if word_str == "$@" || word_str == "\"$@\"" {
        return Ok(get_effective_positional(env, positional));
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
        Err(_) => return Ok(vec![word_str.to_string()]),
    };

    // Phase 1: build the expanded value and a parallel glob pattern.  The
    // value is what the command receives; the pattern escapes wildcard
    // characters originating in quotes or backslash escapes, while leaving
    // unquoted text and unquoted expansions eligible for pathname expansion.
    let mut expanded = String::new();
    let mut glob_pattern = String::new();
    let mut has_glob = false;
    let mut had_quoted = false;

    for wp in &pieces {
        match &wp.piece {
            WordPiece::Text(t) => {
                append_unquoted(&mut expanded, &mut glob_pattern, &mut has_glob, t)
            }
            WordPiece::SingleQuotedText(t) => {
                append_literal(
                    &mut expanded,
                    &mut glob_pattern,
                    t,
                    preserve_pattern_quoting,
                );
                had_quoted = true;
            }
            WordPiece::DoubleQuotedSequence(seq) => {
                had_quoted = true;
                for inner in seq {
                    match &inner.piece {
                        WordPiece::Text(t) => append_literal(
                            &mut expanded,
                            &mut glob_pattern,
                            t,
                            preserve_pattern_quoting,
                        ),
                        WordPiece::ParameterExpansion(pe) => {
                            let val = eval_parameter_expr(pe, env, positional)?;
                            append_literal(
                                &mut expanded,
                                &mut glob_pattern,
                                &val,
                                preserve_pattern_quoting,
                            );
                        }
                        WordPiece::CommandSubstitution(cmd) => {
                            let out = run_command_subst(cmd, env)?;
                            append_literal(
                                &mut expanded,
                                &mut glob_pattern,
                                &out,
                                preserve_pattern_quoting,
                            );
                        }
                        WordPiece::ArithmeticExpression(expr) => {
                            let out = eval_arithmetic(&expr.value, env)?;
                            append_literal(
                                &mut expanded,
                                &mut glob_pattern,
                                &out,
                                preserve_pattern_quoting,
                            );
                        }
                        WordPiece::EscapeSequence(s) => append_literal(
                            &mut expanded,
                            &mut glob_pattern,
                            unescape_word_piece(s),
                            preserve_pattern_quoting,
                        ),
                        _ => {}
                    }
                }
            }
            WordPiece::ParameterExpansion(pe) => {
                let val = eval_parameter_expr(pe, env, positional)?;
                append_unquoted(&mut expanded, &mut glob_pattern, &mut has_glob, &val);
            }
            WordPiece::TildeExpansion(te) => {
                let home = env.home_dir().to_string_lossy().into_owned();
                match te {
                    word::TildeExpr::Home | word::TildeExpr::UserHome(_) => append_literal(
                        &mut expanded,
                        &mut glob_pattern,
                        &home,
                        preserve_pattern_quoting,
                    ),
                    _ => append_literal(
                        &mut expanded,
                        &mut glob_pattern,
                        &home,
                        preserve_pattern_quoting,
                    ),
                }
            }
            WordPiece::CommandSubstitution(cmd) => {
                let out = run_command_subst(cmd, env)?;
                append_unquoted(&mut expanded, &mut glob_pattern, &mut has_glob, &out);
            }
            WordPiece::BackquotedCommandSubstitution(cmd) => {
                let out = run_command_subst(cmd, env)?;
                append_unquoted(&mut expanded, &mut glob_pattern, &mut has_glob, &out);
            }
            WordPiece::ArithmeticExpression(expr) => {
                let out = eval_arithmetic(&expr.value, env)?;
                append_unquoted(&mut expanded, &mut glob_pattern, &mut has_glob, &out);
            }
            WordPiece::AnsiCQuotedText(t) => {
                let value = unescape_ansi_c(t);
                append_literal(
                    &mut expanded,
                    &mut glob_pattern,
                    &value,
                    preserve_pattern_quoting,
                );
                had_quoted = true;
            }
            WordPiece::EscapeSequence(s) => append_literal(
                &mut expanded,
                &mut glob_pattern,
                unescape_word_piece(s),
                preserve_pattern_quoting,
            ),
            _ => {}
        }
    }

    // Phase 2: field splitting (only if not quoted)
    let fields = if had_quoted {
        vec![expanded.clone()]
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
        if contains_expansion && do_field_split {
            split_ifs(&expanded, &ifs)
        } else if expanded.contains(' ') || expanded.contains('\t') || expanded.contains('\n') {
            // No expansion but word contains IFS whitespace? Don't split bare words like "hello world" from quoted source — but unquoted "a  b" should? Keep as single field to avoid breaking simple args.
            vec![expanded.clone()]
        } else {
            vec![expanded.clone()]
        }
    };

    // Phase 3: pathname expansion
    if cfg.do_glob && has_glob {
        let mut result = Vec::new();
        if fields.len() == 1 {
            result.extend(expand_glob(&glob_pattern, &expanded, &env.cwd()));
        } else {
            // Unquoted field splitting can produce multiple independent
            // pathname patterns.  Expand only fields that still contain
            // wildcard syntax.
            for field in fields {
                if field.contains('*') || field.contains('?') || field.contains('[') {
                    result.extend(expand_glob(&field, &field, &env.cwd()));
                } else {
                    result.push(field);
                }
            }
        }
        Ok(result)
    } else {
        Ok(fields)
    }
}

fn append_literal(
    expanded: &mut String,
    glob_pattern: &mut String,
    value: &str,
    preserve_pattern_quoting: bool,
) {
    let escaped = glob::Pattern::escape(value);
    if preserve_pattern_quoting {
        expanded.push_str(&escaped);
    } else {
        expanded.push_str(value);
    }
    glob_pattern.push_str(&escaped);
}

fn append_unquoted(
    expanded: &mut String,
    glob_pattern: &mut String,
    has_glob: &mut bool,
    value: &str,
) {
    expanded.push_str(value);
    glob_pattern.push_str(value);
    *has_glob |= value.contains('*') || value.contains('?') || value.contains('[');
}

fn unescape_word_piece(value: &str) -> &str {
    value.strip_prefix('\\').unwrap_or(value)
}

fn eval_parameter_expr(
    expr: &word::ParameterExpr,
    env: &fshell_engine::Env,
    positional: &[String],
) -> Result<String, fshell_engine::EngineError> {
    use word::ParameterExpr as PE;
    let value = match expr {
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
            let (val, is_set) = resolve_parameter_with_presence(parameter, env, positional);
            let should_use_default = match test_type {
                word::ParameterTestType::UnsetOrNull => !is_set || val.is_empty(),
                word::ParameterTestType::Unset => !is_set,
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
            test_type,
            ..
        } => {
            let (val, is_set) = resolve_parameter_with_presence(parameter, env, positional);
            let should_assign = match test_type {
                word::ParameterTestType::UnsetOrNull => !is_set || val.is_empty(),
                word::ParameterTestType::Unset => !is_set,
            };
            if should_assign {
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
            test_type,
            ..
        } => {
            let (val, is_set) = resolve_parameter_with_presence(parameter, env, positional);
            let should_error = match test_type {
                word::ParameterTestType::UnsetOrNull => !is_set || val.is_empty(),
                word::ParameterTestType::Unset => !is_set,
            };
            if should_error {
                let message = match error_message {
                    Some(raw) => expand_word_internal(
                        raw,
                        env,
                        &ExpansionConfig {
                            do_glob: false,
                            ..Default::default()
                        },
                        positional,
                        false,
                        false,
                    )?
                    .join(""),
                    None => match test_type {
                        word::ParameterTestType::UnsetOrNull => {
                            format!("parameter {parameter} is unset or null")
                        }
                        word::ParameterTestType::Unset => {
                            format!("parameter {parameter} is unset")
                        }
                    },
                };
                return Err(fshell_engine::EngineError::ParameterExpansion {
                    parameter: parameter.to_string(),
                    message,
                    span: None,
                });
            }
            val
        }
        PE::UseAlternativeValue {
            parameter,
            alternative_value,
            test_type,
            ..
        } => {
            let (val, is_set) = resolve_parameter_with_presence(parameter, env, positional);
            let should_use_alternative = match test_type {
                word::ParameterTestType::UnsetOrNull => !is_set || val.is_empty(),
                word::ParameterTestType::Unset => !is_set,
            };
            if should_use_alternative {
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
                let no_glob_cfg = ExpansionConfig {
                    do_glob: false,
                    ..Default::default()
                };
                let pat_expanded =
                    expand_word_internal(pat, env, &no_glob_cfg, positional, false, false)?
                        .join("");
                if let Ok(g) = globset::Glob::new(&pat_expanded) {
                    let matcher = g.compile_matcher();
                    for (idx, _) in val.char_indices() {
                        let prefix = &val[..idx];
                        if matcher.is_match(prefix) {
                            return Ok(val[idx..].to_string());
                        }
                    }
                    if matcher.is_match(&val) {
                        return Ok(String::new());
                    }
                }
                val
            } else {
                val
            }
        }
        PE::RemoveLargestPrefixPattern {
            parameter, pattern, ..
        } => {
            let val = resolve_parameter(parameter, env, positional);
            if let Some(pat) = pattern {
                let no_glob_cfg = ExpansionConfig {
                    do_glob: false,
                    ..Default::default()
                };
                let pat_expanded =
                    expand_word_internal(pat, env, &no_glob_cfg, positional, false, false)?
                        .join("");
                if let Ok(g) = globset::Glob::new(&pat_expanded) {
                    let matcher = g.compile_matcher();
                    if matcher.is_match(&val) {
                        return Ok(String::new());
                    }
                    for (idx, _) in val.char_indices().rev() {
                        let prefix = &val[..idx];
                        if matcher.is_match(prefix) {
                            return Ok(val[idx..].to_string());
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
                let no_glob_cfg = ExpansionConfig {
                    do_glob: false,
                    ..Default::default()
                };
                let pat_expanded =
                    expand_word_internal(pat, env, &no_glob_cfg, positional, false, false)?
                        .join("");
                if let Ok(g) = globset::Glob::new(&pat_expanded) {
                    let matcher = g.compile_matcher();
                    for (idx, _) in val.char_indices().rev() {
                        let suffix = &val[idx..];
                        if matcher.is_match(suffix) {
                            return Ok(val[..idx].to_string());
                        }
                    }
                    if matcher.is_match(&val) {
                        return Ok(String::new());
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
                let no_glob_cfg = ExpansionConfig {
                    do_glob: false,
                    ..Default::default()
                };
                let pat_expanded =
                    expand_word_internal(pat, env, &no_glob_cfg, positional, false, false)?
                        .join("");
                if let Ok(g) = globset::Glob::new(&pat_expanded) {
                    let matcher = g.compile_matcher();
                    if matcher.is_match(&val) {
                        return Ok(String::new());
                    }
                    for (idx, _) in val.char_indices() {
                        let suffix = &val[idx..];
                        if matcher.is_match(suffix) {
                            return Ok(val[..idx].to_string());
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
            let no_glob_cfg = ExpansionConfig {
                do_glob: false,
                ..Default::default()
            };
            let offset_str =
                expand_word_internal(&offset.value, env, &no_glob_cfg, positional, false, false)?
                    .join("");
            let off = crate::arithmetic::eval_arithmetic_expr(offset_str.trim(), env)
                .map_err(crate::arithmetic::to_engine_error)?;
            let chars: Vec<char> = val.chars().collect();
            let len = chars.len() as i64;
            let start = if off < 0 {
                (len + off).max(0) as usize
            } else {
                usize::try_from(off).unwrap_or(usize::MAX).min(chars.len())
            };
            let end = if let Some(len_expr) = length {
                let len_str = expand_word_internal(
                    &len_expr.value,
                    env,
                    &no_glob_cfg,
                    positional,
                    false,
                    false,
                )?
                .join("");
                let l = crate::arithmetic::eval_arithmetic_expr(len_str.trim(), env)
                    .map_err(crate::arithmetic::to_engine_error)?;
                if l < 0 {
                    return Err(fshell_engine::EngineError::Generic {
                        message: "substring expression < 0".to_string(),
                        span: None,
                    });
                } else {
                    start
                        .saturating_add(usize::try_from(l).unwrap_or(usize::MAX))
                        .min(chars.len())
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
            let no_glob_cfg = ExpansionConfig {
                do_glob: false,
                ..Default::default()
            };
            let pat = expand_word_internal(pattern, env, &no_glob_cfg, positional, false, false)?
                .join("");
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
    };
    Ok(value)
}

/// Expand a POSIX pathname pattern against an explicit logical cwd.
///
/// The `glob` crate is used as the filesystem traversal engine rather than
/// walking a bounded portion of the cwd.  The old implementation treated all
/// slash-containing patterns as relative to the process cwd and capped the
/// traversal depth, which made absolute paths and deeper directory trees
/// silently fail to expand.  Building an absolute search pattern from the
/// shell's logical cwd keeps this function correct for both ordinary commands
/// and environments whose cwd differs from the process cwd.
fn expand_glob(pattern: &str, fallback: &str, cwd: &std::path::Path) -> Vec<String> {
    let is_absolute = std::path::Path::new(fallback).is_absolute();
    let search_pattern = if is_absolute {
        pattern.to_string()
    } else {
        let cwd_pattern = glob::Pattern::escape(&cwd.to_string_lossy());
        format!("{cwd_pattern}/{pattern}")
    };

    let matcher = match glob::Pattern::new(&search_pattern) {
        Ok(pattern) => pattern,
        // Invalid bracket expressions are ordinary unmatched shell words.
        Err(_) => return vec![fallback.to_string()],
    };
    let traversal_options = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        // Filter hidden components with the complete pattern below.  The
        // glob crate prunes dotfiles before matching when this is true, which
        // incorrectly rejects patterns whose component explicitly begins '.'.
        require_literal_leading_dot: false,
    };
    let paths = match glob::glob_with(&search_pattern, traversal_options) {
        Ok(paths) => paths,
        // Invalid bracket expressions are ordinary unmatched shell words.
        Err(_) => return vec![fallback.to_string()],
    };

    let mut matches = Vec::new();
    let matching_options = glob::MatchOptions {
        require_literal_leading_dot: true,
        ..traversal_options
    };
    for path in paths.flatten() {
        if !matcher.matches_path_with(&path, matching_options) {
            continue;
        }
        let rendered = if is_absolute {
            path.to_string_lossy().into_owned()
        } else {
            // `search_pattern` retains lexical `./` and `../` components, so
            // stripping the logical cwd preserves the spelling the user gave.
            path.strip_prefix(cwd)
                .map(|relative| relative.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path.to_string_lossy().into_owned())
        };
        matches.push(rendered);
    }

    if matches.is_empty() {
        // POSIX leaves an unmatched pathname pattern unchanged.
        vec![fallback.to_string()]
    } else {
        matches.sort();
        matches
    }
}

fn run_command_subst(
    cmd: &str,
    env: &fshell_engine::Env,
) -> Result<String, fshell_engine::EngineError> {
    let child_env = crate::bridge::fork_env_for_subshell(env);
    let parsed = crate::parser::parse_posix_script(cmd)?;
    let bytes = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| fshell_engine::EngineError::Generic {
                        message: format!("command substitution runtime failed: {error}"),
                        span: None,
                    })?;
                runtime
                    .block_on(async { crate::eval::eval_source_capture(&parsed, &child_env).await })
            })
            .join()
            .map_err(|_| fshell_engine::EngineError::Generic {
                message: "command substitution task panicked".to_string(),
                span: None,
            })?
    })?;

    let mut output = String::from_utf8_lossy(&bytes).into_owned();
    while output.ends_with('\n') || output.ends_with('\r') {
        output.pop();
    }
    Ok(output)
}

fn eval_arithmetic(
    expr: &str,
    env: &fshell_engine::Env,
) -> Result<String, fshell_engine::EngineError> {
    crate::arithmetic::eval_arithmetic_expr(expr, env)
        .map(|value| value.to_string())
        .map_err(crate::arithmetic::to_engine_error)
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
        let r = expand_glob(
            "no_such_file_zzz_12345",
            "no_such_file_zzz_12345",
            std::path::Path::new("."),
        );
        assert_eq!(r, vec!["no_such_file_zzz_12345"]);
    }

    #[test]
    fn test_glob_expands_absolute_and_deep_logical_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let mut nested = tmp.path().to_path_buf();
        for component in ["one", "two", "three", "four", "five", "six", "seven"] {
            nested.push(component);
        }
        std::fs::create_dir_all(&nested).unwrap();
        let file = nested.join("result.txt");
        std::fs::write(&file, b"ok").unwrap();

        let env = fshell_engine::Env::for_command();
        env.set_cwd(tmp.path().to_path_buf());
        let cfg = ExpansionConfig::default();
        let relative =
            expand_word("one/two/three/four/five/six/seven/*.txt", &env, &cfg, &[]).unwrap();
        assert_eq!(
            relative,
            vec!["one/two/three/four/five/six/seven/result.txt"]
        );

        let absolute_pattern = format!("{}/*.txt", nested.display());
        let absolute = expand_word(&absolute_pattern, &env, &cfg, &[]).unwrap();
        assert_eq!(absolute, vec![file.to_string_lossy().into_owned()]);
    }

    #[test]
    fn test_glob_preserves_dotfile_rules_and_unmatched_patterns() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("visible.txt"), b"ok").unwrap();
        std::fs::write(tmp.path().join(".hidden.txt"), b"ok").unwrap();
        std::fs::write(tmp.path().join("literal-visible.txt"), b"ok").unwrap();

        let env = fshell_engine::Env::for_command();
        env.set_cwd(tmp.path().to_path_buf());
        let cfg = ExpansionConfig::default();
        assert_eq!(
            expand_word("*.txt", &env, &cfg, &[]).unwrap(),
            vec!["literal-visible.txt", "visible.txt"]
        );
        assert_eq!(
            expand_word(".*.txt", &env, &cfg, &[]).unwrap(),
            vec![".hidden.txt"]
        );
        assert_eq!(
            expand_word("missing-*.txt", &env, &cfg, &[]).unwrap(),
            vec!["missing-*.txt"]
        );
        assert_eq!(
            expand_word(r#""*.txt""#, &env, &cfg, &[]).unwrap(),
            vec!["*.txt"]
        );
        assert_eq!(
            expand_word(r"\*.txt", &env, &cfg, &[]).unwrap(),
            vec!["*.txt"]
        );
        assert_eq!(
            expand_word("literal-*.txt", &env, &cfg, &[]).unwrap(),
            vec!["literal-visible.txt"]
        );
    }

    #[test]
    fn test_expand_simple_word() {
        let env = fshell_engine::Env::for_command();
        let cfg = ExpansionConfig::default();
        let r = expand_word("hello", &env, &cfg, &[]).unwrap();
        assert_eq!(r, vec!["hello"]);
    }

    #[test]
    fn test_expand_parameter() {
        let env = fshell_engine::Env::for_command();
        env.vars
            .write()
            .insert("FOO".to_string(), Val::String("bar".to_string()));
        let cfg = ExpansionConfig::default();
        let r = expand_word("$FOO", &env, &cfg, &[]).unwrap();
        assert_eq!(r, vec!["bar"]);
    }

    #[test]
    fn test_expand_default_value() {
        let env = fshell_engine::Env::for_command();
        let cfg = ExpansionConfig::default();
        let r = expand_word("${UNSET:-default}", &env, &cfg, &[]).unwrap();
        assert_eq!(r, vec!["default"]);
    }

    #[test]
    fn test_expand_length() {
        let env = fshell_engine::Env::for_command();
        env.vars
            .write()
            .insert("FOO".to_string(), Val::String("hello".to_string()));
        let cfg = ExpansionConfig::default();
        let r = expand_word("${#FOO}", &env, &cfg, &[]).unwrap();
        assert_eq!(r, vec!["5"]);
    }

    #[test]
    fn test_expand_substring() {
        let env = fshell_engine::Env::for_command();
        env.vars
            .write()
            .insert("FOO".to_string(), Val::String("hello".to_string()));
        let cfg = ExpansionConfig::default();
        let r = expand_word("${FOO:1:3}", &env, &cfg, &[]).unwrap();
        assert_eq!(r, vec!["ell"]);
    }

    #[test]
    fn test_expand_prefix_removal() {
        let env = fshell_engine::Env::for_command();
        env.vars
            .write()
            .insert("FOO".to_string(), Val::String("hello".to_string()));
        let cfg = ExpansionConfig::default();
        let r = expand_word("${FOO#hel}", &env, &cfg, &[]).unwrap();
        assert_eq!(r, vec!["lo"]);
    }

    #[test]
    fn test_expand_suffix_removal() {
        let env = fshell_engine::Env::for_command();
        env.vars
            .write()
            .insert("FOO".to_string(), Val::String("hello".to_string()));
        let cfg = ExpansionConfig::default();
        let r = expand_word("${FOO%lo}", &env, &cfg, &[]).unwrap();
        assert_eq!(r, vec!["hel"]);
    }
}
