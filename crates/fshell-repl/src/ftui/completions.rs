// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::FshellCompleter;
use crate::autocomplete::ranking::{RankedCompletion, rank_candidates};
use crate::autocomplete::{Completer, CompletionCandidate, CompletionKind, TextSpan};
use crate::theme_ext::ThemeColorRatatui;
use fshell_core::theme::{CompletionsTheme, Theme};
use fshell_engine::Env;
use lscolors::{LsColors, Style as LsStyle};
use nucleo_matcher::{Config as NucleoConfig, Matcher};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier as StyleModifier, Style};
use ratatui::text::{Line, Span};
use std::ops::Range;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Truncate a string to a maximum display width, appending "…" if truncated.
pub fn truncate_by_width(s: &str, max_width: usize) -> String {
    if s.width() <= max_width {
        return s.to_string();
    }
    if max_width < 2 {
        return "…".to_string();
    }
    let mut out = String::with_capacity(max_width);
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw > max_width - 1 {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// Format byte length into human-readable size string.
pub fn format_file_size(len: u64) -> String {
    if len < 1024 {
        format!("{len} B")
    } else if len < 1024 * 1024 {
        format!("{:.1} KB", len as f64 / 1024.0)
    } else if len < 1024 * 1024 * 1024 {
        format!("{:.1} MB", len as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", len as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Layout mode kind chosen by completion context
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionLayoutKind {
    List,
    Grid,
}

/// Resolved concrete layout configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionLayout {
    List,
    Grid { cols: usize, col_width: usize },
}

/// Category groups for rendering completions with clean textual badges
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionCategory {
    Directory,
    File,
    Command,
    Builtin,
    Alias,
    Function,
    Variable,
    Job,
    Flag,
    Pipeline,
    Keyword,
    History,
    Ref,
}

impl CompletionCategory {
    pub fn label(self) -> &'static str {
        match self {
            CompletionCategory::Directory => "dir",
            CompletionCategory::File => "file",
            CompletionCategory::Command => "cmd",
            CompletionCategory::Builtin => "builtin",
            CompletionCategory::Alias => "alias",
            CompletionCategory::Function => "fn",
            CompletionCategory::Variable => "var",
            CompletionCategory::Job => "job",
            CompletionCategory::Flag => "flag",
            CompletionCategory::Pipeline => "pipe",
            CompletionCategory::Keyword => "keyword",
            CompletionCategory::History => "history",
            CompletionCategory::Ref => "branch",
        }
    }

    pub fn badge(self) -> &'static str {
        self.label()
    }

    pub fn icon(self) -> &'static str {
        ""
    }

    pub fn name(self) -> &'static str {
        match self {
            CompletionCategory::Directory => "Directory",
            CompletionCategory::File => "File",
            CompletionCategory::Command => "Command",
            CompletionCategory::Builtin => "Builtin",
            CompletionCategory::Alias => "Alias",
            CompletionCategory::Function => "Function",
            CompletionCategory::Variable => "Variable",
            CompletionCategory::Job => "Job",
            CompletionCategory::Flag => "Flag",
            CompletionCategory::Pipeline => "Pipeline",
            CompletionCategory::Keyword => "Keyword",
            CompletionCategory::History => "History",
            CompletionCategory::Ref => "Reference",
        }
    }

    pub fn icon_style(self, theme: &CompletionsTheme) -> Style {
        match self {
            CompletionCategory::Directory => theme.header_directory.to_style_bold(),
            CompletionCategory::File => theme.header_file.to_style_dim(),
            CompletionCategory::Command => theme.header_command.to_style_bold(),
            CompletionCategory::Builtin => theme.header_builtin.to_style_bold(),
            CompletionCategory::Alias => theme.header_alias.to_style_bold(),
            CompletionCategory::Function => theme.header_function.to_style_bold(),
            CompletionCategory::Variable => theme.header_variable.to_style_bold(),
            CompletionCategory::Flag => theme.header_flag.to_style_dim(),
            CompletionCategory::Pipeline => theme.header_pipeline.to_style_bold(),
            CompletionCategory::Keyword => theme.header_keyword.to_style_bold(),
            CompletionCategory::Job => theme.header_job.to_style_dim(),
            CompletionCategory::History => theme.header_history.to_style_dim(),
            CompletionCategory::Ref => theme.header_ref.to_style_bold(),
        }
    }

    pub fn badge_style(self, theme: &CompletionsTheme) -> Style {
        self.icon_style(theme)
    }

    pub fn header_style(self, theme: &CompletionsTheme) -> Style {
        self.icon_style(theme)
    }

    /// Get the value style for a completion item (non-selected state).
    pub fn value_style(self, theme: &CompletionsTheme) -> Style {
        match self {
            CompletionCategory::Directory | CompletionCategory::File => {
                // lscolors will override this if available
                theme.header_directory.to_style()
            }
            CompletionCategory::Command => theme.header_command.to_style(),
            CompletionCategory::Builtin => theme.header_builtin.to_style(),
            CompletionCategory::Alias => theme.header_alias.to_style(),
            CompletionCategory::Function => theme.header_function.to_style(),
            CompletionCategory::Variable => theme.header_variable.to_style(),
            CompletionCategory::Flag => theme.header_flag.to_style(),
            CompletionCategory::Pipeline => theme.header_pipeline.to_style(),
            CompletionCategory::Keyword => theme.header_keyword.to_style(),
            CompletionCategory::Job => theme.header_job.to_style(),
            CompletionCategory::History => theme.header_history.to_style(),
            CompletionCategory::Ref => theme.header_ref.to_style(),
        }
    }
}

/// Helper to render matched substring characters with highlight styling
pub fn render_highlighted_spans(
    text: &str,
    match_indices: Option<&[usize]>,
    base_style: Style,
    highlight_style: Style,
) -> Vec<Span<'static>> {
    let Some(indices) = match_indices else {
        return vec![Span::styled(text.to_string(), base_style)];
    };
    if indices.is_empty() {
        return vec![Span::styled(text.to_string(), base_style)];
    }

    let mut spans = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut current_segment = String::new();
    let mut is_highlighted = false;

    for (i, &c) in chars.iter().enumerate() {
        let matches = indices.contains(&i);
        if matches != is_highlighted && !current_segment.is_empty() {
            let style = if is_highlighted {
                highlight_style
            } else {
                base_style
            };
            spans.push(Span::styled(std::mem::take(&mut current_segment), style));
        }
        is_highlighted = matches;
        current_segment.push(c);
    }
    if !current_segment.is_empty() {
        let style = if is_highlighted {
            highlight_style
        } else {
            base_style
        };
        spans.push(Span::styled(current_segment, style));
    }
    spans
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayRow {
    Header(String),
    Suggestion { candidate_index: usize },
    GridRow { candidate_indices: Vec<usize> },
}

#[derive(Debug, Clone)]
pub struct CompletionSession {
    pub replacement_range: Range<usize>,
    pub query: String,
    pub candidates: Vec<RankedCompletion>,
    pub selected: usize,
    pub layout: CompletionLayout,
    pub display_rows: Vec<DisplayRow>,
}

