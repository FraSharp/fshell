// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::terminal_mode::FullscreenTerminalGuard;
use chrono::{Local, Utc};
use fshell_core::Val;
use fshell_terminal::input::{UnixEventSource, InputEvent, InputPoll, Key};
use std::fmt::Write;
use std::io::IsTerminal;
use std::sync::Arc;
use unicode_width::UnicodeWidthStr;
use ustr::ustr;

fn apply_horizontal_offset(
    spans: Vec<ratatui::text::Span<'static>>,
    offset_x: usize,
) -> Vec<ratatui::text::Span<'static>> {
    if offset_x == 0 {
        return spans;
    }
    let mut skipped = 0;
    let mut result = Vec::new();
    for span in spans {
        let char_count = span.content.chars().count();
        if skipped + char_count <= offset_x {
            skipped += char_count;
            continue;
        }
        let to_skip = offset_x.saturating_sub(skipped);
        let remaining: String = span.content.chars().skip(to_skip).collect();
        skipped += char_count;
        result.push(ratatui::text::Span::styled(remaining, span.style));
    }
    result
}

pub fn show_text_pager(text: &str) {
    show_text_pager_with_theme(text, &fshell_core::theme::Theme::default_theme());
}

pub fn show_text_pager_with_theme(text: &str, theme: &fshell_core::theme::Theme) {
    let raw_lines: Vec<&str> = text.lines().collect();
    if raw_lines.is_empty() {
        return;
    }

    if fshell_engine::is_test_mode()
        || !std::io::stdout().is_terminal()
        || !std::io::stdin().is_terminal()
    {
        print!("{}", text);
        return;
    }

    let _guard = match FullscreenTerminalGuard::enter(false) {
        Ok(g) => g,
        Err(_) => {
            print!("{}", text);
            return;
        }
    };

    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal = match ratatui::Terminal::new(backend) {
        Ok(t) => t,
        Err(_) => {
            print!("{}", text);
            return;
        }
    };
    let _ = terminal.clear();

    let mut input = UnixEventSource::new();
    let mut offset_y: usize = 0;
    let mut offset_x: usize = 0;
    let mut show_line_numbers = false;
    let mut search_bar = crate::tui::components::SearchBarState::new();
    let mut searching = false;
    let mut matches: Vec<usize> = Vec::new();
    let mut match_idx: usize = 0;

    let line_count = raw_lines.len();

    loop {
        let mut visible_height = 20usize;

        let draw_res = terminal.draw(|frame| {
            let size = frame.area();
            if size.width < 10 || size.height < 3 {
                return;
            }

            let constraints = if searching {
                vec![
                    ratatui::layout::Constraint::Min(2),
                    ratatui::layout::Constraint::Length(1),
                    ratatui::layout::Constraint::Length(1),
                ]
            } else {
                vec![
                    ratatui::layout::Constraint::Min(2),
                    ratatui::layout::Constraint::Length(1),
                ]
            };

            let chunks = ratatui::layout::Layout::default()
                .direction(ratatui::layout::Direction::Vertical)
                .constraints(constraints)
                .split(size);

            let content_area = chunks[0];
            visible_height = content_area.height as usize;

            if offset_y + visible_height > line_count {
                offset_y = line_count.saturating_sub(visible_height);
            }

            let max_digits = format!("{}", line_count).len();
            let mut formatted_lines = Vec::new();

            for (idx_rel, line_str) in raw_lines
                .iter()
                .skip(offset_y)
                .take(visible_height)
                .enumerate()
            {
                let line_idx = offset_y + idx_rel;
                let mut line_spans = Vec::new();

                if show_line_numbers {
                    let num_str = format!("{:>width$} │ ", line_idx + 1, width = max_digits);
                    line_spans.push(ratatui::text::Span::styled(
                        num_str,
                        crate::tui::theme::muted_style(theme),
                    ));
                }

                let line_text = if !search_bar.query.is_empty() {
                    highlight_match(line_str, &search_bar.query)
                } else {
                    line_str.to_string()
                };

                let content_spans = crate::ftui::ansi::ansi_to_spans(&line_text);
                let trimmed_spans = apply_horizontal_offset(content_spans, offset_x);
                line_spans.extend(trimmed_spans);

                formatted_lines.push(ratatui::text::Line::from(line_spans));
            }

            frame.render_widget(
                ratatui::widgets::Paragraph::new(formatted_lines),
                content_area,
            );

            let mut scroll_state = crate::tui::components::ScrollState::new();
            scroll_state.update(line_count, visible_height);
            scroll_state.offset = offset_y;
            scroll_state.render_scrollbar(content_area, frame.buffer_mut(), theme);

            let footer_chunk = if searching {
                search_bar.render(
                    chunks[1],
                    frame.buffer_mut(),
                    theme,
                    " / ",
                    "Type to search...",
                    true,
                );
                chunks[2]
            } else {
                chunks[1]
            };

            let pct = if line_count > visible_height {
                (offset_y * 100 / line_count.saturating_sub(visible_height)).min(100)
            } else {
                100
            };
            let status_text = format!(
                "{:>3}% │ {}:{} (col {})",
                pct,
                offset_y + 1,
                line_count,
                offset_x + 1
            );

            let hints: &[crate::tui::components::KeyHint] = if searching {
                &[
                    crate::tui::components::KeyHint::new("Enter", "Done"),
                    crate::tui::components::KeyHint::new("Esc", "Cancel"),
                ]
            } else {
                &[
                    crate::tui::components::KeyHint::new("/", "Search"),
                    crate::tui::components::KeyHint::new("n/N", "Next/Prev"),
                    crate::tui::components::KeyHint::new("h/l", "Pan"),
                    crate::tui::components::KeyHint::new("#", "Numbers"),
                    crate::tui::components::KeyHint::new("g/G", "Top/Bot"),
                    crate::tui::components::KeyHint::new("q", "Quit"),
                ]
            };

            let status_span = if searching {
                if !search_bar.query.is_empty() {
                    if !matches.is_empty() {
                        ratatui::text::Span::styled(
                            format!(" Match {}/{} ", match_idx + 1, matches.len()),
                            crate::tui::theme::status_ok_style(theme),
                        )
                    } else {
                        ratatui::text::Span::styled(
                            " No matches ",
                            crate::tui::theme::status_error_style(theme),
                        )
                    }
                } else {
                    ratatui::text::Span::styled(
                        " Search Mode ",
                        crate::tui::theme::title_style(theme),
                    )
                }
            } else {
                ratatui::text::Span::styled(status_text, crate::tui::theme::title_style(theme))
            };

            crate::tui::components::StatusFooter::new(theme, hints)
                .with_status(status_span)
                .render(footer_chunk, frame.buffer_mut());
        });

        if draw_res.is_err() {
            break;
        }

        let key = match input.poll(std::time::Duration::from_millis(100)) {
            Ok(InputPoll::Event(InputEvent::Key(key))) => key,
            Ok(InputPoll::Event(_) | InputPoll::Timeout) => continue,
            Ok(InputPoll::Closed) | Err(_) => break,
        };

        if searching {
            match key.key {
                Key::Enter => {
                    searching = false;
                }
                Key::Escape => {
                    searching = false;
                    search_bar.clear();
                    matches.clear();
                }
                _ => {
                    if search_bar.handle_key(&key) {
                        let q = search_bar.query.to_lowercase();
                        matches = raw_lines
                            .iter()
                            .enumerate()
                            .filter(|(_, l)| {
                                crate::ftui::ansi::strip_ansi_codes(l)
                                    .to_lowercase()
                                    .contains(&q)
                            })
                            .map(|(i, _)| i)
                            .collect();
                        if !matches.is_empty() {
                            match_idx = 0;
                            offset_y = matches[0].saturating_sub(visible_height / 3);
                        }
                    }
                }
            }
        } else {
            match key.key {
                Key::Character('q') | Key::Escape => break,
                Key::Character('c')
                    if key
                        .modifiers
                        .contains(fshell_terminal::input::Modifiers::CONTROL) =>
                {
                    break;
                }
                Key::Up | Key::Character('k') => {
                    offset_y = offset_y.saturating_sub(1);
                }
                Key::Down | Key::Character('j') => {
                    if offset_y + 1 < line_count {
                        offset_y += 1;
                    }
                }
                Key::Left | Key::Character('h') => {
                    offset_x = offset_x.saturating_sub(4);
                }
                Key::Right | Key::Character('l') => {
                    offset_x = offset_x.saturating_add(4);
                }
                Key::PageUp | Key::Character('u')
                    if key
                        .modifiers
                        .contains(fshell_terminal::input::Modifiers::CONTROL) =>
                {
                    offset_y = offset_y.saturating_sub(visible_height);
                }
                Key::PageUp => {
                    offset_y = offset_y.saturating_sub(visible_height);
                }
                Key::PageDown | Key::Character('d')
                    if key
                        .modifiers
                        .contains(fshell_terminal::input::Modifiers::CONTROL) =>
                {
                    offset_y = (offset_y + visible_height).min(line_count.saturating_sub(1));
                }
                Key::PageDown => {
                    offset_y = (offset_y + visible_height).min(line_count.saturating_sub(1));
                }
                Key::Home | Key::Character('g') => {
                    offset_y = 0;
                }
                Key::End | Key::Character('G') => {
                    offset_y = line_count.saturating_sub(visible_height);
                }
                Key::Character('#') => {
                    show_line_numbers = !show_line_numbers;
                }
                Key::Character('/') => {
                    searching = true;
                }
                Key::Character('n') if !matches.is_empty() => {
                    match_idx = (match_idx + 1) % matches.len();
                    offset_y = matches[match_idx].saturating_sub(visible_height / 3);
                }
                Key::Character('N') if !matches.is_empty() => {
                    match_idx = if match_idx == 0 {
                        matches.len() - 1
                    } else {
                        match_idx - 1
                    };
                    offset_y = matches[match_idx].saturating_sub(visible_height / 3);
                }
                _ => {}
            }
        }
    }
}

