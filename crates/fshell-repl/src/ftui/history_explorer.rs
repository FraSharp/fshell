// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Fullscreen interactive SQLite history explorer and execution log viewer.

use crate::history::query_history;
use chrono::TimeZone;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph},
};
use std::io;

#[derive(Debug, Clone, PartialEq)]
pub enum TuiResult {
    Execute(String),
    Edit(String),
    Cancel,
}

#[derive(Debug, Copy, Clone, PartialEq)]
enum FilterMode {
    Global,
    Host,
    Cwd,
    Session,
}

impl FilterMode {
    fn next(self) -> Self {
        match self {
            FilterMode::Global => FilterMode::Host,
            FilterMode::Host => FilterMode::Cwd,
            FilterMode::Cwd => FilterMode::Session,
            FilterMode::Session => FilterMode::Global,
        }
    }

    fn name(self) -> &'static str {
        match self {
            FilterMode::Global => "GLOBAL",
            FilterMode::Host => "HOST",
            FilterMode::Cwd => "DIRECTORY",
            FilterMode::Session => "SESSION",
        }
    }

    fn color(self) -> Color {
        match self {
            FilterMode::Global => Color::Cyan,
            FilterMode::Host => Color::LightBlue,
            FilterMode::Cwd => Color::LightYellow,
            FilterMode::Session => Color::LightMagenta,
        }
    }
}

struct TerminalGuard;

