// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Modern interactive item selector builtin for fshell.
//!
//! Supports streaming pipeline input, keyboard navigation, fuzzy filtering,
//! single- and multi-item selection, and preserves structured `Val`s for downstream stages.

use std::sync::Arc;

use futures::stream::{self, BoxStream, StreamExt};
use miette::SourceSpan;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState, StatefulWidget,
};
use unicode_width::UnicodeWidthStr;

use fshell_core::diagnostic::ErrorCode;
use fshell_core::theme::{Theme, ThemeColor};
use fshell_core::{ShellError, Val};
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use fshell_terminal::input::{InputEvent, Key, KeyAction, Modifiers, MouseAction, UnixEventStream};
use fshell_terminal::runner::{AppFlow, ShellTuiApp, run_tui};
use fshell_terminal::session::{
    TerminalDevice, TerminalMode, TerminalSession, TerminalSessionOptions,
};

/// Format any structured `Val` into a readable, concise display string.
pub fn format_val_for_display(val: &Val) -> String {
    match val {
        Val::String(s) => s.clone(),
        Val::Map(map) => {
            let primary_keys = [
                "name", "title", "command", "path", "id", "pid", "key", "branch",
            ];
            let mut primary = None;
            for pk in primary_keys {
                let u = ustr::ustr(pk);
                if let Some(v) = map.get(&u) {
                    primary = Some((pk, v.to_text()));
                    break;
                }
            }
            if let Some((pk, pv)) = primary {
                let rest: Vec<String> = map
                    .iter()
                    .filter(|(k, _)| k.as_str() != pk)
                    .map(|(k, v)| format!("{}: {}", k, v.to_text()))
                    .collect();
                if rest.is_empty() {
                    pv
                } else {
                    format!("{} ({})", pv, rest.join(", "))
                }
            } else {
                let items: Vec<String> = map
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, v.to_text()))
                    .collect();
                items.join(", ")
            }
        }
        Val::List(list) => {
            let items: Vec<String> = list.iter().map(Val::to_text).collect();
            format!("[{}]", items.join(", "))
        }
        other => other.to_text(),
    }
}

fn to_ratatui_color(tc: &ThemeColor) -> Color {
    let (r, g, b) = tc.to_rgb();
    Color::Rgb(r, g, b)
}

/// An item in the selection list.
#[derive(Debug, Clone)]
pub struct SelectItem {
    pub value: Val,
    pub display: String,
    pub checked: bool,
}

impl SelectItem {
    pub fn new(value: Val) -> Self {
        let display = format_val_for_display(&value);
        Self {
            value,
            display,
            checked: false,
        }
    }
}

/// Incoming events accepted by the `SelectApp`.
#[derive(Debug, Clone)]
pub enum SelectMessage {
    Input(InputEvent),
    Item(Val),
    StreamEnded,
}

/// Interactive TUI application state for `select`.
pub struct SelectApp<'a> {
    pub prompt: &'a str,
    pub items: Vec<SelectItem>,
    pub theme: &'a Theme,
    pub multi: bool,
    pub query: String,
    pub selected_idx: usize,
    pub scroll_offset: usize,
    cached_filtered: Vec<usize>,
    filter_dirty: bool,
}

impl<'a> SelectApp<'a> {
    pub fn new(prompt: &'a str, initial_items: Vec<Val>, theme: &'a Theme, multi: bool) -> Self {
        let items: Vec<SelectItem> = initial_items.into_iter().map(SelectItem::new).collect();
        let cached_filtered = (0..items.len()).collect();
        Self {
            prompt,
            items,
            theme,
            multi,
            query: String::new(),
            selected_idx: 0,
            scroll_offset: 0,
            cached_filtered,
            filter_dirty: false,
        }
    }

    pub fn ensure_filtered(&mut self) {
        if self.filter_dirty {
            self.cached_filtered = self.compute_filtered();
            self.filter_dirty = false;
        }
    }

