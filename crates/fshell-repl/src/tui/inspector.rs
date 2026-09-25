// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Interactive fullscreen Table Inspector for structured data exploration.

use crate::terminal_mode::FullscreenTerminalGuard;
use crate::tui::components::data_grid::{DataGrid, DataGridState};
use crate::tui::components::modal_dialog;
use crate::tui::components::search_bar::SearchBarState;
use crate::tui::theme;
use fshell_core::Val;
use fshell_core::theme::Theme;
use fshell_terminal::input::{CrosstermEventSource, InputEvent, InputPoll, Key, Modifiers};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use std::io::IsTerminal;
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthStr;

/// Run the interactive table inspector for a slice of values.
pub fn run_table_inspector(items: Vec<Val>, title: &str, theme: &Theme) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }

    if fshell_engine::is_test_mode()
        || !std::io::stdout().is_terminal()
        || !std::io::stdin().is_terminal()
    {
        return Ok(());
    }

    let _guard = FullscreenTerminalGuard::enter(false)
        .map_err(|e| format!("inspector: failed to enter raw terminal mode: {e}"))?;

    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = ratatui::Terminal::new(backend)
        .map_err(|e| format!("inspector: failed to create terminal backend: {e}"))?;
    let _ = terminal.clear();

    let mut input = CrosstermEventSource::new();
    let mut state = DataGridState::new();
    let mut search_bar = SearchBarState::new();
    let mut is_searching = false;
    let mut is_viewing_detail = false;
    let mut detail_scroll: usize = 0;
    let mut toast_message: Option<(String, Instant)> = None;

    let columns = DataGrid::extract_columns(&items);

    loop {
        // Expire toast message after 3 seconds
        if let Some((_, created)) = &toast_message
            && created.elapsed() > Duration::from_secs(3)
        {
            toast_message = None;
        }

        // Prepare row indices according to current search/sort
        let filtered_indices = DataGrid::filter_and_sort_indices(
            &items,
            &state.filter_query,
            state.sort_column.as_ref(),
        );
        let total_rows = filtered_indices.len();

        let draw_res = terminal.draw(|f| {
            let size = f.area();
            if size.width < 20 || size.height < 6 {
                let msg = Paragraph::new("Terminal too small for inspector")
                    .style(theme::error_style(theme));
                f.render_widget(msg, size);
                return;
            }

            // Outer container
            let sort_info = match &state.sort_column {
                Some((col, dir)) => format!(" (sorted: {col} {})", dir.symbol()),
                None => String::new(),
            };

            let title_text = format!(" Table Inspector [{title}] ");
            let count_info = format!(" {} items{} ", total_rows, sort_info);

            let outer_block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(theme::muted_style(theme))
                .title(Span::styled(title_text, theme::title_style(theme)))
                .title(
                    Line::from(Span::styled(count_info, theme::muted_style(theme))).right_aligned(),
                );

            f.render_widget(outer_block, size);

            let inner = Rect::new(
                size.x.saturating_add(1),
                size.y.saturating_add(1),
                size.width.saturating_sub(2),
                size.height.saturating_sub(2),
            );

            // Split into table, record preview line, and status/search footer
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(3),
                    Constraint::Length(1),
                    Constraint::Length(1),
                ])
                .split(inner);

            let table_area = chunks[0];
            let preview_area = chunks[1];
            let footer_area = chunks[2];

            // Render table grid
            let grid = DataGrid::new(&items, theme, &mut state);
            grid.render(table_area, f.buffer_mut());

            // Render preview line for selected record
            let sel_idx = state.selected();
            let preview_text = if sel_idx < total_rows {
                let sel_val = &items[filtered_indices[sel_idx]];
                let compact_repr = format_val_compact_preview(sel_val, preview_area.width as usize);
                format!("Record {} of {}: {}", sel_idx + 1, total_rows, compact_repr)
            } else {
                format!("Record 0 of {total_rows}")
            };

            let preview_widget = Paragraph::new(Line::from(vec![
                Span::styled("─ ", theme::muted_style(theme)),
                Span::styled(preview_text, theme::foreground_style(theme)),
            ]));
            f.render_widget(preview_widget, preview_area);

            // Render footer
            if is_searching {
                let prompt = Span::styled("/ ", theme::keyword_style(theme));
                let search_content = search_bar.query.clone();
                let line = Line::from(vec![
                    prompt,
                    Span::styled(search_content, theme::foreground_style(theme)),
                ]);
                f.render_widget(Paragraph::new(line), footer_area);
                // Set cursor position
                f.set_cursor_position((
                    (footer_area.x + 2 + search_bar.cursor as u16)
                        .min(footer_area.right().saturating_sub(1)),
                    footer_area.y,
                ));
            } else if let Some((msg, _)) = &toast_message {
                let toast_widget = Paragraph::new(Line::from(vec![
                    Span::styled("✔ ", theme::ok_style(theme)),
                    Span::styled(msg.as_str(), theme::title_style(theme)),
                ]));
                f.render_widget(toast_widget, footer_area);
            } else {
                let active_col_name = if !columns.is_empty() {
                    let idx = state.active_column_idx.min(columns.len() - 1);
                    &columns[idx]
                } else {
                    "—"
                };

                let shortcuts = Line::from(vec![
                    Span::styled("s", theme::keyword_style(theme)),
                    Span::styled(
                        format!(": Sort [{active_col_name}]   "),
                        theme::muted_style(theme),
                    ),
                    Span::styled("h/l", theme::keyword_style(theme)),
                    Span::styled(": Col   ", theme::muted_style(theme)),
                    Span::styled("j/k", theme::keyword_style(theme)),
                    Span::styled(": Row   ", theme::muted_style(theme)),
                    Span::styled("/", theme::keyword_style(theme)),
                    Span::styled(": Filter   ", theme::muted_style(theme)),
                    Span::styled("Enter", theme::keyword_style(theme)),
                    Span::styled(": Drill Down   ", theme::muted_style(theme)),
                    Span::styled("e", theme::keyword_style(theme)),
                    Span::styled(": Export   ", theme::muted_style(theme)),
                    Span::styled("q", theme::keyword_style(theme)),
                    Span::styled(": Exit", theme::muted_style(theme)),
                ]);
                f.render_widget(Paragraph::new(shortcuts), footer_area);
            }

            // Render Detail Card modal if open
            if is_viewing_detail && let Some(&row_idx) = filtered_indices.get(sel_idx) {
                let sel_val = &items[row_idx];
                let dialog_area = modal_dialog::centered_percent(75, 70, size);
                let modal_area = modal_dialog::render_modal_frame(
                    dialog_area,
                    f.buffer_mut(),
                    theme,
                    &format!(
                        "Record Detail ({}/{}) — Esc/Enter to close",
                        sel_idx + 1,
                        total_rows
                    ),
                );

                let detail_lines = format_record_detail(sel_val, theme);
                let visible_lines: Vec<Line> = detail_lines
                    .into_iter()
                    .skip(detail_scroll)
                    .take(modal_area.height as usize)
                    .collect();

                f.render_widget(Paragraph::new(visible_lines), modal_area);
            }
        });

        if draw_res.is_err() {
            break;
        }

        // Input event processing
        match input.poll(Duration::from_millis(50)) {
            Ok(InputPoll::Event(InputEvent::Key(key_event))) => {
                if is_viewing_detail {
                    match key_event.key {
                        Key::Escape | Key::Character('q') | Key::Enter => {
                            is_viewing_detail = false;
                            detail_scroll = 0;
                        }
                        Key::Character('j') | Key::Down => {
                            detail_scroll += 1;
                        }
                        Key::Character('k') | Key::Up => {
                            detail_scroll = detail_scroll.saturating_sub(1);
                        }
                        Key::PageDown => {
                            detail_scroll = detail_scroll.saturating_add(10);
                        }
                        Key::PageUp => {
                            detail_scroll = detail_scroll.saturating_sub(10);
                        }
                        Key::Character('d') if key_event.modifiers.contains(Modifiers::CONTROL) => {
                            detail_scroll = detail_scroll.saturating_add(10);
                        }
                        Key::Character('u') if key_event.modifiers.contains(Modifiers::CONTROL) => {
                            detail_scroll = detail_scroll.saturating_sub(10);
                        }
                        _ => {}
                    }
                    continue;
                }

                if is_searching {
                    match key_event.key {
                        Key::Escape => {
                            is_searching = false;
                            search_bar.clear();
                            state.filter_query.clear();
                        }
                        Key::Enter => {
                            is_searching = false;
                        }
                        _ => {
                            if search_bar.handle_key(&key_event) {
                                state.filter_query = search_bar.query.clone();
                            }
                        }
                    }
                    continue;
                }

                // Normal mode
                match key_event.key {
                    Key::Character('q') | Key::Escape => {
                        break;
                    }
                    Key::Character('c') if key_event.modifiers.contains(Modifiers::CONTROL) => {
                        break;
                    }
                    Key::Character('j') | Key::Down => {
                        state.select_next(total_rows);
                    }
                    Key::Character('k') | Key::Up => {
                        state.select_prev(total_rows);
                    }
                    Key::PageDown => {
                        state.page_down(total_rows, 10);
                    }
                    Key::PageUp => {
                        state.page_up(10);
                    }
                    Key::Character('d') if key_event.modifiers.contains(Modifiers::CONTROL) => {
                        state.page_down(total_rows, 10);
                    }
                    Key::Character('u') if key_event.modifiers.contains(Modifiers::CONTROL) => {
                        state.page_up(10);
                    }
                    Key::Home | Key::Character('g') => {
                        state.table_state.select(Some(0));
                    }
                    Key::End | Key::Character('G') => {
                        if total_rows > 0 {
                            state.table_state.select(Some(total_rows - 1));
                        }
                    }
                    Key::Character('l') | Key::Right => {
                        state.next_column(columns.len());
                    }
                    Key::Character('h') | Key::Left => {
                        state.prev_column(columns.len());
                    }
                    Key::Character('s') => {
                        state.toggle_active_sort(&columns);
                    }
                    Key::Character('/') => {
                        is_searching = true;
                    }
                    Key::Enter => {
                        if total_rows > 0 {
                            is_viewing_detail = true;
                            detail_scroll = 0;
                        }
                    }
                    Key::Character('e') => {
                        let timestamp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        let export_path = format!("/tmp/fshell_export_{timestamp}.json");
                        let export_items: Vec<&Val> =
                            filtered_indices.iter().map(|&idx| &items[idx]).collect();
                        match export_to_json(&export_items, &export_path) {
                            Ok(count) => {
                                toast_message = Some((
                                    format!("Exported {count} records to {export_path}"),
                                    Instant::now(),
                                ));
                            }
                            Err(e) => {
                                toast_message =
                                    Some((format!("Export failed: {e}"), Instant::now()));
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(InputPoll::Event(InputEvent::Paste(text))) => {
                if is_searching {
                    search_bar.query.push_str(&text);
                    search_bar.cursor = search_bar.query.chars().count();
                    state.filter_query = search_bar.query.clone();
                }
            }
            Ok(InputPoll::Event(_)) | Ok(InputPoll::Timeout) => {}
            Ok(InputPoll::Closed) | Err(_) => break,
        }
    }

    let _ = terminal.clear();
    Ok(())
}

fn format_val_compact_preview(val: &Val, max_width: usize) -> String {
    match val {
        Val::Map(map) => {
            let mut parts = Vec::new();
            for (k, v) in map {
                parts.push(format!("{}: {}", k.as_str(), v.to_text()));
            }
            let full = format!("{{ {} }}", parts.join(", "));
            if full.width() > max_width {
                let mut truncated = String::new();
                let mut w = 0;
                for c in full.chars() {
                    let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
                    if w + cw + 3 > max_width {
                        break;
                    }
                    truncated.push(c);
                    w += cw;
                }
                truncated.push_str("...");
                truncated
            } else {
                full
            }
        }
        other => other.to_text(),
    }
}

fn format_record_detail<'a>(val: &'a Val, theme: &'a Theme) -> Vec<Line<'a>> {
    let mut lines = Vec::new();

    match val {
        Val::Map(map) => {
            for (k, v) in map {
                let key_span =
                    Span::styled(format!("{:<20}", k.as_str()), theme::keyword_style(theme));

                let (type_name, value_str) = match v {
                    Val::Null => ("null", "null".to_string()),
                    Val::Bool(b) => ("bool", b.to_string()),
                    Val::Int(i) => ("int", i.to_string()),
                    Val::Float(f) => ("float", format!("{f}")),
                    Val::String(s) => ("string", format!("\"{s}\"")),
                    Val::DateTime(dt) => ("datetime", dt.to_rfc3339()),
                    Val::List(l) => ("list", format!("[{} items]", l.len())),
                    Val::Map(m) => ("map", format!("{{{} fields}}", m.len())),
                    Val::Blob(b) => ("blob", format!("<{} bytes>", b.len())),
                    other => ("any", format!("{other:?}")),
                };

                let type_span =
                    Span::styled(format!("[{type_name:<8}] "), theme::muted_style(theme));

                let val_span = Span::styled(value_str, theme::foreground_style(theme));

                lines.push(Line::from(vec![key_span, type_span, val_span]));

                // If nested Map or List, indent child items
                if let Val::Map(nested_map) = v {
                    for (nk, nv) in nested_map {
                        lines.push(Line::from(vec![
                            Span::raw("    "),
                            Span::styled(format!("{}: ", nk.as_str()), theme::title_style(theme)),
                            Span::styled(nv.to_text(), theme::muted_style(theme)),
                        ]));
                    }
                } else if let Val::List(nested_list) = v {
                    for (idx, item) in nested_list.iter().take(10).enumerate() {
                        lines.push(Line::from(vec![
                            Span::raw("    "),
                            Span::styled(format!("[{idx}]: "), theme::title_style(theme)),
                            Span::styled(item.to_text(), theme::muted_style(theme)),
                        ]));
                    }
                    if nested_list.len() > 10 {
                        lines.push(Line::from(vec![
                            Span::raw("    "),
                            Span::styled(
                                format!("... and {} more items", nested_list.len() - 10),
                                theme::muted_style(theme),
                            ),
                        ]));
                    }
                }
            }
        }
        other => {
            lines.push(Line::from(Span::styled(
                other.to_text(),
                theme::foreground_style(theme),
            )));
        }
    }

    lines
}

fn export_to_json(items: &[&Val], path: &str) -> Result<usize, String> {
    use std::io::Write;
    let file = std::fs::File::create(path).map_err(|e| format!("failed to create file: {e}"))?;
    let mut writer = std::io::BufWriter::new(file);

    let mut json_items = Vec::new();
    for item in items {
        if let Ok(json_val) = serde_json::to_value(item) {
            json_items.push(json_val);
        }
    }

    let json_text = serde_json::to_string_pretty(&json_items)
        .map_err(|e| format!("serialization error: {e}"))?;

    writer
        .write_all(json_text.as_bytes())
        .map_err(|e| format!("write error: {e}"))?;

    Ok(json_items.len())
}