/// Print dynamic fshell values beautifully in tabular format for maps.
pub fn print_compact_names(list: &[Val], theme: &fshell_core::theme::Theme) {
    struct Entry<'a> {
        name: &'a str,
        is_dir: bool,
        is_exec: bool,
        is_link: bool,
    }

    let entries: Vec<Entry> = list
        .iter()
        .filter_map(|v| match v {
            Val::Map(map) => {
                let name = match map.get(&ustr("name")) {
                    Some(Val::String(s)) => s.as_str(),
                    _ => return None,
                };
                let is_dir = match map.get(&ustr("type")) {
                    Some(Val::String(t)) => t == "dir",
                    _ => false,
                };
                let is_exec = match map.get(&ustr("is_executable")) {
                    Some(Val::Bool(b)) => *b,
                    _ => false,
                };
                let is_link = match map.get(&ustr("is_symlink")) {
                    Some(Val::Bool(b)) => *b,
                    _ => false,
                };
                Some(Entry {
                    name,
                    is_dir,
                    is_exec,
                    is_link,
                })
            }
            _ => None,
        })
        .collect();

    if entries.is_empty() {
        return;
    }

    let (term_width, term_height) = crossterm::terminal::size().unwrap_or((80, 24));
    let max_len = entries
        .iter()
        .map(|e| UnicodeWidthStr::width(crate::ftui::ansi::strip_ansi_codes(e.name).as_str()))
        .max()
        .unwrap_or(10);
    let col_width = max_len + 2;
    let cols = std::cmp::max(1, term_width as usize / col_width);
    let rows = entries.len().div_ceil(cols);

    use crate::theme_ext::ThemeColorNu;
    let dir_style = theme.completions.header_directory.to_style_bold();
    let link_style = theme.completions.header_flag.to_style_bold();
    let exec_style = theme.completions.header_command.to_style_bold();
    let normal_style = nu_ansi_term::Style::default();

    let mut out = String::new();
    for r in 0..rows {
        for c in 0..cols {
            let idx = c * rows + r;
            if idx < entries.len() {
                let entry = &entries[idx];
                let display_str = format!("{:<width$}", entry.name, width = col_width);
                if entry.is_dir {
                    let _ = write!(out, "{}", dir_style.paint(&display_str));
                } else if entry.is_link {
                    let _ = write!(out, "{}", link_style.paint(&display_str));
                } else if entry.is_exec {
                    let _ = write!(out, "{}", exec_style.paint(&display_str));
                } else {
                    let _ = write!(out, "{}", normal_style.paint(&display_str));
                }
            }
        }
        out.push('\n');
    }

    let is_terminal = std::io::stdout().is_terminal() && std::io::stdin().is_terminal();
    let needs_pager = is_terminal && out.lines().count() >= term_height.saturating_sub(4) as usize;
    if needs_pager {
        show_text_pager_with_theme(&out, theme);
    } else {
        print!("{}", out);
    }
}

