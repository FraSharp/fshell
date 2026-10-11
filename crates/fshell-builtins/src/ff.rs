// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::search_output::{Output, byte_value, search_error, utility_error};
use chrono::{DateTime, Utc};
use fshell_core::{FxIndexMap, ShellError, Val};
use fshell_engine::{Env, PipeSender, PipeStream};
use fshell_search::{
    Control, SearchError,
    traversal::{Compare, EntrySink, FileEntry, FileKind, NamePredicate, TraversalPlan, walk_with},
};
use miette::SourceSpan;
#[cfg(any(feature = "replace", test))]
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;
use ustr::ustr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Records,
    Paths,
    Count,
}

struct Config {
    plan: TraversalPlan,
    mode: Mode,
    null: bool,
}

/// Synchronous registration deliberately puts this whole operation on the
/// engine blocking pool. Traversal workers are scoped and joined, not spawned
/// into detached Tokio tasks.
pub fn ff_builtin(
    _input: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let config =
        parse_args(&args, env).map_err(|e| utility_error("ff", e).maybe_with_span(span))?;
    let output = Output::new(env, tx);
    for path in &config.plan.roots {
        env.track_read(path.clone());
    }
    let count = Arc::new(AtomicU64::new(0));
    let terminal_paths = config.mode == Mode::Paths && env.is_last_stage && !env.is_captured;
    walk_with(
        &config.plan,
        output.hooks("ff"),
        Arc::new(Control::default()),
        || FinderSink {
            output: output.clone(),
            mode: config.mode,
            raw_paths: terminal_paths || config.null,
            delimiter: if config.null { b'\0' } else { b'\n' },
            buffer: Vec::with_capacity(64 * 1024),
            count: count.clone(),
        },
    )
    .map_err(|e| search_error("ff", e).maybe_with_span(span))?;
    if config.mode == Mode::Count {
        output.data(Val::Int(count.load(Ordering::Relaxed) as i64));
    }
    Ok(())
}

struct FinderSink {
    output: Output,
    mode: Mode,
    raw_paths: bool,
    delimiter: u8,
    buffer: Vec<u8>,
    count: Arc<AtomicU64>,
}

impl EntrySink for FinderSink {
    fn entry(&mut self, entry: FileEntry) -> Result<bool, SearchError> {
        if self.output.stopped() {
            return Ok(false);
        }
        match self.mode {
            Mode::Count => {
                self.count.fetch_add(1, Ordering::Relaxed);
                Ok(true)
            }
            Mode::Paths if self.raw_paths => {
                self.buffer
                    .extend_from_slice(entry.path.as_os_str().as_encoded_bytes());
                self.buffer.push(self.delimiter);
                if self.buffer.len() >= 64 * 1024 {
                    Ok(self.flush())
                } else {
                    Ok(true)
                }
            }
            Mode::Paths => Ok(self
                .output
                .data(byte_value(entry.path.as_os_str().as_encoded_bytes()))),
            Mode::Records => {
                let metadata = entry
                    .metadata
                    .ok_or_else(|| SearchError::Plan("record output requires metadata".into()))?;
                let path = byte_value(entry.path.as_os_str().as_encoded_bytes());
                let name = byte_value(
                    entry
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .as_encoded_bytes(),
                );
                let kind = match entry.kind {
                    FileKind::File => "file",
                    FileKind::Directory => "dir",
                    FileKind::Symlink => "symlink",
                    FileKind::Other => "other",
                };
                let modified: DateTime<Utc> = metadata.modified.into();
                let size = i64::try_from(metadata.size).map_err(|_| {
                    SearchError::Plan("file size exceeds shell integer range".into())
                })?;
                let mut map =
                    FxIndexMap::with_capacity_and_hasher(6, fshell_hash::FxBuildHasher::default());
                map.insert(ustr("path"), path.clone());
                map.insert(ustr("abs_path"), path);
                map.insert(ustr("name"), name);
                map.insert(ustr("type"), Val::String(kind.into()));
                map.insert(ustr("size"), Val::Int(size));
                map.insert(ustr("modified"), Val::String(modified.to_rfc3339()));
                Ok(self.output.data(Val::Map(map)))
            }
        }
    }
    fn finish(&mut self) -> Result<(), SearchError> {
        self.flush();
        Ok(())
    }
}
impl FinderSink {
    fn flush(&mut self) -> bool {
        if self.buffer.is_empty() {
            return true;
        }
        self.output.bytes(std::mem::replace(
            &mut self.buffer,
            Vec::with_capacity(64 * 1024),
        ))
    }
}

