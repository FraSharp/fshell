// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::{Control, SearchError};
use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{
    BinaryDetection, Encoding, Searcher, SearcherBuilder, Sink, SinkContext, SinkFinish, SinkMatch,
};
use std::io::{self, Read};

#[derive(Clone, Debug, Default)]
pub struct MatchOptions {
    pub fixed: bool,
    pub ignore_case: bool,
    pub word: bool,
    pub whole_line: bool,
}

/// Immutable matcher shared by file workers and the structured-stream adapter.
#[derive(Clone)]
pub struct CompiledMatcher {
    regex: RegexMatcher,
}

impl CompiledMatcher {
    pub fn compile(patterns: &[String], options: &MatchOptions) -> Result<Self, SearchError> {
        if patterns.is_empty() {
            return Err(SearchError::Plan("a pattern is required".into()));
        }
        let regex = RegexMatcherBuilder::new()
            .fixed_strings(options.fixed)
            .case_insensitive(options.ignore_case)
            .word(options.word)
            .whole_line(options.whole_line)
            .line_terminator(Some(b'\n'))
            .build_many(patterns)
            .map_err(|e| SearchError::Plan(e.to_string()))?;
        Ok(Self { regex })
    }

    pub fn is_match(&self, bytes: &[u8]) -> io::Result<bool> {
        self.regex.is_match(bytes).map_err(io::Error::other)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub enum BinaryPolicy {
    #[default]
    Skip,
    Text,
}

#[derive(Clone, Debug, Default)]
pub struct ScanOptions {
    pub invert: bool,
    pub before: usize,
    pub after: usize,
    pub binary: BinaryPolicy,
    pub encoding: Option<String>,
    pub max_matches: Option<u64>,
    pub quiet: bool,
    pub count_only: bool,
}

#[derive(Debug)]
pub enum ScanEvent<'a> {
    Line {
        bytes: &'a [u8],
        line: u64,
        offset: u64,
        matched: bool,
    },
    ContextBreak,
}

#[derive(Default, Debug)]
pub struct ScanSummary {
    pub matches: u64,
    pub binary_offset: Option<u64>,
}

/// Reuse one scanner per worker: buffer allocation is not repeated per file.
pub struct Scanner {
    searcher: Searcher,
    options: ScanOptions,
}

impl Scanner {
    pub fn new(options: ScanOptions) -> Result<Self, SearchError> {
        let encoding = options
            .encoding
            .as_deref()
            .map(Encoding::new)
            .transpose()
            .map_err(|e| SearchError::Plan(e.to_string()))?;
        let mut builder = SearcherBuilder::new();
        builder
            .line_number(true)
            .invert_match(options.invert)
            .before_context(options.before)
            .after_context(options.after)
            .encoding(encoding)
            .bom_sniffing(false)
            .binary_detection(match options.binary {
                BinaryPolicy::Skip => BinaryDetection::quit(b'\0'),
                BinaryPolicy::Text => BinaryDetection::none(),
            });
        Ok(Self {
            searcher: builder.build(),
            options,
        })
    }

    pub fn search(
        &mut self,
        matcher: &CompiledMatcher,
        reader: impl Read,
        control: &Control,
        cancelled: impl Fn() -> bool,
        mut emit: impl FnMut(ScanEvent<'_>) -> io::Result<bool>,
    ) -> io::Result<ScanSummary> {
        if self.options.max_matches == Some(0) || control.is_stopped() || cancelled() {
            return Ok(ScanSummary::default());
        }
        let reader = CancelReader {
            reader,
            control,
            cancelled: &cancelled,
        };
        let mut summary = ScanSummary::default();
        let mut sink = SearchSink {
            options: &self.options,
            summary: &mut summary,
            control,
            cancelled: &cancelled,
            emit: &mut emit,
        };
        self.searcher
            .search_reader(&matcher.regex, reader, &mut sink)?;
        Ok(summary)
    }
}

struct CancelReader<'a, R, C> {
    reader: R,
    control: &'a Control,
    cancelled: &'a C,
}
impl<R: Read, C: Fn() -> bool> Read for CancelReader<'_, R, C> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if self.control.is_stopped() || (self.cancelled)() {
            Ok(0)
        } else {
            self.reader.read(bytes)
        }
    }
}

struct SearchSink<'a, C, E> {
    options: &'a ScanOptions,
    summary: &'a mut ScanSummary,
    control: &'a Control,
    cancelled: &'a C,
    emit: &'a mut E,
}

