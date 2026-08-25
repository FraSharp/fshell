// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Daemon-side rendering.
//!
//! Uses a persistent ratatui `Terminal` across frames so that diffing
//! works correctly — only changed cells emit escape sequences, eliminating
//! flicker. The resulting ANSI bytes are sent to the client.

use std::io;

use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::{Terminal, Viewport};

use crate::app::statusbar;
use crate::app::theme;
use crate::grid::Grid;

use super::session::Session;
use super::window::Window;

use std::sync::{Arc, Mutex};

/// Shared buffer for capturing ANSI bytes.
struct SharedBuffer {
    data: Arc<Mutex<Vec<u8>>>,
}

impl SharedBuffer {
    fn new() -> (Self, BufferReader) {
        let data = Arc::new(Mutex::new(Vec::new()));
        (Self { data: data.clone() }, BufferReader { data })
    }
}

impl io::Write for SharedBuffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.data
            .lock()
            .expect("SharedBuffer mutex poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Reader that extracts the captured bytes.
struct BufferReader {
    data: Arc<Mutex<Vec<u8>>>,
}

impl BufferReader {
    fn take_bytes(&self) -> Vec<u8> {
        std::mem::take(&mut self.data.lock().expect("SharedBuffer mutex poisoned"))
    }
}

/// Persistent renderer that keeps a ratatui `Terminal` alive across frames.
///
/// This allows ratatui's internal diffing to work: it remembers the previous
/// frame and only emits escape sequences for cells that actually changed.
pub struct FrameRenderer {
    terminal: Terminal<CrosstermBackend<SharedBuffer>>,
    reader: BufferReader,
}

impl FrameRenderer {
    /// Create a new renderer for the given terminal dimensions.
    pub fn new(cols: u16, rows: u16) -> io::Result<Self> {
        let area = Rect::new(0, 0, cols, rows);
        let (writer, reader) = SharedBuffer::new();
        let backend = CrosstermBackend::new(writer);
        let terminal = Terminal::with_options(
            backend,
            ratatui::TerminalOptions {
                viewport: Viewport::Fixed(area),
            },
        )?;

        Ok(Self { terminal, reader })
    }

    /// Resize the terminal to new dimensions.
    pub fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        let _ = self.terminal.resize(Rect::new(0, 0, cols, rows));
        Ok(())
    }

    /// Render a single frame and return the ANSI bytes.
    ///
    /// ratatui's diffing ensures only changed cells are emitted.
    pub fn render_frame(
        &mut self,
        session: &Session,
        active_window: &Window,
        cols: u16,
        rows: u16,
    ) -> io::Result<Vec<u8>> {
        let area = Rect::new(0, 0, cols, rows);

        // Reserve rows for status bar and optional rename bar.
        let rename_height: u16 = if active_window.rename_state.is_some() {
            1
        } else {
            0
        };
        let status_height: u16 = 1;
        let ui_height = status_height + rename_height;
        let pane_area = Rect {
            x: 0,
            y: 0,
            width: cols,
            height: rows.saturating_sub(ui_height),
        };
        let status_area = Rect {
            x: 0,
            y: rows.saturating_sub(status_height),
            width: cols,
            height: status_height,
        };

        let layout = active_window.bsp.compute_layout(pane_area);

        self.terminal.draw(|frame| {
            // Render each pane.
            for (pane_id, rect) in &layout {
                let is_focused = *pane_id == active_window.focus.focused_pane;

                // In prefix mode, no pane looks focused — the status bar
                // carries the PREFIX indicator instead.
                let border_style = if is_focused && !active_window.focus.prefix_active {
                    theme::border_focused()
                } else {
                    theme::border_unfocused()
                };

                let title_style = if is_focused && !active_window.focus.prefix_active {
                    theme::title_focused()
                } else {
                    theme::title_unfocused()
                };

                // Build pane title: " ID shell_name " or " ID label "
                let title = if let Some(pane) = active_window.panes.get(pane_id) {
                    let display_name = pane.label.as_deref().unwrap_or(&pane.shell_name);
                    format!(" {} {} ", pane_id, display_name)
                } else {
                    format!(" {} ", pane_id)
                };

                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(Span::styled(title, title_style));
                frame.render_widget(block, *rect);

                let inner = Rect {
                    x: rect.x + 1,
                    y: rect.y + 1,
                    width: rect.width.saturating_sub(2),
                    height: rect.height.saturating_sub(2),
                };

                if inner.width == 0 || inner.height == 0 {
                    continue;
                }

                if let Some(pane) = active_window.panes.get(pane_id)
                    && let Ok(grid) = pane.grid.try_read()
                {
                    render_grid_content(frame, &grid, inner, is_focused);
                }
            }

            // Status bar.
            let status_ctx = statusbar::StatusBarContext {
                session_name: &session.name,
                windows: &session.windows,
                active_window_idx: session.active_window,
                prefix_active: active_window.focus.prefix_active,
                panes: &active_window.panes,
                focus: &active_window.focus,
            };
            statusbar::render_status_bar(frame, status_area, &status_ctx);

            // Rename bar.
            if let Some(ref state) = active_window.rename_state {
                let rename_y = rows.saturating_sub(ui_height);
                let rename_area = Rect {
                    x: 0,
                    y: rename_y,
                    width: cols,
                    height: 1,
                };
                let prompt = match state.target {
                    crate::daemon::window::RenameTarget::Pane => " Rename Pane: ",
                    crate::daemon::window::RenameTarget::Window => " Rename Window: ",
                };
                render_rename_bar(frame, rename_area, prompt, &state.buffer);
            }

            // Help overlay.
            if active_window.show_help {
                render_help_overlay(frame, active_window.help_scroll, area);
            }
        })?;

        // Extract only the bytes written since last call.
        Ok(self.reader.take_bytes())
    }
}

