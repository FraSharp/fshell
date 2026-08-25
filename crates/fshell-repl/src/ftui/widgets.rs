// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![allow(clippy::result_large_err)]

use super::buffer::TextBuffer;
use super::clipboard;
use super::completions::CompletionsManager;
use super::history::HistoryManager;
use fshell_engine::Env;
use fshell_engine::keybindings::KeyMapMode;

/// Result action returned by widget execution to drive the FTUI event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WidgetAction {
    Continue,
    Redraw,
    AcceptLine,
    Abort,
    Exit,
    EditInExternalEditor,
    InsertMacro(String),
}

/// Convert a crossterm KeyEvent into a fshell KeyChord.
pub fn crossterm_key_to_chord(
    key: &crossterm::event::KeyEvent,
) -> fshell_engine::keybindings::KeyChord {
    use crossterm::event::{KeyCode, KeyModifiers};
    use fshell_engine::keybindings::{KeyChord, KeyCodeDef, KeyModifiersDef};

    let modifiers = KeyModifiersDef {
        control: key.modifiers.contains(KeyModifiers::CONTROL),
        alt: key.modifiers.contains(KeyModifiers::ALT),
        shift: key.modifiers.contains(KeyModifiers::SHIFT),
    };

    let code = match key.code {
        KeyCode::Char(c) => KeyCodeDef::Char(c.to_ascii_lowercase()),
        KeyCode::Enter => KeyCodeDef::Enter,
        KeyCode::Backspace => KeyCodeDef::Backspace,
        KeyCode::Tab => KeyCodeDef::Tab,
        KeyCode::BackTab => KeyCodeDef::BackTab,
        KeyCode::Esc => KeyCodeDef::Esc,
        KeyCode::Up => KeyCodeDef::Up,
        KeyCode::Down => KeyCodeDef::Down,
        KeyCode::Left => KeyCodeDef::Left,
        KeyCode::Right => KeyCodeDef::Right,
        KeyCode::Home => KeyCodeDef::Home,
        KeyCode::End => KeyCodeDef::End,
        KeyCode::PageUp => KeyCodeDef::PageUp,
        KeyCode::PageDown => KeyCodeDef::PageDown,
        KeyCode::Delete => KeyCodeDef::Delete,
        KeyCode::Insert => KeyCodeDef::Insert,
        KeyCode::F(n) => KeyCodeDef::F(n),
        KeyCode::Null => KeyCodeDef::Null,
        _ => KeyCodeDef::Null,
    };

    KeyChord { code, modifiers }
}

/// Transactional context supplied to editor widgets.
pub struct WidgetContext<'a> {
    pub text_buf: &'a mut TextBuffer,
    pub env: &'a Env,
    pub keymap_mode: &'a mut KeyMapMode,
    pub history_mgr: &'a mut HistoryManager,
    pub comp_mgr: &'a mut CompletionsManager,
    pub widget_explorer: &'a mut super::widget_explorer::WidgetExplorerManager,
    pub current_dir: &'a str,
    pub hostname: &'a str,
    pub session_id: &'a str,
    pub help_visible: &'a mut bool,
    pub last_kill: &'a mut Option<String>,
    pub current_hint: &'a str,
    pub history_index: &'a mut Option<usize>,
    pub filtered_history: &'a mut Vec<String>,
    pub temp_input: &'a mut String,
}

