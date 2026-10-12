// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::search_output::{Output, byte_value, search_error, utility_error};
use fshell_core::{FxIndexMap, ShellError, Val};
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use fshell_search::{
    Control, SearchError,
    content::{BinaryPolicy, CompiledMatcher, MatchOptions, ScanEvent, ScanOptions, Scanner},
    framing::LineFramer,
    traversal::{EntrySink, FileEntry, FileKind, TraversalPlan, visit_files, walk_with},
};
use miette::SourceSpan;
use std::collections::VecDeque;
use std::io::{self, IsTerminal};
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use ustr::ustr;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Lines,
    Count,
    Quiet,
    FilesWithMatches,
    FilesWithoutMatches,
}
struct Config {
    patterns: Vec<String>,
    matching: MatchOptions,
    scan: ScanOptions,
    paths: Vec<PathBuf>,
    labels: Vec<PathBuf>,
    recursive: bool,
    stdin: bool,
    follow_links: bool,
    hidden: bool,
    ignore: bool,
    threads: usize,
    sorted: bool,
    mode: Mode,
    number: bool,
    filename: Option<bool>,
    structured: bool,
    text_output: bool,
}

pub fn grep_builtin(
    input: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let mut config =
        parse_args(&args, env).map_err(|e| utility_error("grep", e).maybe_with_span(span))?;
    // Only an otherwise input-less terminal invocation defaults to searching
    // cwd. A pipeline (even an empty one) or redirected stdin remains a stream.
    if config.paths.is_empty() && !config.stdin && input.is_none() && io::stdin().is_terminal() {
        config.paths.push(env.cwd());
        config.labels.push(PathBuf::from("."));
        config.recursive = true;
    }
    let matcher = CompiledMatcher::compile(&config.patterns, &config.matching)
        .map_err(|e| search_error("grep", e).maybe_with_span(span))?;
    let output = Output::new(env, tx);
    let hits = if config.paths.is_empty() {
        stream_search(input, &config, &matcher, &output)?
    } else {
        // Files explicitly supplied to grep replace stdin, just as in Unix grep.
        drop(input);
        files_search(&config, &matcher, &output)
            .map_err(|e| search_error("grep", e).maybe_with_span(span))?
    };
    if hits == 0 && !output.stopped() {
        Err(ShellError::condition_false())
    } else {
        Ok(())
    }
}

fn files_search(
    config: &Config,
    matcher: &CompiledMatcher,
    output: &Output,
) -> Result<u64, SearchError> {
    let control = Arc::new(Control::default());
    let hits = Arc::new(AtomicU64::new(0));
    let mut roots = vec![];
    let mut labels = vec![];
    let mut directories = false;
    for (path, label) in config.paths.iter().zip(&config.labels) {
        output
            .env
            .enforce_capability("grep", fshell_engine::CapAction::ReadFile(path.clone()))
            .map_err(|e| SearchError::Denied(e.to_string()))?;
        let metadata = path.metadata().map_err(|source| SearchError::Io {
            path: path.clone(),
            source,
        })?;
        directories |= metadata.is_dir();
        if metadata.is_dir() && !config.recursive {
            return Err(SearchError::Plan(format!(
                "{} is a directory; use -r",
                path.display()
            )));
        }
        if !metadata.is_dir() && !metadata.is_file() {
            return Err(SearchError::Plan(format!(
                "{} is not a regular file",
                path.display()
            )));
        }
        output.env.track_read(path.clone());
        let root = if metadata.is_dir() && path.is_symlink() {
            path.canonicalize().map_err(|source| SearchError::Io {
                path: path.clone(),
                source,
            })?
        } else {
            path.clone()
        };
        labels.push((root.clone(), label.clone()));
        roots.push(root);
    }
    let mut plan = TraversalPlan::new(roots[0].clone());
    plan.roots = roots;
    plan.follow_links = config.follow_links;
    plan.read_contents = true;
    plan.hidden = config.hidden;
    plan.respect_ignore = config.recursive && config.ignore;
    plan.threads = config.threads;
    plan.sorted = config.sorted;
    let show_filename = config
        .filename
        .unwrap_or(config.paths.len() > 1 || config.recursive);
    let structured = config.structured
        || (!config.text_output && (!output.env.is_last_stage || output.env.is_captured));
    // Validate scanner configuration before starting workers (including encoding).
    let mut scan = config.scan.clone();
    scan.line_numbers = config.number || structured;
    let scanner = Scanner::new(scan)?;
    let make_sink = || FileSink {
        scanner: scanner.clone(),
        matcher,
        config,
        labels: &labels,
        output: output.clone(),
        show_filename,
        structured,
        control: control.clone(),
        hits: hits.clone(),
        buffer: Vec::with_capacity(64 * 1024),
    };
    if directories {
        walk_with(&plan, output.hooks("grep"), control.clone(), make_sink)?;
    } else {
        if config.sorted {
            plan.roots.sort();
        }
        visit_files(
            &plan.roots,
            output.hooks("grep"),
            control.clone(),
            if config.sorted { 1 } else { config.threads },
            make_sink,
        )?;
    }
    Ok(hits.load(Ordering::Relaxed))
}

