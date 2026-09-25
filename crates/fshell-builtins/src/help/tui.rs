// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Modern, two-pane interactive help reference browser built on Ratatui.

use crate::help::{HelpTopic, TOPICS};
use crossterm::{
    cursor::{Hide, Show},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use fshell_core::ShellError;
use fshell_core::theme::{Theme, ThemeColor};
use fshell_engine::Env;
use fshell_terminal::input::{
    CrosstermEventSource, InputEvent, InputPoll, Key, KeyAction, Modifiers,
};
use nucleo_matcher::{Config, Matcher, Utf32String};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, StatefulWidget,
    },
};
use std::io;
use unicode_width::UnicodeWidthStr;

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalGuard {
    fn new() -> Result<Self, String> {
        enable_raw_mode().map_err(|e| e.to_string())?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, Hide).map_err(|e| e.to_string())?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).map_err(|e| e.to_string())?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(self.terminal.backend_mut(), Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

fn to_color(c: &ThemeColor) -> Color {
    let (r, g, b) = c.to_rgb();
    Color::Rgb(r, g, b)
}

fn to_style(c: &ThemeColor) -> Style {
    Style::default().fg(to_color(c))
}

fn to_style_bold(c: &ThemeColor) -> Style {
    Style::default().fg(to_color(c)).add_modifier(Modifier::BOLD)
}

fn to_style_dim(c: &ThemeColor) -> Style {
    Style::default().fg(to_color(c)).add_modifier(Modifier::DIM)
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum FocusArea {
    Search,
    Topics,
    Doc,
}

impl FocusArea {
    pub fn next(self) -> Self {
        match self {
            Self::Search => Self::Topics,
            Self::Topics => Self::Doc,
            Self::Doc => Self::Search,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Search => Self::Doc,
            Self::Topics => Self::Search,
            Self::Doc => Self::Topics,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Search => "Search",
            Self::Topics => "Topics",
            Self::Doc => "Documentation",
        }
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
    matched.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name.cmp(b.0.name)));
    matched.into_iter().map(|(t, _)| t).collect()
}

pub fn run_tui(env: &Env) -> Result<(), ShellError> {
    if fshell_engine::is_test_mode() {
        return Ok(());
    }

    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let stdin_fd = std::io::stdin().as_raw_fd();
        if unsafe { libc::isatty(stdin_fd) } == 0 {
            return Ok(());
        }
    }

    let mut guard = TerminalGuard::new().map_err(ShellError::from)?;
    guard.terminal.clear().map_err(|e| ShellError::from(e.to_string()))?;

    let theme = env.active_theme();
    let mut query = String::new();
    let mut focus = FocusArea::Search;
    let mut selected_index = 0usize;
    let mut doc_scroll = 0usize;
    let mut matches = get_matching_topics(&query);
    let mut input = CrosstermEventSource::new();
    let mut list_state = ListState::default();
    list_state.select(Some(0));

    loop {
        let topic_count = matches.len();
        if topic_count == 0 {
            selected_index = 0;
            list_state.select(None);
        } else if selected_index >= topic_count {
            selected_index = topic_count.saturating_sub(1);
            list_state.select(Some(selected_index));
        } else {
            list_state.select(Some(selected_index));
        }

        // Selected topic structured lines
        let selected_topic = matches.get(selected_index).copied();
        let doc_lines = selected_topic
            .map(|t| format_topic_lines(t, &theme))
            .unwrap_or_else(|| vec![Line::from("No topic selected.")]);
        let total_doc_lines = doc_lines.len();

        guard
            .terminal
            .draw(|f| {
                let size = f.area();
                if size.width < 10 || size.height < 6 {
                    return;
                }

                // Outer master block
                let outer_block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(to_style_dim(&theme.status.muted))
                    .title(" Help Reference ")
                    .title_style(to_style_bold(&theme.widgets.title));

                let inner = outer_block.inner(size);
                f.render_widget(outer_block, size);

                // Main vertical division:
                // [0] Search row (1 line)
                // [1] Divider (1 line)
                // [2] Master-detail content (Min 4 lines)
                // [3] Divider (1 line)
                // [4] Status Footer (1 line)
                let layout = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Length(1),
                        Constraint::Min(4),
                        Constraint::Length(1),
                        Constraint::Length(1),
                    ])
                    .split(inner);

                let search_row = layout[0];
                let top_div = layout[1];
                let middle_content = layout[2];
                let bot_div = layout[3];
                let footer_row = layout[4];

                // 1. Search Bar
                let count_str = format!("Topics: {topic_count} matches ");
                let count_w = count_str.width() as u16;
                let input_w = search_row.width.saturating_sub(count_w + 2);

                let search_area = Rect::new(search_row.x, search_row.y, input_w, 1);
                let count_area = Rect::new(
                    search_row.x + search_row.width.saturating_sub(count_w),
                    search_row.y,
                    count_w,
                    1,
                );

                let search_focused = focus == FocusArea::Search;
                let search_prefix_style = if search_focused {
                    to_style_bold(&theme.widgets.title)
                } else {
                    to_style_dim(&theme.status.muted)
                };

                let mut search_spans = vec![Span::styled("/ ", search_prefix_style)];
                if query.is_empty() && !search_focused {
                    search_spans.push(Span::styled(
                        "Type to search topics...",
                        to_style_dim(&theme.status.muted),
                    ));
                } else {
                    search_spans.push(Span::raw(&query));
                    if search_focused {
                        search_spans.push(Span::styled("█", to_style_bold(&theme.widgets.title)));
                    }
                }
                f.render_widget(Paragraph::new(Line::from(search_spans)), search_area);

                f.render_widget(
                    Paragraph::new(Span::styled(
                        count_str,
                        to_style_dim(&theme.status.muted),
                    )),
                    count_area,
                );

                // Dividers
                let div_line = "─".repeat(inner.width as usize);
                let div_style = to_style_dim(&theme.status.muted);
                f.render_widget(Paragraph::new(Span::styled(&div_line, div_style)), top_div);
                f.render_widget(Paragraph::new(Span::styled(&div_line, div_style)), bot_div);

                // 2. Middle Content: Sidebar (30%) + Reading Pane (70%)
                let content_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
                    .split(middle_content);

                let sidebar_area = content_chunks[0];
                let reading_area = content_chunks[1];

                // Left Sidebar: Topics list
                let topics_focused = focus == FocusArea::Topics;
                let sidebar_border_style = if topics_focused {
                    to_style_bold(&theme.widgets.title)
                } else {
                    to_style_dim(&theme.status.muted)
                };

                let sidebar_block = Block::default()
                    .borders(Borders::RIGHT)
                    .border_style(sidebar_border_style)
                    .title(" Topics ")
                    .title_style(if topics_focused {
                        to_style_bold(&theme.widgets.title)
                    } else {
                        to_style_dim(&theme.status.muted)
                    });

                let sidebar_inner = sidebar_block.inner(sidebar_area);
                f.render_widget(sidebar_block, sidebar_area);

                let list_items: Vec<ListItem> = if matches.is_empty() {
                    vec![ListItem::new("  (no matches)").style(to_style_dim(&theme.status.muted))]
                } else {
                    matches
                        .iter()
                        .enumerate()
                        .map(|(i, t)| {
                            let is_sel = i == selected_index;
                            let prefix = if is_sel { "❯ " } else { "  " };

                            let mut spans = vec![
                                Span::styled(
                                    prefix,
                                    if is_sel {
                                        to_style_bold(&theme.widgets.title)
                                    } else {
                                        Style::default()
                                    },
                                ),
                                Span::styled(
                                    t.name,
                                    if is_sel {
                                        Style::default().add_modifier(Modifier::BOLD)
                                    } else {
                                        Style::default()
                                    },
                                ),
                            ];

                            let cat_tag = match t.category {
                                crate::help::HelpCategory::Builtin => " builtin",
                                crate::help::HelpCategory::Pipeline => " pipe",
                                crate::help::HelpCategory::Language => " lang",
                                crate::help::HelpCategory::Security => " sec",
                                crate::help::HelpCategory::Concepts => " info",
                            };
                            spans.push(Span::styled(cat_tag, to_style_dim(&theme.status.muted)));

                            let row_style = if is_sel {
                                Style::default()
                                    .bg(to_color(&theme.widgets.item_selected_bg))
                                    .fg(to_color(&theme.widgets.item_selected_fg))
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default()
                            };

                            ListItem::new(Line::from(spans)).style(row_style)
                        })
                        .collect()
                };

                let list_widget = List::new(list_items)
                    .highlight_symbol("❯ ")
                    .highlight_style(
                        Style::default()
                            .bg(to_color(&theme.widgets.item_selected_bg))
                            .fg(to_color(&theme.widgets.item_selected_fg))
                            .add_modifier(Modifier::BOLD),
                    );

                f.render_stateful_widget(list_widget, sidebar_inner, &mut list_state);

                // Right Reading Pane
                let doc_focused = focus == FocusArea::Doc;
                let doc_title = selected_topic
                    .map(|t| format!(" {} ", t.name))
                    .unwrap_or_else(|| " Documentation ".to_string());

                let doc_block = Block::default()
                    .borders(Borders::NONE)
                    .title(doc_title)
                    .title_style(if doc_focused {
                        to_style_bold(&theme.widgets.title)
                    } else {
                        to_style_dim(&theme.status.muted)
                    });

                let doc_inner = doc_block.inner(reading_area);
                f.render_widget(doc_block, reading_area);

                let visible_doc_h = doc_inner.height as usize;
                if doc_scroll + visible_doc_h > total_doc_lines {
                    doc_scroll = total_doc_lines.saturating_sub(visible_doc_h);
                }

                let visible_lines: Vec<Line> = doc_lines
                    .into_iter()
                    .skip(doc_scroll)
                    .take(visible_doc_h)
                    .collect();

                let doc_p = Paragraph::new(visible_lines);
                f.render_widget(doc_p, doc_inner);

                // Scrollbar for doc
                if total_doc_lines > visible_doc_h && doc_inner.width > 2 {
                    let scrollbar_area = Rect::new(
                        doc_inner.x + doc_inner.width.saturating_sub(1),
                        doc_inner.y,
                        1,
                        doc_inner.height,
                    );
                    let mut sbar_state =
                        ScrollbarState::new(total_doc_lines).position(doc_scroll);
                    Scrollbar::default()
                        .orientation(ScrollbarOrientation::VerticalRight)
                        .begin_symbol(None)
                        .end_symbol(None)
                        .track_symbol(Some("│"))
                        .thumb_symbol("┃")
                        .style(to_style_dim(&theme.status.muted))
                        .thumb_style(to_style_bold(&theme.widgets.title))
                        .render(scrollbar_area, f.buffer_mut(), &mut sbar_state);
                }

                // 3. Status Footer
                let key_style = to_style_bold(&theme.status.info);
                let label_style = to_style(&theme.status.muted);

                let mut footer_spans = vec![
                    Span::styled(format!(" [Focus: {}]  ", focus.name()), to_style_bold(&theme.widgets.title)),
                ];

                let hints: &[(&str, &str)] = match focus {
                    FocusArea::Search => &[
                        ("Enter/↓", "Topics"),
                        ("Tab", "Switch"),
                        ("Esc", "Exit"),
                    ],
                    FocusArea::Topics => &[
                        ("j/k", "Select"),
                        ("Enter/→", "Read Doc"),
                        ("/", "Search"),
                        ("Tab", "Switch"),
                        ("q", "Quit"),
                    ],
                    FocusArea::Doc => &[
                        ("j/k", "Scroll Line"),
                        ("Ctrl-D/U", "Half Page"),
                        ("h/Esc", "Topics"),
                        ("/", "Search"),
                        ("Tab", "Switch"),
                        ("q", "Quit"),
                    ],
                };

                for (key, desc) in hints {
                    footer_spans.push(Span::styled(format!("[{key}] "), key_style));
                    footer_spans.push(Span::styled(format!("{desc}  "), label_style));
                }

                f.render_widget(Paragraph::new(Line::from(footer_spans)), footer_row);
            })
            .map_err(|e| ShellError::from(e.to_string()))?;

        // Read input
        let key = match input
            .poll(std::time::Duration::from_millis(100))
            .map_err(|e| ShellError::from(e.to_string()))?
        {
            InputPoll::Event(InputEvent::Key(key)) => key,
            InputPoll::Event(_) | InputPoll::Timeout => continue,
            InputPoll::Closed => break,
        };

        if key.action != KeyAction::Press {
            continue;
        }

        // Global shortcuts
        if key.modifiers.contains(Modifiers::CONTROL) && key.key == Key::Character('c') {
            break;
        }

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
            _ => {}
        }

        // Focus-specific key handling
        match focus {
            FocusArea::Search => match key.key {
                Key::Enter | Key::Down => {
                    focus = FocusArea::Topics;
                }
                Key::Escape => {
                    if query.is_empty() {
                        break;
                    }
                    query.clear();
                    matches = get_matching_topics(&query);
                    selected_index = 0;
                    doc_scroll = 0;
                }
                Key::Backspace => {
                    query.pop();
                    matches = get_matching_topics(&query);
                    selected_index = 0;
                    doc_scroll = 0;
                }
                Key::Character('u') if key.modifiers.contains(Modifiers::CONTROL) => {
                    query.clear();
                    matches = get_matching_topics(&query);
                    selected_index = 0;
                    doc_scroll = 0;
                }
                Key::Character('w') if key.modifiers.contains(Modifiers::CONTROL) => {
                    while let Some(c) = query.pop() {
                        if c.is_whitespace() {
                            break;
                        }
                    }
                    matches = get_matching_topics(&query);
                    selected_index = 0;
                    doc_scroll = 0;
                }
                Key::Character(c) if !key.modifiers.contains(Modifiers::CONTROL) => {
                    query.push(c);
                    matches = get_matching_topics(&query);
                    selected_index = 0;
                    doc_scroll = 0;
                }
                _ => {}
            },
            FocusArea::Topics => match key.key {
                Key::Character('q') | Key::Escape => {
                    break;
                }
                Key::Character('/') => {
                    focus = FocusArea::Search;
                }
                Key::Enter | Key::Right | Key::Character('l') => {
                    focus = FocusArea::Doc;
                }
                Key::Up | Key::Character('k') if topic_count > 0 => {
                    if selected_index > 0 {
                        selected_index -= 1;
                    } else {
                        selected_index = topic_count - 1;
                    }
                    doc_scroll = 0;
                }
                Key::Down | Key::Character('j') if topic_count > 0 => {
                    if selected_index + 1 < topic_count {
                        selected_index += 1;
                    } else {
                        selected_index = 0;
                    }
                    doc_scroll = 0;
                }
                Key::PageUp => {
                    selected_index = selected_index.saturating_sub(10);
                    doc_scroll = 0;
                }
                Key::PageDown if topic_count > 0 => {
                    selected_index = (selected_index + 10).min(topic_count - 1);
                    doc_scroll = 0;
                }
                Key::Home | Key::Character('g') => {
                    selected_index = 0;
                    doc_scroll = 0;
                }
                Key::End | Key::Character('G') if topic_count > 0 => {
                    selected_index = topic_count - 1;
                    doc_scroll = 0;
                }
                _ => {}
            },
            FocusArea::Doc => match key.key {
                Key::Character('q') => {
                    break;
                }
                Key::Escape | Key::Left | Key::Character('h') => {
                    focus = FocusArea::Topics;
                }
                Key::Character('/') => {
                    focus = FocusArea::Search;
                }
                Key::Up | Key::Character('k') => {
                    doc_scroll = doc_scroll.saturating_sub(1);
                }
                Key::Down | Key::Character('j') => {
                    if doc_scroll + 1 < total_doc_lines {
                        doc_scroll += 1;
                    }
                }
                Key::PageUp | Key::Character('u') if key.modifiers.contains(Modifiers::CONTROL) => {
                    doc_scroll = doc_scroll.saturating_sub(12);
                }
                Key::PageUp => {
                    doc_scroll = doc_scroll.saturating_sub(12);
                }
                Key::PageDown | Key::Character('d') if key.modifiers.contains(Modifiers::CONTROL) => {
                    if doc_scroll + 12 < total_doc_lines {
                        doc_scroll += 12;
                    }
                }
                Key::PageDown => {
                    if doc_scroll + 12 < total_doc_lines {
                        doc_scroll += 12;
                    }
                }
                Key::Home | Key::Character('g') => {
                    doc_scroll = 0;
                }
                Key::End | Key::Character('G') => {
                    doc_scroll = total_doc_lines.saturating_sub(5);
                }
                _ => {}
            },
        }
    }

    Ok(())
}