impl CompletionSession {
    pub fn new(
        candidates: Vec<RankedCompletion>,
        query: String,
        replacement_range: Range<usize>,
        layout: CompletionLayout,
    ) -> Self {
        let display_rows = match layout {
            CompletionLayout::List => candidates
                .iter()
                .enumerate()
                .map(|(i, _)| DisplayRow::Suggestion { candidate_index: i })
                .collect(),
            CompletionLayout::Grid { cols, .. } => {
                let cols = cols.max(1);
                (0..candidates.len())
                    .collect::<Vec<_>>()
                    .chunks(cols)
                    .map(|chunk| DisplayRow::GridRow {
                        candidate_indices: chunk.to_vec(),
                    })
                    .collect()
            }
        };
        Self {
            replacement_range,
            query,
            candidates,
            selected: 0,
            layout,
            display_rows,
        }
    }

    pub fn selected_candidate(&self) -> Option<&RankedCompletion> {
        self.candidates.get(self.selected)
    }

    pub fn selected_row_index(&self) -> usize {
        match self.layout {
            CompletionLayout::List => self
                .display_rows
                .iter()
                .position(|r| {
                    matches!(
                        r,
                        DisplayRow::Suggestion { candidate_index } if *candidate_index == self.selected
                    )
                })
                .unwrap_or(0),
            CompletionLayout::Grid { cols, .. } => {
                let cols = cols.max(1);
                self.selected / cols
            }
        }
    }

    pub fn select_next(&mut self) {
        if self.candidates.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.candidates.len();
    }

    pub fn select_prev(&mut self) {
        if self.candidates.is_empty() {
            return;
        }
        if self.selected == 0 {
            self.selected = self.candidates.len() - 1;
        } else {
            self.selected -= 1;
        }
    }

    pub fn select_down(&mut self) {
        if self.candidates.is_empty() {
            return;
        }
        match self.layout {
            CompletionLayout::List => self.select_next(),
            CompletionLayout::Grid { cols, .. } => {
                let cols = cols.max(1);
                let next = self.selected + cols;
                if next < self.candidates.len() {
                    self.selected = next;
                } else {
                    self.selected %= cols;
                }
            }
        }
    }

    pub fn select_up(&mut self) {
        if self.candidates.is_empty() {
            return;
        }
        match self.layout {
            CompletionLayout::List => self.select_prev(),
            CompletionLayout::Grid { cols, .. } => {
                let cols = cols.max(1);
                if self.selected >= cols {
                    self.selected -= cols;
                } else {
                    let col = self.selected % cols;
                    let total = self.candidates.len();
                    let last_row_start = (total / cols) * cols;
                    let target = last_row_start + col;
                    if target < total {
                        self.selected = target;
                    } else if last_row_start >= cols {
                        self.selected = target - cols;
                    }
                }
            }
        }
    }

    pub fn select_right(&mut self) {
        self.select_next();
    }

    pub fn select_left(&mut self) {
        self.select_prev();
    }

    pub fn page_down(&mut self, page_size: usize) {
        if self.candidates.is_empty() {
            return;
        }
        match self.layout {
            CompletionLayout::List => {
                let next = self.selected.saturating_add(page_size.max(1));
                self.selected = next.min(self.candidates.len().saturating_sub(1));
            }
            CompletionLayout::Grid { cols, .. } => {
                let cols = cols.max(1);
                let jump = page_size.max(1) * cols;
                let next = self.selected.saturating_add(jump);
                self.selected = next.min(self.candidates.len().saturating_sub(1));
            }
        }
    }

    pub fn page_up(&mut self, page_size: usize) {
        if self.candidates.is_empty() {
            return;
        }
        match self.layout {
            CompletionLayout::List => {
                self.selected = self.selected.saturating_sub(page_size.max(1));
            }
            CompletionLayout::Grid { cols, .. } => {
                let cols = cols.max(1);
                let jump = page_size.max(1) * cols;
                self.selected = self.selected.saturating_sub(jump);
            }
        }
    }
}

/// Compute column count and column width for a grid layout based on candidate display widths.
pub fn compute_grid_geometry(
    candidates: &[CompletionCandidate],
    inner_width: usize,
) -> (usize, usize) {
    if candidates.is_empty() || inner_width < 16 {
        return (1, inner_width.max(1));
    }
    let max_val_w = candidates
        .iter()
        .map(|c| c.value.width())
        .max()
        .unwrap_or(12);

    let cell_w = (max_val_w + 3).clamp(14, 32);
    let cols = (inner_width / cell_w).max(1);
    let col_width = (inner_width / cols).max(1);
    (cols, col_width)
}

impl From<CompletionKind> for CompletionCategory {
    fn from(kind: CompletionKind) -> Self {
        match kind {
            CompletionKind::Directory => CompletionCategory::Directory,
            CompletionKind::File => CompletionCategory::File,
            CompletionKind::Builtin => CompletionCategory::Builtin,
            CompletionKind::UserFunction => CompletionCategory::Function,
            CompletionKind::ExternalCommand => CompletionCategory::Command,
            CompletionKind::Keyword => CompletionCategory::Keyword,
            CompletionKind::PipeOperator => CompletionCategory::Pipeline,
            CompletionKind::Variable => CompletionCategory::Variable,
            CompletionKind::Flag => CompletionCategory::Flag,
            CompletionKind::HelpTopic => CompletionCategory::Keyword,
            CompletionKind::GitBranch => CompletionCategory::Ref,
            CompletionKind::Job => CompletionCategory::Job,
            CompletionKind::Custom("history") => CompletionCategory::History,
            CompletionKind::Custom(_) => CompletionCategory::Command,
        }
    }
}

pub fn categorize(s: &CompletionCandidate) -> CompletionCategory {
    CompletionCategory::from(s.kind)
}

#[derive(Clone)]
struct CompletionRequest {
    generation: u64,
    line: String,
    cursor_pos: usize,
    force_visible: bool,
}

struct CompletionResponse {
    request: CompletionRequest,
    suggestions: Vec<CompletionCandidate>,
}

fn spawn_completion_worker(
    env: Env,
) -> Option<(
    watch::Sender<Option<CompletionRequest>>,
    mpsc::UnboundedReceiver<CompletionResponse>,
)> {
    let runtime = tokio::runtime::Handle::try_current().ok()?;
    let (request_tx, mut request_rx) = watch::channel::<Option<CompletionRequest>>(None);
    let (result_tx, result_rx) = mpsc::unbounded_channel();

    runtime.spawn(async move {
        loop {
            if request_rx.changed().await.is_err() {
                break;
            }
            let Some(mut request) = request_rx.borrow_and_update().clone() else {
                continue;
            };

            loop {
                let worker_env = env.clone();
                let worker_request = request.clone();
                let task = tokio::task::spawn_blocking(move || {
                    compute_completion_candidates(&worker_env, &worker_request)
                });
                let suggestions = task.await.unwrap_or_default();

                match request_rx.borrow_and_update().clone() {
                    Some(latest) if latest.generation != request.generation => {
                        request = latest;
                    }
                    Some(_) => {
                        let _ = result_tx.send(CompletionResponse {
                            request,
                            suggestions,
                        });
                        break;
                    }
                    None => break,
                }
            }
        }
    });

    Some((request_tx, result_rx))
}