/// Render grid content into a ratatui Frame area.
fn render_grid_content(frame: &mut ratatui::Frame, grid: &Grid, inner: Rect, is_focused: bool) {
    for row_idx in 0..inner.height {
        let grid_row = row_idx as usize;
        if grid_row >= grid.height() {
            break;
        }

        if let Some(viewport_row) = grid.viewport_iter().nth(grid_row) {
            let cells = viewport_row.cells();
            let mut spans = Vec::new();
            let mut current_style = Style::default();
            let mut current_text = String::new();

            for col in 0..inner.width {
                let grid_col = col as usize;
                if grid_col < cells.len() {
                    let cell = &cells[grid_col];

                    // Skip wide-continuation cells (they take no display space)
                    if cell.wide_continuation {
                        continue;
                    }

                    let fg = cell.pen.fg.map(|c| match c {
                        crate::grid::pen::Color::Indexed(i) => Color::Indexed(i),
                        crate::grid::pen::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
                        _ => Color::Reset,
                    });
                    let bg = cell.pen.bg.map(|c| match c {
                        crate::grid::pen::Color::Indexed(i) => Color::Indexed(i),
                        crate::grid::pen::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
                        _ => Color::Reset,
                    });

                    let mut style = Style::default();
                    if let Some(fg) = fg {
                        style = style.fg(fg);
                    }
                    if let Some(bg) = bg {
                        style = style.bg(bg);
                    }
                    if cell.pen.bold {
                        style = style.add_modifier(Modifier::BOLD);
                    }
                    if cell.pen.italic {
                        style = style.add_modifier(Modifier::ITALIC);
                    }
                    if cell.pen.underline {
                        style = style.add_modifier(Modifier::UNDERLINED);
                    }
                    if cell.pen.dim {
                        style = style.add_modifier(Modifier::DIM);
                    }
                    if cell.pen.inverse {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                    if cell.pen.hidden {
                        style = style.add_modifier(Modifier::HIDDEN);
                    }
                    if cell.pen.strikethrough {
                        style = style.add_modifier(Modifier::CROSSED_OUT);
                    }

                    if style != current_style && !current_text.is_empty() {
                        spans.push(Span::styled(current_text.clone(), current_style));
                        current_text.clear();
                    }
                    current_style = style;

                    let ch = if cell.character == '\0' || cell.character == '\x20' {
                        ' '
                    } else {
                        cell.character
                    };
                    current_text.push(ch);
                }
            }

            if !current_text.is_empty() {
                spans.push(Span::styled(current_text, current_style));
            }

            let line_area = Rect {
                x: inner.x,
                y: inner.y + row_idx,
                width: inner.width,
                height: 1,
            };
            let line = Line::from(spans);
            let paragraph = Paragraph::new(line);
            frame.render_widget(paragraph, line_area);
        }
    }

    // Scroll position indicator.
    if !grid.is_at_bottom() && inner.width > 12 && inner.height > 2 {
        let pos = grid.scroll_position();
        let indicator = format!(" ↑ {}/{}", pos.offset, pos.scrollback_len);
        let indicator_width = indicator.len() as u16;
        if indicator_width <= inner.width {
            let indicator_area = Rect {
                x: inner.x + inner.width - indicator_width,
                y: inner.y,
                width: indicator_width,
                height: 1,
            };
            let scroll_indicator = Paragraph::new(Line::from(Span::styled(
                indicator,
                theme::scroll_indicator(),
            )));
            frame.render_widget(scroll_indicator, indicator_area);
        }
    }

    // Cursor for focused pane — only show when at the bottom (live output).
    if is_focused && grid.cursor_visible && grid.is_at_bottom() {
        let (cursor_row, cursor_col) = grid.cursor();
        let cursor_x = inner.x + cursor_col as u16;
        let cursor_y = inner.y + cursor_row as u16;
        if cursor_x < inner.x + inner.width && cursor_y < inner.y + inner.height {
            frame.set_cursor_position((cursor_x, cursor_y));
        }
    }
}

/// Render the rename input bar.
fn render_rename_bar(frame: &mut ratatui::Frame, area: Rect, prompt: &str, buffer: &str) {
    let cursor = "█";
    let display = format!("{}{}{}", prompt, buffer, cursor);

    // Pad to fill the width.
    let padding = area.width.saturating_sub(display.len() as u16);
    let padded = format!("{}{}", display, " ".repeat(padding as usize));

    let line = Line::from(vec![Span::styled(
        padded,
        Style::default().fg(theme::TEXT).bg(theme::BG_DARK),
    )]);
    let bar = Paragraph::new(line);
    frame.render_widget(bar, area);
}

/// Render a help/cheatsheet overlay centered on screen.
fn render_help_overlay(frame: &mut ratatui::Frame, scroll: u16, area: Rect) {
    if area.width < 30 || area.height < 15 {
        return;
    }

    let entries = help_entries();

    // Calculate content height.
    let mut content_height: u16 = 0;
    let mut current_section = "";
    for entry in &entries {
        if entry.section != current_section {
            current_section = entry.section;
            content_height += 2;
        }
        content_height += 1;
    }

    let footer_height: u16 = 1;
    let total_height = content_height + footer_height + 4;

    let panel_width = (area.width as f32 * 0.6).min(80.0) as u16;
    let panel_height = total_height.min((area.height as f32 * 0.85) as u16);

    let horizontal = Layout::horizontal([
        Constraint::Length((area.width.saturating_sub(panel_width)) / 2),
        Constraint::Length(panel_width),
        Constraint::Length((area.width.saturating_sub(panel_width)) / 2),
    ]);
    let vertical = Layout::vertical([
        Constraint::Length((area.height.saturating_sub(panel_height)) / 2),
        Constraint::Length(panel_height),
        Constraint::Length((area.height.saturating_sub(panel_height)) / 2),
    ]);
    let chunks = vertical.split(horizontal.split(area)[1]);
    let panel_area = chunks[1];

    frame.render_widget(Clear, panel_area);

    let shows_scroll = total_height > panel_height;
    let footer_text = if shows_scroll {
        "↑/↓ scroll · Esc close"
    } else {
        "Esc to close"
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title("fshell-panes help")
        .title_bottom(Line::from(vec![Span::styled(
            footer_text,
            theme::help_footer(),
        )]))
        .border_style(theme::help_border());

    // Build content lines.
    let sep_width = panel_width.saturating_sub(4) as usize;
    let mut lines = Vec::new();
    let mut current_section = "";
    for entry in &entries {
        if entry.section != current_section {
            current_section = entry.section;
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(vec![Span::styled(
                entry.section,
                theme::help_section(),
            )]));
            lines.push(Line::from(vec![Span::styled(
                "─".repeat(sep_width),
                Style::default().fg(theme::TEXT_DIM),
            )]));
        }
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(entry.key.to_string(), theme::help_key()),
            Span::raw("    "),
            Span::raw(entry.description),
        ]));
    }

    let paragraph = Paragraph::new(lines)
        .scroll((scroll, 0))
        .wrap(Wrap { trim: false });

    frame.render_widget(block, panel_area);
    let inner = panel_area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    frame.render_widget(paragraph, inner);
}