    fn compute_filtered(&self) -> Vec<usize> {
        if self.query.is_empty() {
            return (0..self.items.len()).collect();
        }
        let q = self.query.to_lowercase();
        let mut scored: Vec<(usize, i64)> = Vec::new();
        for (i, item) in self.items.iter().enumerate() {
            let text_lower = item.display.to_lowercase();
            if let Some(pos) = text_lower.find(&q) {
                let score = 1000 - (pos as i64 * 10);
                scored.push((i, score));
            } else {
                let mut it = text_lower.chars();
                let mut matched = true;
                for qc in q.chars() {
                    if !it.any(|tc| tc == qc) {
                        matched = false;
                        break;
                    }
                }
                if matched {
                    scored.push((i, 100));
                }
            }
        }
        scored.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
        scored.into_iter().map(|(i, _)| i).collect()
    }

    pub fn filtered_indices(&self) -> Vec<usize> {
        if !self.filter_dirty {
            self.cached_filtered.clone()
        } else {
            self.compute_filtered()
        }
    }
}

impl<'a> ShellTuiApp for SelectApp<'a> {
    type Message = SelectMessage;
    type Output = Vec<Val>;

    fn handle_message(&mut self, msg: Self::Message) -> AppFlow<Self::Output> {
        match msg {
            SelectMessage::Item(val) => {
                self.items.push(SelectItem::new(val));
                self.filter_dirty = true;
                AppFlow::Continue
            }
            SelectMessage::StreamEnded => AppFlow::Ignore,
            SelectMessage::Input(event) => match event {
                InputEvent::Mouse(mouse) => {
                    self.ensure_filtered();
                    let filtered_len = self.cached_filtered.len();
                    match mouse.action {
                        MouseAction::ScrollUp => {
                            if filtered_len > 0 {
                                self.selected_idx = self.selected_idx.saturating_sub(3);
                            }
                            AppFlow::Continue
                        }
                        MouseAction::ScrollDown => {
                            if filtered_len > 0 {
                                self.selected_idx =
                                    (self.selected_idx + 3).min(filtered_len.saturating_sub(1));
                            }
                            AppFlow::Continue
                        }
                        _ => AppFlow::Ignore,
                    }
                }
                InputEvent::Key(key) => {
                    if key.action == KeyAction::Release {
                        return AppFlow::Ignore;
                    }

                    self.ensure_filtered();
                    let filtered_len = self.cached_filtered.len();
                    let target_idx = if filtered_len > 0 {
                        Some(self.cached_filtered[self.selected_idx.min(filtered_len - 1)])
                    } else {
                        None
                    };

                    match key.key {
                        Key::Escape => AppFlow::Break(Vec::new()),
                        Key::Character('c') if key.modifiers.contains(Modifiers::CONTROL) => {
                            AppFlow::Break(Vec::new())
                        }
                        Key::Character('g') if key.modifiers.contains(Modifiers::CONTROL) => {
                            AppFlow::Break(Vec::new())
                        }
                        Key::Enter => {
                            let Some(idx) = target_idx else {
                                return AppFlow::Break(Vec::new());
                            };
                            if self.multi {
                                let checked: Vec<Val> = self
                                    .items
                                    .iter()
                                    .filter(|it| it.checked)
                                    .map(|it| it.value.clone())
                                    .collect();
                                if !checked.is_empty() {
                                    return AppFlow::Break(checked);
                                }
                            }
                            AppFlow::Break(vec![self.items[idx].value.clone()])
                        }
                        Key::Character(' ') | Key::Tab if self.multi => {
                            if let Some(idx) = target_idx {
                                self.items[idx].checked = !self.items[idx].checked;
                                if self.selected_idx + 1 < filtered_len {
                                    self.selected_idx += 1;
                                }
                                AppFlow::Continue
                            } else {
                                AppFlow::Ignore
                            }
                        }
                        Key::Up | Key::Character('p')
                            if key.modifiers.contains(Modifiers::CONTROL) =>
                        {
                            self.selected_idx = self.selected_idx.saturating_sub(1);
                            AppFlow::Continue
                        }
                        Key::Up => {
                            self.selected_idx = self.selected_idx.saturating_sub(1);
                            AppFlow::Continue
                        }
                        Key::Down | Key::Character('n')
                            if key.modifiers.contains(Modifiers::CONTROL) =>
                        {
                            if filtered_len > 0 && self.selected_idx + 1 < filtered_len {
                                self.selected_idx += 1;
                            }
                            AppFlow::Continue
                        }
                        Key::Down => {
                            if filtered_len > 0 && self.selected_idx + 1 < filtered_len {
                                self.selected_idx += 1;
                            }
                            AppFlow::Continue
                        }
                        Key::Home | Key::Character('a')
                            if key.modifiers.contains(Modifiers::CONTROL) =>
                        {
                            self.selected_idx = 0;
                            AppFlow::Continue
                        }
                        Key::Home => {
                            self.selected_idx = 0;
                            AppFlow::Continue
                        }
                        Key::End | Key::Character('e')
                            if key.modifiers.contains(Modifiers::CONTROL) =>
                        {
                            self.selected_idx = filtered_len.saturating_sub(1);
                            AppFlow::Continue
                        }
                        Key::End => {
                            self.selected_idx = filtered_len.saturating_sub(1);
                            AppFlow::Continue
                        }
                        Key::PageUp => {
                            self.selected_idx = self.selected_idx.saturating_sub(10);
                            AppFlow::Continue
                        }
                        Key::PageDown => {
                            if filtered_len > 0 {
                                self.selected_idx =
                                    (self.selected_idx + 10).min(filtered_len.saturating_sub(1));
                            }
                            AppFlow::Continue
                        }
                        Key::Backspace => {
                            if self.query.pop().is_some() {
                                self.filter_dirty = true;
                                self.selected_idx = 0;
                                AppFlow::Continue
                            } else {
                                AppFlow::Ignore
                            }
                        }
                        Key::Character('w') if key.modifiers.contains(Modifiers::CONTROL) => {
                            let trimmed = self.query.trim_end();
                            if let Some(pos) = trimmed.rfind(' ') {
                                self.query.truncate(pos + 1);
                            } else {
                                self.query.clear();
                            }
                            self.filter_dirty = true;
                            self.selected_idx = 0;
                            AppFlow::Continue
                        }
                        Key::Character('u') if key.modifiers.contains(Modifiers::CONTROL) => {
                            self.query.clear();
                            self.filter_dirty = true;
                            self.selected_idx = 0;
                            AppFlow::Continue
                        }
                        Key::Character(c) => {
                            self.query.push(c);
                            self.filter_dirty = true;
                            self.selected_idx = 0;
                            AppFlow::Continue
                        }
                        _ => AppFlow::Ignore,
                    }
                }
                _ => AppFlow::Ignore,
            },
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect) {
        if area.width < 10 || area.height < 4 {
            return;
        }

        self.ensure_filtered();
        let filtered = self.cached_filtered.clone();
        if filtered.is_empty() {
            self.selected_idx = 0;
        } else if self.selected_idx >= filtered.len() {
            self.selected_idx = filtered.len().saturating_sub(1);
        }

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(2),
                Constraint::Length(1),
            ])
            .split(area);