pub fn print_value_beautifully(val: &Val, theme: &fshell_core::theme::Theme) {
    let (_, term_height) = crossterm::terminal::size().unwrap_or((80, 24));
    let is_terminal = std::io::stdout().is_terminal() && std::io::stdin().is_terminal();

    let needs_pager = is_terminal
        && match val {
            Val::List(list) => list.len() >= term_height.saturating_sub(4) as usize,
            Val::Map(map) => map.len() >= term_height.saturating_sub(4) as usize,
            _ => false,
        };

    let text = render_val_to_string(val, theme);

    if needs_pager {
        show_text_pager_with_theme(&text, theme);
    } else {
        print!("{}", text);
    }
}

/// Render a value without blocking the async REPL runtime while a pager or
/// other fullscreen formatter waits for terminal input.
pub async fn print_value_beautifully_async(
    val: Val,
    theme: Arc<fshell_core::theme::Theme>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || print_value_beautifully(&val, &theme))
        .await
        .map_err(|error| format!("value formatter task failed: {error}"))
}

fn render_table(list: &[Val], out: &mut String, theme: &fshell_core::theme::Theme) {
    let mut keys = Vec::new();
    let mut keys_seen = std::collections::HashSet::new();
    for item in list {
        if let Val::Map(map) = item {
            for (k, _) in map {
                if keys_seen.insert(k.as_str()) {
                    keys.push(k.as_str());
                }
            }
        }
    }

    let mut widths = std::collections::HashMap::new();
    let mut right_align = std::collections::HashSet::new();
    for k in &keys {
        widths.insert(
            *k,
            UnicodeWidthStr::width(crate::ftui::ansi::strip_ansi_codes(k).as_str()),
        );
    }
    let mut formatted_rows: Vec<Vec<(String, bool)>> = Vec::with_capacity(list.len());
    for item in list {
        if let Val::Map(map) = item {
            let mut row_cells = Vec::with_capacity(keys.len());
            for k in &keys {
                let cell = format_table_cell(
                    k,
                    match map.get(&ustr::ustr(k)) {
                        Some(v) => v,
                        None => &Val::Null,
                    },
                    theme,
                );
                let cell_w =
                    UnicodeWidthStr::width(crate::ftui::ansi::strip_ansi_codes(&cell).as_str());
                let entry = widths.entry(*k).or_insert(0);
                if cell_w > *entry {
                    *entry = cell_w;
                }
                let stripped = crate::ftui::ansi::strip_ansi_codes(&cell);
                let is_numeric = !stripped.is_empty()
                    && stripped
                        .chars()
                        .all(|c| c.is_ascii_digit() || c == '.' || c == '-');
                if is_numeric {
                    right_align.insert(*k);
                } else {
                    right_align.remove(k);
                }
                row_cells.push((cell, is_numeric));
            }
            formatted_rows.push(row_cells);
        }
    }

    use crate::theme_ext::ThemeColorNu;
    let header_style = theme.widgets.title.to_style_bold().underline();
    for k in &keys {
        let width = widths.get(k).unwrap_or(&10);
        let mut cell = k.to_string();
        let pad = width.saturating_sub(cell.len());
        for _ in 0..pad {
            cell.push(' ');
        }
        out.push_str(&header_style.paint(&cell).to_string());
        out.push_str("   ");
    }
    out.push('\n');

    for row_cells in &formatted_rows {
        for (i, (cell, _)) in row_cells.iter().enumerate() {
            let k = keys[i];
            let width = widths.get(k).unwrap_or(&10);
            let is_right = right_align.contains(k);
            let cell_w = UnicodeWidthStr::width(crate::ftui::ansi::strip_ansi_codes(cell).as_str());
            if is_right {
                let pad = width.saturating_sub(cell_w);
                for _ in 0..pad {
                    out.push(' ');
                }
                out.push_str(cell);
            } else {
                out.push_str(cell);
                let pad = width.saturating_sub(cell_w);
                for _ in 0..pad {
                    out.push(' ');
                }
            }
            out.push_str("   ");
        }
        out.push('\n');
    }
}