struct FileSink<'a> {
    scanner: Scanner,
    matcher: &'a CompiledMatcher,
    config: &'a Config,
    labels: &'a [(PathBuf, PathBuf)],
    output: Output,
    show_filename: bool,
    structured: bool,
    control: Arc<Control>,
    hits: Arc<AtomicU64>,
    buffer: Vec<u8>,
}
impl EntrySink for FileSink<'_> {
    fn entry(&mut self, entry: FileEntry) -> Result<bool, SearchError> {
        let open_path = entry.path;
        if entry.kind != FileKind::File
            && (entry.kind != FileKind::Symlink
                || !self.config.paths.contains(&open_path)
                || !open_path.is_file())
        {
            return Ok(true);
        }
        let path = self
            .labels
            .iter()
            .find_map(|(root, label)| {
                open_path.strip_prefix(root).ok().map(|suffix| {
                    if suffix.as_os_str().is_empty() {
                        label.clone()
                    } else {
                        label.join(suffix)
                    }
                })
            })
            .unwrap_or_else(|| open_path.clone());
        let file = std::fs::File::open(&open_path).map_err(|source| SearchError::Io {
            path: path.clone(),
            source,
        })?;
        let output = &self.output;
        let summary = self
            .scanner
            .search(
                self.matcher,
                file,
                &self.control,
                || output.stopped(),
                |event| {
                    if self.structured {
                        if let ScanEvent::Line {
                            bytes,
                            line,
                            offset,
                            matched,
                        } = event
                        {
                            let mut map = FxIndexMap::with_capacity_and_hasher(
                                5,
                                fshell_hash::FxBuildHasher::default(),
                            );
                            map.insert(
                                ustr("file"),
                                byte_value(path.as_os_str().as_encoded_bytes()),
                            );
                            map.insert(ustr("line"), Val::Int(line as i64));
                            map.insert(ustr("offset"), Val::Int(offset as i64));
                            map.insert(ustr("text"), byte_value(without_newline(bytes)));
                            map.insert(ustr("matched"), Val::Bool(matched));
                            return Ok(output.data(Val::Map(map)));
                        }
                    } else {
                        match event {
                            ScanEvent::ContextBreak => self.buffer.extend_from_slice(b"--\n"),
                            ScanEvent::Line {
                                bytes,
                                line,
                                matched,
                                ..
                            } => {
                                let separator = if matched { b':' } else { b'-' };
                                if self.show_filename {
                                    self.buffer
                                        .extend_from_slice(path.as_os_str().as_encoded_bytes());
                                    self.buffer.push(separator);
                                }
                                if self.config.number {
                                    self.buffer.extend_from_slice(line.to_string().as_bytes());
                                    self.buffer.push(separator);
                                }
                                self.buffer.extend_from_slice(bytes);
                                if !bytes.ends_with(b"\n") {
                                    self.buffer.push(b'\n');
                                }
                            }
                        }
                        if self.buffer.len() >= 64 * 1024 {
                            return Ok(output.bytes(std::mem::replace(
                                &mut self.buffer,
                                Vec::with_capacity(64 * 1024),
                            )));
                        }
                    }
                    Ok(!output.stopped())
                },
            )
            .map_err(|source| SearchError::Io {
                path: path.clone(),
                source,
            })?;
        self.hits.fetch_add(summary.matches, Ordering::Relaxed);
        match self.config.mode {
            Mode::Count => {
                if self.structured && self.show_filename {
                    let mut map = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
                    map.insert(
                        ustr("file"),
                        byte_value(path.as_os_str().as_encoded_bytes()),
                    );
                    map.insert(ustr("count"), Val::Int(summary.matches as i64));
                    if !self.output.data(Val::Map(map)) {
                        return Ok(false);
                    }
                } else if self.structured {
                    if !self.output.data(Val::Int(summary.matches as i64)) {
                        return Ok(false);
                    }
                } else {
                    if self.show_filename {
                        self.buffer
                            .extend_from_slice(path.as_os_str().as_encoded_bytes());
                        self.buffer.push(b':');
                    }
                    self.buffer
                        .extend_from_slice(format!("{}\n", summary.matches).as_bytes());
                }
            }
            Mode::Quiet if summary.matches > 0 => {
                self.control.stop();
                return Ok(false);
            }
            Mode::FilesWithMatches | Mode::FilesWithoutMatches => {
                let selected =
                    (summary.matches > 0) == (self.config.mode == Mode::FilesWithMatches);
                if selected {
                    if self.structured {
                        if !self
                            .output
                            .data(byte_value(path.as_os_str().as_encoded_bytes()))
                        {
                            return Ok(false);
                        }
                    } else {
                        self.buffer
                            .extend_from_slice(path.as_os_str().as_encoded_bytes());
                        self.buffer.push(b'\n');
                    }
                }
            }
            _ => {}
        }
        Ok(!self.output.stopped())
    }
    fn finish(&mut self) -> Result<(), SearchError> {
        if !self.buffer.is_empty() {
            self.output.bytes(std::mem::take(&mut self.buffer));
        }
        Ok(())
    }
}