fn compute_completion_candidates(
    env: &Env,
    request: &CompletionRequest,
) -> Vec<CompletionCandidate> {
    if request.line.trim().is_empty() {
        if !request.force_visible {
            return Vec::new();
        }

        let builtins = env.get_all_builtins();
        let mut raw: Vec<CompletionCandidate> = builtins
            .into_iter()
            .map(|builtin| {
                let desc = crate::autocomplete::command_description(&builtin)
                    .unwrap_or("Built-in command");
                CompletionCandidate::new(builtin, CompletionKind::Builtin, TextSpan::new(0, 0))
                    .with_description(desc)
            })
            .collect();

        for (command, desc) in crate::autocomplete::COMMON_EXTERNAL_COMMANDS {
            if !raw.iter().any(|candidate| candidate.value == *command) {
                raw.push(
                    CompletionCandidate::new(
                        *command,
                        CompletionKind::ExternalCommand,
                        TextSpan::new(0, 0),
                    )
                    .with_description(*desc),
                );
            }
        }

        if let Ok(entries) = crate::history::query_frequent_by_prefix("", 10) {
            for (command, frequency) in &entries {
                if !raw.iter().any(|candidate| candidate.value == *command) {
                    raw.push(
                        CompletionCandidate::new(
                            command.clone(),
                            CompletionKind::Custom("history"),
                            TextSpan::new(0, 0),
                        )
                        .with_description(format!(
                            "History ({} use{})",
                            frequency,
                            if *frequency == 1 { "" } else { "s" }
                        )),
                    );
                }
            }
        }

        raw.sort_by_key(|candidate| candidate.value.to_lowercase());
        raw.truncate(50);
        return raw;
    }

    let mut completer = FshellCompleter { env: env.clone() };
    let cursor_byte = request
        .line
        .char_indices()
        .nth(request.cursor_pos)
        .map(|(index, _)| index)
        .unwrap_or(request.line.len());
    completer.complete(&request.line, cursor_byte)
}

pub struct CompletionsManager {
    completer: FshellCompleter,
    completion_request_tx: Option<watch::Sender<Option<CompletionRequest>>>,
    completion_result_rx: Option<mpsc::UnboundedReceiver<CompletionResponse>>,
    generation: u64,
    pub loading: bool,
    pub session: Option<CompletionSession>,
    /// Full unfiltered suggestion list from the completer (never mutated by filter)
    pub all_suggestions: Vec<CompletionCandidate>,
    /// Kept for backwards compatibility and test convenience
    pub suggestions: Vec<CompletionCandidate>,
    /// Current partial word being filtered against
    pub filter_query: String,
    /// Reusable nucleo matcher instance
    nucleo_matcher: Matcher,
    pub selected_idx: usize,
    pub scroll_offset: usize,
    pub visible: bool,
    pub session_active: bool,
    pub lscolors: LsColors,
    /// True when the longest common prefix has already been filled in by a previous Tab
    pub prefix_accepted: bool,
    pub theme: Arc<Theme>,
    pub layout_kind: CompletionLayoutKind,
    pub inner_width: usize,
}

impl CompletionsManager {
    /// Create a synchronous manager for direct callers and tests.
    pub fn new(env: Env) -> Self {
        Self::new_internal(env, false)
    }

    /// Create a manager that computes completion results off the input loop.
    pub fn new_async(env: Env) -> Self {
        Self::new_internal(env, true)
    }

    fn new_internal(env: Env, background: bool) -> Self {
        let worker = background
            .then(|| spawn_completion_worker(env.clone()))
            .flatten();
        Self {
            completer: FshellCompleter { env },
            completion_request_tx: worker.as_ref().map(|(tx, _)| tx.clone()),
            completion_result_rx: worker.map(|(_, rx)| rx),
            generation: 0,
            loading: false,
            session: None,
            all_suggestions: Vec::new(),
            suggestions: Vec::new(),
            filter_query: String::new(),
            nucleo_matcher: Matcher::new(NucleoConfig::DEFAULT),
            selected_idx: 0,
            scroll_offset: 0,
            visible: false,
            session_active: false,
            lscolors: LsColors::from_env().unwrap_or_default(),
            prefix_accepted: false,
            theme: Arc::new(Theme::default_theme()),
            layout_kind: CompletionLayoutKind::List,
            inner_width: 80,
        }
    }

    pub fn update_theme(&mut self, theme: Arc<Theme>) {
        self.theme = theme;
    }

    pub fn update(&mut self, line: &str, cursor_pos: usize, force_visible: bool) {
        if !force_visible && !self.session_active {
            return;
        }
        self.update_owned(line.to_string(), cursor_pos, force_visible);
    }

    pub fn update_from_buffer(
        &mut self,
        text_buf: &crate::ftui::buffer::TextBuffer,
        force_visible: bool,
    ) {
        if !force_visible && !self.session_active {
            return;
        }
        self.update_owned(text_buf.text(), text_buf.cursor(), force_visible);
    }

    fn update_owned(&mut self, line: String, cursor_pos: usize, force_visible: bool) {
        if force_visible {
            self.session_active = true;
        }

        if line.trim().is_empty() && !force_visible {
            self.clear();
            return;
        }

        self.generation = self.generation.wrapping_add(1);
        let request = CompletionRequest {
            generation: self.generation,
            line,
            cursor_pos,
            force_visible,
        };

        if let Some(request_tx) = &self.completion_request_tx {
            request_tx.send_replace(Some(request));
            self.loading = true;
            self.visible = true;
            self.all_suggestions.clear();
            self.suggestions.clear();
            self.session = None;
            self.selected_idx = 0;
            self.scroll_offset = 0;
        } else {
            let suggestions = compute_completion_candidates(&self.completer.env, &request);
            self.apply_result(&request, suggestions);
        }
    }