/// Help entry for the cheatsheet.
struct HelpEntry {
    section: &'static str,
    key: &'static str,
    description: &'static str,
}

/// Returns all help entries in display order.
fn help_entries() -> Vec<HelpEntry> {
    vec![
        // Pane Management
        HelpEntry {
            section: "Pane Management",
            key: "Ctrl-A \"",
            description: "Split horizontal",
        },
        HelpEntry {
            section: "Pane Management",
            key: "Ctrl-A %",
            description: "Split vertical",
        },
        HelpEntry {
            section: "Pane Management",
            key: "Ctrl-A x",
            description: "Close pane",
        },
        HelpEntry {
            section: "Pane Management",
            key: "Ctrl-A ,",
            description: "Rename pane",
        },
        HelpEntry {
            section: "Pane Management",
            key: "  Enter",
            description: "  Confirm rename",
        },
        HelpEntry {
            section: "Pane Management",
            key: "  Esc",
            description: "  Cancel rename",
        },
        HelpEntry {
            section: "Pane Management",
            key: "  Bksp",
            description: "  Delete character",
        },
        // Navigation
        HelpEntry {
            section: "Navigation",
            key: "Ctrl-A ↑↓←→",
            description: "Focus adjacent pane",
        },
        HelpEntry {
            section: "Navigation",
            key: "Click",
            description: "Focus clicked pane",
        },
        // Windows
        HelpEntry {
            section: "Windows",
            key: "Ctrl-A c",
            description: "New window",
        },
        HelpEntry {
            section: "Windows",
            key: "Ctrl-A n",
            description: "Next window",
        },
        HelpEntry {
            section: "Windows",
            key: "Ctrl-A p",
            description: "Previous window",
        },
        HelpEntry {
            section: "Windows",
            key: "Ctrl-A 0-9",
            description: "Switch to window N",
        },
        HelpEntry {
            section: "Windows",
            key: "Ctrl-A &",
            description: "Close window",
        },
        HelpEntry {
            section: "Windows",
            key: "Ctrl-A W",
            description: "Rename window",
        },
        // Scrolling
        HelpEntry {
            section: "Scrolling",
            key: "Shift+↑/↓",
            description: "Scroll 3 lines",
        },
        HelpEntry {
            section: "Scrolling",
            key: "PgUp/PgDn",
            description: "Scroll 1 page",
        },
        HelpEntry {
            section: "Scrolling",
            key: "Mouse Wheel",
            description: "Scroll 1 line",
        },
        // Mode
        HelpEntry {
            section: "Mode",
            key: "Ctrl-A",
            description: "Enter prefix mode",
        },
        HelpEntry {
            section: "Mode",
            key: "Esc",
            description: "Exit prefix mode",
        },
        HelpEntry {
            section: "Mode",
            key: "Ctrl-A ?",
            description: "Toggle this help",
        },
        // General
        HelpEntry {
            section: "General",
            key: "Ctrl-A q",
            description: "Quit fshell-panes",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Grid;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn dummy_session() -> (Session, Window) {
        let grid = Arc::new(RwLock::new(Grid::new(78, 22, 100)));
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let mut window = Window::new(0, 0);
        window.name = "bash".to_string();
        window.add_pane(0, grid, tx, "bash".to_string());

        let mut session = Session::new("test".to_string(), 0);
        session.add_window(window);

        // Take the window back out for the test.
        let window = session.windows.remove(0);
        (session, window)
    }

    // NOTE: Renderer tests require an actual terminal and are skipped in CI.
    // These tests are best run manually with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn render_frame_produces_bytes() {
        let (session, window) = dummy_session();
        let mut renderer = FrameRenderer::new(80, 24).unwrap();
        let result = renderer.render_frame(&session, &window, 80, 24);
        assert!(result.is_ok());
        let bytes = result.unwrap();
        assert!(!bytes.is_empty());
        assert!(bytes.windows(1).any(|w| w[0] == 0x1b));
    }

    #[test]
    #[ignore]
    fn render_frame_diffing_works() {
        let (session, window) = dummy_session();
        let mut renderer = FrameRenderer::new(80, 24).unwrap();

        // First frame: should produce bytes (full draw).
        let first = renderer.render_frame(&session, &window, 80, 24).unwrap();
        assert!(!first.is_empty());

        // Second frame with no changes: should produce fewer bytes (diff only).
        let second = renderer.render_frame(&session, &window, 80, 24).unwrap();
        // The second frame should be smaller because only diffs are sent.
        assert!(second.len() <= first.len());
    }
}
