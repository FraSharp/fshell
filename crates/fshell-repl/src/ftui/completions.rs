// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::FshellCompleter;
use crate::theme_ext::ThemeColorRatatui;
use fshell_core::theme::{CompletionsTheme, Theme};
use fshell_engine::Env;
use lscolors::{LsColors, Style as LsStyle};
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as NucleoConfig, Matcher, Utf32String};
use ratatui::style::{Color, Modifier as StyleModifier, Style};
use ratatui::text::{Line, Span};
use reedline::{Completer, Suggestion};
use std::path::Path;
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

/// Category groups for rendering completions with headers
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
    pub fn icon(self) -> &'static str {
        match self {
            CompletionCategory::Directory => "d",
            CompletionCategory::File => "f",
            CompletionCategory::Command => "!",
            CompletionCategory::Builtin => "*",
            CompletionCategory::Alias => "@",
            CompletionCategory::Function => "ƒ",
            CompletionCategory::Variable => "$",
            CompletionCategory::Job => "%",
            CompletionCategory::Flag => "—",
            CompletionCategory::Pipeline => "▸",
            CompletionCategory::Keyword => "kw",
            CompletionCategory::History => "#",
            CompletionCategory::Ref => "&",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            CompletionCategory::Directory => "Dirs",
            CompletionCategory::File => "Files",
            CompletionCategory::Command => "Commands",
            CompletionCategory::Builtin => "Builtins",
            CompletionCategory::Alias => "Aliases",
            CompletionCategory::Function => "Functions",
            CompletionCategory::Variable => "Variables",
            CompletionCategory::Job => "Jobs",
            CompletionCategory::Flag => "Flags",
            CompletionCategory::Pipeline => "Pipeline",
            CompletionCategory::Keyword => "Keywords",
            CompletionCategory::History => "History",
            CompletionCategory::Ref => "References",
        }
    }

    pub fn header_style(self, theme: &CompletionsTheme) -> Style {
        match self {
            CompletionCategory::Directory => theme.header_directory.to_style_dim(),
            CompletionCategory::File => theme.header_file.to_style_dim(),
            CompletionCategory::Command => theme.header_command.to_style_dim(),
            CompletionCategory::Builtin => theme.header_builtin.to_style_dim(),
            CompletionCategory::Alias => theme.header_alias.to_style_dim(),
            CompletionCategory::Function => theme.header_function.to_style_dim(),
            CompletionCategory::Variable => theme.header_variable.to_style_dim(),
            CompletionCategory::Flag => theme.header_flag.to_style_dim(),
            CompletionCategory::Pipeline => theme.header_pipeline.to_style_dim(),
            CompletionCategory::Keyword => theme.header_keyword.to_style_dim(),
            CompletionCategory::Job => theme.header_job.to_style_dim(),
            CompletionCategory::History => theme.header_history.to_style_dim(),
            CompletionCategory::Ref => theme.header_ref.to_style_dim(),
        }
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

/// A categorized suggestion with its group info
#[derive(Debug, Clone)]
pub struct CategorizedSuggestion {
    pub suggestion: Suggestion,
    pub category: CompletionCategory,
}

impl CategorizedSuggestion {
    fn from_suggestion(s: Suggestion) -> Self {
        let cat = categorize(&s);
        Self {
            suggestion: s,
            category: cat,
        }
    }
}

fn categorize(s: &Suggestion) -> CompletionCategory {
    let v = &s.value;
    let desc = s.description.as_deref().unwrap_or("");

    if v.starts_with('$') {
        return CompletionCategory::Variable;
    }
    if v.starts_with('%') {
        return CompletionCategory::Job;
    }
    if v.starts_with("--") || v.starts_with('-') && v.len() > 1 {
        return CompletionCategory::Flag;
    }
    // Path detection: directories end with /, files don't
    if v.ends_with('/') {
        return CompletionCategory::Directory;
    }
    if v.contains('/') || v.starts_with('.') || v.starts_with('~') {
        return CompletionCategory::File;
    }
    if Path::new(v).exists() {
        if Path::new(v).is_dir() {
            return CompletionCategory::Directory;
        }
        return CompletionCategory::File;
    }
    if desc.contains("History") || desc.contains("use") {
        return CompletionCategory::History;
    }
    if desc.contains("Function") || desc == "User-defined function" {
        return CompletionCategory::Function;
    }
    if desc.contains("Alias") || desc.starts_with("->") {
        return CompletionCategory::Alias;
    }
    if desc == "Keyword" || desc == "keyword" {
        return CompletionCategory::Keyword;
    }
    if desc == "Built-in command" || desc == "built-in" || desc.contains("builtin") {
        return CompletionCategory::Builtin;
    }
    if desc.contains("frecency") || desc.contains("jump") || desc.contains("cd ") {
        return CompletionCategory::Directory;
    }
    if desc.contains("pipeline")
        || desc.contains("boundary")
        || matches!(
            v.as_str(),
            "filter"
                | "map"
                | "sort"
                | "grep"
                | "count"
                | "limit"
                | "@json"
                | "@yaml"
                | "@msgpack"
                | "@text"
                | "@csv"
                | "@table"
                | "@bar"
        )
    {
        return CompletionCategory::Pipeline;
    }
    if desc.contains("ref")
        || desc.contains("branch")
        || desc.contains("tag")
        || desc.contains("git")
    {
        return CompletionCategory::Ref;
    }
    // If description mentions "command", or common cmd patterns
    if desc.contains("command") || desc.contains("cmd") {
        return CompletionCategory::Command;
    }
    // Fallback: check known command lists
    if let Some(cmd) = v.strip_suffix(' ')
        && crate::COMMON_EXTERNAL_COMMANDS
            .iter()
            .any(|(n, _)| *n == cmd)
    {
        return CompletionCategory::Command;
    }
    // Default to Command for non-path, non-special suggestions
    CompletionCategory::Command
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
    fn new(raw: Vec<Suggestion>) -> Self {
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
    pub suggestions: Vec<Suggestion>,
    pub grouped: Option<GroupedSuggestions>,
    /// Full unfiltered suggestion list from the completer (never mutated by filter)
    pub all_suggestions: Vec<Suggestion>,
    /// Current partial word being filtered against
    pub filter_query: String,
    /// Reusable nucleo matcher instance
    nucleo_matcher: Matcher,
    pub selected_idx: usize,
    pub scroll_offset: usize,
    pub visible: bool,
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
        if line.trim().is_empty() {
            if force_visible {
                // Tab on empty line — show curated command list
                let builtins = self.completer.env.get_all_builtins();
                let mut raw: Vec<reedline::Suggestion> = builtins
                    .into_iter()
                    .map(|b| {
                        let desc = crate::command_description(&b)
                            .unwrap_or("Built-in command")
                            .to_string();
                        reedline::Suggestion {
                            value: b,
                            description: Some(desc),
                            extra: None,
                            span: reedline::Span::new(0, 0),
                            append_whitespace: true,
                            style: None,
                            display_override: None,
                            match_indices: None,
                        }
                    })
                    .collect();

                for (cmd, desc) in crate::COMMON_EXTERNAL_COMMANDS {
                    if !raw.iter().any(|s| s.value == *cmd) {
                        raw.push(reedline::Suggestion {
                            value: cmd.to_string(),
                            description: Some(desc.to_string()),
                            extra: None,
                            span: reedline::Span::new(0, 0),
                            append_whitespace: true,
                            style: None,
                            display_override: None,
                            match_indices: None,
                        });
                    }
                }

                if let Ok(entries) = crate::history::query_frequent_by_prefix("", 10) {
                    for (cmd, freq) in &entries {
                        if !raw.iter().any(|s| s.value == *cmd) {
                            raw.push(reedline::Suggestion {
                                value: cmd.clone(),
                                description: Some(format!(
                                    "History ({} use{})",
                                    freq,
                                    if *freq == 1 { "" } else { "s" }
                                )),
                                extra: None,
                                span: reedline::Span::new(0, 0),
                                append_whitespace: false,
                                style: None,
                                display_override: None,
                                match_indices: None,
                            });
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
                self.visible = true;
            } else {
                self.suggestions.clear();
                self.grouped = None;
                self.all_suggestions.clear();
                self.selected_idx = 0;
                self.scroll_offset = 0;
                self.visible = false;
            }
            return;
        }

        // Skip full completer when completions aren't visible and not explicitly requested
        if !force_visible && !self.visible {
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

        if self.all_suggestions.is_empty() {
            self.visible = false;
            self.selected_idx = 0;
            self.scroll_offset = 0;
        } else if force_visible {
            self.visible = true;
            if self.selected_idx >= self.suggestions.len() {
                self.selected_idx = 0;
                self.scroll_offset = 0;
            }
        } else if self.visible && self.selected_idx >= self.suggestions.len() {
            self.selected_idx = 0;
            self.scroll_offset = 0;
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

    /// Fuzzy-filter `all_suggestions` against `partial` word, updating
    /// `suggestions` and `grouped`. Does NOT call the completer backend.
    /// Resets `prefix_accepted` so Tab can fill the new LCP after narrowing.
    pub fn filter(&mut self, partial: &str) {
        // Long-term fix for audit R8: LCP state must follow the filtered set,
        // not the initial one. Any filter change invalidates prior prefix_accept.
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

        // Score all items, keep only those that match
        let mut scored: Vec<(u32, usize, &Suggestion)> = self
            .all_suggestions
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let haystack = Utf32String::from(s.value.as_str());
                let score = pattern.score(haystack.slice(..), &mut self.nucleo_matcher)?;
                Some((score, i, s))
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

        self.suggestions = scored.into_iter().map(|(_, _, s)| s.clone()).collect();

        self.grouped = if self.suggestions.is_empty() {
            None
        } else {
            let grouped = GroupedSuggestions::new(self.suggestions.clone());
            // Re-order suggestions to match grouped.items order so that
            // selected_idx is consistent between suggestions and grouped navigation.
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

    /// Advance selection by `page_size` visible display lines, accounting for group headers.
    /// In grouped mode, headers consume display rows that don't correspond to suggestion indices.
    /// We walk forward suggestion-by-suggestion, counting the display lines consumed (including headers),
    /// until we've consumed at least `page_size` display lines.
    pub fn page_down(&mut self, page_size: usize) {
        if self.suggestions.is_empty() {
            return;
        }
        let page_size = page_size.max(1);
        if let Some(ref grouped) = self.grouped {
            let group_sizes = grouped.group_sizes();
            let mut display_consumed = 0usize;
            let mut target_idx = self.selected_idx;
            // Walk forward through groups
            for (gi, count) in group_sizes.iter().enumerate() {
                let group_start: usize = group_sizes[..gi].iter().sum();
                let group_end = group_start + count;
                if target_idx < group_start {
                    // Haven't reached this group yet — skip header too
                    continue;
                }
                if target_idx >= group_end {
                    // Past this group — skip
                    continue;
                }
                // We're inside this group
                let remaining_in_group = group_end - target_idx;
                let display_needed = page_size.saturating_sub(display_consumed);
                if display_needed <= remaining_in_group {
                    target_idx += display_needed;
                    display_consumed = page_size;
                    break;
                } else {
                    target_idx = group_end; // end of this group
                    display_consumed += remaining_in_group;
                    // Add 1 for this group's header (we'll "consume" it when we enter next group)
                    // Actually we already counted the header at the start — the header for next group
                    // is only consumed when we step into it
                    if gi + 1 < group_sizes.len() {
                        // To 'enter' the next group, we must consume 1 display line (its header)
                        if display_consumed + 1 >= page_size {
                            // We'd land on the next group's header — pick first item of next group
                            target_idx = group_end; // first item of next group
                            display_consumed = page_size;
                            break;
                        }
                        display_consumed += 1; // consume the header
                        // Continue to next group
                    }
                }
            }
            if display_consumed < page_size {
                // Hit the end — go to last item
                target_idx = self.suggestions.len().saturating_sub(1);
            }
            self.selected_idx = target_idx.min(self.suggestions.len().saturating_sub(1));
        } else {
            // Ungrouped: simple advance
            let new_idx = self.selected_idx.saturating_add(page_size);
            self.selected_idx = new_idx.min(self.suggestions.len().saturating_sub(1));
        }
    }

    /// Move selection backward by `page_size` visible display lines, accounting for group headers.
    pub fn page_up(&mut self, page_size: usize) {
        if self.suggestions.is_empty() {
            return;
        }
        let page_size = page_size.max(1);
        if let Some(ref grouped) = self.grouped {
            let group_sizes = grouped.group_sizes();
            let mut display_consumed = 0usize;
            let mut target_idx = self.selected_idx;
            // Walk backward through groups (reversed iteration)
            for gi in (0..group_sizes.len()).rev() {
                let count = group_sizes[gi];
                let group_start: usize = group_sizes[..gi].iter().sum();
                let group_end = group_start + count;
                if target_idx >= group_end {
                    // Past this group — haven't reached it yet going backward
                    continue;
                }
                if target_idx < group_start {
                    // Before this group — skip
                    continue;
                }
                // We're inside (or at the boundary of) this group
                let offset_in_group = target_idx - group_start;
                // Items we can move back within this group
                let display_needed = page_size.saturating_sub(display_consumed);
                if display_needed <= offset_in_group {
                    target_idx -= display_needed;
                    display_consumed = page_size;
                    break;
                } else {
                    display_consumed += offset_in_group;
                    target_idx = group_start; // back to start of this group
                    // Consume header to go to previous group
                    if gi > 0 && display_consumed + 1 >= page_size {
                        // Land on last item of previous group
                        let prev_group_end: usize = group_sizes[..gi].iter().sum();
                        target_idx = prev_group_end.saturating_sub(1);
                        display_consumed = page_size;
                        break;
                    }
                    if gi > 0 {
                        display_consumed += 1; // consume this group's header
                        // Move to end of previous group for next iteration
                        let prev_group_end: usize = group_sizes[..gi].iter().sum();
                        target_idx = prev_group_end.saturating_sub(1);
                        // Continue backward
                    }
                }
            }
            if display_consumed < page_size {
                // Hit the start
                target_idx = 0;
            }
            self.selected_idx = target_idx;
        } else {
            // Ungrouped: simple retreat
            self.selected_idx = self.selected_idx.saturating_sub(page_size);
        }
    }

    pub fn get_selected_suggestion(&self) -> Option<&Suggestion> {
        if self.visible && !self.suggestions.is_empty() {
            self.suggestions.get(self.selected_idx)
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.suggestions.clear();
        self.grouped = None;
        self.selected_idx = 0;
        self.scroll_offset = 0;
        self.visible = false;
        self.active_selection = false;
        self.prefix_accepted = false;
    }

    /// After a completion has been applied to the buffer, decide whether to keep
    /// the menu visible (drill into directory) or close it (file/final completion).
    ///
    /// If the text under the cursor ends with `/` (directory completion), the
    /// menu refreshes to show the directory's contents.  For any other case
    /// (file, flag, command, …) the menu is dismissed as before.
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

    /// Map a raw suggestion index to its flat display index, accounting for
    /// group header lines (each group header is +1 display line).
    pub fn flat_index_of(&self, raw_idx: usize) -> usize {
        let Some(ref grouped) = self.grouped else {
            return raw_idx;
        };
        if raw_idx >= grouped.items.len() {
            return grouped.display_lines();
        }
        let group_sizes = grouped.group_sizes();
        let mut flat = 0usize;
        let mut seen = 0usize;
        for count in &group_sizes {
            flat += 1; // header
            if raw_idx >= seen && raw_idx < seen + count {
                return flat + (raw_idx - seen);
            }
            flat += count;
            seen += count;
        }
        flat
    }

    /// Compute the longest common prefix among all suggestion values
    pub fn longest_common_prefix(&self) -> Option<String> {
        if self.suggestions.len() <= 1 {
            return None;
        }
        let values: Vec<&str> = self.suggestions.iter().map(|s| s.value.as_str()).collect();
        let first = values.first()?;
        let mut end = first.len();
        for other in &values[1..] {
            let common = first
                .chars()
                .zip(other.chars())
                .take_while(|(a, b)| a == b)
                .count();
            end = end.min(common);
        }
        if end == 0 {
            None
        } else {
            Some(first[..end].to_string())
        }
    }

    /// Render the popup content as grouped ListItems.
    /// Only builds items that fall within the visible window to avoid
    /// allocating thousands of ListItem objects for large completion lists.
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
        usize, /* total display lines */
    ) {
        let Some(ref grouped) = self.grouped else {
            return (Vec::new(), 0);
        };
        if grouped.items.is_empty() {
            return (Vec::new(), 0);
        }

        // Reserve at least 14 cols for descriptions, or 38% of width for wider popups
        let max_desc_width = if area_width > 70 {
            ((area_width as f64) * 0.38).min(36.0) as usize
        } else {
            ((area_width as usize).saturating_sub(20) / 2).clamp(14, 22)
        };
        let max_value_width = (area_width as usize).saturating_sub(max_desc_width + 6);

        let total_display = grouped.display_lines();

        // Determine which flat display lines are visible
        let vis_start = self.scroll_offset;
        let vis_end = (self.scroll_offset + visible_lines).min(total_display);

        let mut sections: Vec<(Option<CompletionCategory>, Vec<ratatui::widgets::ListItem>)> =
            Vec::new();
        let group_sizes = grouped.group_sizes();

        let mut flat_idx = 0;
        let mut current_items: Vec<ratatui::widgets::ListItem> = Vec::new();

        // Helper: flush current_items into sections and clear it
        macro_rules! flush_section {
            ($cat:expr) => {
                if !current_items.is_empty() {
                    sections.push(($cat, std::mem::take(&mut current_items)));
                }
            };
        }

        for (gi, cat) in grouped.groups.iter().enumerate() {
            let count = group_sizes[gi];

            // Header line
            if flat_idx >= vis_end {
                break;
            }
            if flat_idx >= vis_start {
                flush_section!(None);
                let header = self.render_header(*cat, count);
                current_items.push(header);
            }
            flat_idx += 1;

            // Items in this group
            let range_start = grouped
                .group_indices
                .iter()
                .position(|g| *g == gi)
                .unwrap_or(0);

            for i in range_start..range_start + count {
                if flat_idx >= vis_end {
                    break;
                }
                let item = &grouped.items[i];
                if flat_idx >= vis_start {
                    // Bug 3.3: Use actual suggestion index `i`, not `flat_idx` which
                    // includes group header lines. `self.selected_idx` is a raw suggestion
                    // index into `self.suggestions` (reordered to match grouped.items order).
                    let is_selected = i == self.selected_idx;
                    let list_item = self.render_list_item(
                        &item.suggestion,
                        item.category,
                        is_selected,
                        max_value_width,
                        max_desc_width,
                    );
                    current_items.push(list_item);
                }
                flat_idx += 1;
            }
            if flat_idx >= vis_end {
                break;
            }
        }
        flush_section!(None);

        (sections, total_display)
    }

    fn render_header(
        &self,
        cat: CompletionCategory,
        count: usize,
    ) -> ratatui::widgets::ListItem<'static> {
        let style = cat.header_style(&self.theme.completions);
        let icon = cat.icon();
        let name = cat.name();
        let text = format!(" {} {}  ({})", icon, name, count);
        ratatui::widgets::ListItem::new(Line::from(Span::styled(text, style)))
    }

    #[allow(clippy::too_many_arguments)]
    fn render_list_item(
        &self,
        suggestion: &Suggestion,
        category: CompletionCategory,
        is_selected: bool,
        max_value_width: usize,
        max_desc_width: usize,
    ) -> ratatui::widgets::ListItem<'static> {
        let value = &suggestion.value;
        let t = &self.theme;

        let selection_bg = t.widgets.item_selected_bg.to_ratatui_color();
        let selection_fg = t.widgets.item_selected_fg.to_ratatui_color();

        let base_style = if is_selected {
            Style::default().bg(selection_bg)
        } else {
            Style::default()
        };

        // Determine value style based on category/path
        let value_style = if is_selected {
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

        // Selection indicator
        let indicator = if is_selected {
            Span::styled("▸ ", Style::default().bg(selection_bg).fg(selection_fg))
        } else {
            Span::raw("  ")
        };

        // Truncate value by display width (Bug 8.2 fix)
        let display_val = if value.width() > max_value_width && max_value_width > 5 {
            truncate_by_width(value, max_value_width.saturating_sub(1))
        } else {
            value.clone()
        };
        let display_width = display_val.width();

        let display_span = Span::styled(
            display_val,
            if is_selected {
                value_style.bg(selection_bg)
            } else {
                value_style
            },
        );

        let mut spans = vec![indicator, display_span];

        // Right-aligned description — truncate by display width (Bug 3.2 fix)
        if let Some(desc) = &suggestion.description {
            let trim_desc = truncate_by_width(desc, max_desc_width);
            if !trim_desc.is_empty() {
                let pad_needed = max_value_width.saturating_sub(display_width);
                let spacing = " ".repeat(pad_needed);

                let desc_style = if is_selected {
                    Style::default().bg(selection_bg).fg(selection_fg)
                } else {
                    t.completions.description.to_style()
                };

                let text_style = if is_selected {
                    Style::default().bg(selection_bg).fg(selection_fg)
                } else {
                    t.widgets.foreground.to_style()
                };

                spans.push(Span::styled(spacing, base_style));
                spans.push(Span::styled("┆ ", desc_style));
                spans.push(Span::styled(trim_desc, text_style));
            }
        }

        ratatui::widgets::ListItem::new(Line::from(spans))
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

    pub fn format_suggestion(&self, suggestion: &Suggestion, is_selected: bool) -> Line<'static> {
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
    suggestion: &Suggestion,
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
    let before_cursor = &line[..byte_idx];
    before_cursor
        .split(|c: char| c.is_whitespace() || c == '|' || c == '>' || c == '<')
        .next_back()
        .unwrap_or("")
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
