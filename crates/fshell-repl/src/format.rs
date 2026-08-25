// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use chrono::{Local, Utc};
use fshell_core::Val;
use std::fmt::Write;
use std::io::IsTerminal;
use unicode_width::UnicodeWidthStr;
use ustr::ustr;

struct PagerGuard;

impl PagerGuard {
    fn new() -> Result<Self, String> {
        use crossterm::terminal::enable_raw_mode;
        enable_raw_mode().map_err(|e| e.to_string())?;
        let mut stdout = std::io::stdout();
        use crossterm::execute;
        use crossterm::terminal::EnterAlternateScreen;
        execute!(stdout, EnterAlternateScreen).map_err(|e| e.to_string())?;
        Ok(PagerGuard)
    }
}

impl Drop for PagerGuard {
    fn drop(&mut self) {
        use crossterm::execute;
        use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

fn show_text_pager(text: &str) {
    use crossterm::event::{self, Event, KeyCode};
    use std::io::Write;

    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return;
    }

    if fshell_engine::is_test_mode()
        || !std::io::stdout().is_terminal()
        || !std::io::stdin().is_terminal()
    {
        print!("{}", text);
        return;
    }

    let _guard = match PagerGuard::new() {
        Ok(g) => g,
        Err(_) => {
            print!("{}", text);
            return;
        }
    };

    let mut stdout = std::io::stdout();

    let mut offset: usize = 0;
    let mut search_term = String::new();
    let mut searching = false;
    let mut matches: Vec<usize> = Vec::new();
    let mut match_idx: usize = 0;

    loop {
        let (_, height) = crossterm::terminal::size().unwrap_or((80, 24));
        let visible = (height as usize).saturating_sub(2); // 2 lines for status bar

        if offset + visible > lines.len() {
            offset = lines.len().saturating_sub(visible);
        }

        let mut buf = String::new();
        buf.push_str("\x1b[2J\x1b[H");

        for (i, line) in lines.iter().enumerate().skip(offset).take(visible) {
            if !matches.is_empty() && matches.contains(&i) {
                buf.push_str(&highlight_match(line, &search_term));
            } else {
                buf.push_str(line);
            }
            buf.push_str("\r\n");
        }

        // Fill remaining lines to keep status bar at bottom
        let drawn = visible.min(lines.len().saturating_sub(offset));
        for _ in drawn..visible {
            buf.push_str("\r\n");
        }

        // --- Status bar ---
        let pct = if lines.len() > visible {
            (offset * 100 / (lines.len() - visible)).min(100)
        } else {
            100
        };
        let line_info = format!("{}:{}", offset + 1, lines.len());

        if searching {
            // Search bar: inverted background
            buf.push_str("\x1b[48;5;236m\x1b[38;5;15m");
            let _ = write!(
                buf,
                " /{}{}",
                search_term,
                if search_term.is_empty() { "" } else { " " }
            );
            if !search_term.is_empty() {
                if !matches.is_empty() {
                    let _ = write!(
                        buf,
                        "\x1b[48;5;22m\x1b[38;5;82m [{}/{}] \x1b[48;5;236m\x1b[38;5;15m",
                        match_idx + 1,
                        matches.len()
                    );
                } else {
                    buf.push_str(
                        "\x1b[48;5;52m\x1b[38;5;9m (no matches) \x1b[48;5;236m\x1b[38;5;15m",
                    );
                }
            }
            // Right-align the keybindings
            let hint =
                " \x1b[38;5;245m[Enter]\x1b[38;5;15m done \x1b[38;5;245m[Esc]\x1b[38;5;15m clear ";
            buf.push_str(hint);
            buf.push_str("\x1b[0m\x1b[K\r\n");
        } else {
            // Normal status bar: styled with keybindings
            buf.push_str("\x1b[48;5;236m\x1b[38;5;15m");

            // Left: scroll percentage + line position
            let _ = write!(
                buf,
                " \x1b[38;5;82m{:>3}%\x1b[38;5;15m | {} ",
                pct, line_info
            );

            // Center/Right: keybinding hints
            let hint = " \x1b[38;5;245m[/]\x1b[38;5;15msearch \x1b[38;5;245m[n]\x1b[38;5;15mnext \x1b[38;5;245m[N]\x1b[38;5;15mprev \x1b[38;5;245m[g]\x1b[38;5;15mtop \x1b[38;5;245m[G]\x1b[38;5;15mbot \x1b[38;5;245m[q]\x1b[38;5;15mquit".to_string();
            buf.push_str(&hint);

            if !search_term.is_empty() && !matches.is_empty() {
                let _ = write!(
                    buf,
                    " \x1b[48;5;22m\x1b[38;5;82m [{}/{}] \x1b[48;5;236m",
                    match_idx + 1,
                    matches.len()
                );
            }

            buf.push_str("\x1b[0m\x1b[K\r\n");
        }

        // --- Search query line (only shown in search mode) ---
        if searching {
            buf.push_str("\x1b[48;5;236m\x1b[38;5;245m");
            buf.push_str(" Type to search, [Enter] to confirm, [Esc] to cancel");
            buf.push_str("\x1b[0m\x1b[K");
        }

        let _ = write!(stdout, "{}", buf);
        let _ = stdout.flush();

        if let Ok(true) = event::poll(std::time::Duration::from_millis(100))
            && let Ok(Event::Key(key)) = event::read()
        {
            if searching {
                match key.code {
                    KeyCode::Char(c) => {
                        search_term.push(c);
                        let q = search_term.to_lowercase();
                        matches = lines
                            .iter()
                            .enumerate()
                            .filter(|(_, l)| {
                                crate::tui::strip_ansi_codes(l).to_lowercase().contains(&q)
                            })
                            .map(|(i, _)| i)
                            .collect();
                        if !matches.is_empty() {
                            match_idx = 0;
                            offset = matches[0].saturating_sub(visible / 3);
                        }
                    }
                    KeyCode::Backspace => {
                        search_term.pop();
                        let q = search_term.to_lowercase();
                        matches = lines
                            .iter()
                            .enumerate()
                            .filter(|(_, l)| {
                                crate::tui::strip_ansi_codes(l).to_lowercase().contains(&q)
                            })
                            .map(|(i, _)| i)
                            .collect();
                        if !matches.is_empty() {
                            match_idx = 0;
                            offset = matches[0].saturating_sub(visible / 3);
                        }
                    }
                    KeyCode::Enter => {
                        searching = false;
                        if !matches.is_empty() {
                            offset = matches[0].saturating_sub(visible / 3);
                            match_idx = 0;
                        }
                    }
                    KeyCode::Esc => {
                        searching = false;
                        search_term.clear();
                        matches.clear();
                    }
                    _ => {}
                }
            } else {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Up | KeyCode::Char('k') if offset > 0 => {
                        offset = offset.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') if offset + visible < lines.len() => {
                        offset += 1;
                    }
                    KeyCode::PageUp => {
                        offset = offset.saturating_sub(visible);
                    }
                    KeyCode::PageDown => {
                        offset =
                            std::cmp::min(offset + visible, lines.len().saturating_sub(visible));
                    }
                    KeyCode::Home | KeyCode::Char('g') => {
                        offset = 0;
                    }
                    KeyCode::End | KeyCode::Char('G') => {
                        offset = lines.len().saturating_sub(visible);
                    }
                    KeyCode::Char('/') => {
                        searching = true;
                        search_term.clear();
                        matches.clear();
                    }
                    KeyCode::Char('n') if !matches.is_empty() => {
                        match_idx = (match_idx + 1) % matches.len();
                        offset = matches[match_idx].saturating_sub(visible / 3);
                    }
                    KeyCode::Char('N') if !matches.is_empty() => {
                        match_idx = if match_idx == 0 {
                            matches.len() - 1
                        } else {
                            match_idx - 1
                        };
                        offset = matches[match_idx].saturating_sub(visible / 3);
                    }
                    _ => {}
                }
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
        .map(|e| UnicodeWidthStr::width(crate::tui::strip_ansi_codes(e.name).as_str()))
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
        show_text_pager(&out);
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
        show_text_pager(&text);
    } else {
        print!("{}", text);
    }
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
            UnicodeWidthStr::width(crate::tui::strip_ansi_codes(k).as_str()),
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
                let cell_w = UnicodeWidthStr::width(crate::tui::strip_ansi_codes(&cell).as_str());
                let entry = widths.entry(*k).or_insert(0);
                if cell_w > *entry {
                    *entry = cell_w;
                }
                let stripped = crate::tui::strip_ansi_codes(&cell);
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
            let cell_w = UnicodeWidthStr::width(crate::tui::strip_ansi_codes(cell).as_str());
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

fn highlight_match(line: &str, query: &str) -> String {
    if query.is_empty() {
        return line.to_string();
    }

    let plain = crate::tui::strip_ansi_codes(line);
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