fn render_val_to_string(val: &Val, theme: &fshell_core::theme::Theme) -> String {
    let mut out = String::new();
    use crate::theme_ext::ThemeColorNu;
    match val {
        Val::Null => {
            let style = theme.status.muted.to_style();
            let _ = writeln!(out, "{}", style.paint("null"));
        }
        Val::Bool(b) => {
            let style = theme.syntax.keyword.to_style_bold();
            let _ = writeln!(out, "{}", style.paint(b.to_string()));
        }
        Val::Int(i) => {
            let style = theme.syntax.number.to_style();
            let _ = writeln!(out, "{}", style.paint(i.to_string()));
        }
        Val::Float(f) => {
            let style = theme.syntax.number.to_style();
            let _ = writeln!(out, "{}", style.paint(f.to_string()));
        }
        Val::String(s) => {
            if s.ends_with('\0') {
                let _ = write!(out, "{}", &s[..s.len() - 1]);
            } else {
                let _ = writeln!(out, "{}", s);
            }
        }
        Val::DateTime(dt) => {
            let style = theme.syntax.type_name.to_style_bold();
            let _ = writeln!(out, "{}", style.paint(dt.to_rfc3339()));
        }
        Val::List(list) => {
            if list.is_empty() {
                out.push_str("[]\n");
                return out;
            }
            if list.iter().all(|item| matches!(item, Val::Map(_))) {
                render_table(list, &mut out, theme);
            } else {
                for item in list {
                    out.push_str(&render_val_to_string(item, theme));
                }
            }
        }
        Val::Map(map) => {
            let key_style = theme.syntax.builtin.to_style_bold();
            for (k, v) in map {
                let _ = writeln!(
                    out,
                    "{}: {}",
                    key_style.paint(k.as_str()),
                    format_val_compact(v, theme)
                );
            }
        }
        Val::Blob(b) => {
            let s = String::from_utf8_lossy(b);
            let _ = writeln!(out, "{s}");
        }
        other => {
            let _ = writeln!(out, "{:?}", other);
        }
    }
    out
}