fn parse_args(args: &[Val], env: &Env) -> Result<Config, String> {
    let args: Vec<String> = args
        .iter()
        .map(|v| match v {
            Val::String(s) => Ok(s.clone()),
            Val::Int(n) => Ok(n.to_string()),
            Val::Float(n) => Ok(n.to_string()),
            _ => Err("arguments must be strings or numbers".into()),
        })
        .collect::<Result<_, String>>()?;
    let mut config = Config {
        plan: TraversalPlan::new(env.cwd()),
        mode: Mode::Records,
        null: false,
    };
    let mut roots = vec![];
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        match arg.as_str() {
            "--" => {
                roots.extend(args[i..].iter().map(|p| env.resolve_path(p)));
                break;
            }
            "and" => {}
            "name" | "size" | "modified" | "type" => {
                let operator = args
                    .get(i)
                    .ok_or_else(|| format!("{arg} requires an operator and value"))?;
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{arg} requires an operator and value"))?;
                i += 2;
                let op = comparison(operator)?;
                match arg.as_str() {
                    "name" => {
                        if !matches!(op, Compare::Equal | Compare::NotEqual) {
                            return Err("name supports only = and !=".into());
                        }
                        config.plan.names.push(
                            NamePredicate::compile(value, matches!(op, Compare::NotEqual), false)
                                .map_err(|e| e.to_string())?,
                        );
                    }
                    "size" => config.plan.sizes.push((op, quantity(value, true)?)),
                    "modified" => config
                        .plan
                        .ages
                        .push((op, Duration::from_secs(quantity(value, false)?))),
                    "type" => {
                        if !matches!(op, Compare::Equal) {
                            return Err("type supports only =".into());
                        }
                        let kind = match value.as_str() {
                            "file" => FileKind::File,
                            "dir" => FileKind::Directory,
                            "symlink" => FileKind::Symlink,
                            "other" => FileKind::Other,
                            _ => return Err(format!("unknown file type {value}")),
                        };
                        if config.plan.kind.replace(kind).is_some() {
                            return Err("type may only be specified once".into());
                        }
                    }
                    _ => unreachable!(),
                }
            }
            "hidden" | "--hidden" | "-H" => config.plan.hidden = true,
            "--no-ignore" => config.plan.respect_ignore = false,
            "--sort" => config.plan.sorted = true,
            "--follow" | "-L" => config.plan.follow_links = true,
            "--paths" => config.mode = Mode::Paths,
            "--count" => config.mode = Mode::Count,
            "--print0" | "-0" => {
                config.mode = Mode::Paths;
                config.null = true;
            }
            "--depth" | "-n" | "--max-results" | "-j" | "--threads" => {
                let value = args
                    .get(i)
                    .ok_or_else(|| format!("{arg} requires a number"))?;
                i += 1;
                let number = value
                    .parse::<usize>()
                    .map_err(|_| format!("invalid nonnegative integer {value}"))?;
                match arg.as_str() {
                    "--depth" => config.plan.max_depth = Some(number),
                    "-j" | "--threads" => {
                        if number == 0 {
                            return Err("worker count must be positive".into());
                        }
                        config.plan.threads = number;
                    }
                    _ => config.plan.max_results = Some(number),
                }
            }
            flag if flag.starts_with('-') => return Err(format!("unknown option {flag}")),
            path => roots.push(env.resolve_path(path)),
        }
    }
    if !roots.is_empty() {
        config.plan.roots = roots;
    }
    config.plan.metadata = config.mode == Mode::Records;
    Ok(config)
}

