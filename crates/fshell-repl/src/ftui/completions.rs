// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::FshellCompleter;
use crate::autocomplete::{Completer, CompletionCandidate, CompletionKind, TextSpan};
use crate::theme_ext::ThemeColorRatatui;
use fshell_core::theme::{CompletionsTheme, Theme};
use fshell_engine::Env;
use lscolors::{LsColors, Style as LsStyle};
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as NucleoConfig, Matcher, Utf32String};
use ratatui::style::{Color, Modifier as StyleModifier, Style};
use ratatui::text::{Line, Span};
use std::sync::Arc;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Truncate a string to a maximum display width, appending "…" if truncated.
fn truncate_by_width(s: &str, max_width: usize) -> String {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionLayoutMode {
    List,
    Grid { cols: usize, col_width: usize },
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

/// A categorized suggestion with its group info
#[derive(Debug, Clone)]
pub struct CategorizedSuggestion {
    pub suggestion: CompletionCandidate,
    pub category: CompletionCategory,
}

impl CategorizedSuggestion {
    fn from_suggestion(s: CompletionCandidate) -> Self {
        let cat = CompletionCategory::from(s.kind);
        Self {
            suggestion: s,
            category: cat,
        }
    }
}

fn categorize(s: &CompletionCandidate) -> CompletionCategory {
    CompletionCategory::from(s.kind)
}

/// A grouped list of categorized suggestions with index tracking
#[derive(Debug, Clone)]
pub struct GroupedSuggestions {
    pub groups: Vec<CompletionCategory>,
    pub items: Vec<CategorizedSuggestion>,
    /// Flat index-to-group mapping (which group each item belongs to)
    pub group_indices: Vec<usize>,
}

impl GroupedSuggestions {
    fn new(raw: Vec<CompletionCandidate>) -> Self {
        let mut items: Vec<CategorizedSuggestion> = raw
            .into_iter()
            .map(CategorizedSuggestion::from_suggestion)
            .collect();

        // Sort: directories first, then files, then everything else
        items.sort_by(|a, b| {
            let order = |cat: &CompletionCategory| -> u8 {
                match cat {
                    CompletionCategory::Directory => 0,
                    CompletionCategory::File => 1,
                    _ => 2,
                }
            };
            order(&a.category).cmp(&order(&b.category))
        });

        // Build group ordering: collect unique categories in order of first appearance
        let mut seen_cats = Vec::new();
        for item in &items {
            if !seen_cats.contains(&item.category) {
                seen_cats.push(item.category);
            }
        }

        // Build flat group_indices: for each item, which group index it belongs to
        let group_indices: Vec<usize> = items
            .iter()
            .map(|item| {
                seen_cats
                    .iter()
                    .position(|c| *c == item.category)
                    .unwrap_or(0)
            })
            .collect();

        Self {
            groups: seen_cats,
            items,
            group_indices,
        }
    }

    pub fn total_items(&self) -> usize {
        self.items.len()
    }

    /// Get the number of items in each group
    pub fn group_sizes(&self) -> Vec<usize> {
        let mut sizes = vec![0usize; self.groups.len()];
        for gi in &self.group_indices {
            if *gi < sizes.len() {
                sizes[*gi] += 1;
            }
        }
        sizes
    }

    /// Get the display line count including group headers
    pub fn display_lines(&self) -> usize {
        let sizes = self.group_sizes();
        sizes.iter().sum::<usize>() + self.groups.len() // +1 per group for header
    }
}

pub struct CompletionsManager {
    completer: FshellCompleter,
    pub suggestions: Vec<CompletionCandidate>,
    pub grouped: Option<GroupedSuggestions>,
    /// Full unfiltered suggestion list from the completer (never mutated by filter)
    pub all_suggestions: Vec<CompletionCandidate>,
    /// Current partial word being filtered against
    pub filter_query: String,
    /// Reusable nucleo matcher instance
    nucleo_matcher: Matcher,
    pub selected_idx: usize,
    pub scroll_offset: usize,
    pub visible: bool,
    pub session_active: bool,
    pub lscolors: LsColors,
    pub active_selection: bool,
    /// True when the longest common prefix has already been filled in by a previous Tab
    pub prefix_accepted: bool,
    pub theme: Arc<Theme>,
}

impl CompletionsManager {
    pub fn new(env: Env) -> Self {
        Self {
            completer: FshellCompleter { env },
            suggestions: Vec::new(),
            grouped: None,
            all_suggestions: Vec::new(),
            filter_query: String::new(),
            nucleo_matcher: Matcher::new(NucleoConfig::DEFAULT),
            selected_idx: 0,
            scroll_offset: 0,
            visible: false,
            session_active: false,
            lscolors: LsColors::from_env().unwrap_or_default(),
            active_selection: false,
            prefix_accepted: false,
            theme: Arc::new(Theme::default_theme()),
        }
    }

    pub fn update_theme(&mut self, theme: Arc<Theme>) {
        self.theme = theme;
    }

    pub fn update(&mut self, line: &str, cursor_pos: usize, force_visible: bool) {
        self.active_selection = false;
        if force_visible {
            self.session_active = true;
        }

        // Skip full completer when completions session is not active and not explicitly requested
        if !force_visible && !self.session_active {
            return;
        }

        if line.trim().is_empty() {
            if force_visible {
                // Tab on empty line — show curated command list
                let builtins = self.completer.env.get_all_builtins();
                let mut raw: Vec<CompletionCandidate> = builtins
                    .into_iter()
                    .map(|b| {
                        let desc = crate::autocomplete::command_description(&b)
                            .unwrap_or("Built-in command");
                        CompletionCandidate::new(b, CompletionKind::Builtin, TextSpan::new(0, 0))
                            .with_description(desc)
                    })
                    .collect();

                for (cmd, desc) in crate::autocomplete::COMMON_EXTERNAL_COMMANDS {
                    if !raw.iter().any(|s| s.value == *cmd) {
                        raw.push(
                            CompletionCandidate::new(
                                cmd.to_string(),
                                CompletionKind::ExternalCommand,
                                TextSpan::new(0, 0),
                            )
                            .with_description(*desc),
                        );
                    }
                }

                if let Ok(entries) = crate::history::query_frequent_by_prefix("", 10) {
                    for (cmd, freq) in &entries {
                        if !raw.iter().any(|s| s.value == *cmd) {
                            raw.push(
                                CompletionCandidate::new(
                                    cmd.clone(),
                                    CompletionKind::Custom("history"),
                                    TextSpan::new(0, 0),
                                )
                                .with_description(format!(
                                    "History ({} use{})",
                                    freq,
                                    if *freq == 1 { "" } else { "s" }
                                )),
                            );
                        }
                    }
                }

                raw.sort_by_key(|a| a.value.to_lowercase());
                raw.truncate(50);

                self.all_suggestions = raw;
                let partial = extract_partial_word(line, cursor_pos);
                self.filter(partial);
                self.selected_idx = 0;
                self.scroll_offset = 0;
                self.visible = !self.suggestions.is_empty();
                self.session_active = self.visible;
            } else {
                self.clear();
            }
            return;
        }

        let cursor_byte = line
            .char_indices()
            .nth(cursor_pos)
            .map(|(i, _)| i)
            .unwrap_or(line.len());

        // Call our FshellCompleter backend
        let mut raw_suggestions = self.completer.complete(line, cursor_byte);

        // Smart sorting: prefix matches first, then by length
        let last_word = line[..cursor_byte]
            .split(|c: char| c.is_whitespace() || c == '|' || c == '>' || c == '<')
            .next_back()
            .unwrap_or("");

        if !last_word.is_empty() {
            let last_word_lower = last_word.to_lowercase();
            raw_suggestions.sort_by(|a, b| {
                let a_val = a.value.to_lowercase();
                let b_val = b.value.to_lowercase();

                let a_exact = a_val.starts_with(&last_word_lower);
                let b_exact = b_val.starts_with(&last_word_lower);

                if a_exact && !b_exact {
                    std::cmp::Ordering::Less
                } else if !a_exact && b_exact {
                    std::cmp::Ordering::Greater
                } else {
                    a_val.len().cmp(&b_val.len())
                }
            });
        }

        self.all_suggestions = raw_suggestions;

        // Apply fuzzy filter against the current partial word
        let partial = extract_partial_word(line, cursor_pos);
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
        if self.suggestions.is_empty() {
            return;
        }
        self.selected_idx = (self.selected_idx + 1) % self.suggestions.len();
    }

    pub fn select_prev(&mut self) {
        if self.suggestions.is_empty() {
            return;
        }
        if self.selected_idx == 0 {
            self.selected_idx = self.suggestions.len() - 1;
        } else {
            self.selected_idx -= 1;
        }
    }

    pub fn select_down(&mut self, cols: usize) {
        if self.suggestions.is_empty() {
            return;
        }
        if cols <= 1 {
            self.select_next();
        } else {
            let next = self.selected_idx + cols;
            if next < self.suggestions.len() {
                self.selected_idx = next;
            } else {
                self.selected_idx %= cols;
            }
        }
    }

    pub fn select_up(&mut self, cols: usize) {
        if self.suggestions.is_empty() {
            return;
        }
        if cols <= 1 {
            self.select_prev();
        } else if self.selected_idx >= cols {
            self.selected_idx -= cols;
        } else {
            let mut target = self.selected_idx;
            while target + cols < self.suggestions.len() {
                target += cols;
            }
            self.selected_idx = target;
        }
    }

    /// Fuzzy-filter `all_suggestions` against `partial` word, updating
    /// `suggestions` and `grouped`. Computes fuzzy match indices for live highlighting.
    pub fn filter(&mut self, partial: &str) {
        self.prefix_accepted = false;
        self.filter_query = partial.to_string();
        if partial.is_empty() {
            self.suggestions = self.all_suggestions.clone();
            self.grouped = if self.all_suggestions.is_empty() {
                None
            } else {
                let grouped = GroupedSuggestions::new(self.all_suggestions.clone());
                self.suggestions = grouped
                    .items
                    .iter()
                    .map(|ci| ci.suggestion.clone())
                    .collect();
                Some(grouped)
            };
            if self.selected_idx >= self.suggestions.len() {
                self.selected_idx = 0;
            }
            if self.suggestions.is_empty() {
                self.visible = false;
            }
            return;
        }

        let pattern = Pattern::new(
            partial,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );

        // Score all items, keep only those that match, and record matched character indices
        let mut scored: Vec<(u32, usize, CompletionCandidate)> = self
            .all_suggestions
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let haystack = Utf32String::from(s.value.as_str());
                let mut indices = Vec::new();
                let score =
                    pattern.indices(haystack.slice(..), &mut self.nucleo_matcher, &mut indices)?;
                let mut suggestion = s.clone();
                indices.sort_unstable();
                suggestion.match_indices =
                    Some(indices.into_iter().map(|idx| idx as usize).collect());
                Some((score, i, suggestion))
            })
            .collect();

        // Sort by: 1) prefix match (starts with partial), 2) score descending, 3) original index
        let partial_lower = partial.to_lowercase();
        scored.sort_by(|a, b| {
            let a_val = a.2.value.to_lowercase();
            let b_val = b.2.value.to_lowercase();
            let a_prefix = a_val.starts_with(&partial_lower);
            let b_prefix = b_val.starts_with(&partial_lower);
            b_prefix
                .cmp(&a_prefix)
                .then_with(|| b.0.cmp(&a.0))
                .then_with(|| a.1.cmp(&b.1))
        });

        self.suggestions = scored.into_iter().map(|(_, _, s)| s).collect();

        self.grouped = if self.suggestions.is_empty() {
            None
        } else {
            let grouped = GroupedSuggestions::new(self.suggestions.clone());
            self.suggestions = grouped
                .items
                .iter()
                .map(|ci| ci.suggestion.clone())
                .collect();
            Some(grouped)
        };

        if self.suggestions.is_empty() {
            self.visible = false;
        }

        if self.selected_idx >= self.suggestions.len() {
            self.selected_idx = 0;
        }
    }

    /// Advance selection by `page_size` visible display lines
    pub fn page_down(&mut self, page_size: usize) {
        if self.suggestions.is_empty() {
            return;
        }
        let next = self.selected_idx.saturating_add(page_size.max(1));
        self.selected_idx = next.min(self.suggestions.len().saturating_sub(1));
    }

    /// Move selection backward by `page_size` visible display lines
    pub fn page_up(&mut self, page_size: usize) {
        if self.suggestions.is_empty() {
            return;
        }
        self.selected_idx = self.selected_idx.saturating_sub(page_size.max(1));
    }

    pub fn get_selected_suggestion(&self) -> Option<&CompletionCandidate> {
        if self.visible && !self.suggestions.is_empty() {
            self.suggestions.get(self.selected_idx)
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.suggestions.clear();
        self.all_suggestions.clear();
        self.grouped = None;
        self.selected_idx = 0;
        self.scroll_offset = 0;
        self.visible = false;
        self.session_active = false;
        self.active_selection = false;
        self.prefix_accepted = false;
    }

    /// After a completion has been applied to the buffer, decide whether to keep
    /// the menu visible (drill into directory) or close it (file/final completion).
    pub fn refresh_after_completion(&mut self, new_line: &str, cursor_char_pos: usize) {
        let last_word = extract_partial_word(new_line, cursor_char_pos);
        if last_word.ends_with('/') {
            self.selected_idx = 0;
            self.scroll_offset = 0;
            self.update(new_line, cursor_char_pos, true);
            if self.visible && !self.suggestions.is_empty() {
                self.active_selection = true;
            }
        } else {
            self.clear();
        }
    }

    /// Compute whether to display in Grid mode or List mode based on contents and popup width
    pub fn compute_layout_mode(&self, area_width: u16) -> CompletionLayoutMode {
        if self.suggestions.is_empty() {
            return CompletionLayoutMode::List;
        }

        // Check if suggestions have long sentence documentation (e.g. flags or command docs)
        let has_long_descriptions = self.suggestions.iter().any(|s| {
            let cat = categorize(s);
            if matches!(
                cat,
                CompletionCategory::Directory | CompletionCategory::File
            ) {
                return false;
            }
            if let Some(ref d) = s.description {
                !d.is_empty() && d.len() > 14
            } else {
                false
            }
        });

        if has_long_descriptions || self.suggestions.len() < 3 {
            return CompletionLayoutMode::List;
        }

        let max_val_len = self
            .suggestions
            .iter()
            .map(|s| s.value.width())
            .max()
            .unwrap_or(10);

        let cell_width = (max_val_len + 4).max(14);
        let usable_width = area_width.saturating_sub(2) as usize;

        let possible_cols = (usable_width / cell_width).clamp(1, 5);
        if possible_cols >= 2 {
            let col_w = usable_width / possible_cols;
            CompletionLayoutMode::Grid {
                cols: possible_cols,
                col_width: col_w,
            }
        } else {
            CompletionLayoutMode::List
        }
    }

    /// Map a raw suggestion index to its display row index
    pub fn flat_index_of(&self, raw_idx: usize) -> usize {
        raw_idx
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

    /// Render the popup content using adaptive dual-mode layout (Grid or List).
    #[allow(clippy::type_complexity)]
    pub fn render_popup(
        &self,
        area_width: u16,
        visible_lines: usize,
    ) -> (
        Vec<(
            Option<CompletionCategory>,
            Vec<ratatui::widgets::ListItem<'static>>,
        )>,
        usize, /* total display rows */
    ) {
        if self.suggestions.is_empty() {
            return (Vec::new(), 0);
        }

        let layout = self.compute_layout_mode(area_width);
        match layout {
            CompletionLayoutMode::List => self.render_list_popup(area_width, visible_lines),
            CompletionLayoutMode::Grid { cols, col_width } => {
                self.render_grid_popup(visible_lines, cols, col_width)
            }
        }
    }

    #[allow(clippy::type_complexity)]
    fn render_list_popup(
        &self,
        area_width: u16,
        visible_lines: usize,
    ) -> (
        Vec<(
            Option<CompletionCategory>,
            Vec<ratatui::widgets::ListItem<'static>>,
        )>,
        usize,
    ) {
        let total_items = self.suggestions.len();
        let total_display = total_items;

        let vis_start = self.scroll_offset.min(total_items);
        let vis_end = (self.scroll_offset + visible_lines).min(total_items);

        let max_desc_width = if area_width > 70 {
            ((area_width as f64) * 0.40).min(38.0) as usize
        } else {
            ((area_width as usize).saturating_sub(24) / 2).clamp(12, 22)
        };
        let max_value_width = (area_width as usize).saturating_sub(max_desc_width + 12);

        let mut list_items = Vec::with_capacity(vis_end.saturating_sub(vis_start));

        for i in vis_start..vis_end {
            let s = &self.suggestions[i];
            let cat = categorize(s);
            let is_selected = i == self.selected_idx;
            let item = self.render_list_item(s, cat, is_selected, max_value_width, max_desc_width);
            list_items.push(item);
        }

        (vec![(None, list_items)], total_display)
    }

    #[allow(clippy::type_complexity)]
    fn render_grid_popup(
        &self,
        visible_lines: usize,
        cols: usize,
        col_width: usize,
    ) -> (
        Vec<(
            Option<CompletionCategory>,
            Vec<ratatui::widgets::ListItem<'static>>,
        )>,
        usize,
    ) {
        let total_items = self.suggestions.len();
        let total_rows = total_items.div_ceil(cols);

        let vis_start = self.scroll_offset.min(total_rows);
        let vis_end = (self.scroll_offset + visible_lines).min(total_rows);

        let mut list_items = Vec::with_capacity(vis_end.saturating_sub(vis_start));

        for r in vis_start..vis_end {
            let mut spans = Vec::new();
            for c in 0..cols {
                let idx = r * cols + c;
                if idx < total_items {
                    let s = &self.suggestions[idx];
                    let cat = categorize(s);
                    let is_selected = idx == self.selected_idx;
                    let cell_spans = self.render_grid_cell(s, cat, is_selected, col_width);
                    spans.extend(cell_spans);
                } else {
                    spans.push(Span::raw(" ".repeat(col_width)));
                }
            }
            list_items.push(ratatui::widgets::ListItem::new(Line::from(spans)));
        }

        (vec![(None, list_items)], total_rows)
    }

    fn render_grid_cell(
        &self,
        suggestion: &CompletionCandidate,
        category: CompletionCategory,
        is_selected: bool,
        col_width: usize,
    ) -> Vec<Span<'static>> {
        let t = &self.theme;
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
        } else {
            match category {
                CompletionCategory::Directory | CompletionCategory::File => {
                    if let Some(ls) = self.lscolors.style_for_path(&suggestion.value) {
                        self.convert_lscolors_style(ls, false)
                    } else {
                        category.value_style(&t.completions)
                    }
                }
                _ => category.value_style(&t.completions),
            }
        };

        let highlight_style = if is_selected {
            Style::default()
                .bg(selection_bg)
                .fg(selection_fg)
                .add_modifier(StyleModifier::BOLD | StyleModifier::UNDERLINED)
        } else {
            base_val_style.add_modifier(StyleModifier::BOLD | StyleModifier::UNDERLINED)
        };

        let max_val_w = col_width.saturating_sub(3);
        let display_val = if suggestion.value.width() > max_val_w && max_val_w > 3 {
            truncate_by_width(&suggestion.value, max_val_w)
        } else {
            suggestion.value.clone()
        };
        let val_w = display_val.width();

        let val_spans = render_highlighted_spans(
            &display_val,
            suggestion.match_indices.as_deref(),
            base_val_style,
            highlight_style,
        );

        let pad_w = col_width.saturating_sub(2 + val_w);
        let pad_span = Span::styled(" ".repeat(pad_w), base_bg);

        let mut out = vec![indicator];
        out.extend(val_spans);
        out.push(pad_span);
        out
    }

    fn render_list_item(
        &self,
        suggestion: &CompletionCandidate,
        category: CompletionCategory,
        is_selected: bool,
        max_value_width: usize,
        max_desc_width: usize,
    ) -> ratatui::widgets::ListItem<'static> {
        let value = &suggestion.value;
        let t = &self.theme;

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
        } else {
            match category {
                CompletionCategory::Directory | CompletionCategory::File => {
                    if let Some(ls) = self.lscolors.style_for_path(value) {
                        self.convert_lscolors_style(ls, false)
                    } else {
                        category.value_style(&t.completions)
                    }
                }
                _ => category.value_style(&t.completions),
            }
        };

        let highlight_style = if is_selected {
            Style::default()
                .bg(selection_bg)
                .fg(selection_fg)
                .add_modifier(StyleModifier::BOLD | StyleModifier::UNDERLINED)
        } else {
            base_val_style.add_modifier(StyleModifier::BOLD | StyleModifier::UNDERLINED)
        };

        let display_val = if value.width() > max_value_width && max_value_width > 5 {
            truncate_by_width(value, max_value_width.saturating_sub(1))
        } else {
            value.clone()
        };
        let display_width = display_val.width();

        let val_spans = render_highlighted_spans(
            &display_val,
            suggestion.match_indices.as_deref(),
            base_val_style,
            highlight_style,
        );

        let mut spans = vec![indicator];
        spans.extend(val_spans);

        let desc_opt = match &suggestion.description {
            Some(desc) if !desc.is_empty() && desc != "Directory" && desc != "File" => {
                Some(truncate_by_width(desc, max_desc_width))
            }
            _ => {
                if !matches!(
                    category,
                    CompletionCategory::Directory | CompletionCategory::File
                ) {
                    Some(category.label().to_string())
                } else {
                    None
                }
            }
        };

        if let Some(desc_text) = desc_opt {
            let pad_needed = max_value_width.saturating_sub(display_width);
            let spacing = " ".repeat(pad_needed.max(2));

            let desc_text_style = if is_selected {
                Style::default()
                    .bg(selection_bg)
                    .fg(selection_fg)
                    .add_modifier(StyleModifier::DIM)
            } else {
                t.completions.description.to_style_dim()
            };

            spans.push(Span::styled(spacing, base_bg));
            spans.push(Span::styled(desc_text, desc_text_style));
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
}
