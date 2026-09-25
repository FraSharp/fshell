// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Interactive search bar component with visible cursor, text editing, and horizontal scrolling.

use crate::tui::theme;
use fshell_core::theme::Theme;
use fshell_terminal::input::{Key, KeyAction, KeyEvent, Modifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

/// State of an interactive text search bar.
#[derive(Debug, Clone, Default)]
pub struct SearchBarState {
    pub query: String,
    pub cursor: usize,        // character index
    pub scroll_offset: usize, // horizontal scroll offset in characters
}

impl SearchBarState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_query(mut self, q: impl Into<String>) -> Self {
        self.set_query(q);
        self
    }

    pub fn set_query(&mut self, q: impl Into<String>) {
        self.query = q.into();
        self.cursor = self.query.chars().count();
        self.scroll_offset = 0;
    }

    pub fn clear(&mut self) {
        self.query.clear();
        self.cursor = 0;
        self.scroll_offset = 0;
    }

    /// Handles keyboard events for text editing. Returns `true` if the search query was modified.
    pub fn handle_key(&mut self, key: &KeyEvent) -> bool {
        if key.action != KeyAction::Press {
            return false;
        }

        let char_count = self.query.chars().count();

        // Control key chords
        if key.modifiers.contains(Modifiers::CONTROL) {
            match key.key {
                Key::Character('a') => {
                    self.cursor = 0;
                    self.scroll_offset = 0;
                    return false;
                }
                Key::Character('e') => {
                    self.cursor = char_count;
                    return false;
                }
                Key::Character('u') => {
                    // Delete from beginning of line to cursor
                    let tail: String = self.query.chars().skip(self.cursor).collect();
                    self.query = tail;
                    self.cursor = 0;
                    self.scroll_offset = 0;
                    return true;
                }
                Key::Character('k') => {
                    // Delete from cursor to end of line
                    let head: String = self.query.chars().take(self.cursor).collect();
                    self.query = head;
                    return true;
                }
                Key::Character('w') => {
                    // Delete word before cursor
                    if self.cursor == 0 {
                        return false;
                    }
                    let chars: Vec<char> = self.query.chars().collect();
                    let mut new_cursor = self.cursor;
                    // Skip any trailing spaces before word
                    while new_cursor > 0 && chars[new_cursor - 1].is_whitespace() {
                        new_cursor -= 1;
                    }
                    // Skip word characters
                    while new_cursor > 0 && !chars[new_cursor - 1].is_whitespace() {
                        new_cursor -= 1;
                    }
                    let head: String = chars[..new_cursor].iter().collect();
                    let tail: String = chars[self.cursor..].iter().collect();
                    self.query = format!("{head}{tail}");
                    self.cursor = new_cursor;
                    return true;
                }
                _ => return false,
            }
        }

        match key.key {
            Key::Left => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                false
            }
            Key::Right => {
                if self.cursor < char_count {
                    self.cursor += 1;
                }
                false
            }
            Key::Home => {
                self.cursor = 0;
                self.scroll_offset = 0;
                false
            }
            Key::End => {
                self.cursor = char_count;
                false
            }
            Key::Backspace => {
                if self.cursor > 0 {
                    let chars: Vec<char> = self.query.chars().collect();
                    let head: String = chars[..self.cursor - 1].iter().collect();
                    let tail: String = chars[self.cursor..].iter().collect();
                    self.query = format!("{head}{tail}");
                    self.cursor -= 1;
                    true
                } else {
                    false
                }
            }
            Key::Delete => {
                if self.cursor < char_count {
                    let chars: Vec<char> = self.query.chars().collect();
                    let head: String = chars[..self.cursor].iter().collect();
                    let tail: String = chars[self.cursor + 1..].iter().collect();
                    self.query = format!("{head}{tail}");
                    true
                } else {
                    false
                }
            }
            Key::Character(c) => {
                let chars: Vec<char> = self.query.chars().collect();
                let head: String = chars[..self.cursor].iter().collect();
                let tail: String = chars[self.cursor..].iter().collect();
                self.query = format!("{head}{c}{tail}");
                self.cursor += 1;
                true
            }
            _ => false,
        }
    }

    /// Renders the search bar into a given area buffer.
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        prefix: &str,
        placeholder: &str,
        focused: bool,
    ) {
        if area.width < 4 || area.height < 1 {
            return;
        }

        let prefix_width = prefix.width();
        let max_content_width = (area.width as usize).saturating_sub(prefix_width + 1);

        // Adjust horizontal scroll offset to keep cursor visible
        if self.cursor < self.scroll_offset {
            self.scroll_offset = self.cursor;
        } else if self.cursor >= self.scroll_offset + max_content_width {
            self.scroll_offset = self.cursor.saturating_sub(max_content_width) + 1;
        }

        let prefix_style = if focused {
            theme::title_style(theme)
        } else {
            theme::muted_style(theme)
        };

        let mut spans = vec![Span::styled(prefix.to_string(), prefix_style)];

        if self.query.is_empty() && !focused {
            spans.push(Span::styled(
                placeholder.to_string(),
                theme::muted_style(theme),
            ));
        } else {
            let chars: Vec<char> = self.query.chars().collect();
            let visible_chars: String = chars
                .iter()
                .skip(self.scroll_offset)
                .take(max_content_width)
                .collect();

            let cursor_rel = self.cursor.saturating_sub(self.scroll_offset);
            let text_style = Style::default();

            if focused {
                let mut char_idx = 0;
                let mut cursor_rendered = false;

                for c in visible_chars.chars() {
                    if char_idx == cursor_rel {
                        // Render cursor on top of the character
                        spans.push(Span::styled(
                            c.to_string(),
                            Style::default().add_modifier(Modifier::REVERSED),
                        ));
                        cursor_rendered = true;
                    } else {
                        spans.push(Span::styled(c.to_string(), text_style));
                    }
                    char_idx += 1;
                }

                // If cursor is at the end of the visible text
                if !cursor_rendered && cursor_rel == char_idx {
                    spans.push(Span::styled("█", theme::title_style(theme)));
                }
            } else {
                spans.push(Span::styled(visible_chars, text_style));
            }
        }

        Paragraph::new(Line::from(spans)).render(area, buf);
    }
}
