// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Shared SGR/CSI → ratatui style translation.
//!
//! Long-term: single place for ANSI→ratatui. Both prompt rendering
//! (`prompt.rs:ansi_to_spans`) and syntax highlighting used to duplicate
//! this logic with subtly different edge cases.
//!
//! This module owns:
//! - `nu_ansi_term::Style` → `ratatui::style::Style` (`convert_ansi_style`)
//! - Full ANSI SGR string → `Vec<Span<'static>>` (`ansi_to_spans`)

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

/// Convert a `nu_ansi_term` style (as produced by `highlighter::highlight`) to
/// a ratatui style. Used on the ftui hot path every frame.
pub fn convert_ansi_style(ns: nu_ansi_term::Style) -> Style {
    let mut style = Style::default();
    if let Some(c) = ns.foreground.and_then(convert_ansi_color) {
        style = style.fg(c);
    }
    if let Some(c) = ns.background.and_then(convert_ansi_color) {
        style = style.bg(c);
    }
    if ns.is_bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if ns.is_italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if ns.is_underline {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style
}

fn convert_ansi_color(nc: nu_ansi_term::Color) -> Option<Color> {
    match nc {
        nu_ansi_term::Color::Black => Some(Color::Black),
        nu_ansi_term::Color::Red => Some(Color::Red),
        nu_ansi_term::Color::Green => Some(Color::Green),
        nu_ansi_term::Color::Yellow => Some(Color::Yellow),
        nu_ansi_term::Color::Blue => Some(Color::Blue),
        nu_ansi_term::Color::Magenta => Some(Color::Magenta),
        nu_ansi_term::Color::Cyan => Some(Color::Cyan),
        nu_ansi_term::Color::White => Some(Color::White),
        nu_ansi_term::Color::DarkGray => Some(Color::DarkGray),
        nu_ansi_term::Color::LightRed => Some(Color::LightRed),
        nu_ansi_term::Color::LightGreen => Some(Color::LightGreen),
        nu_ansi_term::Color::LightYellow => Some(Color::LightYellow),
        nu_ansi_term::Color::LightBlue => Some(Color::LightBlue),
        nu_ansi_term::Color::LightMagenta => Some(Color::LightMagenta),
        nu_ansi_term::Color::LightCyan => Some(Color::LightCyan),
        nu_ansi_term::Color::LightGray => Some(Color::Gray),
        nu_ansi_term::Color::Fixed(n) => Some(Color::Indexed(n)),
        nu_ansi_term::Color::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
        _ => None,
    }
}

/// Parse an ANSI/SGR-escaped string into ratatui `Span`s.
///
/// Handles standard 3/4-bit (30-37, 40-47, 90-97, 100-107), 256-color
/// (`38;5;n`/`48;5;n`), 24-bit (`38;2;r;g;b`/`48;2;r;g;b`), bold/italic/
/// underline/reverse and reset. Non-SGR CSI (cursor, clear, etc.) stripped.
pub fn ansi_to_spans(s: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut current_style = Style::default();
    let mut text_buf = String::new();
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            if !text_buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut text_buf), current_style));
            }
            chars.next(); // '['
            let mut params = String::new();
            let mut command = None;
            while let Some(&c) = chars.peek() {
                match c {
                    '0'..='9' | ';' | '?' | '>' | '<' | '=' | ' ' | '$' | '"' | '\'' | '!' => {
                        params.push(c);
                        chars.next();
                    }
                    ' '..='/' => {
                        params.push(c);
                        chars.next();
                    }
                    '@'..='~' => {
                        command = Some(c);
                        chars.next();
                        break;
                    }
                    _ => break,
                }
            }
            if command == Some('m') {
                apply_sgr_codes(&params, &mut current_style);
            }
        } else {
            text_buf.push(ch);
        }
    }
    if !text_buf.is_empty() {
        spans.push(Span::styled(text_buf, current_style));
    }
    if current_style != Style::default() {
        spans.push(Span::styled(String::new(), Style::default()));
    }
    spans
}