fn format_table_cell(key: &str, val: &Val, theme: &fshell_core::theme::Theme) -> String {
    use crate::theme_ext::ThemeColorNu;
    match (key, val) {
        ("size", Val::Int(n)) => {
            let style = theme.syntax.number.to_style();
            style.paint(format_size_human(*n as u64)).to_string()
        }
        ("last_modified", Val::DateTime(dt)) => {
            let style = theme.status.muted.to_style();
            style.paint(format_datetime_ls_style(dt)).to_string()
        }
        _ => format_val_compact(val, theme),
    }
}

fn format_size_human(size: u64) -> String {
    const UNITS: &[&str] = &["B", "K", "M", "G", "T", "P"];
    let mut s = size as f64;
    let mut unit_idx = 0;
    while s >= 1024.0 && unit_idx < UNITS.len() - 1 {
        s /= 1024.0;
        unit_idx += 1;
    }
    if unit_idx == 0 {
        format!("{}{}", s as u64, UNITS[unit_idx])
    } else {
        format!("{:.1}{}", s, UNITS[unit_idx])
    }
}

fn format_datetime_ls_style(dt: &chrono::DateTime<Utc>) -> String {
    let six_months_secs: i64 = 6 * 30 * 24 * 60 * 60;
    let is_recent = (dt.timestamp() - Utc::now().timestamp()).abs() < six_months_secs;
    let local = dt.with_timezone(&Local);
    if is_recent {
        local.format("%b %e %H:%M").to_string()
    } else {
        local.format("%b %e  %Y").to_string()
    }
}

