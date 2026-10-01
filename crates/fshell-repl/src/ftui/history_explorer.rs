// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Fullscreen interactive SQLite history explorer and execution log viewer.

use crate::history::query_history;
use crate::terminal_mode::FullscreenTerminalGuard;
use crate::tui::components::{KeyHint, ScrollState, SearchBarState, StatusFooter};
use crate::tui::theme;
use chrono::TimeZone;
use fshell_core::theme::Theme;
use fshell_terminal::FshellBackend;
use fshell_terminal::input::{InputEvent, InputPoll, Key, KeyAction, Modifiers, UnixEventSource};
use ratatui::{
    Terminal,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph},
};
use std::io;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TuiResult {
    Execute(String),
    Edit(String),
    Cancel,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum FocusPane {
    Search,
    List,
    Preview,
}

impl FocusPane {
    pub fn next(self) -> Self {
        match self {
            Self::Search => Self::List,
            Self::List => Self::Preview,
            Self::Preview => Self::Search,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Search => Self::Preview,
            Self::List => Self::Search,
            Self::Preview => Self::List,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Search => "Search",
            Self::List => "List",
            Self::Preview => "Preview",
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum FilterMode {
    Global,
    Host,
    Cwd,
    Session,
}

impl FilterMode {
    fn next(self) -> Self {
        match self {
            Self::Global => Self::Host,
            Self::Host => Self::Cwd,
            Self::Cwd => Self::Session,
            Self::Session => Self::Global,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Global => "GLOBAL",
            Self::Host => "HOST",
            Self::Cwd => "DIRECTORY",
            Self::Session => "SESSION",
        }
    }
}

pub fn run_history_tui(
    current_cwd: &str,
    current_host: &str,
    current_session: &str,
) -> Result<TuiResult, String> {
    run_history_tui_with_theme(
        current_cwd,
        current_host,
        current_session,
        &Theme::default_theme(),
    )
}

/// Runs the fullscreen interactive history explorer TUI with active theme.
pub fn run_history_tui_with_theme(
    current_cwd: &str,
    current_host: &str,
    current_session: &str,
    theme: &Theme,
) -> Result<TuiResult, String> {
    if fshell_engine::is_test_mode() {
        return Ok(TuiResult::Cancel);
    }

    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let stdin_fd = std::io::stdin().as_raw_fd();
        if unsafe { libc::isatty(stdin_fd) } == 0 {
            return Ok(TuiResult::Cancel);
        }
    }

    let _guard = FullscreenTerminalGuard::enter(true)
        .map_err(|e| format!("Failed to initialize terminal TUI: {e}"))?;
    let mut stdout = io::stdout();
    let backend = FshellBackend::new(&mut stdout);
    let mut terminal =
        Terminal::new(backend).map_err(|e| format!("Failed to create terminal: {e}"))?;
    terminal.clear().map_err(|e| e.to_string())?;

    let mut search_bar = SearchBarState::new();
    let mut focus = FocusPane::Search;
    let mut filter_mode = FilterMode::Global;
    let mut list_state = ListState::default();
    list_state.select(Some(0));

    let mut preview_scroll = 0usize;
    let mut list_scroll_state = ScrollState::new();
    let mut preview_scroll_state = ScrollState::new();

    let mut should_requery = true;
    let mut entries = Vec::new();
    let mut input = UnixEventSource::new();

    loop {
        if should_requery {
            let search_for_sql = if search_bar.query.trim().is_empty() {
                None
            } else {
                Some(search_bar.query.trim())
            };
            entries = query_history(
                Some(300),
                search_for_sql,
                match filter_mode {
                    FilterMode::Cwd => Some(current_cwd),
                    _ => None,
                },
                match filter_mode {
                    FilterMode::Session => Some(current_session),
                    _ => None,
                },
                match filter_mode {
                    FilterMode::Host => Some(current_host),
                    _ => None,
                },
                None,
            )
            .unwrap_or_default();
            should_requery = false;

            let len = entries.len();
            if len == 0 {
                list_state.select(None);
            } else {
                let cur = list_state.selected().unwrap_or(0);
                if cur >= len {
                    list_state.select(Some(len - 1));
                }
            }
            preview_scroll = 0;
        }

        let len = entries.len();

        terminal
            .draw(|f| {
                let size = f.area();
                if size.width < 10 || size.height < 6 {
                    return;
                }

                // Outer master block
                let outer_block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(theme::border_style(theme))
                    .title(" History Explorer ")
                    .title_style(theme::title_style(theme));

                let inner = outer_block.inner(size);
                f.render_widget(outer_block, size);

                // Main vertical division:
                // [0] Search Bar (1 line)
                // [1] Divider line (1 line)
                // [2] Middle Content (List + Preview)
                // [3] Divider line (1 line)
                // [4] Status Footer (1 line)
                let layout = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1), // Search Bar
                        Constraint::Length(1), // Divider
                        Constraint::Min(4),    // Middle panes
                        Constraint::Length(1), // Divider
                        Constraint::Length(1), // Footer
                    ])
                    .split(inner);

                let search_area = layout[0];
                let top_div_area = layout[1];
                let middle_area = layout[2];
                let bot_div_area = layout[3];
                let footer_area = layout[4];

                // 1. Search Bar with Filter Tag on right
                let filter_str = format!("Filter: {} ", filter_mode.name());
                let filter_w = filter_str.width() as u16;
                let search_input_w = search_area.width.saturating_sub(filter_w + 2);

                let search_sub_area = Rect::new(search_area.x, search_area.y, search_input_w, 1);
                let filter_tag_area = Rect::new(
                    search_area.x + search_area.width.saturating_sub(filter_w),
                    search_area.y,
                    filter_w,
                    1,
                );

                search_bar.render(
                    search_sub_area,
                    f.buffer_mut(),
                    theme,
                    "/ ",
                    "Type to search history...",
                    focus == FocusPane::Search,
                );

                let filter_badge = Line::from(vec![
                    Span::styled("Filter: ", theme::muted_style(theme)),
                    Span::styled(filter_mode.name(), theme::title_style(theme)),
                ]);
                f.render_widget(Paragraph::new(filter_badge), filter_tag_area);

                // Dividers
                let div_line = "─".repeat(inner.width as usize);
                f.render_widget(
                    Paragraph::new(Span::styled(&div_line, theme::border_style(theme))),
                    top_div_area,
                );
                f.render_widget(
                    Paragraph::new(Span::styled(&div_line, theme::border_style(theme))),
                    bot_div_area,
                );

                // 2. Middle area: List (55%) and Preview (45%)
                let middle_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
                    .split(middle_area);

                let list_area = middle_chunks[0];
                let preview_area = middle_chunks[1];

                // --- Left List Pane ---
                let list_focused = focus == FocusPane::List;
                let list_border_style = if list_focused {
                    theme::border_focused_style(theme)
                } else {
                    theme::border_style(theme)
                };

                let list_block = Block::default()
                    .borders(Borders::RIGHT)
                    .border_style(list_border_style)
                    .title(format!(" Commands ({len}) "))
                    .title_style(if list_focused {
                        theme::title_style(theme)
                    } else {
                        theme::muted_style(theme)
                    });

                let list_inner = list_block.inner(list_area);
                f.render_widget(list_block, list_area);

                let visible_list_h = list_inner.height as usize;
                list_scroll_state.update(len, visible_list_h);
                if let Some(sel) = list_state.selected() {
                    list_scroll_state.ensure_visible(sel);
                }

                let items: Vec<ListItem> = entries
                    .iter()
                    .skip(list_scroll_state.offset)
                    .take(visible_list_h)
                    .enumerate()
                    .map(|(offset_i, entry)| {
                        let item_idx = list_scroll_state.offset + offset_i;
                        let is_selected = list_state.selected() == Some(item_idx);

                        let status_span = match entry.exit_code {
                            Some(0) => Span::styled("✔ ", theme::status_ok_style(theme)),
                            Some(_) => Span::styled("✘ ", theme::status_error_style(theme)),
                            None => Span::styled("? ", theme::status_warn_style(theme)),
                        };

                        let duration_text = if entry.duration_ms < 1000 {
                            format!("{:>5}ms", entry.duration_ms)
                        } else {
                            format!("{:>4.1}s", entry.duration_ms as f64 / 1000.0)
                        };

                        let dur_span =
                            Span::styled(format!(" {duration_text}"), theme::muted_style(theme));
                        let cmd_span = Span::raw(&entry.command);

                        let row_style = if is_selected {
                            theme::selected_style(theme)
                        } else {
                            Style::default()
                        };

                        ListItem::new(Line::from(vec![status_span, cmd_span, dur_span]))
                            .style(row_style)
                    })
                    .collect();

                let list_widget = List::new(items)
                    .highlight_style(theme::selected_style(theme))
                    .highlight_symbol("❯ ");

                f.render_stateful_widget(list_widget, list_inner, &mut list_state);
                list_scroll_state.render_scrollbar(list_inner, f.buffer_mut(), theme);

                // --- Right Preview Pane ---
                let preview_focused = focus == FocusPane::Preview;
                let preview_title_style = if preview_focused {
                    theme::title_style(theme)
                } else {
                    theme::muted_style(theme)
                };

                let preview_block = Block::default()
                    .borders(Borders::NONE)
                    .title(" Execution Context ")
                    .title_style(preview_title_style);

                let preview_inner = preview_block.inner(preview_area);
                f.render_widget(preview_block, preview_area);

                let selected_entry = list_state.selected().and_then(|idx| entries.get(idx));
                if let Some(entry) = selected_entry {
                    let (status_text, status_style) = match entry.exit_code {
                        Some(0) => ("Success (0)", theme::status_ok_style(theme)),
                        Some(code) => (
                            format!("Failure ({code})").leak() as &str,
                            theme::status_error_style(theme),
                        ),
                        None => ("In Progress / Terminated", theme::status_warn_style(theme)),
                    };

                    let datetime = chrono::Utc
                        .timestamp_millis_opt(entry.timestamp_ms)
                        .single()
                        .unwrap_or_else(chrono::Utc::now);
                    let local_time = datetime.with_timezone(&chrono::Local);
                    let time_str = local_time.format("%Y-%m-%d %H:%M:%S").to_string();

                    let label_style = theme::muted_style(theme);
                    let val_style = Style::default();

                    let lines = vec![
                        Line::from(Span::styled("Command:", theme::title_style(theme))),
                        Line::from(Span::styled(
                            &entry.command,
                            theme::key_hint_key_style(theme),
                        )),
                        Line::raw(""),
                        Line::from(vec![
                            Span::styled("Exit Code:   ", label_style),
                            Span::styled(status_text, status_style),
                        ]),
                        Line::from(vec![
                            Span::styled("Executed:    ", label_style),
                            Span::styled(time_str, val_style),
                        ]),
                        Line::from(vec![
                            Span::styled("Working Dir: ", label_style),
                            Span::styled(&entry.cwd, theme::to_style(&theme.syntax.string)),
                        ]),
                        Line::from(vec![
                            Span::styled("Host / User: ", label_style),
                            Span::styled(
                                format!("{} @ {}", entry.username, entry.hostname),
                                val_style,
                            ),
                        ]),
                        Line::from(vec![
                            Span::styled("Session ID:  ", label_style),
                            Span::styled(&entry.session_id, label_style),
                        ]),
                        Line::from(vec![
                            Span::styled("Duration:    ", label_style),
                            Span::styled(format!("{} ms", entry.duration_ms), val_style),
                        ]),
                    ];

                    let total_lines = lines.len();
                    let visible_preview_h = preview_inner.height as usize;
                    preview_scroll_state.update(total_lines, visible_preview_h);
                    preview_scroll_state.offset = preview_scroll;

                    let visible_lines: Vec<Line> = lines
                        .into_iter()
                        .skip(preview_scroll)
                        .take(visible_preview_h)
                        .collect();

                    f.render_widget(Paragraph::new(visible_lines), preview_inner);
                    preview_scroll_state.render_scrollbar(preview_inner, f.buffer_mut(), theme);
                } else {
                    let empty_msg =
                        Paragraph::new("\n  No record selected").style(theme::muted_style(theme));
                    f.render_widget(empty_msg, preview_inner);
                }

                // 3. Status Footer
                let hints = &[
                    KeyHint::new("Enter", "Run"),
                    KeyHint::new("e", "Edit"),
                    KeyHint::new("Tab", "Focus"),
                    KeyHint::new("Ctrl-R", "Filter"),
                    KeyHint::new("j/k", "Navigate"),
                    KeyHint::new("Esc", "Quit"),
                ];

                let status_label = format!("Focus: {}", focus.name());
                StatusFooter::new(theme, hints)
                    .with_status(Span::styled(status_label, theme::title_style(theme)))
                    .render(footer_area, f.buffer_mut());
            })
            .map_err(|e| format!("Failed to draw UI: {e}"))?;

        let key = match input
            .poll(std::time::Duration::from_millis(100))
            .map_err(|error| error.to_string())?
        {
            InputPoll::Event(InputEvent::Key(key)) => key,
            InputPoll::Event(_) | InputPoll::Timeout => continue,
            InputPoll::Closed => return Ok(TuiResult::Cancel),
        };

        if key.action != KeyAction::Press {
            continue;
        }

        if key.modifiers.contains(Modifiers::CONTROL) && key.key == Key::Character('c') {
            return Ok(TuiResult::Cancel);
        }

        if key.modifiers.contains(Modifiers::CONTROL) && key.key == Key::Character('r') {
            filter_mode = filter_mode.next();
            list_state.select(Some(0));
            should_requery = true;
            continue;
        }

        // Global keys
        match key.key {
            Key::Tab => {
                focus = if key.modifiers.contains(Modifiers::SHIFT) {
                    focus.prev()
                } else {
                    focus.next()
                };
                continue;
            }
            Key::BackTab => {
                focus = focus.prev();
                continue;
            }
            Key::Enter => {
                if let Some(idx) = list_state.selected()
                    && let Some(entry) = entries.get(idx)
                {
                    return Ok(TuiResult::Execute(entry.command.clone()));
                }
                return Ok(TuiResult::Cancel);
            }
            Key::Escape => {
                return Ok(TuiResult::Cancel);
            }
            _ => {}
        }

        // Pane-specific keys
        match focus {
            FocusPane::Search => match key.key {
                Key::Down => {
                    focus = FocusPane::List;
                }
                Key::Character('e') if key.modifiers.contains(Modifiers::CONTROL) => {
                    if let Some(idx) = list_state.selected()
                        && let Some(entry) = entries.get(idx)
                    {
                        return Ok(TuiResult::Edit(entry.command.clone()));
                    }
                }
                _ => {
                    if search_bar.handle_key(&key) {
                        list_state.select(Some(0));
                        should_requery = true;
                    }
                }
            },
            FocusPane::List => match key.key {
                Key::Character('/') => {
                    focus = FocusPane::Search;
                }
                Key::Character('e') | Key::Character('E') => {
                    if let Some(idx) = list_state.selected()
                        && let Some(entry) = entries.get(idx)
                    {
                        return Ok(TuiResult::Edit(entry.command.clone()));
                    }
                }
                Key::Up | Key::Character('k') if len > 0 => {
                    let current = list_state.selected().unwrap_or(0);
                    if current > 0 {
                        list_state.select(Some(current - 1));
                    } else {
                        list_state.select(Some(len - 1));
                    }
                }
                Key::Down | Key::Character('j') if len > 0 => {
                    let current = list_state.selected().unwrap_or(0);
                    if current + 1 < len {
                        list_state.select(Some(current + 1));
                    } else {
                        list_state.select(Some(0));
                    }
                }
                Key::PageUp => {
                    let current = list_state.selected().unwrap_or(0);
                    list_state.select(Some(current.saturating_sub(10)));
                }
                Key::PageDown if len > 0 => {
                    let current = list_state.selected().unwrap_or(0);
                    list_state.select(Some((current + 10).min(len - 1)));
                }
                Key::Home | Key::Character('g') => {
                    list_state.select(Some(0));
                }
                Key::End | Key::Character('G') if len > 0 => {
                    list_state.select(Some(len - 1));
                }
                _ => {}
            },
            FocusPane::Preview => match key.key {
                Key::Character('/') => {
                    focus = FocusPane::Search;
                }
                Key::Up | Key::Character('k') => {
                    preview_scroll = preview_scroll.saturating_sub(1);
                }
                Key::Down | Key::Character('j') => {
                    preview_scroll = preview_scroll.saturating_add(1);
                }
                Key::Home | Key::Character('g') => {
                    preview_scroll = 0;
                }
                _ => {}
            },
        }
    }
}