    /// Apply any completed background request. Stale results are ignored so an
    /// older, slower directory scan can never replace suggestions for newer text.
    pub fn apply_pending_results(&mut self) -> bool {
        let responses = {
            let Some(result_rx) = self.completion_result_rx.as_mut() else {
                return false;
            };
            let mut responses = Vec::new();
            while let Ok(response) = result_rx.try_recv() {
                responses.push(response);
            }
            responses
        };

        let mut changed = false;
        for response in responses {
            if response.request.generation != self.generation {
                continue;
            }
            self.loading = false;
            self.apply_result(&response.request, response.suggestions);
            changed = true;
        }
        changed
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn apply_result(&mut self, request: &CompletionRequest, suggestions: Vec<CompletionCandidate>) {
        self.loading = false;
        self.all_suggestions = suggestions;

        if request.line.trim().is_empty() && request.force_visible {
            self.layout_kind = CompletionLayoutKind::List;
            self.filter(extract_partial_word(&request.line, request.cursor_pos));
            self.selected_idx = 0;
            self.scroll_offset = 0;
            self.visible = !self.suggestions.is_empty();
            self.session_active = self.visible;
            return;
        }

        // Context-driven layout: establish layout mode upon session opening, and keep stable
        if !self.session_active || self.session.is_none() {
            let is_fs = !self.all_suggestions.is_empty()
                && self.all_suggestions.iter().all(|suggestion| {
                    matches!(
                        categorize(suggestion),
                        CompletionCategory::Directory | CompletionCategory::File
                    )
                });
            self.layout_kind = if is_fs {
                CompletionLayoutKind::Grid
            } else {
                CompletionLayoutKind::List
            };
        }

        let partial = extract_partial_word(&request.line, request.cursor_pos);
        self.filter(partial);
        if self.suggestions.is_empty() {
            self.visible = false;
            self.selected_idx = 0;
            self.scroll_offset = 0;
        } else {
            self.visible = true;
            if self.selected_idx >= self.suggestions.len() {
                self.selected_idx = 0;
                self.scroll_offset = 0;
            }
        }
    }

    pub fn select_next(&mut self) {
        if let Some(ref mut s) = self.session {
            s.select_next();
            self.selected_idx = s.selected;
        } else if !self.suggestions.is_empty() {
            self.selected_idx = (self.selected_idx + 1) % self.suggestions.len();
        }
    }

    pub fn select_prev(&mut self) {
        if let Some(ref mut s) = self.session {
            s.select_prev();
            self.selected_idx = s.selected;
        } else if !self.suggestions.is_empty() {
            if self.selected_idx == 0 {
                self.selected_idx = self.suggestions.len() - 1;
            } else {
                self.selected_idx -= 1;
            }
        }
    }

    pub fn select_down(&mut self) {
        if let Some(ref mut s) = self.session {
            s.select_down();
            self.selected_idx = s.selected;
        } else if !self.suggestions.is_empty() {
            self.selected_idx = (self.selected_idx + 1) % self.suggestions.len();
        }
    }

    pub fn select_up(&mut self) {
        if let Some(ref mut s) = self.session {
            s.select_up();
            self.selected_idx = s.selected;
        } else if !self.suggestions.is_empty() {
            if self.selected_idx == 0 {
                self.selected_idx = self.suggestions.len() - 1;
            } else {
                self.selected_idx -= 1;
            }
        }
    }

    pub fn select_left(&mut self) {
        if let Some(ref mut s) = self.session {
            s.select_left();
            self.selected_idx = s.selected;
        } else if !self.suggestions.is_empty() {
            if self.selected_idx == 0 {
                self.selected_idx = self.suggestions.len() - 1;
            } else {
                self.selected_idx -= 1;
            }
        }
    }

    pub fn select_right(&mut self) {
        if let Some(ref mut s) = self.session {
            s.select_right();
            self.selected_idx = s.selected;
        } else if !self.suggestions.is_empty() {
            self.selected_idx = (self.selected_idx + 1) % self.suggestions.len();
        }
    }

    /// Fuzzy-filter `all_suggestions` against `partial` word, updating
    /// `suggestions` and `session`. Computes fuzzy match indices for live highlighting.
    pub fn filter(&mut self, partial: &str) {
        self.prefix_accepted = false;
        self.filter_query = partial.to_string();

        let recent = crate::history::get_recent_commands_cached();
        let ranked = rank_candidates(
            self.all_suggestions.clone(),
            partial,
            &mut self.nucleo_matcher,
            Some(&recent),
        );

        let replacement_range = if let Some(first) = ranked.first() {
            first.candidate.span.start..first.candidate.span.end
        } else {
            0..0
        };

        self.suggestions = ranked.iter().map(|r| r.candidate.clone()).collect();

        let layout = match self.layout_kind {
            CompletionLayoutKind::List => CompletionLayout::List,
            CompletionLayoutKind::Grid => {
                let (cols, col_width) = compute_grid_geometry(&self.suggestions, self.inner_width);
                CompletionLayout::Grid { cols, col_width }
            }
        };

        let session =
            CompletionSession::new(ranked, partial.to_string(), replacement_range, layout);
        self.session = Some(session);

        if self.suggestions.is_empty() {
            self.visible = false;
            self.selected_idx = 0;
            self.scroll_offset = 0;
        } else {
            if self.selected_idx >= self.suggestions.len() {
                self.selected_idx = 0;
                self.scroll_offset = 0;
            }
            if let Some(ref mut s) = self.session {
                s.selected = self.selected_idx;
            }
        }
    }

    /// Advance selection by `page_size` visible display lines
    pub fn page_down(&mut self, page_size: usize) {
        if let Some(ref mut s) = self.session {
            s.page_down(page_size);
            self.selected_idx = s.selected;
        } else if !self.suggestions.is_empty() {
            let next = self.selected_idx.saturating_add(page_size.max(1));
            self.selected_idx = next.min(self.suggestions.len().saturating_sub(1));
        }
    }

    /// Move selection backward by `page_size` visible display lines
    pub fn page_up(&mut self, page_size: usize) {
        if let Some(ref mut s) = self.session {
            s.page_up(page_size);
            self.selected_idx = s.selected;
        } else if !self.suggestions.is_empty() {
            self.selected_idx = self.selected_idx.saturating_sub(page_size.max(1));
        }
    }

    pub fn get_selected_suggestion(&self) -> Option<&CompletionCandidate> {
        if self.visible && !self.suggestions.is_empty() {
            self.suggestions.get(self.selected_idx)
        } else {
            None
        }
    }

    pub fn filter_query(&self) -> &str {
        &self.filter_query
    }

    pub fn is_all_files_or_dirs(&self) -> bool {
        if self.suggestions.is_empty() {
            return false;
        }
        self.suggestions.iter().all(|s| {
            matches!(
                categorize(s),
                CompletionCategory::Directory | CompletionCategory::File
            )
        })
    }

    /// Resolve a mouse position inside the rendered popup to a suggestion index.
    pub fn suggestion_index_at(
        &self,
        area: Rect,
        scroll_offset: usize,
        column: u16,
        row: u16,
    ) -> Option<usize> {
        let right = area.x.saturating_add(area.width);
        let bottom = area.y.saturating_add(area.height);
        let inner_x = area.x.saturating_add(1);
        let inner_y = area.y.saturating_add(1);
        if column < inner_x
            || column >= right.saturating_sub(1)
            || row < inner_y
            || row >= bottom.saturating_sub(1)
        {
            return None;
        }

        let has_footer = area.height >= 6;
        let visible_rows = if has_footer {
            area.height.saturating_sub(4)
        } else {
            area.height.saturating_sub(2)
        } as usize;

        let rel_y = row.saturating_sub(inner_y) as usize;
        if rel_y >= visible_rows {
            return None;
        }

        let display_idx = scroll_offset.saturating_add(rel_y);
        let Some(ref s) = self.session else {
            return (display_idx < self.suggestions.len()).then_some(display_idx);
        };

        match s.display_rows.get(display_idx) {
            Some(DisplayRow::Suggestion { candidate_index }) => Some(*candidate_index),
            Some(DisplayRow::GridRow { candidate_indices }) => match s.layout {
                CompletionLayout::Grid { col_width, .. } => {
                    let rel_x = column.saturating_sub(inner_x) as usize;
                    let col_idx = rel_x.checked_div(col_width).unwrap_or(0);
                    candidate_indices.get(col_idx).copied()
                }
                _ => candidate_indices.first().copied(),
            },
            Some(DisplayRow::Header(_)) | None => None,
        }
    }

    pub fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(request_tx) = &self.completion_request_tx {
            request_tx.send_replace(None);
        }
        self.loading = false;
        self.suggestions.clear();
        self.all_suggestions.clear();
        self.session = None;
        self.selected_idx = 0;
        self.scroll_offset = 0;
        self.visible = false;
        self.session_active = false;
        self.prefix_accepted = false;
        self.layout_kind = CompletionLayoutKind::List;
    }