fn without_newline(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

/// Record-aware stream selection. Only byte payloads use framing; structured
/// records keep their identity. Count/quiet modes never allocate per-match Vals.
struct Stream<'a> {
    config: &'a Config,
    matcher: &'a CompiledMatcher,
    output: &'a Output,
    hits: u64,
    line: u64,
    before: VecDeque<(u64, Arc<Val>)>,
    after: usize,
    last_emitted: u64,
}
impl Stream<'_> {
    fn matching_value(&self, value: &Val) -> io::Result<bool> {
        match value {
            Val::String(s) => self.matcher.is_match(s.as_bytes()),
            Val::Blob(b) => self.matcher.is_match(b),
            Val::Map(map) => {
                for value in map.values() {
                    if self.matching_value(value)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Val::List(values) => {
                for value in values {
                    if self.matching_value(value)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            _ => Ok(false),
        }
    }
    fn bytes(&mut self, bytes: &[u8]) -> io::Result<bool> {
        let bytes = without_newline(bytes);
        let selected = self.matcher.is_match(bytes)? != self.config.scan.invert;
        if matches!(self.config.scan.binary, BinaryPolicy::Skip) && bytes.contains(&0) {
            return Ok(false);
        }
        // Count/quiet avoid even constructing values for selected lines.
        if matches!(self.config.mode, Mode::Count | Mode::Quiet) {
            self.line += 1;
            if selected {
                self.hits += 1;
            }
            return Ok(self.keep_going());
        }
        if !selected && self.config.scan.before == 0 && self.after == 0 {
            self.line += 1;
            return Ok(self.keep_going());
        }
        self.record(Arc::new(byte_value(bytes)), Some(selected))
    }
    fn record(&mut self, value: Arc<Val>, selected: Option<bool>) -> io::Result<bool> {
        self.line += 1;
        let capped = self
            .config
            .scan
            .max_matches
            .is_some_and(|max| self.hits >= max);
        let selected = !capped
            && match selected {
                Some(selected) => selected,
                None => self.matching_value(&value)? != self.config.scan.invert,
            };
        if selected {
            self.hits += 1;
            if self.config.mode == Mode::Quiet {
                return Ok(false);
            }
            if self.config.mode == Mode::Count {
                return Ok(self.keep_going());
            }
            let earliest = self.before.front().map_or(self.line, |(line, _)| *line);
            if self.last_emitted > 0
                && earliest > self.last_emitted + 1
                && (self.config.scan.before > 0 || self.config.scan.after > 0)
                && !self.output.data(Val::String("--".into()))
            {
                return Ok(false);
            }
            while let Some((line, value)) = self.before.pop_front() {
                if line > self.last_emitted && !self.emit(line, value, false) {
                    return Ok(false);
                }
            }
            if !self.emit(self.line, value, true) {
                return Ok(false);
            }
            self.after = self.config.scan.after;
        } else if self.after > 0 {
            if !self.emit(self.line, value, false) {
                return Ok(false);
            }
            self.after -= 1;
        } else if self.config.scan.before > 0
            && !matches!(self.config.mode, Mode::Count | Mode::Quiet)
        {
            self.before.push_back((self.line, value));
            if self.before.len() > self.config.scan.before {
                self.before.pop_front();
            }
        }
        Ok(self.keep_going() || self.after > 0)
    }
    fn keep_going(&self) -> bool {
        !self.output.stopped()
            && !(self.config.mode == Mode::Quiet && self.hits > 0)
            && self
                .config
                .scan
                .max_matches
                .is_none_or(|max| self.hits < max)
    }
    fn emit(&mut self, line: u64, value: Arc<Val>, matched: bool) -> bool {
        self.last_emitted = line;
        if self.config.text_output
            || (self.config.number
                && self.output.env.is_last_stage
                && !self.output.env.is_captured
                && !self.config.structured)
        {
            let mut bytes = Vec::new();
            if self.config.number {
                bytes.extend_from_slice(line.to_string().as_bytes());
                bytes.push(if matched { b':' } else { b'-' });
            }
            match &*value {
                Val::Blob(blob) => bytes.extend_from_slice(blob),
                other => bytes.extend_from_slice(other.to_text().as_bytes()),
            }
            bytes.push(b'\n');
            return self.output.bytes(bytes);
        }
        if self.config.number {
            let mut map = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            map.insert(ustr("line"), Val::Int(line as i64));
            map.insert(ustr("text"), (*value).clone());
            self.output.data(Val::Map(map))
        } else {
            self.output.send(PipelinePayload::Data(value))
        }
    }
}

fn stream_search(
    input: Option<PipeStream>,
    config: &Config,
    matcher: &CompiledMatcher,
    output: &Output,
) -> Result<u64, ShellError> {
    if matches!(
        config.mode,
        Mode::FilesWithMatches | Mode::FilesWithoutMatches
    ) {
        return Err(utility_error("grep", "filename modes require files"));
    }
    if config.scan.encoding.is_some() {
        return Err(utility_error("grep", "--encoding requires file input"));
    }
    let mut stream = Stream {
        config,
        matcher,
        output,
        hits: 0,
        line: 0,
        before: VecDeque::new(),
        after: 0,
        last_emitted: 0,
    };
    if let Some(mut input) = input {
        let mut pending = None;
        let mut scanner = Scanner::new(config.scan.clone()).map_err(|e| search_error("grep", e))?;
        let control = Control::default();
        let mut framer = LineFramer::default();
        let mut error = None;
        let mut running = config.scan.max_matches != Some(0);
        while running {
            let payload = pending.take().or_else(|| output.receive(&mut input));
            let Some(payload) = payload else {
                break;
            };
            match payload {
                PipelinePayload::Bytes(bytes)
                    if matches!(config.mode, Mode::Count | Mode::Quiet) =>
                {
                    scanner.set_max_matches(
                        config
                            .scan
                            .max_matches
                            .map(|max| max.saturating_sub(stream.hits)),
                    );
                    let reader = crate::search_input::ByteSegment::new(
                        &mut input,
                        output,
                        &mut pending,
                        bytes,
                    );
                    let summary = scanner
                        .search(matcher, reader, &control, || output.stopped(), |_| Ok(true))
                        .map_err(|e| utility_error("grep", e))?;
                    stream.hits += summary.matches;
                    running = stream.keep_going() && summary.binary_offset.is_none();
                }
                PipelinePayload::Bytes(bytes) => {
                    running = framer.push(&bytes, |line| match stream.bytes(line) {
                        Ok(go) => go,
                        Err(e) => {
                            error = Some(e);
                            false
                        }
                    });
                }
                PipelinePayload::Data(value) => {
                    running = framer.finish(|line| match stream.bytes(line) {
                        Ok(go) => go,
                        Err(e) => {
                            error = Some(e);
                            false
                        }
                    });
                    if !running {
                        break;
                    }
                    if let Val::String(text) = &*value {
                        if text.contains('\n') {
                            for line in text.split_inclusive('\n') {
                                running = stream
                                    .bytes(line.as_bytes())
                                    .map_err(|e| utility_error("grep", e))?;
                                if !running {
                                    break;
                                }
                            }
                        } else {
                            running = stream
                                .record(value, None)
                                .map_err(|e| utility_error("grep", e))?;
                        }
                    } else {
                        running = stream
                            .record(value, None)
                            .map_err(|e| utility_error("grep", e))?;
                    }
                }
                PipelinePayload::Structured(diag) => {
                    if !output.send(PipelinePayload::Structured(diag)) {
                        break;
                    }
                }
            }
        }
        if running {
            framer.finish(|line| match stream.bytes(line) {
                Ok(go) => go,
                Err(e) => {
                    error = Some(e);
                    false
                }
            });
        }
        if let Some(e) = error {
            return Err(utility_error("grep", e));
        }
    } else {
        // Keep stdin reads cancellable even when the terminal or an upstream
        // writer has no complete line available. The scanner supplies buffering.
        let stdin = io::stdin();
        let stdin = stdin.lock();
        let reader = crate::search_input::InterruptibleReader::new(stdin.as_fd(), output);
        let mut scanner = Scanner::new(config.scan.clone()).map_err(|e| search_error("grep", e))?;
        let summary = scanner
            .search(
                matcher,
                reader,
                &Control::default(),
                || output.stopped(),
                |event| match event {
                    ScanEvent::Line {
                        bytes,
                        line,
                        matched,
                        ..
                    } => Ok(stream.emit(
                        line,
                        Arc::new(byte_value(without_newline(bytes))),
                        matched,
                    )),
                    ScanEvent::ContextBreak => Ok(output.data(Val::String("--".into()))),
                },
            )
            .map_err(|e| utility_error("grep", e))?;
        stream.hits = summary.matches;
    }
    if config.mode == Mode::Count {
        output.data(Val::Int(stream.hits as i64));
    }
    Ok(stream.hits)
}

fn parse_args(args: &[Val], env: &Env) -> Result<Config, String> {
    let args: Vec<String> = args
        .iter()
        .map(|arg| match arg {
            Val::String(s) => Ok(s.clone()),
            Val::Int(n) => Ok(n.to_string()),
            _ => Err("arguments must be strings or integers".into()),
        })
        .collect::<Result<_, String>>()?;
    let mut config = Config {
        patterns: vec![],
        matching: MatchOptions::default(),
        scan: ScanOptions::default(),
        paths: vec![],
        labels: vec![],
        recursive: false,
        stdin: false,
        follow_links: false,
        hidden: false,
        ignore: true,
        threads: std::thread::available_parallelism().map_or(1, |n| n.get().min(8)),
        sorted: false,
        mode: Mode::Lines,
        number: false,
        filename: None,
        structured: false,
        text_output: false,
    };
    let mut positional = vec![];
    let mut options = true;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if options && arg == "--" {
            options = false;
            continue;
        }
        if options && arg.starts_with("--") {
            let (flag, inline) = arg
                .split_once('=')
                .map_or((arg.as_str(), None), |(f, v)| (f, Some(v)));
            match flag {
                "--fixed-strings" => config.matching.fixed = true,
                "--ignore-case" => config.matching.ignore_case = true,
                "--invert-match" => config.scan.invert = true,
                "--line-number" => config.number = true,
                "--with-filename" => config.filename = Some(true),
                "--no-filename" => config.filename = Some(false),
                "--count" => config.mode = Mode::Count,
                "--quiet" => config.mode = Mode::Quiet,
                "--recursive" => config.recursive = true,
                "--stdin" => config.stdin = true,
                "--hidden" => config.hidden = true,
                "--no-ignore" => config.ignore = false,
                "--sort" => config.sorted = true,
                "--structured" => config.structured = true,
                "--text-output" => config.text_output = true,
                "--text" => config.scan.binary = BinaryPolicy::Text,
                "--word-regexp" => config.matching.word = true,
                "--line-regexp" => config.matching.whole_line = true,
                "--regexp" | "--encoding" | "--threads" | "--max-count" | "--before-context"
                | "--after-context" | "--context" => {
                    let value = if let Some(v) = inline {
                        v
                    } else {
                        let v = args
                            .get(i)
                            .ok_or_else(|| format!("{flag} requires a value"))?;
                        i += 1;
                        v
                    };
                    option_value(&mut config, flag, value)?;
                }
                _ => return Err(format!("unknown option {flag}")),
            }
            if inline.is_some()
                && !matches!(
                    flag,
                    "--regexp"
                        | "--encoding"
                        | "--threads"
                        | "--max-count"
                        | "--before-context"
                        | "--after-context"
                        | "--context"
                )
            {
                return Err(format!("{flag} does not take a value"));
            }
        } else if options && arg.starts_with('-') && arg != "-" {
            let flags: Vec<(usize, char)> = arg.char_indices().skip(1).collect();
            for (offset, flag) in flags {
                match flag {
                    'E' => {}
                    'F' => config.matching.fixed = true,
                    'i' => config.matching.ignore_case = true,
                    'v' => config.scan.invert = true,
                    'n' => config.number = true,
                    'H' => config.filename = Some(true),
                    'h' => config.filename = Some(false),
                    'c' => config.mode = Mode::Count,
                    'q' => config.mode = Mode::Quiet,
                    'l' => config.mode = Mode::FilesWithMatches,
                    'L' => config.mode = Mode::FilesWithoutMatches,
                    'r' => config.recursive = true,
                    'R' => {
                        config.recursive = true;
                        config.follow_links = true;
                    }
                    'a' => config.scan.binary = BinaryPolicy::Text,
                    'w' => config.matching.word = true,
                    'x' => config.matching.whole_line = true,
                    'e' | 'm' | 'A' | 'B' | 'C' | 'j' => {
                        let tail = &arg[offset + flag.len_utf8()..];
                        let value = if tail.is_empty() {
                            let v = args
                                .get(i)
                                .ok_or_else(|| format!("-{flag} requires a value"))?;
                            i += 1;
                            v
                        } else {
                            tail
                        };
                        let name = match flag {
                            'e' => "--regexp",
                            'm' => "--max-count",
                            'A' => "--after-context",
                            'B' => "--before-context",
                            'C' => "--context",
                            _ => "--threads",
                        };
                        option_value(&mut config, name, value)?;
                        break;
                    }
                    _ => return Err(format!("unknown option -{flag}")),
                }
            }
        } else {
            positional.push(arg.clone());
        }
    }
    if config.patterns.is_empty() {
        if positional.is_empty() {
            return Err("a pattern is required".into());
        }
        config.patterns.push(positional.remove(0));
    }
    if config.stdin && !positional.is_empty() {
        return Err("--stdin cannot be combined with file paths".into());
    }
    config.paths = positional
        .iter()
        .map(|path| env.resolve_path(path))
        .collect();
    config.labels = positional.iter().map(PathBuf::from).collect();
    config.scan.quiet = matches!(config.mode, Mode::Quiet | Mode::FilesWithMatches);
    config.scan.count_only = config.mode != Mode::Lines;
    Ok(config)
}
fn option_value(config: &mut Config, option: &str, value: &str) -> Result<(), String> {
    match option {
        "--regexp" => config.patterns.push(value.into()),
        "--encoding" => config.scan.encoding = Some(value.into()),
        "--max-count" => {
            config.scan.max_matches = Some(
                value
                    .parse()
                    .map_err(|_| format!("invalid count {value}"))?,
            )
        }
        _ => {
            let n: usize = value
                .parse()
                .map_err(|_| format!("invalid nonnegative integer {value}"))?;
            match option {
                "--threads" => {
                    if n == 0 {
                        return Err("worker count must be positive".into());
                    }
                    config.threads = n;
                }
                "--before-context" => config.scan.before = n,
                "--after-context" => config.scan.after = n,
                "--context" => {
                    config.scan.before = n;
                    config.scan.after = n;
                }
                _ => return Err(format!("unknown option {option}")),
            }
        }
    }
    Ok(())
}