fn format_topic_lines(topic: &HelpTopic, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    let title_style = to_style_bold(&theme.widgets.title);
    let section_header_style = to_style_bold(&theme.syntax.keyword);
    let normal_style = Style::default();
    let muted_style = to_style_dim(&theme.status.muted);
    let flag_style = to_style_bold(&theme.syntax.variable);
    let syntax_style = to_style(&theme.syntax.string);
    let example_input_style = to_style(&theme.syntax.operator);

    // 1. Header: Name + Category badge
    lines.push(Line::from(vec![
        Span::styled(topic.name.to_string(), title_style),
        Span::styled(format!("  [{}]", topic.category.label()), to_style(&theme.syntax.keyword)),
    ]));

    let rule = "─".repeat(50);
    lines.push(Line::from(Span::styled(rule, to_style_dim(&theme.status.muted))));
    lines.push(Line::from(Span::styled(topic.summary.to_string(), normal_style)));
    lines.push(Line::raw(""));

    // 2. Syntax
    if !topic.syntax.is_empty() {
        lines.push(Line::from(Span::styled("SYNTAX", section_header_style)));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(topic.syntax.to_string(), syntax_style),
        ]));
        lines.push(Line::raw(""));
    }

    // 3. Description
    if !topic.description.is_empty() {
        lines.push(Line::from(Span::styled("DESCRIPTION", section_header_style)));
        for d_line in topic.description.lines() {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(d_line.to_string(), normal_style),
            ]));
        }
        lines.push(Line::raw(""));
    }

    // 4. Flags / Options
    if !topic.flags.is_empty() {
        lines.push(Line::from(Span::styled("OPTIONS", section_header_style)));
        for f in topic.flags {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{:<20}", f.flag), flag_style),
                Span::styled(f.desc.to_string(), muted_style),
            ]));
        }
        lines.push(Line::raw(""));
    }

    // 5. Examples
    if !topic.examples.is_empty() {
        lines.push(Line::from(Span::styled("EXAMPLES", section_header_style)));
        for ex in topic.examples {
            lines.push(Line::from(vec![
                Span::styled("  fsh> ", to_style_dim(&theme.status.muted)),
                Span::styled(ex.input.to_string(), example_input_style),
            ]));
            if !ex.explanation.is_empty() {
                lines.push(Line::from(vec![
                    Span::raw("       "),
                    Span::styled(ex.explanation.to_string(), muted_style),
                ]));
            }
            lines.push(Line::raw(""));
        }
    }

    // 6. See Also
    if !topic.related.is_empty() {
        lines.push(Line::from(Span::styled("SEE ALSO", section_header_style)));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(topic.related.join(", "), to_style(&theme.status.info)),
        ]));
        lines.push(Line::raw(""));
    }

    lines
}