    /// After a completion has been applied to the buffer, decide whether to keep
    /// the menu visible (drill into directory) or close it (file/final completion).
    pub fn refresh_after_completion(&mut self, new_line: &str, cursor_char_pos: usize) {
        let last_word = extract_partial_word(new_line, cursor_char_pos);
        if last_word.ends_with('/') {
            self.selected_idx = 0;
            self.scroll_offset = 0;
            self.update(new_line, cursor_char_pos, true);
        } else {
            self.clear();
        }
    }

    /// Compute the longest common prefix among all suggestion values
    pub fn longest_common_prefix(&self) -> Option<String> {
        if self.suggestions.len() <= 1 {
            return None;
        }
        let values: Vec<&str> = self.suggestions.iter().map(|s| s.value.as_str()).collect();
        let first = values.first()?;
        let first_chars: Vec<char> = first.chars().collect();
        let mut char_count = first_chars.len();
        for other in &values[1..] {
            let common = first_chars
                .iter()
                .copied()
                .zip(other.chars())
                .take_while(|(a, b)| a == b)
                .count();
            char_count = char_count.min(common);
        }
        if char_count == 0 {
            None
        } else {
            Some(first_chars[..char_count].iter().collect())
        }
    }

    /// Render the popup content using adaptive 1D or multi-column grid layout.
    pub fn render_popup(
        &mut self,
        area_width: u16,
        visible_lines: usize,
    ) -> (Vec<ratatui::widgets::ListItem<'static>>, usize) {
        if self.suggestions.is_empty() {
            return (Vec::new(), 0);
        }

        let inner_width = (area_width as usize).saturating_sub(2);
        self.inner_width = inner_width;

        // Ensure session layout geometry is synchronized with current inner_width
        if let Some(ref mut s) = self.session
            && let CompletionLayout::Grid { cols, col_width } = s.layout
        {
            let (new_cols, new_col_w) = compute_grid_geometry(&self.suggestions, inner_width);
            if new_cols != cols || new_col_w != col_width {
                s.layout = CompletionLayout::Grid {
                    cols: new_cols,
                    col_width: new_col_w,
                };
                s.display_rows = (0..s.candidates.len())
                    .collect::<Vec<_>>()
                    .chunks(new_cols)
                    .map(|chunk| DisplayRow::GridRow {
                        candidate_indices: chunk.to_vec(),
                    })
                    .collect();
            }
        }

        let total_rows = if let Some(ref s) = self.session {
            s.display_rows.len()
        } else {
            self.suggestions.len()
        };

        let vis_start = self.scroll_offset.min(total_rows);
        let vis_end = (self.scroll_offset + visible_lines).min(total_rows);

        let mut list_items = Vec::with_capacity(vis_end.saturating_sub(vis_start));

        for row_idx in vis_start..vis_end {
            let row_type = if let Some(ref s) = self.session {
                s.display_rows.get(row_idx)
            } else {
                None
            };

            match row_type {
                Some(DisplayRow::Header(title)) => {
                    let header_str = truncate_by_width(title, inner_width.saturating_sub(2));
                    list_items.push(ratatui::widgets::ListItem::new(Line::from(Span::styled(
                        format!(" ── {header_str} ──"),
                        self.theme.status.muted.to_style_dim(),
                    ))));
                }
                Some(DisplayRow::GridRow { candidate_indices }) => {
                    let (cols, col_width) = if let Some(ref s) = self.session {
                        match s.layout {
                            CompletionLayout::Grid { cols, col_width } => (cols, col_width),
                            _ => (1, inner_width),
                        }
                    } else {
                        (1, inner_width)
                    };
                    list_items.push(self.render_grid_row(
                        candidate_indices,
                        cols,
                        col_width,
                        inner_width,
                    ));
                }
                Some(DisplayRow::Suggestion { candidate_index }) => {
                    if let Some(s) = self.suggestions.get(*candidate_index) {
                        let is_selected = *candidate_index == self.selected_idx;
                        let item = self.render_adaptive_row(s, is_selected, inner_width);
                        list_items.push(item);
                    }
                }
                None => {
                    if let Some(s) = self.suggestions.get(row_idx) {
                        let is_selected = row_idx == self.selected_idx;
                        let item = self.render_adaptive_row(s, is_selected, inner_width);
                        list_items.push(item);
                    }
                }
            }
        }

        (list_items, total_rows)
    }

    fn render_grid_row(
        &self,
        candidate_indices: &[usize],
        cols: usize,
        col_width: usize,
        inner_width: usize,
    ) -> ratatui::widgets::ListItem<'static> {
        let t = &self.theme;
        let selection_bg = t.widgets.item_selected_bg.to_ratatui_color();
        let selection_fg = t.widgets.item_selected_fg.to_ratatui_color();

        let mut line_spans = Vec::new();

        for col_idx in 0..cols {
            let cell_cand_idx = candidate_indices.get(col_idx).copied();
            if let Some(cand_idx) = cell_cand_idx
                && let Some(s) = self.suggestions.get(cand_idx)
            {
                let is_selected = cand_idx == self.selected_idx;
                let category = categorize(s);

                let cell_bg = if is_selected {
                    Style::default().bg(selection_bg)
                } else {
                    Style::default()
                };

                let indicator = if is_selected {
                    Span::styled(
                        "▸ ",
                        Style::default()
                            .bg(selection_bg)
                            .fg(selection_fg)
                            .add_modifier(StyleModifier::BOLD),
                    )
                } else {
                    Span::styled("  ", cell_bg)
                };

                let base_val_style = if is_selected {
                    Style::default()
                        .bg(selection_bg)
                        .fg(selection_fg)
                        .add_modifier(StyleModifier::BOLD)
                } else if let Some(ls) = self.lscolors.style_for_path(&s.value) {
                    self.convert_lscolors_style(ls, false)
                } else {
                    category.value_style(&t.completions)
                };

                let highlight_style = if is_selected {
                    Style::default()
                        .bg(selection_bg)
                        .fg(t.syntax.keyword.to_ratatui_color())
                        .add_modifier(StyleModifier::BOLD | StyleModifier::UNDERLINED)
                } else {
                    t.syntax
                        .keyword
                        .to_style_bold()
                        .add_modifier(StyleModifier::UNDERLINED)
                };

                let avail_val_w = col_width.saturating_sub(3);
                let display_val = if s.value.width() > avail_val_w && avail_val_w > 3 {
                    truncate_by_width(&s.value, avail_val_w)
                } else {
                    s.value.clone()
                };

                let val_spans = render_highlighted_spans(
                    &display_val,
                    s.match_indices.as_deref(),
                    base_val_style,
                    highlight_style,
                );

                let val_w = display_val.width();
                let pad_w = col_width.saturating_sub(2 + val_w);

                line_spans.push(indicator);
                line_spans.extend(val_spans);
                if pad_w > 0 {
                    line_spans.push(Span::styled(" ".repeat(pad_w), cell_bg));
                }
                continue;
            }

            // Empty cell padding if fewer items than columns on the last row
            if col_width > 0 {
                line_spans.push(Span::raw(" ".repeat(col_width)));
            }
        }

