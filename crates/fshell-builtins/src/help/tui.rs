// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::help::{HelpTopic, TOPICS, render_full};
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use fshell_core::ShellError;
use fshell_engine::Env;
use nucleo_matcher::{Config, Matcher, Utf32String};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use std::io;

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalGuard {
    fn new() -> Result<Self, String> {
        enable_raw_mode().map_err(|e| e.to_string())?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen).map_err(|e| e.to_string())?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).map_err(|e| e.to_string())?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

pub fn get_matching_topics(query: &str) -> Vec<&'static HelpTopic> {
    if query.is_empty() {
        return TOPICS.iter().collect();
    }
    let mut matcher = Matcher::new(Config::DEFAULT);
    let needle = Utf32String::from(query);
    let needle_slice = needle.slice(..);
    let mut matched = Vec::new();
    for topic in TOPICS {
        let mut best_score = None;
        if let Some(score) =
            matcher.fuzzy_match(Utf32String::from(topic.name).slice(..), needle_slice)
        {
            best_score = Some(best_score.unwrap_or(0).max(score * 10 + 1000));
        }
        if let Some(score) =
            matcher.fuzzy_match(Utf32String::from(topic.summary).slice(..), needle_slice)
        {
            best_score = Some(best_score.unwrap_or(0).max(score * 2 + 100));
        }
        if let Some(score) =
            matcher.fuzzy_match(Utf32String::from(topic.description).slice(..), needle_slice)
        {
            best_score = Some(best_score.unwrap_or(0).max(score));
        }
        if let Some(score) = best_score {
            matched.push((topic, score));
        }
    }
    // Sort by score descending, then by name for stable ordering of equal scores
    matched.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name.cmp(b.0.name)));
    matched.into_iter().map(|(t, _)| t).collect()
}