fn apply_sgr_codes(params: &str, current_style: &mut Style) {
    if params.is_empty() {
        *current_style = Style::default();
        return;
    }
    let codes: Vec<&str> = params.split(';').collect();
    let mut i = 0;
    while i < codes.len() {
        match codes[i] {
            "" | "0" => *current_style = Style::default(),
            "1" => *current_style = current_style.add_modifier(Modifier::BOLD),
            "3" => *current_style = current_style.add_modifier(Modifier::ITALIC),
            "4" => *current_style = current_style.add_modifier(Modifier::UNDERLINED),
            "7" => *current_style = current_style.add_modifier(Modifier::REVERSED),
            "38" => {
                if i + 1 < codes.len() {
                    match codes[i + 1] {
                        "5" if i + 2 < codes.len() => {
                            if let Ok(n) = codes[i + 2].parse::<u8>() {
                                *current_style = current_style.fg(Color::Indexed(n));
                            }
                            i += 2;
                        }
                        "2" if i + 4 < codes.len() => {
                            let (r, g, b) = (
                                codes[i + 2].parse::<u8>().ok(),
                                codes[i + 3].parse::<u8>().ok(),
                                codes[i + 4].parse::<u8>().ok(),
                            );
                            if let (Some(r), Some(g), Some(b)) = (r, g, b) {
                                *current_style = current_style.fg(Color::Rgb(r, g, b));
                            }
                            i += 4;
                        }
                        _ => {}
                    }
                }
            }
            "48" => {
                if i + 1 < codes.len() {
                    match codes[i + 1] {
                        "5" if i + 2 < codes.len() => {
                            if let Ok(n) = codes[i + 2].parse::<u8>() {
                                *current_style = current_style.bg(Color::Indexed(n));
                            }
                            i += 2;
                        }
                        "2" if i + 4 < codes.len() => {
                            let (r, g, b) = (
                                codes[i + 2].parse::<u8>().ok(),
                                codes[i + 3].parse::<u8>().ok(),
                                codes[i + 4].parse::<u8>().ok(),
                            );
                            if let (Some(r), Some(g), Some(b)) = (r, g, b) {
                                *current_style = current_style.bg(Color::Rgb(r, g, b));
                            }
                            i += 4;
                        }
                        _ => {}
                    }
                }
            }
            "30" => *current_style = current_style.fg(Color::Black),
            "31" => *current_style = current_style.fg(Color::Red),
            "32" => *current_style = current_style.fg(Color::Green),
            "33" => *current_style = current_style.fg(Color::Yellow),
            "34" => *current_style = current_style.fg(Color::Blue),
            "35" => *current_style = current_style.fg(Color::Magenta),
            "36" => *current_style = current_style.fg(Color::Cyan),
            "37" => *current_style = current_style.fg(Color::White),
            "40" => *current_style = current_style.bg(Color::Black),
            "41" => *current_style = current_style.bg(Color::Red),
            "42" => *current_style = current_style.bg(Color::Green),
            "43" => *current_style = current_style.bg(Color::Yellow),
            "44" => *current_style = current_style.bg(Color::Blue),
            "45" => *current_style = current_style.bg(Color::Magenta),
            "46" => *current_style = current_style.bg(Color::Cyan),
            "47" => *current_style = current_style.bg(Color::White),
            "90" => *current_style = current_style.fg(Color::DarkGray),
            "91" => *current_style = current_style.fg(Color::LightRed),
            "92" => *current_style = current_style.fg(Color::LightGreen),
            "93" => *current_style = current_style.fg(Color::LightYellow),
            "94" => *current_style = current_style.fg(Color::LightBlue),
            "95" => *current_style = current_style.fg(Color::LightMagenta),
            "96" => *current_style = current_style.fg(Color::LightCyan),
            "97" => *current_style = current_style.fg(Color::White),
            "100" => *current_style = current_style.bg(Color::DarkGray),
            "101" => *current_style = current_style.bg(Color::LightRed),
            "102" => *current_style = current_style.bg(Color::LightGreen),
            "103" => *current_style = current_style.bg(Color::LightYellow),
            "104" => *current_style = current_style.bg(Color::LightBlue),
            "105" => *current_style = current_style.bg(Color::LightMagenta),
            "106" => *current_style = current_style.bg(Color::LightCyan),
            "107" => *current_style = current_style.bg(Color::White),
            "39" => *current_style = current_style.fg(Color::Reset),
            "49" => *current_style = current_style.bg(Color::Reset),
            _ => {}
        }
        i += 1;
    }
}

/// Strip ANSI escape sequences from a string.
pub fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b'
            && let Some('[') = chars.peek()
        {
            let _ = chars.next(); // consume '['
            for nc in chars.by_ref() {
                if nc.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        result.push(c);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn simple() {
        let s = ansi_to_spans("hello");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].content, "hello");
    }
    #[test]
    fn colored() {
        let s = ansi_to_spans("\x1b[31mred\x1b[0mnormal");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].content, "red");
        assert_eq!(s[1].content, "normal");
    }
    #[test]
    fn skips_non_sgr() {
        let s = ansi_to_spans("before\x1b[2Kafter");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].content, "before");
        assert_eq!(s[1].content, "after");
    }
}