        let current_w: usize = line_spans.iter().map(|s| s.width()).sum();
        if current_w < inner_width {
            line_spans.push(Span::raw(" ".repeat(inner_width - current_w)));
        }

        ratatui::widgets::ListItem::new(Line::from(line_spans))
    }

    fn render_adaptive_row(
        &self,
        suggestion: &CompletionCandidate,
        is_selected: bool,
        inner_width: usize,
    ) -> ratatui::widgets::ListItem<'static> {
        let t = &self.theme;
        let category = categorize(suggestion);
        let is_file_or_dir = matches!(
            category,
            CompletionCategory::Directory | CompletionCategory::File
        );

        let selection_bg = t.widgets.item_selected_bg.to_ratatui_color();
        let selection_fg = t.widgets.item_selected_fg.to_ratatui_color();

        let base_bg = if is_selected {
            Style::default().bg(selection_bg)
        } else {
            Style::default()
        };

        let indicator = if is_selected {
            Span::styled(
                "▸ ",
                Style::default()
                    .bg(selection_bg)
                    .fg(selection_fg)
                    .add_modifier(StyleModifier::BOLD),
            )
        } else {
            Span::raw("  ")
        };

        let base_val_style = if is_selected {
            Style::default()
                .bg(selection_bg)
                .fg(selection_fg)
                .add_modifier(StyleModifier::BOLD)
        } else if is_file_or_dir {
            if let Some(ls) = self.lscolors.style_for_path(&suggestion.value) {
                self.convert_lscolors_style(ls, false)
            } else {
                category.value_style(&t.completions)
            }
        } else {
            category.value_style(&t.completions)
        };

        let highlight_style = if is_selected {
            Style::default()
                .bg(selection_bg)
                .fg(t.syntax.keyword.to_ratatui_color())
                .add_modifier(StyleModifier::BOLD | StyleModifier::UNDERLINED)
        } else {
            t.syntax
                .keyword
                .to_style_bold()
                .add_modifier(StyleModifier::UNDERLINED)
        };

        let avail = inner_width.saturating_sub(2);

        let kind_str = match category {
            CompletionCategory::Directory => "dir".to_string(),
            CompletionCategory::File => {
                let path = std::path::Path::new(&suggestion.value);
                if let Ok(meta) = std::fs::symlink_metadata(path) {
                    if meta.is_symlink() {
                        "symlink".to_string()
                    } else {
                        format_file_size(meta.len())
                    }
                } else {
                    "file".to_string()
                }
            }
            CompletionCategory::Command => "command".to_string(),
            CompletionCategory::Builtin => "builtin".to_string(),
            CompletionCategory::Alias => "alias".to_string(),
            CompletionCategory::Function => "fn".to_string(),
            CompletionCategory::Variable => "var".to_string(),
            CompletionCategory::Flag => "flag".to_string(),
            CompletionCategory::Pipeline => "pipe".to_string(),
            CompletionCategory::Keyword => "keyword".to_string(),
            CompletionCategory::Job => "job".to_string(),
            CompletionCategory::History => "history".to_string(),
            CompletionCategory::Ref => "branch".to_string(),
        };

        let desc_opt = match &suggestion.description {
            Some(d) if !d.is_empty() && d != "Directory" && d != "File" => Some(d.as_str()),
            _ => None,
        };

        let mut spans = vec![indicator];

        if is_file_or_dir {
            let kind_w = kind_str.width();
            let max_val_w = avail.saturating_sub(kind_w + 2);
            let display_val = if suggestion.value.width() > max_val_w && max_val_w > 4 {
                truncate_by_width(&suggestion.value, max_val_w)
            } else {
                suggestion.value.clone()
            };
            let val_w = display_val.width();
            let pad_w = avail.saturating_sub(val_w + kind_w);

            let val_spans = render_highlighted_spans(
                &display_val,
                suggestion.match_indices.as_deref(),
                base_val_style,
                highlight_style,
            );
            spans.extend(val_spans);

            if avail >= 25 {
                spans.push(Span::styled(" ".repeat(pad_w.max(1)), base_bg));
                let kind_style = if is_selected {
                    Style::default()
                        .bg(selection_bg)
                        .fg(selection_fg)
                        .add_modifier(StyleModifier::DIM)
                } else {
                    t.status.muted.to_style_dim()
                };
                spans.push(Span::styled(kind_str, kind_style));
            }
        } else if avail < 30 {
            let display_val = if suggestion.value.width() > avail && avail > 4 {
                truncate_by_width(&suggestion.value, avail)
            } else {
                suggestion.value.clone()
            };
            let val_spans = render_highlighted_spans(
                &display_val,
                suggestion.match_indices.as_deref(),
                base_val_style,
                highlight_style,
            );
            spans.extend(val_spans);
        } else if avail < 55 {
            let kind_col_w = 10;
            let max_val_w = avail.saturating_sub(kind_col_w + 2);
            let display_val = if suggestion.value.width() > max_val_w && max_val_w > 4 {
                truncate_by_width(&suggestion.value, max_val_w)
            } else {
                suggestion.value.clone()
            };
            let val_w = display_val.width();
            let pad_w = avail.saturating_sub(val_w + kind_str.width());

            let val_spans = render_highlighted_spans(
                &display_val,
                suggestion.match_indices.as_deref(),
                base_val_style,
                highlight_style,
            );
            spans.extend(val_spans);
            spans.push(Span::styled(" ".repeat(pad_w.max(1)), base_bg));

            let kind_style = if is_selected {
                Style::default()
                    .bg(selection_bg)
                    .fg(selection_fg)
                    .add_modifier(StyleModifier::DIM)
            } else {
                category.badge_style(&t.completions)
            };
            spans.push(Span::styled(kind_str, kind_style));
        } else {
            let val_col_w = 18.min(avail / 3);
            let kind_col_w = 11;
            let desc_col_w = avail.saturating_sub(val_col_w + kind_col_w + 2);

            let display_val = if suggestion.value.width() > val_col_w.saturating_sub(1) {
                truncate_by_width(&suggestion.value, val_col_w.saturating_sub(1))
            } else {
                suggestion.value.clone()
            };
            let val_w = display_val.width();
            let val_pad = val_col_w.saturating_sub(val_w);

            let val_spans = render_highlighted_spans(
                &display_val,
                suggestion.match_indices.as_deref(),
                base_val_style,
                highlight_style,
            );
            spans.extend(val_spans);
            spans.push(Span::styled(" ".repeat(val_pad.max(1)), base_bg));

            let kind_pad = kind_col_w.saturating_sub(kind_str.width());
            let kind_style = if is_selected {
                Style::default()
                    .bg(selection_bg)
                    .fg(selection_fg)
                    .add_modifier(StyleModifier::DIM)
            } else {
                category.badge_style(&t.completions)
            };
            spans.push(Span::styled(kind_str, kind_style));
            spans.push(Span::styled(" ".repeat(kind_pad.max(1)), base_bg));

            if let Some(desc) = desc_opt {
                let display_desc = if desc.width() > desc_col_w {
                    truncate_by_width(desc, desc_col_w)
                } else {
                    desc.to_string()
                };
                let desc_style = if is_selected {
                    Style::default()
                        .bg(selection_bg)
                        .fg(selection_fg)
                        .add_modifier(StyleModifier::DIM)
                } else {
                    t.completions.description.to_style_dim()
                };
                spans.push(Span::styled(display_desc, desc_style));
            }
        }

        let current_width: usize = spans.iter().map(|s| s.width()).sum();
        if current_width < inner_width {
            spans.push(Span::styled(
                " ".repeat(inner_width - current_width),
                base_bg,
            ));
        }

        ratatui::widgets::ListItem::new(Line::from(spans).style(base_bg))
    }

    fn convert_lscolors_style(&self, ls: &LsStyle, is_selected: bool) -> Style {
        if is_selected {
            let t = &self.theme;
            return Style::default()
                .fg(t.widgets.item_selected_fg.to_ratatui_color())
                .bg(t.widgets.item_selected_bg.to_ratatui_color())
                .add_modifier(StyleModifier::BOLD);
        }

        let mut style = Style::default();
        if let Some(c) = ls.foreground.as_ref().and_then(|fg| self.convert_color(fg)) {
            style = style.fg(c);
        }
        if let Some(c) = ls.background.as_ref().and_then(|bg| self.convert_color(bg)) {
            style = style.bg(c);
        }
        if ls.font_style.bold {
            style = style.add_modifier(StyleModifier::BOLD);
        }
        if ls.font_style.italic {
            style = style.add_modifier(StyleModifier::ITALIC);
        }
        if ls.font_style.underline {
            style = style.add_modifier(StyleModifier::UNDERLINED);
        }
        style
    }

    #[allow(unreachable_patterns)]
    fn convert_color(&self, color: &lscolors::Color) -> Option<Color> {
        match color {
            lscolors::Color::Black => Some(Color::Black),
            lscolors::Color::Red => Some(Color::Red),
            lscolors::Color::Green => Some(Color::Green),
            lscolors::Color::Yellow => Some(Color::Yellow),
            lscolors::Color::Blue => Some(Color::Blue),
            lscolors::Color::Magenta => Some(Color::Magenta),
            lscolors::Color::Cyan => Some(Color::Cyan),
            lscolors::Color::White => Some(Color::White),
            lscolors::Color::BrightBlack => Some(Color::DarkGray),
            lscolors::Color::BrightRed => Some(Color::LightRed),
            lscolors::Color::BrightGreen => Some(Color::LightGreen),
            lscolors::Color::BrightYellow => Some(Color::LightYellow),
            lscolors::Color::BrightBlue => Some(Color::LightBlue),
            lscolors::Color::BrightMagenta => Some(Color::LightMagenta),
            lscolors::Color::BrightCyan => Some(Color::LightCyan),
            lscolors::Color::BrightWhite => Some(Color::White),
            lscolors::Color::Fixed(n) => Some(Color::Indexed(*n)),
            lscolors::Color::RGB(r, g, b) => Some(Color::Rgb(*r, *g, *b)),
            _ => None,
        }
    }

    pub fn format_suggestion(
        &self,
        suggestion: &CompletionCandidate,
        is_selected: bool,
    ) -> Line<'static> {
        let cat = categorize(suggestion);
        let t = &self.theme;
        let mut spans = Vec::new();

        let indicator = if is_selected { "▸ " } else { "  " };
        spans.push(Span::raw(indicator));

        let selection_bg = t.widgets.item_selected_bg.to_ratatui_color();
        let selection_fg = t.widgets.item_selected_fg.to_ratatui_color();

        let value_style = if is_selected {
            Style::default()
                .fg(selection_fg)
                .bg(selection_bg)
                .add_modifier(StyleModifier::BOLD)
        } else {
            match cat {
                CompletionCategory::Directory | CompletionCategory::File => {
                    if let Some(ls) = self.lscolors.style_for_path(&suggestion.value) {
                        self.convert_lscolors_style(ls, false)
                    } else {
                        cat.value_style(&t.completions)
                    }
                }
                _ => cat.value_style(&t.completions),
            }
        };

        spans.push(Span::styled(suggestion.value.clone(), value_style));

        if let Some(desc) = &suggestion.description {
            let desc_style = if is_selected {
                Style::default().fg(selection_fg).bg(selection_bg)
            } else {
                t.completions.description.to_style()
            };
            spans.push(Span::styled(format!("  —  {}", desc), desc_style));
        }

        Line::from(spans)
    }
}