pub fn run_tui(_env: &Env) -> Result<(), ShellError> {
    let mut guard = TerminalGuard::new().map_err(ShellError::from)?;
    let mut query = String::new();
    let mut search_focused = true;
    let mut selected_index = 0;
    let mut detail_scroll = 0;
    let mut matches = get_matching_topics(&query);

    loop {
        // Draw the interface
        let current_query = query.clone();
        let current_search_focused = search_focused;
        let current_selected_index = selected_index;
        let current_detail_scroll = detail_scroll;
        let current_matches = matches.clone();

        guard
            .terminal
            .draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3), // Search box
                        Constraint::Min(5),    // Middle columns
                        Constraint::Length(1), // Status bar
                    ])
                    .split(f.area());

                // 1. Search box rendering
                let search_style = if current_search_focused {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                let search_title = if current_search_focused {
                    " Search (fuzzy) "
                } else {
                    " Search — / to focus, Esc to navigate "
                };
                let search_block = Block::default()
                    .borders(Borders::ALL)
                    .title(search_title)
                    .border_style(search_style);
                let search_paragraph = Paragraph::new(current_query.as_str()).block(search_block);
                f.render_widget(search_paragraph, chunks[0]);

                // 2. Middle area rendering
                let middle_columns = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(30), // Left column (sidebar)
                        Constraint::Percentage(70), // Right column (detail)
                    ])
                    .split(chunks[1]);

                // Left column: List of matching topics
                let list_items: Vec<ListItem> = if current_matches.is_empty() {
                    vec![
                        ListItem::new(" (no matches) ").style(Style::default().fg(Color::DarkGray)),
                    ]
                } else {
                    current_matches
                        .iter()
                        .enumerate()
                        .map(|(i, t)| {
                            let style = if i == current_selected_index {
                                Style::default().fg(Color::Black).bg(Color::Cyan)
                            } else {
                                Style::default()
                            };
                            ListItem::new(format!(" {}", t.name)).style(style)
                        })
                        .collect()
                };

                let list_block = Block::default()
                    .borders(Borders::ALL)
                    .title(" Topics ")
                    .border_style(Style::default().fg(Color::DarkGray));
                let list = List::new(list_items).block(list_block);
                f.render_widget(list, middle_columns[0]);

                // Right column: Detailed scrollable text of the selected topic
                if let Some(topic) = current_matches.get(current_selected_index) {
                    let detail_text = render_full(topic, false);
                    let detail_block = Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" {} Help ", topic.name))
                        .border_style(Style::default().fg(Color::DarkGray));

                    let detail_paragraph = Paragraph::new(detail_text)
                        .block(detail_block)
                        .scroll((current_detail_scroll, 0));

                    f.render_widget(detail_paragraph, middle_columns[1]);
                } else {
                    let detail_block = Block::default()
                        .borders(Borders::ALL)
                        .title(" Help ")
                        .border_style(Style::default().fg(Color::DarkGray));
                    let detail_paragraph = Paragraph::new("No topic selected.").block(detail_block);
                    f.render_widget(detail_paragraph, middle_columns[1]);
                }

                // 3. Status bar rendering
                let status_style = Style::default().fg(Color::Black).bg(Color::Gray);
                let n_matches = current_matches.len();
                let topic_count_str = format!("{} topics", n_matches);
                let item_str = if current_selected_index < n_matches {
                    format!(" · item {}/{}", current_selected_index + 1, n_matches)
                } else {
                    String::new()
                };
                let mode_hint_str = if current_search_focused {
                    " [Enter/Esc] Confirm ".to_string()
                } else {
                    " [/] Search [j/k] Navigate [q] Quit ".to_string()
                };
                let status_text = Line::from(vec![
                    Span::styled(" ", status_style),
                    Span::styled(&topic_count_str, status_style),
                    Span::styled(&item_str, status_style),
                    Span::styled(" ·", status_style),
                    Span::styled(mode_hint_str, status_style),
                ]);
                let status_paragraph = Paragraph::new(status_text);
                f.render_widget(status_paragraph, chunks[2]);

                // Cursor: only visible in search mode, hidden in nav mode
                if current_search_focused {
                    f.set_cursor_position((
                        chunks[0].x + 1 + current_query.len() as u16,
                        chunks[0].y + 1,
                    ));
                }
            })
            .map_err(|e| ShellError::from(e.to_string()))?;

        // Read event
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let stdin_fd = std::io::stdin().as_raw_fd();
            if unsafe { libc::isatty(stdin_fd) } == 0 {
                break;
            }
        }
        if !event::poll(std::time::Duration::from_millis(100))
            .map_err(|e| ShellError::from(e.to_string()))?
        {
            continue;
        }
        if let Event::Key(key) = event::read().map_err(|e| ShellError::from(e.to_string()))? {
            // Global shortcuts — work in both modes
            if key.code == KeyCode::PageUp {
                let height = guard.terminal.size().map(|s| s.height).unwrap_or(24);
                let scroll_amount = height.saturating_sub(6).max(1);
                detail_scroll = detail_scroll.saturating_sub(scroll_amount);
                continue;
            }
            if key.code == KeyCode::PageDown {
                let height = guard.terminal.size().map(|s| s.height).unwrap_or(24);
                let scroll_amount = height.saturating_sub(6).max(1);

                // Get current topic line count to clamp scroll
                if let Some(topic) = matches.get(selected_index) {
                    let total_lines = render_full(topic, false).lines().count();
                    let detail_height = height.saturating_sub(6) as usize;
                    let max_scroll = total_lines.saturating_sub(detail_height) as u16;
                    detail_scroll = (detail_scroll + scroll_amount).min(max_scroll);
                }
                continue;
            }

            if search_focused {
                match key.code {
                    KeyCode::Char(c)
                        if (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
                    {
                        query.push(c);
                        detail_scroll = 0;
                        matches = get_matching_topics(&query);
                        if matches.is_empty() {
                            selected_index = 0;
                        } else if selected_index >= matches.len() {
                            selected_index = matches.len() - 1;
                        }
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        detail_scroll = 0;
                        matches = get_matching_topics(&query);
                        if matches.is_empty() {
                            selected_index = 0;
                        } else if selected_index >= matches.len() {
                            selected_index = matches.len() - 1;
                        }
                    }
                    KeyCode::Esc | KeyCode::Enter => {
                        search_focused = false;
                    }
                    KeyCode::Up if selected_index > 0 => {
                        selected_index -= 1;
                        detail_scroll = 0;
                    }
                    KeyCode::Down if !matches.is_empty() && selected_index < matches.len() - 1 => {
                        selected_index += 1;
                        detail_scroll = 0;
                    }
                    _ => {}
                }
            } else {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        break;
                    }
                    KeyCode::Char('/') => {
                        search_focused = true;
                    }
                    KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        search_focused = true;
                    }
                    KeyCode::Up | KeyCode::Char('k') if selected_index > 0 => {
                        selected_index -= 1;
                        detail_scroll = 0;
                    }
                    KeyCode::Down | KeyCode::Char('j')
                        if !matches.is_empty() && selected_index < matches.len() - 1 =>
                    {
                        selected_index += 1;
                        detail_scroll = 0;
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}