        // 1. Search / Title Header
        let focused_border_color = to_ratatui_color(&self.theme.widgets.border_focus);
        let border_color = to_ratatui_color(&self.theme.widgets.border);
        let title_color = to_ratatui_color(&self.theme.widgets.title);
        let fg_color = to_ratatui_color(&self.theme.chrome.foreground);
        let selected_bg = to_ratatui_color(&self.theme.widgets.item_selected_bg);
        let selected_fg = to_ratatui_color(&self.theme.widgets.item_selected_fg);
        let match_color = to_ratatui_color(&self.theme.status.warning);
        let success_color = to_ratatui_color(&self.theme.status.ok);
        let muted_color = to_ratatui_color(&self.theme.status.info);

        let checked_count = self.items.iter().filter(|it| it.checked).count();
        let count_tag = if self.multi {
            format!(
                " {}/{} selected ({}/{}) ",
                checked_count,
                self.items.len(),
                filtered.len(),
                self.items.len()
            )
        } else {
            format!(" {}/{} ", filtered.len(), self.items.len())
        };

        let header_title = format!(" {} ", self.prompt.trim());
        let search_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(focused_border_color))
            .title(header_title)
            .title_style(
                Style::default()
                    .fg(title_color)
                    .add_modifier(Modifier::BOLD),
            );

        let search_inner = search_block.inner(chunks[0]);
        frame.render_widget(search_block, chunks[0]);

        // Right-aligned count info in header border
        let tag_len = count_tag.width() as u16;
        if chunks[0].width > tag_len + 4 {
            let tag_area = Rect::new(
                chunks[0].x + chunks[0].width.saturating_sub(tag_len + 2),
                chunks[0].y,
                tag_len,
                1,
            );
            frame.render_widget(
                Paragraph::new(Span::styled(count_tag, Style::default().fg(muted_color))),
                tag_area,
            );
        }

        // Search bar content inside header
        let search_line = if self.query.is_empty() {
            Line::from(vec![
                Span::styled(
                    "> ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("Type to filter...", Style::default().fg(muted_color)),
            ])
        } else {
            Line::from(vec![
                Span::styled(
                    "> ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(&self.query, Style::default().fg(fg_color)),
            ])
        };
        frame.render_widget(Paragraph::new(search_line), search_inner);

        // 2. Items List
        let list_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color));
        let list_inner = list_block.inner(chunks[1]);
        frame.render_widget(list_block, chunks[1]);

        let visible_height = list_inner.height as usize;
        if self.selected_idx < self.scroll_offset {
            self.scroll_offset = self.selected_idx;
        } else if self.selected_idx >= self.scroll_offset + visible_height {
            self.scroll_offset = self
                .selected_idx
                .saturating_sub(visible_height.saturating_sub(1));
        }

        let query_lower = self.query.to_lowercase();
        let list_items: Vec<ListItem> = filtered
            .iter()
            .skip(self.scroll_offset)
            .take(visible_height)
            .enumerate()
            .map(|(offset_i, &real_idx)| {
                let item = &self.items[real_idx];
                let is_cursor = self.scroll_offset + offset_i == self.selected_idx;

                let mut spans = Vec::new();
                if is_cursor {
                    spans.push(Span::styled(
                        "❯ ",
                        Style::default()
                            .fg(title_color)
                            .add_modifier(Modifier::BOLD),
                    ));
                } else {
                    spans.push(Span::raw("  "));
                }

                if self.multi {
                    if item.checked {
                        spans.push(Span::styled(
                            "[✓] ",
                            Style::default()
                                .fg(success_color)
                                .add_modifier(Modifier::BOLD),
                        ));
                    } else {
                        spans.push(Span::styled("[ ] ", Style::default().fg(muted_color)));
                    }
                }

                let text = &item.display;
                if !query_lower.is_empty() {
                    let text_lower = text.to_lowercase();
                    if let Some(pos) = text_lower.find(&query_lower) {
                        if pos > 0 {
                            spans.push(Span::raw(&text[..pos]));
                        }
                        let end = pos + query_lower.len();
                        spans.push(Span::styled(
                            &text[pos..end],
                            Style::default()
                                .fg(match_color)
                                .add_modifier(Modifier::UNDERLINED | Modifier::BOLD),
                        ));
                        if end < text.len() {
                            spans.push(Span::raw(&text[end..]));
                        }
                    } else {
                        spans.push(Span::raw(text));
                    }
                } else {
                    spans.push(Span::raw(text));
                }

                let row_style = if is_cursor {
                    Style::default().fg(selected_fg).bg(selected_bg)
                } else {
                    Style::default().fg(fg_color)
                };

                ListItem::new(Line::from(spans)).style(row_style)
            })
            .collect();

        frame.render_widget(List::new(list_items), list_inner);

        if filtered.len() > visible_height && chunks[1].width > 2 && chunks[1].height > 2 {
            let scrollbar_area = Rect::new(
                chunks[1].x + chunks[1].width.saturating_sub(1),
                chunks[1].y + 1,
                1,
                chunks[1].height.saturating_sub(2),
            );
            let mut sbar_state = ScrollbarState::new(filtered.len()).position(self.selected_idx);
            Scrollbar::default()
                .orientation(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("│"))
                .thumb_symbol("┃")
                .style(Style::default().fg(border_color))
                .thumb_style(Style::default().fg(title_color))
                .render(scrollbar_area, frame.buffer_mut(), &mut sbar_state);
        }

        // 3. Footer / Key hints
        let footer_spans = if self.multi {
            vec![
                Span::styled(
                    "[Space] ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("Toggle  "),
                Span::styled(
                    "[Enter] ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("Confirm  "),
                Span::styled(
                    "[↑/↓] ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("Navigate  "),
                Span::styled(
                    "[Esc] ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("Cancel"),
            ]
        } else {
            vec![
                Span::styled(
                    "[Enter] ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("Select  "),
                Span::styled(
                    "[↑/↓] ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("Navigate  "),
                Span::styled(
                    "[Esc] ",
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("Cancel"),
            ]
        };
        frame.render_widget(Paragraph::new(Line::from(footer_spans)), chunks[2]);
    }
}

/// Builtin handler for `select`.
pub fn select_builtin(
    in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let mut prompt = "Select item:".to_string();
    let mut multi = false;
    let mut height = 10u16;
    let mut fullscreen = false;
    let mut initial_items = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let s = match &args[i] {
            Val::String(st) => st.clone(),
            other => other.to_text(),
        };
        if s == "-p" || s == "--prompt" {
            if i + 1 < args.len() {
                i += 1;
                prompt = args[i].to_text();
            } else {
                return Err(ShellError::new(
                    ErrorCode::InvalidArgument,
                    "select: expected prompt string after -p / --prompt",
                )
                .maybe_with_span(span));
            }
        } else if s == "-m" || s == "--multi" {
            multi = true;
        } else if s == "--height" {
            if i + 1 < args.len() {
                i += 1;
                if let Ok(h) = args[i].to_text().parse::<u16>() {
                    height = h.max(4);
                }
            }
        } else if s == "--fullscreen" {
            fullscreen = true;
        } else if s == "-h" || s == "--help" {
            let help_text = "\
Usage: select [OPTIONS] [ITEMS...]
       upstream | select [OPTIONS]

Interactively choose item(s) from arguments or upstream pipeline streams.

Options:
  -p, --prompt <TEXT>   Prompt title (default: 'Select item:')
  -m, --multi           Enable multi-item selection (Space toggles, Enter confirms)
      --height <N>      Inline viewport height (default: 10)
      --fullscreen      Run in fullscreen alternate screen mode
  -h, --help            Print help information
";
            let tx_clone = tx.clone();
            tokio::spawn(async move {
                let _ = tx_clone
                    .send(PipelinePayload::Data(Arc::new(Val::String(
                        help_text.to_string(),
                    ))))
                    .await;
            });
            return Ok(());
        } else {
            initial_items.push(args[i].clone());
        }
        i += 1;
    }

    let theme = env.active_theme();

    tokio::spawn(async move {
        run_select_task(
            in_rx,
            initial_items,
            tx,
            prompt,
            multi,
            height,
            fullscreen,
            theme,
        )
        .await;
    });

    Ok(())
}

struct PipeStreamState {
    rx: PipeStream,
    buffer: std::collections::VecDeque<Val>,
}

fn extract_payload_items(payload: PipelinePayload, buffer: &mut std::collections::VecDeque<Val>) {
    match payload {
        PipelinePayload::Data(v) => match (*v).clone() {
            Val::List(items) => {
                for item in items {
                    buffer.push_back(item);
                }
            }
            other => {
                buffer.push_back(other);
            }
        },
        PipelinePayload::Bytes(b) => {
            let s = String::from_utf8_lossy(&b);
            for line in s.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    buffer.push_back(Val::String(trimmed.to_string()));
                }
            }
        }
        PipelinePayload::Structured(_) => {}
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_select_task(
    in_rx: Option<PipeStream>,
    mut initial_items: Vec<Val>,
    tx: PipeSender,
    prompt: String,
    multi: bool,
    height: u16,
    fullscreen: bool,
    theme: Arc<Theme>,
) {
    let mut pending_buffer = std::collections::VecDeque::new();
    let rx = if let Some(mut r) = in_rx {
        if fshell_engine::is_test_mode() {
            // In test mode, collect upstream items completely for deterministic test evaluation
            while let Some(payload) = r.recv().await {
                if let PipelinePayload::Structured(diag) = payload {
                    let _ = tx.send(PipelinePayload::Structured(diag)).await;
                    return;
                }
                extract_payload_items(payload, &mut pending_buffer);
            }
            None
        } else {
            // Interactive mode: wait for the first payload if empty, drain ready items, then stream
            if initial_items.is_empty() {
                if let Some(payload) = r.recv().await {
                    if let PipelinePayload::Structured(diag) = payload {
                        let _ = tx.send(PipelinePayload::Structured(diag)).await;
                        return;
                    }
                    extract_payload_items(payload, &mut pending_buffer);
                    while let Ok(payload) = r.try_recv() {
                        extract_payload_items(payload, &mut pending_buffer);
                    }
                }
            } else {
                while let Ok(payload) = r.try_recv() {
                    extract_payload_items(payload, &mut pending_buffer);
                }
            }
            Some(r)
        }
    } else {
        None
    };

    while let Some(item) = pending_buffer.pop_front() {
        initial_items.push(item);
    }

    if initial_items.is_empty() && rx.is_none() {
        return;
    }

    if fshell_engine::is_test_mode() {
        if multi
            && let Some(indices_os) = fshell_core::get_var("FSH_TEST_SELECT_INDICES")
            && let Ok(indices_str) = indices_os.into_string()
        {
            let mut selected = Vec::new();
            for part in indices_str.split(',') {
                if let Ok(idx) = part.trim().parse::<usize>()
                    && idx < initial_items.len()
                {
                    selected.push(initial_items[idx].clone());
                }
            }
            for item in selected {
                let _ = tx.send(PipelinePayload::Data(Arc::new(item))).await;
            }
            return;
        }
        if let Some(idx_os) = fshell_core::get_var("FSH_TEST_SELECT_INDEX")
            && let Ok(idx_str) = idx_os.into_string()
            && let Ok(idx) = idx_str.parse::<usize>()
            && idx < initial_items.len()
        {
            let _ = tx
                .send(PipelinePayload::Data(Arc::new(initial_items[idx].clone())))
                .await;
            return;
        }
        if let Some(first) = initial_items.into_iter().next() {
            let _ = tx.send(PipelinePayload::Data(Arc::new(first))).await;
        }
        return;
    }

    let device = match TerminalDevice::auto() {
        Ok(dev) => dev,
        Err(_) => {
            if let Some(first) = initial_items.into_iter().next() {
                let _ = tx.send(PipelinePayload::Data(Arc::new(first))).await;
            }
            return;
        }
    };

    let mode = if fullscreen {
        TerminalMode::Fullscreen
    } else {
        TerminalMode::Inline { height }
    };

    let options = TerminalSessionOptions {
        mode,
        hide_cursor: true,
        enable_mouse: true,
        ..Default::default()
    };

    let mut session = match TerminalSession::enter(device, options) {
        Ok(s) => s,
        Err(_) => return,
    };

    let input_stream = UnixEventStream::new().map(SelectMessage::Input);

    let pipe_stream: BoxStream<'static, SelectMessage> = if let Some(rx) = rx {
        futures::stream::unfold(
            PipeStreamState {
                rx,
                buffer: std::collections::VecDeque::new(),
            },
            |mut state| async move {
                loop {
                    if let Some(val) = state.buffer.pop_front() {
                        return Some((SelectMessage::Item(val), state));
                    }
                    match state.rx.recv().await {
                        Some(payload) => {
                            extract_payload_items(payload, &mut state.buffer);
                        }
                        None => return None,
                    }
                }
            },
        )
        .boxed()
    } else {
        stream::empty().boxed()
    };

    let events = stream::select(input_stream, pipe_stream).boxed();

    let mut app = SelectApp::new(&prompt, initial_items, &theme, multi);

    let selected_res = run_tui(&mut app, &mut session, events).await;

    if let Ok(Some(selected_items)) = selected_res {
        for item in selected_items {
            let _ = tx.send(PipelinePayload::Data(Arc::new(item))).await;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use fshell_terminal::input::KeyEvent;
    use ratatui::backend::TestBackend;

    #[test]
    fn test_format_val_for_display() {
        let s = Val::String("hello world".into());
        assert_eq!(format_val_for_display(&s), "hello world");

        let i = Val::Int(42);
        assert_eq!(format_val_for_display(&i), "42");

        let mut map = fshell_core::FxIndexMap::default();
        map.insert(ustr::ustr("name"), Val::String("my_app".into()));
        map.insert(ustr::ustr("pid"), Val::Int(1234));
        let m = Val::Map(map);
        let formatted = format_val_for_display(&m);
        assert!(formatted.starts_with("my_app"));
        assert!(formatted.contains("pid: 1234"));
    }

    #[test]
    fn test_select_app_single_selection() {
        let items = vec![
            Val::String("alpha".into()),
            Val::String("beta".into()),
            Val::String("gamma".into()),
        ];
        let theme = Theme::default_theme();
        let mut app = SelectApp::new("Select item:", items, &theme, false);

        // Navigate Down to "beta"
        let flow = app.handle_message(SelectMessage::Input(InputEvent::Key(KeyEvent::new(
            Key::Down,
            Modifiers::empty(),
        ))));
        assert_eq!(flow, AppFlow::Continue);
        assert_eq!(app.selected_idx, 1);

        // Press Enter to confirm selection
        let flow = app.handle_message(SelectMessage::Input(InputEvent::Key(KeyEvent::new(
            Key::Enter,
            Modifiers::empty(),
        ))));
        assert_eq!(flow, AppFlow::Break(vec![Val::String("beta".into())]));
    }

    #[test]
    fn test_select_app_multi_selection() {
        let items = vec![
            Val::String("apple".into()),
            Val::String("banana".into()),
            Val::String("cherry".into()),
        ];
        let theme = Theme::default_theme();
        let mut app = SelectApp::new("Select fruits:", items, &theme, true);

        // Toggle first item (apple)
        let flow = app.handle_message(SelectMessage::Input(InputEvent::Key(KeyEvent::new(
            Key::Character(' '),
            Modifiers::empty(),
        ))));
        assert_eq!(flow, AppFlow::Continue);
        assert!(app.items[0].checked);
        assert_eq!(app.selected_idx, 1); // Cursor auto-advanced

        // Move down past banana to cherry
        let flow = app.handle_message(SelectMessage::Input(InputEvent::Key(KeyEvent::new(
            Key::Down,
            Modifiers::empty(),
        ))));
        assert_eq!(flow, AppFlow::Continue);
        assert_eq!(app.selected_idx, 2);

        // Toggle cherry
        let flow = app.handle_message(SelectMessage::Input(InputEvent::Key(KeyEvent::new(
            Key::Character(' '),
            Modifiers::empty(),
        ))));
        assert_eq!(flow, AppFlow::Continue);
        assert!(app.items[2].checked);

        // Press Enter to confirm multi-selection
        let flow = app.handle_message(SelectMessage::Input(InputEvent::Key(KeyEvent::new(
            Key::Enter,
            Modifiers::empty(),
        ))));
        assert_eq!(
            flow,
            AppFlow::Break(vec![
                Val::String("apple".into()),
                Val::String("cherry".into())
            ])
        );
    }

    #[test]
    fn test_select_app_live_stream_arrival_and_filter() {
        let items = vec![Val::String("first_item".into())];
        let theme = Theme::default_theme();
        let mut app = SelectApp::new("Stream:", items, &theme, false);

        // Dynamically receive an item from upstream pipeline
        let flow = app.handle_message(SelectMessage::Item(Val::String("second_item".into())));
        assert_eq!(flow, AppFlow::Continue);
        assert_eq!(app.items.len(), 2);

        // Type 'sec' into filter
        app.handle_message(SelectMessage::Input(InputEvent::Key(KeyEvent::new(
            Key::Character('s'),
            Modifiers::empty(),
        ))));
        app.handle_message(SelectMessage::Input(InputEvent::Key(KeyEvent::new(
            Key::Character('e'),
            Modifiers::empty(),
        ))));
        app.handle_message(SelectMessage::Input(InputEvent::Key(KeyEvent::new(
            Key::Character('c'),
            Modifiers::empty(),
        ))));
        assert_eq!(app.query, "sec");

        let filtered = app.filtered_indices();
        assert_eq!(filtered.len(), 1);
        assert_eq!(app.items[filtered[0]].display, "second_item");

        // Press Enter to choose filtered item
        let flow = app.handle_message(SelectMessage::Input(InputEvent::Key(KeyEvent::new(
            Key::Enter,
            Modifiers::empty(),
        ))));
        assert_eq!(
            flow,
            AppFlow::Break(vec![Val::String("second_item".into())])
        );
    }

    #[test]
    fn test_select_app_cancel() {
        let items = vec![Val::String("item".into())];
        let theme = Theme::default_theme();
        let mut app = SelectApp::new("Select:", items, &theme, false);

        let flow = app.handle_message(SelectMessage::Input(InputEvent::Key(KeyEvent::new(
            Key::Escape,
            Modifiers::empty(),
        ))));
        assert_eq!(flow, AppFlow::Break(Vec::new()));
    }

    #[test]
    fn test_select_app_headless_rendering() {
        let items = vec![
            Val::String("alpha".into()),
            Val::String("beta".into()),
            Val::String("gamma".into()),
        ];
        let theme = Theme::default_theme();
        let mut app = SelectApp::new("Choose:", items, &theme, true);

        // Headless render in inline mode (80x10)
        let backend = TestBackend::new(80, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.render(frame, area);
            })
            .unwrap();

        // Headless render in fullscreen mode (80x24)
        let backend_full = TestBackend::new(80, 24);
        let mut terminal_full = ratatui::Terminal::new(backend_full).unwrap();
        terminal_full
            .draw(|frame| {
                let area = frame.area();
                app.render(frame, area);
            })
            .unwrap();
    }

    #[test]
    fn test_select_app_home_end_and_ctrl_w() {
        use fshell_terminal::input::MouseEvent;

        let items: Vec<Val> = (0..20)
            .map(|i| Val::String(format!("item_{:02}", i)))
            .collect();
        let theme = Theme::default_theme();
        let mut app = SelectApp::new("Pick:", items, &theme, false);

        // Jump to End
        app.handle_message(SelectMessage::Input(InputEvent::Key(KeyEvent::new(
            Key::End,
            Modifiers::empty(),
        ))));
        assert_eq!(app.selected_idx, 19);

        // Jump to Home
        app.handle_message(SelectMessage::Input(InputEvent::Key(KeyEvent::new(
            Key::Home,
            Modifiers::empty(),
        ))));
        assert_eq!(app.selected_idx, 0);

        // Jump to End with Ctrl-E
        let mut ctrl = Modifiers::empty();
        ctrl |= Modifiers::CONTROL;
        app.handle_message(SelectMessage::Input(InputEvent::Key(KeyEvent::new(
            Key::Character('e'),
            ctrl,
        ))));
        assert_eq!(app.selected_idx, 19);

        // Jump to Home with Ctrl-A
        app.handle_message(SelectMessage::Input(InputEvent::Key(KeyEvent::new(
            Key::Character('a'),
            ctrl,
        ))));
        assert_eq!(app.selected_idx, 0);

        // Mouse scroll down (scrolls by 3)
        app.handle_message(SelectMessage::Input(InputEvent::Mouse(MouseEvent {
            action: MouseAction::ScrollDown,
            column: 10,
            row: 5,
        })));
        assert_eq!(app.selected_idx, 3);

        // Mouse scroll up
        app.handle_message(SelectMessage::Input(InputEvent::Mouse(MouseEvent {
            action: MouseAction::ScrollUp,
            column: 10,
            row: 5,
        })));
        assert_eq!(app.selected_idx, 0);

        // Type multiple words
        for c in "foo bar baz".chars() {
            app.handle_message(SelectMessage::Input(InputEvent::Key(KeyEvent::new(
                Key::Character(c),
                Modifiers::empty(),
            ))));
        }
        assert_eq!(app.query, "foo bar baz");

        // Ctrl-W kill word: deletes "baz", leaving "foo bar "
        app.handle_message(SelectMessage::Input(InputEvent::Key(KeyEvent::new(
            Key::Character('w'),
            ctrl,
        ))));
        assert_eq!(app.query, "foo bar ");

        // Another Ctrl-W: deletes "bar ", leaving "foo "
        app.handle_message(SelectMessage::Input(InputEvent::Key(KeyEvent::new(
            Key::Character('w'),
            ctrl,
        ))));
        assert_eq!(app.query, "foo ");

        // Another Ctrl-W: deletes "foo ", leaving empty
        app.handle_message(SelectMessage::Input(InputEvent::Key(KeyEvent::new(
            Key::Character('w'),
            ctrl,
        ))));
        assert_eq!(app.query, "");
    }
}