fn comparison(op: &str) -> Result<Compare, String> {
    match op {
        "<" => Ok(Compare::Less),
        "<=" => Ok(Compare::LessEqual),
        "=" | "==" => Ok(Compare::Equal),
        "!=" => Ok(Compare::NotEqual),
        ">=" => Ok(Compare::GreaterEqual),
        ">" => Ok(Compare::Greater),
        _ => Err(format!("unknown comparison {op}")),
    }
}

/// Exact checked decimal arithmetic: no NaN, negative values, floating-point
/// saturation, truncation of fractional units, or boundary +/-1 overflow.
fn quantity(raw: &str, size: bool) -> Result<u64, String> {
    let raw = raw.trim().to_ascii_lowercase();
    let split = raw
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(raw.len());
    let (number, unit) = raw.split_at(split);
    let multiplier: u128 = if size {
        match unit {
            "" | "b" => 1,
            "k" | "kb" => 1000,
            "m" | "mb" => 1_000_000,
            "g" | "gb" => 1_000_000_000,
            "kib" => 1024,
            "mib" => 1024 * 1024,
            "gib" => 1024 * 1024 * 1024,
            _ => return Err(format!("unknown size unit {unit}")),
        }
    } else {
        match unit {
            "" | "s" => 1,
            "m" | "min" => 60,
            "h" => 3600,
            "d" => 86400,
            "w" => 604800,
            _ => return Err(format!("unknown duration unit {unit}")),
        }
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty() || fraction.contains('.') {
        return Err(format!("invalid quantity {raw}"));
    }
    let whole = whole
        .parse::<u128>()
        .map_err(|_| format!("invalid quantity {raw}"))?;
    let scale = 10u128
        .checked_pow(fraction.len().try_into().map_err(|_| "quantity overflow")?)
        .ok_or("quantity overflow")?;
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<u128>()
            .map_err(|_| format!("invalid quantity {raw}"))?
    };
    let numerator = whole
        .checked_mul(scale)
        .and_then(|n| n.checked_add(fraction))
        .and_then(|n| n.checked_mul(multiplier))
        .ok_or("quantity overflow")?;
    if numerator % scale != 0 {
        return Err("quantity must resolve to whole bytes or seconds".into());
    }
    u64::try_from(numerator / scale).map_err(|_| "quantity overflow".into())
}

// Shared by replace's existing frontend; search plans themselves compile once.
#[cfg(any(feature = "replace", test))]
pub(crate) fn glob_match(pattern: &str, name: &str) -> bool {
    NamePredicate::compile(pattern, false, false).is_ok_and(|p| p.matches(&PathBuf::from(name)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quantities_are_exact_and_checked() {
        assert_eq!(quantity("1.5mb", true).unwrap(), 1_500_000);
        assert_eq!(quantity("1mib", true).unwrap(), 1_048_576);
        assert_eq!(quantity("30min", false).unwrap(), 1800);
        for bad in ["-1", "NaN", "inf", "1.5b", "18446744073709551616"] {
            assert!(quantity(bad, true).is_err(), "{bad}");
        }
    }
    #[test]
    fn parse_errors_do_not_ignore_unknown_options() {
        let env = Env::new();
        assert!(parse_args(&[Val::String("--typo".into())], &env).is_err());
        assert!(parse_args(&[Val::String("size".into())], &env).is_err());
        let config = parse_args(
            &[
                Val::String("-n".into()),
                Val::Int(0),
                Val::String("--paths".into()),
            ],
            &env,
        )
        .unwrap();
        assert_eq!(config.plan.max_results, Some(0));
        assert!(!config.plan.metadata);
    }
    #[test]
    fn filename_classes_are_supported() {
        assert!(glob_match("[ab]*.rs", "a.rs"));
    }
}