/// Execute a widget by its canonical name against the supplied WidgetContext.
pub fn execute_widget(widget_name: &str, ctx: &mut WidgetContext<'_>) -> WidgetAction {
    if widget_name != "history-search-backward"
        && widget_name != "history-search-forward"
        && widget_name != "up-line-or-history"
        && widget_name != "down-line-or-history"
        && widget_name != "previous-line"
        && widget_name != "next-line"
    {
        *ctx.history_index = None;
    }

    match widget_name {
        // --- Navigation ---
        "history-search-backward" | "up-line-or-history" | "previous-line" => {
            ctx.text_buf.clear_selection();
            if ctx.text_buf.move_up() {
                ctx.comp_mgr.clear();
                return WidgetAction::Redraw;
            }
            if ctx.text_buf.text().contains('\n')
                && ctx.history_index.is_none()
                && !ctx.text_buf.is_empty()
            {
                ctx.comp_mgr.clear();
                return WidgetAction::Redraw;
            }
            if ctx.history_index.is_none() {
                if !ctx.text_buf.is_empty() {
                    ctx.text_buf.commit_transaction();
                }
                *ctx.temp_input = ctx.text_buf.text();
                let prefix = ctx.temp_input.clone();
                if prefix.is_empty() {
                    *ctx.filtered_history =
                        crate::history::query_history_prefix("", 100).unwrap_or_default();
                } else {
                    *ctx.filtered_history =
                        crate::history::query_history_prefix(&prefix, 256).unwrap_or_default();
                    if ctx.filtered_history.is_empty() {
                        *ctx.filtered_history =
                            crate::history::query_history_prefix("", 100).unwrap_or_default();
                    }
                }
            }
            if !ctx.filtered_history.is_empty() {
                let next_idx = match *ctx.history_index {
                    None => Some(0),
                    Some(idx) => {
                        if idx + 1 < ctx.filtered_history.len() {
                            Some(idx + 1)
                        } else {
                            Some(idx)
                        }
                    }
                };
                if let Some(idx) = next_idx {
                    *ctx.history_index = Some(idx);
                    ctx.text_buf.replace_content(&ctx.filtered_history[idx]);
                }
            }
            ctx.comp_mgr.clear();
            WidgetAction::Redraw
        }
        "history-search-forward" | "down-line-or-history" | "next-line" => {
            ctx.text_buf.clear_selection();
            if ctx.text_buf.move_down() {
                ctx.comp_mgr.clear();
                return WidgetAction::Redraw;
            }
            if ctx.text_buf.text().contains('\n') && ctx.history_index.is_none() {
                ctx.comp_mgr.clear();
                return WidgetAction::Redraw;
            }
            if !ctx.filtered_history.is_empty()
                && let Some(idx) = *ctx.history_index
            {
                if idx > 0 {
                    *ctx.history_index = Some(idx - 1);
                    ctx.text_buf.replace_content(&ctx.filtered_history[idx - 1]);
                } else {
                    *ctx.history_index = None;
                    ctx.text_buf.replace_content(ctx.temp_input);
                }
            }
            ctx.comp_mgr.clear();
            WidgetAction::Redraw
        }
        "beginning-of-line" => {
            ctx.text_buf.move_to_line_start();
            ctx.comp_mgr.clear();
            WidgetAction::Redraw
        }
        "end-of-line" => {
            if ctx.text_buf.cursor() == ctx.text_buf.len() && !ctx.current_hint.is_empty() {
                ctx.text_buf.clear_selection();
                ctx.text_buf.insert_str(ctx.current_hint);
            } else {
                ctx.text_buf.move_to_line_end();
            }
            ctx.comp_mgr.clear();
            WidgetAction::Redraw
        }
        "beginning-of-buffer" => {
            ctx.text_buf.move_to_start();
            ctx.comp_mgr.clear();
            WidgetAction::Redraw
        }
        "end-of-buffer" => {
            if ctx.text_buf.cursor() == ctx.text_buf.len() && !ctx.current_hint.is_empty() {
                ctx.text_buf.clear_selection();
                ctx.text_buf.insert_str(ctx.current_hint);
            } else {
                ctx.text_buf.move_to_end();
            }
            ctx.comp_mgr.clear();
            WidgetAction::Redraw
        }
        "forward-char" => {
            if ctx.text_buf.cursor() == ctx.text_buf.len() && !ctx.current_hint.is_empty() {
                ctx.text_buf.clear_selection();
                ctx.text_buf.insert_str(ctx.current_hint);
            } else {
                ctx.text_buf.move_right();
            }
            ctx.comp_mgr.clear();
            WidgetAction::Redraw
        }
        "backward-char" => {
            ctx.text_buf.move_left();
            ctx.comp_mgr.clear();
            WidgetAction::Redraw
        }
        "forward-word" => {
            if ctx.text_buf.cursor() == ctx.text_buf.len() && !ctx.current_hint.is_empty() {
                ctx.text_buf.clear_selection();
                let hint = ctx.current_hint;
                let trimmed = hint.trim_start();
                let leading_spaces = hint.len() - trimmed.len();
                let word_len = trimmed
                    .split_whitespace()
                    .next()
                    .map(|w| w.len())
                    .unwrap_or(trimmed.len());
                let accept_len = (leading_spaces + word_len).min(hint.len());
                ctx.text_buf.insert_str(&hint[..accept_len]);
            } else {
                ctx.text_buf.move_word_right();
            }
            ctx.comp_mgr.clear();
            WidgetAction::Redraw
        }
        "backward-word" => {
            ctx.text_buf.move_word_left();
            ctx.comp_mgr.clear();
            WidgetAction::Redraw
        }

        // --- Editing & Killing ---
        "delete-char" => {
            ctx.text_buf.delete_right();
            WidgetAction::Redraw
        }
        "backward-delete-char" => {
            ctx.text_buf.delete_left();
            WidgetAction::Redraw
        }
        "kill-line" => {
            let cursor = ctx.text_buf.cursor();
            let chars = ctx.text_buf.chars();
            let remainder: String = chars.iter().skip(cursor).collect();
            if let Some(newline_pos) = remainder.find('\n') {
                let killed = &remainder[..newline_pos];
                *ctx.last_kill = Some(killed.to_string());
                clipboard::copy_to_clipboard(killed);
                for _ in 0..killed.chars().count() {
                    ctx.text_buf.delete_right();
                }
            } else {
                *ctx.last_kill = Some(remainder.clone());
                clipboard::copy_to_clipboard(&remainder);
                for _ in 0..remainder.chars().count() {
                    ctx.text_buf.delete_right();
                }
            }
            WidgetAction::Redraw
        }
        "backward-kill-line" => {
            let cursor = ctx.text_buf.cursor();
            let chars = ctx.text_buf.chars();
            let before: &[char] = &chars[..cursor.min(chars.len())];
            let line_start = before
                .iter()
                .rposition(|&c| c == '\n')
                .map(|p| p + 1)
                .unwrap_or(0);
            let killed: String = before[line_start..].iter().collect();
            *ctx.last_kill = Some(killed.clone());
            clipboard::copy_to_clipboard(&killed);
            for _ in 0..killed.chars().count() {
                ctx.text_buf.delete_left();
            }
            WidgetAction::Redraw
        }
        "kill-word" => {
            let cursor = ctx.text_buf.cursor();
            let old_text = ctx.text_buf.text();
            ctx.text_buf.move_word_right();
            let new_cursor = ctx.text_buf.cursor();
            ctx.text_buf.set_cursor(cursor);
            if new_cursor > cursor {
                let killed: String = old_text
                    .chars()
                    .skip(cursor)
                    .take(new_cursor - cursor)
                    .collect();
                *ctx.last_kill = Some(killed.clone());
                clipboard::copy_to_clipboard(&killed);
                for _ in 0..(new_cursor - cursor) {
                    ctx.text_buf.delete_right();
                }
            }
            WidgetAction::Redraw
        }
        "backward-kill-word" => {
            let cursor = ctx.text_buf.cursor();
            let old_text = ctx.text_buf.text();
            ctx.text_buf.move_word_left();
            let new_cursor = ctx.text_buf.cursor();
            ctx.text_buf.set_cursor(cursor);
            if cursor > new_cursor {
                let killed: String = old_text
                    .chars()
                    .skip(new_cursor)
                    .take(cursor - new_cursor)
                    .collect();
                *ctx.last_kill = Some(killed.clone());
                clipboard::copy_to_clipboard(&killed);
                for _ in 0..(cursor - new_cursor) {
                    ctx.text_buf.delete_left();
                }
            }
            WidgetAction::Redraw
        }
        "kill-buffer" | "kill-whole-line" | "clear-line" => {
            let text = ctx.text_buf.text();
            *ctx.last_kill = Some(text.clone());
            clipboard::copy_to_clipboard(&text);
            ctx.text_buf.replace_content("");
            WidgetAction::Redraw
        }
        "capitalize-word" => {
            let cursor = ctx.text_buf.cursor();
            let old_text = ctx.text_buf.text();
            ctx.text_buf.move_word_right();
            let new_cursor = ctx.text_buf.cursor();
            ctx.text_buf.set_cursor(cursor);
            if new_cursor > cursor {
                let segment: String = old_text
                    .chars()
                    .skip(cursor)
                    .take(new_cursor - cursor)
                    .collect();
                let mut capitalized = String::new();
                let mut first = true;
                for c in segment.chars() {
                    if first && c.is_alphabetic() {
                        capitalized.extend(c.to_uppercase());
                        first = false;
                    } else {
                        capitalized.extend(c.to_lowercase());
                    }
                }
                for _ in 0..(new_cursor - cursor) {
                    ctx.text_buf.delete_right();
                }
                ctx.text_buf.insert_str(&capitalized);
            }
            WidgetAction::Redraw
        }
        "upcase-word" => {
            let cursor = ctx.text_buf.cursor();
            let old_text = ctx.text_buf.text();
            ctx.text_buf.move_word_right();
            let new_cursor = ctx.text_buf.cursor();
            ctx.text_buf.set_cursor(cursor);
            if new_cursor > cursor {
                let segment: String = old_text
                    .chars()
                    .skip(cursor)
                    .take(new_cursor - cursor)
                    .collect();
                let uppercased = segment.to_uppercase();
                for _ in 0..(new_cursor - cursor) {
                    ctx.text_buf.delete_right();
                }
                ctx.text_buf.insert_str(&uppercased);
            }
            WidgetAction::Redraw
        }
        "downcase-word" => {
            let cursor = ctx.text_buf.cursor();
            let old_text = ctx.text_buf.text();
            ctx.text_buf.move_word_right();
            let new_cursor = ctx.text_buf.cursor();
            ctx.text_buf.set_cursor(cursor);
            if new_cursor > cursor {
                let segment: String = old_text
                    .chars()
                    .skip(cursor)
                    .take(new_cursor - cursor)
                    .collect();
                let lowercased = segment.to_lowercase();
                for _ in 0..(new_cursor - cursor) {
                    ctx.text_buf.delete_right();
                }
                ctx.text_buf.insert_str(&lowercased);
            }
            WidgetAction::Redraw
        }
        "transpose-chars" => {
            let cursor = ctx.text_buf.cursor();
            let len = ctx.text_buf.len();
            if len >= 2 {
                let idx = if cursor == len {
                    cursor - 1
                } else {
                    cursor.max(1)
                };
                let chars = ctx.text_buf.chars();
                let c1 = chars[idx - 1];
                let c2 = chars[idx];
                ctx.text_buf.set_cursor(idx - 1);
                ctx.text_buf.delete_right();
                ctx.text_buf.delete_right();
                ctx.text_buf.insert_char(c2);
                ctx.text_buf.insert_char(c1);
            }
            WidgetAction::Redraw
        }
        "transpose-words" => {
            let cursor = ctx.text_buf.cursor();
            let chars = ctx.text_buf.chars();
            let mut words: Vec<(usize, usize)> = Vec::new();
            let mut in_word = false;
            let mut word_start = 0;
            for (idx, &c) in chars.iter().enumerate() {
                if !c.is_whitespace() {
                    if !in_word {
                        in_word = true;
                        word_start = idx;
                    }
                } else if in_word {
                    in_word = false;
                    words.push((word_start, idx));
                }
            }
            if in_word {
                words.push((word_start, chars.len()));
            }

            if words.len() >= 2 {
                let w2_idx = words
                    .iter()
                    .rposition(|&(start, _)| start <= cursor)
                    .unwrap_or(words.len() - 1);
                if w2_idx > 0 {
                    let (w1_start, w1_end) = words[w2_idx - 1];
                    let (w2_start, w2_end) = words[w2_idx];
                    let before: String = chars[..w1_start].iter().collect();
                    let w1: String = chars[w1_start..w1_end].iter().collect();
                    let between: String = chars[w1_end..w2_start].iter().collect();
                    let w2: String = chars[w2_start..w2_end].iter().collect();
                    let after: String = chars[w2_end..].iter().collect();

                    let mut new_text = before;
                    new_text.push_str(&w2);
                    new_text.push_str(&between);
                    new_text.push_str(&w1);
                    new_text.push_str(&after);
                    let new_cursor = w1_start
                        + w2.chars().count()
                        + between.chars().count()
                        + w1.chars().count();
                    ctx.text_buf.replace_content(&new_text);
                    ctx.text_buf.set_cursor(new_cursor);
                }
            }
            WidgetAction::Redraw
        }
        "yank" => {
            if let Some(killed) = ctx.last_kill.as_ref() {
                ctx.text_buf.insert_str(killed);
            } else if let Some(pasted) = clipboard::paste_from_clipboard() {
                ctx.text_buf.insert_str(&pasted);
            }
            WidgetAction::Redraw
        }
        "undo" => {
            ctx.text_buf.undo();
            ctx.comp_mgr.clear();
            WidgetAction::Redraw
        }
        "redo" => {
            ctx.text_buf.redo();
            ctx.comp_mgr.clear();
            WidgetAction::Redraw
        }
        "clear-screen" | "reset-prompt" => {
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
                crossterm::cursor::MoveTo(0, 0),
            );
            ctx.comp_mgr.clear();
            WidgetAction::Redraw
        }

        // --- History & Overlays ---
        "interactive-history-search" | "history-incremental-search-backward" => {
            ctx.history_mgr.active = true;
            ctx.history_mgr.explorer_active = false;
            ctx.history_mgr.aborted_active = false;
            ctx.history_mgr.query.clear();
            ctx.history_mgr.selected_idx = 0;
            ctx.history_mgr
                .update_results(ctx.current_dir, ctx.hostname, ctx.session_id);
            WidgetAction::Redraw
        }
        "history-explorer" => {
            ctx.history_mgr.active = true;
            ctx.history_mgr.explorer_active = true;
            ctx.history_mgr.aborted_active = false;
            ctx.history_mgr.query.clear();
            ctx.history_mgr.selected_idx = 0;
            ctx.history_mgr
                .update_results(ctx.current_dir, ctx.hostname, ctx.session_id);
            WidgetAction::Redraw
        }
        "aborted-history-search" => {
            ctx.history_mgr.active = true;
            ctx.history_mgr.explorer_active = false;
            ctx.history_mgr.aborted_active = true;
            ctx.history_mgr.query.clear();
            ctx.history_mgr.selected_idx = 0;
            ctx.history_mgr
                .update_results(ctx.current_dir, ctx.hostname, ctx.session_id);
            WidgetAction::Redraw
        }
        "toggle-help" => {
            if ctx.widget_explorer.active {
                ctx.widget_explorer.close();
            } else {
                ctx.widget_explorer.open(ctx.env);
            }
            WidgetAction::Redraw
        }

        // --- Line Execution & Control ---
        "accept-line" => WidgetAction::AcceptLine,
        "newline-and-indent" => {
            let indent = fshell_core::compute_indent_depth(&ctx.text_buf.text());
            ctx.text_buf.insert_char('\n');
            for _ in 0..(indent * 4) {
                ctx.text_buf.insert_char(' ');
            }
            ctx.comp_mgr.clear();
            WidgetAction::Redraw
        }
        "edit-command-line" => WidgetAction::EditInExternalEditor,

        // --- FZF Integrations ---
        "fzf-file-widget" => {
            if let Ok(output) = std::process::Command::new("fzf")
                .arg("--height")
                .arg("40%")
                .arg("--reverse")
                .output()
            {
                let selected = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if output.status.success() && !selected.is_empty() {
                    ctx.text_buf.insert_str(&selected);
                }
            }
            WidgetAction::Redraw
        }
        "fzf-cd-widget" => {
            if let Ok(output) = std::process::Command::new("fzf")
                .arg("--height")
                .arg("40%")
                .arg("--reverse")
                .output()
            {
                let selected = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if output.status.success() && !selected.is_empty() {
                    ctx.text_buf.replace_content(&format!("cd {}", selected));
                    return WidgetAction::AcceptLine;
                }
            }
            WidgetAction::Redraw
        }

        // --- Completion Widgets ---
        "expand-or-complete" => {
            *ctx.history_index = None;
            if !ctx.comp_mgr.visible {
                ctx.comp_mgr
                    .update(&ctx.text_buf.text(), ctx.text_buf.cursor(), true);
                if ctx.comp_mgr.visible && !ctx.comp_mgr.suggestions.is_empty() {
                    if ctx.comp_mgr.suggestions.len() == 1 {
                        if let Some(s) = ctx.comp_mgr.suggestions.first() {
                            let line = ctx.text_buf.text();
                            super::apply_completion(ctx.text_buf, &line, s);
                        }
                        ctx.comp_mgr.clear();
                    } else if !ctx.comp_mgr.prefix_accepted {
                        if let Some(prefix) = ctx.comp_mgr.longest_common_prefix() {
                            let line = ctx.text_buf.text();
                            let current_word = crate::ftui::completions::extract_partial_word(
                                &line,
                                ctx.text_buf.cursor(),
                            );
                            if prefix.len() > current_word.len() {
                                let extra = &prefix[current_word.len()..];
                                for c in extra.chars() {
                                    ctx.text_buf.insert_char(c);
                                }
                                super::append_slash_if_dir(ctx.text_buf, &prefix);
                                ctx.comp_mgr.prefix_accepted = true;
                            } else {
                                ctx.comp_mgr.active_selection = true;
                                ctx.comp_mgr.prefix_accepted = true;
                            }
                        } else {
                            ctx.comp_mgr.active_selection = true;
                            ctx.comp_mgr.prefix_accepted = true;
                        }
                    } else {
                        ctx.comp_mgr.active_selection = true;
                        ctx.comp_mgr.select_next();
                    }
                }
            } else {
                ctx.comp_mgr.active_selection = true;
                ctx.comp_mgr.select_next();
            }
            WidgetAction::Redraw
        }
        "reverse-menu-complete" => {
            *ctx.history_index = None;
            if ctx.comp_mgr.visible && !ctx.comp_mgr.suggestions.is_empty() {
                ctx.comp_mgr.active_selection = true;
                ctx.comp_mgr.select_prev();
            }
            WidgetAction::Redraw
        }
        "abort" | "interrupt" => {
            *ctx.history_index = None;
            ctx.comp_mgr.clear();
            WidgetAction::Abort
        }
        "delete-char-or-list" => {
            *ctx.history_index = None;
            if ctx.text_buf.is_empty() {
                WidgetAction::Exit
            } else {
                ctx.text_buf.delete_right();
                ctx.comp_mgr
                    .update(&ctx.text_buf.text(), ctx.text_buf.cursor(), false);
                WidgetAction::Redraw
            }
        }

        // --- Vi Mode Switching & Actions ---
        "vi-insert-mode" => {
            *ctx.keymap_mode = KeyMapMode::ViInsert;
            WidgetAction::Redraw
        }
        "vi-normal-mode" => {
            *ctx.keymap_mode = KeyMapMode::ViNormal;
            WidgetAction::Redraw
        }
        "vi-append-mode" => {
            *ctx.keymap_mode = KeyMapMode::ViInsert;
            ctx.text_buf.move_right();
            WidgetAction::Redraw
        }
        "vi-change-to-eol" => {
            let cursor = ctx.text_buf.cursor();
            let chars = ctx.text_buf.chars();
            let remainder: String = chars.iter().skip(cursor).collect();
            *ctx.last_kill = Some(remainder.clone());
            clipboard::copy_to_clipboard(&remainder);
            for _ in 0..remainder.chars().count() {
                ctx.text_buf.delete_right();
            }
            *ctx.keymap_mode = KeyMapMode::ViInsert;
            WidgetAction::Redraw
        }
        "vi-delete-to-eol" => {
            let cursor = ctx.text_buf.cursor();
            let chars = ctx.text_buf.chars();
            let remainder: String = chars.iter().skip(cursor).collect();
            *ctx.last_kill = Some(remainder.clone());
            clipboard::copy_to_clipboard(&remainder);
            for _ in 0..remainder.chars().count() {
                ctx.text_buf.delete_right();
            }
            WidgetAction::Redraw
        }
        "vi-yank-eol" => {
            let cursor = ctx.text_buf.cursor();
            let chars = ctx.text_buf.chars();
            let remainder: String = chars.iter().skip(cursor).collect();
            *ctx.last_kill = Some(remainder.clone());
            clipboard::copy_to_clipboard(&remainder);
            WidgetAction::Redraw
        }
        "vi-forward-char" => {
            ctx.text_buf.move_right();
            ctx.comp_mgr.clear();
            WidgetAction::Redraw
        }
        "vi-backward-char" => {
            ctx.text_buf.move_left();
            ctx.comp_mgr.clear();
            WidgetAction::Redraw
        }
        "vi-delete-char" => {
            ctx.text_buf.delete_right();
            WidgetAction::Redraw
        }
        "vi-kill-line" => {
            ctx.text_buf.replace_content("");
            WidgetAction::Redraw
        }

        _ => WidgetAction::Continue,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ftui::buffer::TextBuffer;
    use crate::ftui::completions::CompletionsManager;
    use crate::ftui::history::HistoryManager;
    use crate::ftui::widget_explorer::WidgetExplorerManager;
    use fshell_engine::Env;

    #[test]
    fn test_kill_line_multibyte_utf8() {
        let mut buf = TextBuffer::new();
        buf.insert_str("echo è à é\nsecond line");
        // Place cursor right after 'è' (char index 6)
        buf.set_cursor(6);

        let env = Env::new();
        let mut mode = KeyMapMode::Emacs;
        let mut hist_mgr = HistoryManager::new();
        let mut comp_mgr = CompletionsManager::new(env.clone());
        let mut explorer = WidgetExplorerManager::new();
        let current_dir = "/tmp";
        let hostname = "test".to_string();
        let session_id = "test".to_string();
        let mut help_visible = false;
        let mut last_kill = None;
        let current_hint = String::new();
        let mut history_index = None;
        let mut filtered_history = Vec::new();
        let mut temp_input = String::new();

        let mut ctx = WidgetContext {
            text_buf: &mut buf,
            env: &env,
            keymap_mode: &mut mode,
            history_mgr: &mut hist_mgr,
            comp_mgr: &mut comp_mgr,
            widget_explorer: &mut explorer,
            current_dir,
            hostname: &hostname,
            session_id: &session_id,
            help_visible: &mut help_visible,
            last_kill: &mut last_kill,
            current_hint: &current_hint,
            history_index: &mut history_index,
            filtered_history: &mut filtered_history,
            temp_input: &mut temp_input,
        };

        execute_widget("kill-line", &mut ctx);
        assert_eq!(ctx.text_buf.text(), "echo è\nsecond line");
        assert_eq!(last_kill.as_deref(), Some(" à é"));
    }

    #[test]
    fn test_backward_kill_line_multibyte_utf8() {
        let mut buf = TextBuffer::new();
        buf.insert_str("line 1\necho è à é");
        buf.move_to_end();

        let env = Env::new();
        let mut mode = KeyMapMode::Emacs;
        let mut hist_mgr = HistoryManager::new();
        let mut comp_mgr = CompletionsManager::new(env.clone());
        let mut explorer = WidgetExplorerManager::new();
        let current_dir = "/tmp";
        let hostname = "test".to_string();
        let session_id = "test".to_string();
        let mut help_visible = false;
        let mut last_kill = None;
        let current_hint = String::new();
        let mut history_index = None;
        let mut filtered_history = Vec::new();
        let mut temp_input = String::new();

        let mut ctx = WidgetContext {
            text_buf: &mut buf,
            env: &env,
            keymap_mode: &mut mode,
            history_mgr: &mut hist_mgr,
            comp_mgr: &mut comp_mgr,
            widget_explorer: &mut explorer,
            current_dir,
            hostname: &hostname,
            session_id: &session_id,
            help_visible: &mut help_visible,
            last_kill: &mut last_kill,
            current_hint: &current_hint,
            history_index: &mut history_index,
            filtered_history: &mut filtered_history,
            temp_input: &mut temp_input,
        };

        execute_widget("backward-kill-line", &mut ctx);
        assert_eq!(ctx.text_buf.text(), "line 1\n");
        assert_eq!(last_kill.as_deref(), Some("echo è à é"));
    }
}