/// Legacy format method — kept for backwards compat in tests / non-popup paths
pub fn format_suggestion_legacy(
    suggestion: &CompletionCandidate,
    is_selected: bool,
    theme: &CompletionsTheme,
) -> Line<'static> {
    use crate::theme_ext::ThemeColorRatatui;

    let cat = categorize(suggestion);
    let indicator = if is_selected { "▸ " } else { "  " };

    let selection_bg = theme.header_default.to_ratatui_color();

    let value_style = if is_selected {
        Style::default()
            .fg(Color::Black)
            .bg(selection_bg)
            .add_modifier(StyleModifier::BOLD)
    } else {
        cat.value_style(theme)
    };

    Line::from(Span::styled(
        format!("{}{}", indicator, suggestion.value),
        value_style,
    ))
}

/// Extract the partial word at cursor for fuzzy filtering.
/// `cursor_char_idx` is a character index into `line`.
/// Returns the text from the last word boundary up to cursor.
pub fn extract_partial_word(line: &str, cursor_char_idx: usize) -> &str {
    let byte_idx = line
        .char_indices()
        .nth(cursor_char_idx)
        .map(|(i, _)| i)
        .unwrap_or(line.len());
    let prefix = &line[..byte_idx];
    extract_quote_aware_token(prefix)
}