impl TerminalGuard {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

/// Runs the fullscreen interactive history explorer TUI.
pub fn run_history_tui(
    current_cwd: &str,
    current_host: &str,
    current_session: &str,
) -> Result<TuiResult, String> {
    // Setup terminal
    let _guard =
        TerminalGuard::new().map_err(|e| format!("Failed to initialize terminal TUI: {}", e))?;
    let mut stdout = io::stdout();
    let backend = CrosstermBackend::new(&mut stdout);
    let mut terminal =
        Terminal::new(backend).map_err(|e| format!("Failed to create terminal: {}", e))?;

    // UI state
    let mut search_query = String::new();
    let mut filter_mode = FilterMode::Global;
    let mut list_state = ListState::default();
    list_state.select(Some(0));

    let mut should_requery = true;
    let mut entries = Vec::new();

    loop {
        if should_requery {
            let search_for_sql = if search_query.trim().is_empty() {
                None
            } else {
                Some(search_query.trim())
            };
            entries = query_history(
                Some(200),
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

            // Adjust selected index if it goes out of bounds
            let len = entries.len();
            if len == 0 {
                list_state.select(None);
            } else {
                match list_state.selected() {
                    Some(idx) => {
                        if idx >= len {
                            list_state.select(Some(len - 1));
                        }
                    }
                    None => {
                        list_state.select(Some(0));
                    }
                }
            }
        }

        let len = entries.len();

        // Draw TUI
        terminal
            .draw(|f| {
                let size = f.area();

                // Main vertical layout
                let main_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3), // Search box
                        Constraint::Min(5),    // Main list & preview area
                        Constraint::Length(1), // Status bar
                    ])
                    .split(size);

                // 1. Search Box
                let search_block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(Span::styled(
                        " Interactive Search History ",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ));
                let search_text = format!("  {}", search_query);
                let search_p = Paragraph::new(search_text).block(search_block);
                f.render_widget(search_p, main_chunks[0]);

                // Split middle area into left (list) and right (preview card)
                let middle_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
                    .split(main_chunks[1]);

                // 2. Left List of commands
                let list_block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(Span::styled(
                        " History Logs ",
                        Style::default().fg(Color::LightGreen),
                    ));

                let items: Vec<ListItem> = entries
                    .iter()
                    .map(|entry| {
                        let status_icon = if entry.exit_code == Some(0) {
                            Span::styled(" [ok] ", Style::default().fg(Color::Green))
                        } else if entry.exit_code.is_none() {
                            Span::styled(" [?] ", Style::default().fg(Color::Yellow))
                        } else {
                            Span::styled(" [!] ", Style::default().fg(Color::Red))
                        };

                        let duration_text = if entry.duration_ms < 1000 {
                            format!("{}ms", entry.duration_ms)
                        } else {
                            format!("{:.1}s", entry.duration_ms as f64 / 1000.0)
                        };

                        let cmd_span =
                            Span::styled(&entry.command, Style::default().fg(Color::White));
                        let dur_span = Span::styled(
                            format!("  ({})", duration_text),
                            Style::default().fg(Color::DarkGray),
                        );

                        ListItem::new(Line::from(vec![status_icon, cmd_span, dur_span]))
                    })
                    .collect();

                let list_widget = List::new(items)
                    .block(list_block)
                    .highlight_style(
                        Style::default()
                            .bg(Color::Rgb(40, 44, 52))
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )
                    .highlight_symbol(" ❯ ");

                f.render_stateful_widget(list_widget, middle_chunks[0], &mut list_state);

                // 3. Right Details Card
                let selected_entry = list_state.selected().and_then(|idx| entries.get(idx));
                let preview_block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Double)
                    .title(Span::styled(
                        " Execution Context ",
                        Style::default()
                            .fg(Color::LightMagenta)
                            .add_modifier(Modifier::BOLD),
                    ));

                if let Some(entry) = selected_entry {
                    let status_str = match entry.exit_code {
                        Some(0) => Span::styled(
                            "Success (0)",
                            Style::default()
                                .fg(Color::Green)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Some(code) => Span::styled(
                            format!("Failure ({})", code),
                            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                        ),
                        None => Span::styled("Unknown/None", Style::default().fg(Color::Yellow)),
                    };

                    let datetime = chrono::Utc
                        .timestamp_millis_opt(entry.timestamp_ms)
                        .single()
                        .unwrap_or_else(chrono::Utc::now);
                    let local_time = datetime.with_timezone(&chrono::Local);
                    let time_str = local_time.format("%Y-%m-%d %H:%M:%S").to_string();

                    let details = vec![
                        Line::from(vec![
                            Span::styled("Command:  ", Style::default().fg(Color::DarkGray)),
                            Span::styled(
                                &entry.command,
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ]),
                        Line::from(""),
                        Line::from(vec![
                            Span::styled("CWD:      ", Style::default().fg(Color::DarkGray)),
                            Span::styled(&entry.cwd, Style::default().fg(Color::LightBlue)),
                        ]),
                        Line::from(""),
                        Line::from(vec![
                            Span::styled("Time:     ", Style::default().fg(Color::DarkGray)),
                            Span::styled(time_str, Style::default().fg(Color::LightGreen)),
                        ]),
                        Line::from(vec![
                            Span::styled("Duration: ", Style::default().fg(Color::DarkGray)),
                            Span::styled(
                                format!("{} ms", entry.duration_ms),
                                Style::default().fg(Color::LightYellow),
                            ),
                        ]),
                        Line::from(vec![
                            Span::styled("Status:   ", Style::default().fg(Color::DarkGray)),
                            status_str,
                        ]),
                        Line::from(""),
                        Line::from(vec![
                            Span::styled("Host:     ", Style::default().fg(Color::DarkGray)),
                            Span::styled(&entry.hostname, Style::default().fg(Color::LightCyan)),
                        ]),
                        Line::from(vec![
                            Span::styled("User:     ", Style::default().fg(Color::DarkGray)),
                            Span::styled(&entry.username, Style::default().fg(Color::LightMagenta)),
                        ]),
                        Line::from(vec![
                            Span::styled("Session:  ", Style::default().fg(Color::DarkGray)),
                            Span::styled(&entry.session_id, Style::default().fg(Color::LightRed)),
                        ]),
                    ];

                    let preview_p = Paragraph::new(details).block(preview_block).scroll((0, 0));
                    f.render_widget(preview_p, middle_chunks[1]);
                } else {
                    let empty_p = Paragraph::new("\n\n   No details to display")
                        .block(preview_block)
                        .dark_gray();
                    f.render_widget(empty_p, middle_chunks[1]);
                }

                // 4. Status Bar
                let status_style = Style::default().fg(Color::White).bg(Color::Rgb(30, 30, 30));

                let status_line = Line::from(vec![
                    Span::styled(
                        " [Enter] ",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("Run ", Style::default().fg(Color::White)),
                    Span::styled(
                        " [Tab] ",
                        Style::default()
                            .fg(Color::LightCyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("Edit ", Style::default().fg(Color::White)),
                    Span::styled(
                        " [Ctrl-R] ",
                        Style::default()
                            .fg(Color::LightYellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("Filter: ", Style::default().fg(Color::White)),
                    Span::styled(
                        format!(" {} ", filter_mode.name()),
                        Style::default()
                            .fg(Color::Black)
                            .bg(filter_mode.color())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "  [Esc] ",
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("Quit", Style::default().fg(Color::White)),
                ]);

                let status_p = Paragraph::new(status_line).style(status_style);
                f.render_widget(status_p, main_chunks[2]);
            })
            .map_err(|e| format!("Failed to draw UI: {}", e))?;

        // Handle keys
        if event::poll(std::time::Duration::from_millis(100)).unwrap_or(false)
            && let Ok(Event::Key(key)) = event::read()
            && key.kind == event::KeyEventKind::Press
        {
            // Check Ctrl-C first
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                return Ok(TuiResult::Cancel);
            }

            // Check Ctrl-R for cycling filters
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('r') {
                filter_mode = filter_mode.next();
                list_state.select(Some(0));
                should_requery = true;
                continue;
            }

            match key.code {
                KeyCode::Esc => {
                    return Ok(TuiResult::Cancel);
                }
                KeyCode::Enter => {
                    if let Some(idx) = list_state.selected()
                        && let Some(entry) = entries.get(idx)
                    {
                        return Ok(TuiResult::Execute(entry.command.clone()));
                    }
                    return Ok(TuiResult::Cancel);
                }
                KeyCode::Tab => {
                    if let Some(idx) = list_state.selected()
                        && let Some(entry) = entries.get(idx)
                    {
                        return Ok(TuiResult::Edit(entry.command.clone()));
                    }
                    return Ok(TuiResult::Cancel);
                }
                KeyCode::Up if len > 0 => {
                    let current = list_state.selected().unwrap_or(0);
                    if current > 0 {
                        list_state.select(Some(current - 1));
                    } else {
                        list_state.select(Some(len - 1));
                    }
                }
                KeyCode::Down if len > 0 => {
                    let current = list_state.selected().unwrap_or(0);
                    if current < len - 1 {
                        list_state.select(Some(current + 1));
                    } else {
                        list_state.select(Some(0));
                    }
                }
                KeyCode::Backspace => {
                    search_query.pop();
                    list_state.select(Some(0));
                    should_requery = true;
                }
                KeyCode::Char(c) => {
                    search_query.push(c);
                    list_state.select(Some(0));
                    should_requery = true;
                }
                _ => {}
            }
        }
    }
}