pub fn format_val_compact(val: &Val, theme: &fshell_core::theme::Theme) -> String {
    use crate::theme_ext::ThemeColorNu;
    match val {
        Val::Null => {
            let style = theme.status.muted.to_style();
            style.paint("null").to_string()
        }
        Val::Bool(b) => {
            let style = theme.syntax.keyword.to_style_bold();
            style.paint(b.to_string()).to_string()
        }
        Val::Int(i) => {
            let style = theme.syntax.number.to_style();
            style.paint(i.to_string()).to_string()
        }
        Val::Float(f) => {
            let style = theme.syntax.number.to_style();
            style.paint(f.to_string()).to_string()
        }
        Val::String(s) => {
            let style = theme.syntax.string.to_style();
            style.paint(s).to_string()
        }
        Val::DateTime(dt) => {
            let style = theme.syntax.type_name.to_style_bold();
            style.paint(dt.to_rfc3339()).to_string()
        }
        Val::List(l) => {
            let style = theme.status.muted.to_style();
            style.paint(format!("[{} items]", l.len())).to_string()
        }
        Val::Map(m) => {
            let style = theme.status.muted.to_style();
            style.paint(format!("{{{} fields}}", m.len())).to_string()
        }
        Val::Blob(b) => {
            let style = theme.syntax.string.to_style();
            style.paint(String::from_utf8_lossy(b).as_ref()).to_string()
        }
        other => format!("{:?}", other),
    }
}

pub fn print_item_streaming(val: &Val, theme: &fshell_core::theme::Theme) {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    use crate::theme_ext::ThemeColorNu;
    match val {
        Val::Map(map) => {
            let key_style = theme.syntax.builtin.to_style_bold();
            let mut parts = Vec::new();
            for (k, v) in map {
                parts.push(format!(
                    "{}: {}",
                    key_style.paint(k.as_str()),
                    format_val_compact(v, theme)
                ));
            }
            let _ = writeln!(handle, "{{ {} }}", parts.join(", "));
        }
        Val::Blob(b) => {
            let _ = handle.write_all(b);
            let _ = handle.flush();
        }
        Val::String(s) if s.starts_with('\0') => {}
        other => {
            drop(handle);
            print_value_beautifully(other, theme);
        }
    }
}

/// Async boundary for streaming output. The synchronous formatter may enter
/// the fullscreen pager, so it must not run on the REPL runtime thread.
pub async fn print_item_streaming_async(
    val: Val,
    theme: Arc<fshell_core::theme::Theme>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || print_item_streaming(&val, &theme))
        .await
        .map_err(|error| format!("streaming formatter task failed: {error}"))
}

fn highlight_match(line: &str, query: &str) -> String {
    if query.is_empty() {
        return line.to_string();
    }

    let plain = crate::ftui::ansi::strip_ansi_codes(line);
    let q_lower = query.to_lowercase();
    let p_lower = plain.to_lowercase();

    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    let mut start = 0;
    while let Some(pos) = p_lower[start..].find(&q_lower) {
        let abs_start = start + pos;
        let abs_end = abs_start + q_lower.len();
        ranges.push(abs_start..abs_end);
        start = abs_end;
    }

    if ranges.is_empty() {
        return line.to_string();
    }

    let mut result = String::new();
    let mut plain_byte = 0;
    let mut in_hl = false;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            result.push(c);
            if let Some(&'[') = chars.peek() {
                result.push(chars.next().expect("peek confirmed '[' present"));
                for nc in chars.by_ref() {
                    result.push(nc);
                    if nc.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }

        let c_len = c.len_utf8();
        let is_match = ranges
            .iter()
            .any(|r| r.start <= plain_byte && plain_byte < r.end);

        if is_match && !in_hl {
            result.push_str("\x1b[7m");
            in_hl = true;
        } else if !is_match && in_hl {
            result.push_str("\x1b[27m");
            in_hl = false;
        }

        result.push(c);
        plain_byte += c_len;
    }

    if in_hl {
        result.push_str("\x1b[27m");
    }

    result
}