/// Extract the last token from a command line prefix, taking into account
/// single and double quotes so tokens with spaces inside quotes aren't split.
pub fn extract_quote_aware_token(prefix: &str) -> &str {
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escaped = false;
    let mut token_start = 0;

    for (idx, ch) in prefix.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && !in_single_quote {
            escaped = true;
            continue;
        }
        if ch == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            continue;
        }
        if ch == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            continue;
        }

        if !in_single_quote
            && !in_double_quote
            && (ch.is_whitespace() || ch == '|' || ch == '>' || ch == '<' || ch == ';' || ch == '&')
        {
            token_start = idx + ch.len_utf8();
        }
    }

    &prefix[token_start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_by_width_ascii() {
        assert_eq!(truncate_by_width("hello", 10), "hello");
        assert_eq!(truncate_by_width("hello", 3), "he…");
    }

    #[test]
    fn test_truncate_by_width_cjk() {
        // Each CJK character is width 2. "中文测试" = 8 columns total.
        assert_eq!(truncate_by_width("中文测试", 8), "中文测试");
        // max_width=4: content space = 3 cols. "中" fits (2≤3). "文" would make 4>3 → "中…"
        assert_eq!(truncate_by_width("中文测试", 4), "中…");
        // max_width=5: content space = 4 cols. "中文" fits (4≤4). "中…" would make 6>4 → "中文…"
        assert_eq!(truncate_by_width("中文测试", 5), "中文…");
    }

    #[test]
    fn test_truncate_by_width_emoji() {
        // Single emoji is usually width 2.
        let s = "🚀 rocket";
        // max_width=5: '🚀'(2) + ' '(1) + 'r'(1) fit in 4 columns; 'o' would be
        // column 5 which is reserved for the ellipsis, so the result is "🚀 r…".
        assert_eq!(truncate_by_width(s, 5), "🚀 r…");
        // max_width=3: '🚀' fills the 2 content columns; ' ' overflows → "🚀…".
        assert_eq!(truncate_by_width(s, 3), "🚀…");
    }

    #[test]
    fn test_completions_with_multibyte_accented_characters() {
        let env = fshell_engine::Env::new();
        let mut mgr = CompletionsManager::new(env);

        // cursor at char index 1 in "è" (2 bytes in UTF-8)
        mgr.update("è", 1, false);
        let partial = extract_partial_word("è", 1);
        assert_eq!(partial, "è");

        // multiple accented characters
        mgr.update("echo è à é", 10, false);
        let partial2 = extract_partial_word("echo è à é", 10);
        assert_eq!(partial2, "é");
    }

    #[tokio::test]
    async fn background_completion_processes_providers_and_discards_stale_input() {
        let env = fshell_engine::Env::new();
        env.vars.write().insert(
            "choices".to_string(),
            fshell_core::Val::List(vec![fshell_core::Val::String("alpha".to_string())]),
        );
        let mut completion = fshell_core::CommandCompletion::default();
        completion
            .dynamic_providers
            .push(fshell_core::DynamicProvider {
                parent_subcmds: Vec::new(),
                command: "($choices)".to_string(),
                cache_ms: None,
            });
        env.completions
            .write()
            .insert("testcmd".to_string(), completion);
        let direct = crate::autocomplete::get_custom_completions("testcmd a", 9, &env);
        assert!(direct.is_some(), "dynamic completion setup should resolve");

        let mut manager = CompletionsManager::new_async(env);
        manager.update("testcmd a", 9, true);
        let stale_generation = manager.generation();
        manager.update("testcmd al", 10, false);
        assert_ne!(manager.generation(), stale_generation);

        let completed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while manager.loading {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                manager.apply_pending_results();
            }
        })
        .await;
        assert!(completed.is_ok(), "completion worker did not return");
        assert_eq!(manager.filter_query(), "al");
        assert!(
            manager
                .suggestions
                .iter()
                .any(|suggestion| suggestion.value == "alpha"),
            "dynamic provider result was missing: {:?}",
            manager
                .suggestions
                .iter()
                .map(|suggestion| suggestion.value.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_popup_mouse_mapping_matches_rendered_rows() {
        let env = fshell_engine::Env::new();
        let mut mgr = CompletionsManager::new(env);
        mgr.suggestions = ["one", "two", "three"]
            .into_iter()
            .map(|value| {
                CompletionCandidate::new(
                    value.to_string(),
                    CompletionKind::ExternalCommand,
                    TextSpan::new(0, value.len()),
                )
            })
            .collect();

        // Standard area with height 7 (has footer, visible_rows = 7 - 4 = 3)
        let area = Rect::new(2, 3, 20, 7);
        assert_eq!(mgr.suggestion_index_at(area, 0, 3, 4), Some(0));
        assert_eq!(mgr.suggestion_index_at(area, 0, 3, 5), Some(1));
        assert_eq!(mgr.suggestion_index_at(area, 0, 3, 6), Some(2));
        assert_eq!(mgr.suggestion_index_at(area, 0, 3, 7), None); // divider
        assert_eq!(mgr.suggestion_index_at(area, 0, 3, 8), None); // footer
        // Scroll offset
        assert_eq!(mgr.suggestion_index_at(area, 1, 3, 4), Some(1));
        // Outside bounds (left border, right border)
        assert_eq!(mgr.suggestion_index_at(area, 0, 2, 4), None);
        assert_eq!(mgr.suggestion_index_at(area, 0, 21, 4), None);

        // Small area with height 5 (< 6, no footer, visible_rows = 5 - 2 = 3)
        let small_area = Rect::new(2, 3, 20, 5);
        assert_eq!(mgr.suggestion_index_at(small_area, 0, 3, 4), Some(0));
        assert_eq!(mgr.suggestion_index_at(small_area, 0, 3, 5), Some(1));
        assert_eq!(mgr.suggestion_index_at(small_area, 0, 3, 6), Some(2));
        assert_eq!(mgr.suggestion_index_at(small_area, 0, 3, 7), None);
    }

    #[test]
    fn test_grid_layout_selection_and_mouse_mapping() {
        let env = fshell_engine::Env::new();
        let mut mgr = CompletionsManager::new(env);
        let candidates: Vec<CompletionCandidate> = (0..6)
            .map(|i| {
                CompletionCandidate::new(
                    format!("file_{i}.txt"),
                    CompletionKind::File,
                    TextSpan::new(0, 10),
                )
            })
            .collect();

        mgr.suggestions = candidates.clone();
        let ranked = crate::autocomplete::ranking::rank_candidates(
            candidates,
            "",
            &mut nucleo_matcher::Matcher::new(nucleo_matcher::Config::DEFAULT),
            None,
        );
        let layout = CompletionLayout::Grid {
            cols: 2,
            col_width: 15,
        };
        let session = CompletionSession::new(ranked, String::new(), 0..0, layout);
        mgr.session = Some(session);

        assert_eq!(mgr.session.as_ref().unwrap().display_rows.len(), 3);
        assert_eq!(mgr.session.as_ref().unwrap().selected_row_index(), 0);

        // 2D navigation
        mgr.select_down();
        assert_eq!(mgr.selected_idx, 2);
        assert_eq!(mgr.session.as_ref().unwrap().selected_row_index(), 1);

        mgr.select_right();
        assert_eq!(mgr.selected_idx, 3);
        assert_eq!(mgr.session.as_ref().unwrap().selected_row_index(), 1);

        mgr.select_up();
        assert_eq!(mgr.selected_idx, 1);
        assert_eq!(mgr.session.as_ref().unwrap().selected_row_index(), 0);

        mgr.select_left();
        assert_eq!(mgr.selected_idx, 0);

        // Mouse mapping on grid
        let area = Rect::new(2, 3, 34, 7);
        assert_eq!(mgr.suggestion_index_at(area, 0, 3, 4), Some(0));
        assert_eq!(mgr.suggestion_index_at(area, 0, 10, 4), Some(0));
        assert_eq!(mgr.suggestion_index_at(area, 0, 18, 4), Some(1));
        assert_eq!(mgr.suggestion_index_at(area, 0, 3, 5), Some(2));
        assert_eq!(mgr.suggestion_index_at(area, 0, 18, 5), Some(3));
    }
}