impl<C: Fn() -> bool, E: FnMut(ScanEvent<'_>) -> io::Result<bool>> Sink for SearchSink<'_, C, E> {
    type Error = io::Error;
    fn matched(&mut self, _: &Searcher, mat: &SinkMatch<'_>) -> io::Result<bool> {
        if self.control.is_stopped() || (self.cancelled)() {
            return Ok(false);
        }
        self.summary.matches += 1;
        if !self.options.count_only
            && !self.options.quiet
            && !(self.emit)(ScanEvent::Line {
                bytes: mat.bytes(),
                line: mat.line_number().unwrap_or(0),
                offset: mat.absolute_byte_offset(),
                matched: true,
            })?
        {
            return Ok(false);
        }
        Ok(!self.options.quiet
            && self
                .options
                .max_matches
                .is_none_or(|max| self.summary.matches < max))
    }
    fn context(&mut self, _: &Searcher, context: &SinkContext<'_>) -> io::Result<bool> {
        if self.control.is_stopped() || (self.cancelled)() {
            return Ok(false);
        }
        if self.options.count_only || self.options.quiet {
            return Ok(true);
        }
        (self.emit)(ScanEvent::Line {
            bytes: context.bytes(),
            line: context.line_number().unwrap_or(0),
            offset: context.absolute_byte_offset(),
            matched: false,
        })
    }
    fn context_break(&mut self, _: &Searcher) -> io::Result<bool> {
        if self.options.count_only || self.options.quiet {
            return Ok(true);
        }
        (self.emit)(ScanEvent::ContextBreak)
    }
    fn finish(&mut self, _: &Searcher, finish: &SinkFinish) -> io::Result<()> {
        self.summary.binary_offset = finish.binary_byte_offset();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffered_scanning_preserves_long_lines_and_context() {
        let matcher =
            CompiledMatcher::compile(&["^Ax*Z$".into()], &MatchOptions::default()).unwrap();
        let mut bytes = b"before\nA".to_vec();
        bytes.extend(std::iter::repeat_n(b'x', 200000));
        bytes.extend_from_slice(b"Z\nafter\n");
        let mut scanner = Scanner::new(ScanOptions {
            before: 1,
            after: 1,
            ..ScanOptions::default()
        })
        .unwrap();
        let mut lines = vec![];
        let summary = scanner
            .search(
                &matcher,
                bytes.as_slice(),
                &Control::default(),
                || false,
                |event| {
                    if let ScanEvent::Line { line, matched, .. } = event {
                        lines.push((line, matched));
                    }
                    Ok(true)
                },
            )
            .unwrap();
        assert_eq!(summary.matches, 1);
        assert_eq!(lines, [(1, false), (2, true), (3, false)]);
    }

    #[test]
    fn regex_errors_are_not_silently_literalized() {
        assert!(CompiledMatcher::compile(&["[".into()], &MatchOptions::default()).is_err());
        assert!(
            CompiledMatcher::compile(
                &["[".into()],
                &MatchOptions {
                    fixed: true,
                    ..MatchOptions::default()
                }
            )
            .unwrap()
            .is_match(b"[")
            .unwrap()
        );
    }

    #[test]
    fn count_and_quiet_do_not_construct_results() {
        let matcher = CompiledMatcher::compile(&["hit".into()], &MatchOptions::default()).unwrap();
        for (quiet, count) in [(false, 2), (true, 1)] {
            let mut scanner = Scanner::new(ScanOptions {
                count_only: true,
                quiet,
                ..ScanOptions::default()
            })
            .unwrap();
            let summary = scanner
                .search(
                    &matcher,
                    &b"hit\nhit\n"[..],
                    &Control::default(),
                    || false,
                    |_| panic!("count allocated a result"),
                )
                .unwrap();
            assert_eq!(summary.matches, count);
        }
    }

    #[test]
    fn cancellation_interrupts_reading_without_a_match() {
        let matcher =
            CompiledMatcher::compile(&["absent".into()], &MatchOptions::default()).unwrap();
        let mut scanner = Scanner::new(ScanOptions::default()).unwrap();
        let summary = scanner
            .search(
                &matcher,
                &b"unmatched\n"[..],
                &Control::default(),
                || true,
                |_| panic!("cancelled search emitted output"),
            )
            .unwrap();
        assert_eq!(summary.matches, 0);
    }
}
