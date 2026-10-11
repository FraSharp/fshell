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

#[derive(Clone, Debug)]
pub struct ScanOptions {
    /// Report line positions for record/numbered output. Raw consumers can omit
    /// this work; trailing context still enables positions for limit handling.
    pub line_numbers: bool,
    pub invert: bool,
    pub before: usize,
    pub after: usize,
    pub binary: BinaryPolicy,
    pub encoding: Option<String>,
    pub max_matches: Option<u64>,
    pub quiet: bool,
    pub count_only: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            line_numbers: true,
            invert: false,
            before: 0,
            after: 0,
            binary: BinaryPolicy::default(),
            encoding: None,
            max_matches: None,
            quiet: false,
            count_only: false,
        }
    }
}

#[derive(Debug)]
pub enum ScanEvent<'a> {
    Line {
        bytes: &'a [u8],
        /// Zero when line positions were not requested.
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
#[derive(Clone)]
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
            .line_number(
                !options.count_only
                    && !options.quiet
                    && (options.line_numbers || options.after > 0),
            )
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

    pub fn set_max_matches(&mut self, max: Option<u64>) {
        self.options.max_matches = max;
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
            stop_after_line: None,
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
    stop_after_line: Option<u64>,
}

impl<C: Fn() -> bool, E: FnMut(ScanEvent<'_>) -> io::Result<bool>> Sink for SearchSink<'_, C, E> {
    type Error = io::Error;
    fn matched(&mut self, _: &Searcher, mat: &SinkMatch<'_>) -> io::Result<bool> {
        if self.control.is_stopped() || (self.cancelled)() {
            return Ok(false);
        }
        let line = mat.line_number().unwrap_or(0);
        if let Some(last) = self.stop_after_line {
            if line > last {
                return Ok(false);
            }
            let keep = (self.emit)(ScanEvent::Line {
                bytes: mat.bytes(),
                line,
                offset: mat.absolute_byte_offset(),
                matched: false,
            })?;
            return Ok(keep && line < last);
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
        if self.options.quiet {
            return Ok(false);
        }
        if self
            .options
            .max_matches
            .is_some_and(|max| self.summary.matches >= max)
        {
            if self.options.count_only || self.options.after == 0 {
                return Ok(false);
            }
            self.stop_after_line = Some(line.saturating_add(self.options.after as u64));
        }
        Ok(true)
    }
    fn context(&mut self, _: &Searcher, context: &SinkContext<'_>) -> io::Result<bool> {
        if self.control.is_stopped() || (self.cancelled)() {
            return Ok(false);
        }
        if self.options.count_only || self.options.quiet {
            return Ok(true);
        }
        let line = context.line_number().unwrap_or(0);
        if self.stop_after_line.is_some_and(|last| line > last) {
            return Ok(false);
        }
        let keep = (self.emit)(ScanEvent::Line {
            bytes: context.bytes(),
            line,
            offset: context.absolute_byte_offset(),
            matched: false,
        })?;
        Ok(keep && self.stop_after_line.is_none_or(|last| line < last))
    }
    fn context_break(&mut self, _: &Searcher) -> io::Result<bool> {
        if self.stop_after_line.is_some() {
            return Ok(false);
        }
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
        let matcher = CompiledMatcher::compile(&["^Ax*Z$".into()], &MatchOptions::default())
            .expect("long-line matcher must compile");
        let mut bytes = b"before\nA".to_vec();
        bytes.extend(std::iter::repeat_n(b'x', 200000));
        bytes.extend_from_slice(b"Z\nafter\n");
        let mut scanner = Scanner::new(ScanOptions {
            before: 1,
            after: 1,
            ..ScanOptions::default()
        })
        .expect("context scanner must initialize");
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
            .expect("context scan must succeed");
        assert_eq!(summary.matches, 1);
        assert_eq!(lines, [(1, false), (2, true), (3, false)]);
    }

    #[test]
    fn raw_scans_omit_positions_but_match_limits_keep_trailing_context() {
        let matcher = CompiledMatcher::compile(&["hit".into()], &MatchOptions::default())
            .expect("fixture matcher must compile");
        for after in [0, 1] {
            let mut scanner = Scanner::new(ScanOptions {
                line_numbers: false,
                after,
                max_matches: Some(1),
                ..ScanOptions::default()
            })
            .expect("fixture scanner must initialize");
            let mut positions = Vec::new();
            let summary = scanner
                .search(
                    &matcher,
                    &b"hit\nhit\nlast\n"[..],
                    &Control::default(),
                    || false,
                    |event| {
                        if let ScanEvent::Line { line, matched, .. } = event {
                            positions.push((line, matched));
                        }
                        Ok(true)
                    },
                )
                .expect("raw scan must succeed");
            assert_eq!(summary.matches, 1);
            assert_eq!(
                positions,
                if after == 0 {
                    vec![(0, true)]
                } else {
                    vec![(1, true), (2, false)]
                }
            );
        }
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
            .expect("fixed bracket matcher must compile")
            .is_match(b"[")
            .expect("fixed matching must succeed")
        );
    }

    #[test]
    fn count_and_quiet_do_not_construct_results() {
        let matcher = CompiledMatcher::compile(&["hit".into()], &MatchOptions::default())
            .expect("count matcher must compile");
        for (quiet, count) in [(false, 2), (true, 1)] {
            let mut scanner = Scanner::new(ScanOptions {
                count_only: true,
                quiet,
                ..ScanOptions::default()
            })
            .expect("count scanner must initialize");
            let summary = scanner
                .search(
                    &matcher,
                    &b"hit\nhit\n"[..],
                    &Control::default(),
                    || false,
                    |_| Err(io::Error::other("count allocated a result")),
                )
                .expect("count scan must succeed without constructing results");
            assert_eq!(summary.matches, count);
        }
    }

    #[test]
    fn cancellation_interrupts_reading_without_a_match() {
        let matcher = CompiledMatcher::compile(&["absent".into()], &MatchOptions::default())
            .expect("absent matcher must compile");
        let mut scanner = Scanner::new(ScanOptions::default()).expect("scanner must initialize");
        let summary = scanner
            .search(
                &matcher,
                &b"unmatched\n"[..],
                &Control::default(),
                || true,
                |_| Err(io::Error::other("cancelled search emitted output")),
            )
            .expect("cancelled scan must stop without output");
        assert_eq!(summary.matches, 0);
    }
}
