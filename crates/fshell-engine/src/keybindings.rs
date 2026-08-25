// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![allow(clippy::result_large_err)]

//! Keybinding and Widget registry for fshell.
//!
//! Provides a first-class editor keymap architecture supporting:
//! - Emacs and Vi (Insert, Normal, Visual) keymaps
//! - Full string parsing for chords (`ctrl-a`, `^A`, `C-a`, `alt-b`, `M-b`, `f1`, etc.)
//! - Macro injection and user function dispatch
//! - Canonical readline/ZLE widget names

use fshell_hash::FxHashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyMapMode {
    Emacs,
    ViInsert,
    ViNormal,
    ViVisual,
}

impl std::fmt::Display for KeyMapMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyMapMode::Emacs => write!(f, "emacs"),
            KeyMapMode::ViInsert => write!(f, "vi-insert"),
            KeyMapMode::ViNormal => write!(f, "vi-normal"),
            KeyMapMode::ViVisual => write!(f, "vi-visual"),
        }
    }
}

impl KeyMapMode {
    pub fn parse_mode(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "emacs" | "e" => Some(KeyMapMode::Emacs),
            "vi" | "vi-insert" | "viins" | "insert" => Some(KeyMapMode::ViInsert),
            "vicmd" | "vi-normal" | "normal" | "cmd" => Some(KeyMapMode::ViNormal),
            "visual" | "vi-visual" => Some(KeyMapMode::ViVisual),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyCodeDef {
    Char(char),
    Enter,
    Backspace,
    Tab,
    BackTab,
    Esc,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    Insert,
    F(u8),
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct KeyModifiersDef {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyChord {
    pub code: KeyCodeDef,
    pub modifiers: KeyModifiersDef,
}

impl std::fmt::Display for KeyChord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        if self.modifiers.control {
            parts.push("ctrl");
        }
        if self.modifiers.alt {
            parts.push("alt");
        }
        if self.modifiers.shift {
            parts.push("shift");
        }

        let code_str = match &self.code {
            KeyCodeDef::Char(' ') => "space".to_string(),
            KeyCodeDef::Char(c) => c.to_string(),
            KeyCodeDef::Enter => "enter".to_string(),
            KeyCodeDef::Backspace => "backspace".to_string(),
            KeyCodeDef::Tab => "tab".to_string(),
            KeyCodeDef::BackTab => "backtab".to_string(),
            KeyCodeDef::Esc => "esc".to_string(),
            KeyCodeDef::Up => "up".to_string(),
            KeyCodeDef::Down => "down".to_string(),
            KeyCodeDef::Left => "left".to_string(),
            KeyCodeDef::Right => "right".to_string(),
            KeyCodeDef::Home => "home".to_string(),
            KeyCodeDef::End => "end".to_string(),
            KeyCodeDef::PageUp => "pageup".to_string(),
            KeyCodeDef::PageDown => "pagedown".to_string(),
            KeyCodeDef::Delete => "delete".to_string(),
            KeyCodeDef::Insert => "insert".to_string(),
            KeyCodeDef::F(n) => format!("f{}", n),
            KeyCodeDef::Null => "null".to_string(),
        };

        if parts.is_empty() {
            write!(f, "{}", code_str)
        } else {
            write!(f, "{}-{}", parts.join("-"), code_str)
        }
    }
}

impl KeyChord {
    pub fn parse(input: &str) -> Result<Self, String> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err("empty key sequence".to_string());
        }

        // Caret notation: ^A, ^R, ^X
        if let Some(ch) = trimmed.strip_prefix('^').and_then(|r| r.chars().next()) {
            let lower = ch.to_ascii_lowercase();
            return Ok(KeyChord {
                code: KeyCodeDef::Char(lower),
                modifiers: KeyModifiersDef {
                    control: true,
                    alt: false,
                    shift: false,
                },
            });
        }

        let parts: Vec<&str> = trimmed
            .split(['-', '+'])
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        if parts.is_empty() {
            return Err(format!("invalid key sequence: {:?}", input));
        }

        let mut modifiers = KeyModifiersDef::default();
        let key_str = parts.last().copied().unwrap_or("");

        for mod_part in &parts[..parts.len() - 1] {
            match mod_part.to_ascii_lowercase().as_str() {
                "ctrl" | "c" | "control" => modifiers.control = true,
                "alt" | "m" | "meta" | "opt" | "option" => modifiers.alt = true,
                "shift" | "s" => modifiers.shift = true,
                other => return Err(format!("unknown modifier: {:?}", other)),
            }
        }

        let code = match key_str.to_ascii_lowercase().as_str() {
            "enter" | "return" | "ret" | "cr" => KeyCodeDef::Enter,
            "backspace" | "bs" => KeyCodeDef::Backspace,
            "tab" => {
                if modifiers.shift {
                    modifiers.shift = false;
                    KeyCodeDef::BackTab
                } else {
                    KeyCodeDef::Tab
                }
            }
            // Note: "shift-tab" is normalized earlier (shift modifier + tab) into
            // the BackTab arm above, so only literal "backtab" arrives here.
            "backtab" => KeyCodeDef::BackTab,
            "esc" | "escape" => KeyCodeDef::Esc,
            "up" => KeyCodeDef::Up,
            "down" => KeyCodeDef::Down,
            "left" => KeyCodeDef::Left,
            "right" => KeyCodeDef::Right,
            "home" => KeyCodeDef::Home,
            "end" => KeyCodeDef::End,
            "pageup" | "pgup" => KeyCodeDef::PageUp,
            "pagedown" | "pgdn" => KeyCodeDef::PageDown,
            "delete" | "del" => KeyCodeDef::Delete,
            "insert" | "ins" => KeyCodeDef::Insert,
            "space" | "spc" => KeyCodeDef::Char(' '),
            s if s.starts_with('f')
                && s.len() > 1
                && s[1..].chars().all(|c| c.is_ascii_digit()) =>
            {
                let n: u8 = s[1..]
                    .parse()
                    .map_err(|_| format!("invalid function key: {}", s))?;
                KeyCodeDef::F(n)
            }
            s if s.chars().count() == 1 => {
                let c = s.chars().next().unwrap_or(' ');
                if c.is_ascii_uppercase() && !modifiers.shift {
                    modifiers.shift = true;
                }
                KeyCodeDef::Char(c.to_ascii_lowercase())
            }
            other => return Err(format!("unknown key code: {:?}", other)),
        };

        Ok(KeyChord { code, modifiers })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyAction {
    Widget(String),
    Macro(String),
    Function(String),
}

impl std::fmt::Display for KeyAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyAction::Widget(w) => write!(f, "{}", w),
            KeyAction::Macro(m) => write!(f, "\"{}\"", m.escape_debug()),
            KeyAction::Function(func) => write!(f, "fn:{}", func),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeybindingRegistry {
    pub active_mode: KeyMapMode,
    pub emacs_bindings: FxHashMap<KeyChord, KeyAction>,
    pub vi_insert_bindings: FxHashMap<KeyChord, KeyAction>,
    pub vi_normal_bindings: FxHashMap<KeyChord, KeyAction>,
    pub vi_visual_bindings: FxHashMap<KeyChord, KeyAction>,
}

impl Default for KeybindingRegistry {
    fn default() -> Self {
        let mut registry = Self {
            active_mode: KeyMapMode::Emacs,
            emacs_bindings: FxHashMap::default(),
            vi_insert_bindings: FxHashMap::default(),
            vi_normal_bindings: FxHashMap::default(),
            vi_visual_bindings: FxHashMap::default(),
        };
        registry.populate_defaults();
        registry
    }
}

impl KeybindingRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn populate_defaults(&mut self) {
        // --- Emacs Keymap Defaults ---
        self.bind(
            KeyMapMode::Emacs,
            "ctrl-a",
            KeyAction::Widget("beginning-of-line".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "ctrl-e",
            KeyAction::Widget("end-of-line".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "ctrl-f",
            KeyAction::Widget("forward-char".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "right",
            KeyAction::Widget("forward-char".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "ctrl-b",
            KeyAction::Widget("backward-char".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "left",
            KeyAction::Widget("backward-char".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "alt-f",
            KeyAction::Widget("forward-word".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "ctrl-right",
            KeyAction::Widget("forward-word".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "alt-b",
            KeyAction::Widget("backward-word".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "ctrl-left",
            KeyAction::Widget("backward-word".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "ctrl-k",
            KeyAction::Widget("kill-line".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "ctrl-u",
            KeyAction::Widget("backward-kill-line".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "ctrl-w",
            KeyAction::Widget("backward-kill-word".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "alt-d",
            KeyAction::Widget("kill-word".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "ctrl-y",
            KeyAction::Widget("yank".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "ctrl-z",
            KeyAction::Widget("undo".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "ctrl-l",
            KeyAction::Widget("clear-screen".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "ctrl-r",
            KeyAction::Widget("interactive-history-search".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "ctrl-h",
            KeyAction::Widget("history-explorer".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "alt-r",
            KeyAction::Widget("aborted-history-search".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "up",
            KeyAction::Widget("history-search-backward".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "down",
            KeyAction::Widget("history-search-forward".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "home",
            KeyAction::Widget("beginning-of-line".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "end",
            KeyAction::Widget("end-of-line".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "alt-home",
            KeyAction::Widget("beginning-of-buffer".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "alt-end",
            KeyAction::Widget("end-of-buffer".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "backspace",
            KeyAction::Widget("backward-delete-char".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "delete",
            KeyAction::Widget("delete-char".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "tab",
            KeyAction::Widget("expand-or-complete".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "backtab",
            KeyAction::Widget("reverse-menu-complete".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "enter",
            KeyAction::Widget("accept-line".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "ctrl-j",
            KeyAction::Widget("newline-and-indent".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "shift-enter",
            KeyAction::Widget("newline-and-indent".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "ctrl-enter",
            KeyAction::Widget("newline-and-indent".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "f1",
            KeyAction::Widget("toggle-help".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "alt-e",
            KeyAction::Widget("edit-command-line".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "ctrl-t",
            KeyAction::Widget("fzf-file-widget".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::Emacs,
            "alt-c",
            KeyAction::Widget("fzf-cd-widget".to_string()),
        )
        .ok();

        // --- Vi Insert Keymap Defaults ---
        self.bind(
            KeyMapMode::ViInsert,
            "esc",
            KeyAction::Widget("vi-normal-mode".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::ViInsert,
            "ctrl-a",
            KeyAction::Widget("beginning-of-line".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::ViInsert,
            "ctrl-e",
            KeyAction::Widget("end-of-line".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::ViInsert,
            "ctrl-w",
            KeyAction::Widget("backward-kill-word".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::ViInsert,
            "ctrl-u",
            KeyAction::Widget("backward-kill-line".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::ViInsert,
            "ctrl-r",
            KeyAction::Widget("interactive-history-search".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::ViInsert,
            "tab",
            KeyAction::Widget("expand-or-complete".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::ViInsert,
            "enter",
            KeyAction::Widget("accept-line".to_string()),
        )
        .ok();

        // --- Vi Normal Keymap Defaults ---
        self.bind(
            KeyMapMode::ViNormal,
            "i",
            KeyAction::Widget("vi-insert-mode".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::ViNormal,
            "a",
            KeyAction::Widget("vi-append-mode".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::ViNormal,
            "h",
            KeyAction::Widget("backward-char".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::ViNormal,
            "l",
            KeyAction::Widget("forward-char".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::ViNormal,
            "j",
            KeyAction::Widget("history-search-forward".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::ViNormal,
            "k",
            KeyAction::Widget("history-search-backward".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::ViNormal,
            "w",
            KeyAction::Widget("forward-word".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::ViNormal,
            "b",
            KeyAction::Widget("backward-word".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::ViNormal,
            "0",
            KeyAction::Widget("beginning-of-line".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::ViNormal,
            "$",
            KeyAction::Widget("end-of-line".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::ViNormal,
            "x",
            KeyAction::Widget("delete-char".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::ViNormal,
            "u",
            KeyAction::Widget("undo".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::ViNormal,
            "ctrl-r",
            KeyAction::Widget("redo".to_string()),
        )
        .ok();
        self.bind(
            KeyMapMode::ViNormal,
            "enter",
            KeyAction::Widget("accept-line".to_string()),
        )
        .ok();
    }

    pub fn bind(
        &mut self,
        mode: KeyMapMode,
        chord_str: &str,
        action: KeyAction,
    ) -> Result<(), String> {
        let chord = KeyChord::parse(chord_str)?;
        let map = match mode {
            KeyMapMode::Emacs => &mut self.emacs_bindings,
            KeyMapMode::ViInsert => &mut self.vi_insert_bindings,
            KeyMapMode::ViNormal => &mut self.vi_normal_bindings,
            KeyMapMode::ViVisual => &mut self.vi_visual_bindings,
        };
        map.insert(chord, action);
        Ok(())
    }

    pub fn unbind(&mut self, mode: KeyMapMode, chord_str: &str) -> Result<bool, String> {
        let chord = KeyChord::parse(chord_str)?;
        let map = match mode {
            KeyMapMode::Emacs => &mut self.emacs_bindings,
            KeyMapMode::ViInsert => &mut self.vi_insert_bindings,
            KeyMapMode::ViNormal => &mut self.vi_normal_bindings,
            KeyMapMode::ViVisual => &mut self.vi_visual_bindings,
        };
        Ok(map.remove(&chord).is_some())
    }

    pub fn get_action(&self, mode: KeyMapMode, chord: &KeyChord) -> Option<&KeyAction> {
        let map = match mode {
            KeyMapMode::Emacs => &self.emacs_bindings,
            KeyMapMode::ViInsert => &self.vi_insert_bindings,
            KeyMapMode::ViNormal => &self.vi_normal_bindings,
            KeyMapMode::ViVisual => &self.vi_visual_bindings,
        };
        map.get(chord)
    }

    pub fn list_bindings(&self, mode: KeyMapMode) -> Vec<(String, String)> {
        let map = match mode {
            KeyMapMode::Emacs => &self.emacs_bindings,
            KeyMapMode::ViInsert => &self.vi_insert_bindings,
            KeyMapMode::ViNormal => &self.vi_normal_bindings,
            KeyMapMode::ViVisual => &self.vi_visual_bindings,
        };
        let mut list: Vec<(String, String)> = map
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        list.sort_by(|a, b| a.0.cmp(&b.0));
        list
    }

    /// Find all key chords bound to a specific widget in the given keymap mode.
    pub fn find_chords_for_widget(&self, mode: KeyMapMode, widget_name: &str) -> Vec<String> {
        let map = match mode {
            KeyMapMode::Emacs => &self.emacs_bindings,
            KeyMapMode::ViInsert => &self.vi_insert_bindings,
            KeyMapMode::ViNormal => &self.vi_normal_bindings,
            KeyMapMode::ViVisual => &self.vi_visual_bindings,
        };
        let mut chords = Vec::new();
        for (chord, action) in map {
            if let KeyAction::Widget(w) = action
                && w == widget_name
            {
                chords.push(chord.to_string());
            }
        }
        chords.sort();
        chords
    }
}

/// Metadata catalog entry describing an editor widget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetInfo {
    pub name: &'static str,
    pub category: &'static str,
    pub description: &'static str,
    pub default_chord_emacs: Option<&'static str>,
    pub default_chord_vi: Option<&'static str>,
}

/// Canonical catalog of all built-in editor widgets.
pub fn all_widgets() -> &'static [WidgetInfo] {
    &[
        // --- Navigation ---
        WidgetInfo {
            name: "beginning-of-line",
            category: "Navigation",
            description: "Move cursor to beginning of line",
            default_chord_emacs: Some("ctrl-a, home"),
            default_chord_vi: Some("0"),
        },
        WidgetInfo {
            name: "end-of-line",
            category: "Navigation",
            description: "Move cursor to end of line",
            default_chord_emacs: Some("ctrl-e, end"),
            default_chord_vi: Some("$"),
        },
        WidgetInfo {
            name: "forward-char",
            category: "Navigation",
            description: "Move cursor right one character or accept ghost hint",
            default_chord_emacs: Some("ctrl-f, right"),
            default_chord_vi: Some("l"),
        },
        WidgetInfo {
            name: "backward-char",
            category: "Navigation",
            description: "Move cursor left one character",
            default_chord_emacs: Some("ctrl-b, left"),
            default_chord_vi: Some("h"),
        },
        WidgetInfo {
            name: "forward-word",
            category: "Navigation",
            description: "Move cursor forward one word or accept next ghost word",
            default_chord_emacs: Some("alt-f, ctrl-right"),
            default_chord_vi: Some("w"),
        },
        WidgetInfo {
            name: "backward-word",
            category: "Navigation",
            description: "Move cursor backward one word",
            default_chord_emacs: Some("alt-b, ctrl-left"),
            default_chord_vi: Some("b"),
        },
        WidgetInfo {
            name: "beginning-of-buffer",
            category: "Navigation",
            description: "Move cursor to start of multi-line buffer",
            default_chord_emacs: Some("alt-home"),
            default_chord_vi: None,
        },
        WidgetInfo {
            name: "end-of-buffer",
            category: "Navigation",
            description: "Move cursor to end of multi-line buffer",
            default_chord_emacs: Some("alt-end"),
            default_chord_vi: None,
        },
        // --- History Recall ---
        WidgetInfo {
            name: "history-search-backward",
            category: "History Recall",
            description: "Prefix search backward in history",
            default_chord_emacs: Some("up"),
            default_chord_vi: Some("k"),
        },
        WidgetInfo {
            name: "history-search-forward",
            category: "History Recall",
            description: "Prefix search forward in history",
            default_chord_emacs: Some("down"),
            default_chord_vi: Some("j"),
        },
        WidgetInfo {
            name: "interactive-history-search",
            category: "History Recall",
            description: "Open interactive fuzzy history search popup",
            default_chord_emacs: Some("ctrl-r"),
            default_chord_vi: None,
        },
        WidgetInfo {
            name: "history-explorer",
            category: "History Recall",
            description: "Open full multi-column history database explorer",
            default_chord_emacs: Some("ctrl-h"),
            default_chord_vi: None,
        },
        WidgetInfo {
            name: "aborted-history-search",
            category: "History Recall",
            description: "Open search of cancelled/aborted commands",
            default_chord_emacs: Some("alt-r"),
            default_chord_vi: None,
        },
        // --- Completions ---
        WidgetInfo {
            name: "expand-or-complete",
            category: "Completions",
            description: "Expand longest common prefix or open completion popup",
            default_chord_emacs: Some("tab"),
            default_chord_vi: Some("tab"),
        },
        WidgetInfo {
            name: "reverse-menu-complete",
            category: "Completions",
            description: "Cycle backward through completion menu candidates",
            default_chord_emacs: Some("backtab, shift-tab"),
            default_chord_vi: None,
        },
        // --- Editing & Killing ---
        WidgetInfo {
            name: "delete-char",
            category: "Editing & Killing",
            description: "Delete character under cursor",
            default_chord_emacs: Some("delete"),
            default_chord_vi: Some("x"),
        },
        WidgetInfo {
            name: "backward-delete-char",
            category: "Editing & Killing",
            description: "Delete character before cursor",
            default_chord_emacs: Some("backspace"),
            default_chord_vi: None,
        },
        WidgetInfo {
            name: "delete-char-or-list",
            category: "Editing & Killing",
            description: "Delete character forward or exit if line is empty",
            default_chord_emacs: Some("ctrl-d"),
            default_chord_vi: None,
        },
        WidgetInfo {
            name: "kill-line",
            category: "Editing & Killing",
            description: "Kill text from cursor to end of line",
            default_chord_emacs: Some("ctrl-k"),
            default_chord_vi: None,
        },
        WidgetInfo {
            name: "backward-kill-line",
            category: "Editing & Killing",
            description: "Kill text from line start to cursor",
            default_chord_emacs: Some("ctrl-u"),
            default_chord_vi: Some("ctrl-u"),
        },
        WidgetInfo {
            name: "kill-word",
            category: "Editing & Killing",
            description: "Kill word forward from cursor",
            default_chord_emacs: Some("alt-d"),
            default_chord_vi: None,
        },
        WidgetInfo {
            name: "backward-kill-word",
            category: "Editing & Killing",
            description: "Kill word backward from cursor",
            default_chord_emacs: Some("ctrl-w"),
            default_chord_vi: Some("ctrl-w"),
        },
        WidgetInfo {
            name: "kill-buffer",
            category: "Editing & Killing",
            description: "Clear entire buffer and copy to killring",
            default_chord_emacs: None,
            default_chord_vi: None,
        },
        WidgetInfo {
            name: "yank",
            category: "Editing & Killing",
            description: "Paste last killed text from killring / clipboard",
            default_chord_emacs: Some("ctrl-y"),
            default_chord_vi: None,
        },
        // --- Transformations ---
        WidgetInfo {
            name: "capitalize-word",
            category: "Transformations",
            description: "Capitalize next word from cursor",
            default_chord_emacs: Some("alt-c"),
            default_chord_vi: None,
        },
        WidgetInfo {
            name: "upcase-word",
            category: "Transformations",
            description: "Convert next word to UPPERCASE",
            default_chord_emacs: Some("alt-u"),
            default_chord_vi: None,
        },
        WidgetInfo {
            name: "downcase-word",
            category: "Transformations",
            description: "Convert next word to lowercase",
            default_chord_emacs: Some("alt-l"),
            default_chord_vi: None,
        },
        WidgetInfo {
            name: "transpose-chars",
            category: "Transformations",
            description: "Swap adjacent characters",
            default_chord_emacs: Some("ctrl-t"),
            default_chord_vi: None,
        },
        WidgetInfo {
            name: "transpose-words",
            category: "Transformations",
            description: "Swap adjacent words",
            default_chord_emacs: Some("alt-t"),
            default_chord_vi: None,
        },
        // --- History & Undo ---
        WidgetInfo {
            name: "undo",
            category: "History & Undo",
            description: "Undo last edit transaction",
            default_chord_emacs: Some("ctrl-z"),
            default_chord_vi: Some("u"),
        },
        WidgetInfo {
            name: "redo",
            category: "History & Undo",
            description: "Redo last undone transaction",
            default_chord_emacs: None,
            default_chord_vi: Some("ctrl-r"),
        },
        WidgetInfo {
            name: "clear-screen",
            category: "History & Undo",
            description: "Clear terminal screen and repaint prompt",
            default_chord_emacs: Some("ctrl-l"),
            default_chord_vi: None,
        },
        // --- Execution & Control ---
        WidgetInfo {
            name: "accept-line",
            category: "Execution & Control",
            description: "Validate and execute current command line",
            default_chord_emacs: Some("enter"),
            default_chord_vi: Some("enter"),
        },
        WidgetInfo {
            name: "newline-and-indent",
            category: "Execution & Control",
            description: "Insert newline with automatic block indentation",
            default_chord_emacs: Some("ctrl-j, shift-enter"),
            default_chord_vi: None,
        },
        WidgetInfo {
            name: "edit-command-line",
            category: "Execution & Control",
            description: "Open current buffer in external $EDITOR",
            default_chord_emacs: Some("alt-e"),
            default_chord_vi: Some("v"),
        },
        WidgetInfo {
            name: "toggle-help",
            category: "Execution & Control",
            description: "Open interactive Keybindings & Widget Explorer modal",
            default_chord_emacs: Some("f1"),
            default_chord_vi: None,
        },
        WidgetInfo {
            name: "abort",
            category: "Execution & Control",
            description: "Cancel multi-line edit or save command to aborted history",
            default_chord_emacs: Some("ctrl-c"),
            default_chord_vi: None,
        },
        // --- FZF Integration ---
        WidgetInfo {
            name: "fzf-file-widget",
            category: "FZF Integration",
            description: "Fuzzy select files and insert into command line",
            default_chord_emacs: Some("ctrl-t"),
            default_chord_vi: None,
        },
        WidgetInfo {
            name: "fzf-cd-widget",
            category: "FZF Integration",
            description: "Fuzzy select directory and change directory",
            default_chord_emacs: Some("alt-c"),
            default_chord_vi: None,
        },
        // --- Vi Mode ---
        WidgetInfo {
            name: "vi-insert-mode",
            category: "Vi Mode",
            description: "Switch to Vi Insert mode",
            default_chord_emacs: None,
            default_chord_vi: Some("i"),
        },
        WidgetInfo {
            name: "vi-normal-mode",
            category: "Vi Mode",
            description: "Switch to Vi Normal (Command) mode",
            default_chord_emacs: None,
            default_chord_vi: Some("esc"),
        },
        WidgetInfo {
            name: "vi-append-mode",
            category: "Vi Mode",
            description: "Move cursor right and switch to Vi Insert mode",
            default_chord_emacs: None,
            default_chord_vi: Some("a"),
        },
        WidgetInfo {
            name: "vi-change-to-eol",
            category: "Vi Mode",
            description: "Kill line to end and switch to Vi Insert mode",
            default_chord_emacs: None,
            default_chord_vi: Some("C"),
        },
        WidgetInfo {
            name: "vi-delete-to-eol",
            category: "Vi Mode",
            description: "Kill line to end in Vi Normal mode",
            default_chord_emacs: None,
            default_chord_vi: Some("D"),
        },
        WidgetInfo {
            name: "vi-yank-eol",
            category: "Vi Mode",
            description: "Yank line to end into killring",
            default_chord_emacs: None,
            default_chord_vi: Some("Y"),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_key_chord() {
        let k1 = KeyChord::parse("ctrl-a").unwrap();
        assert_eq!(k1.code, KeyCodeDef::Char('a'));
        assert!(k1.modifiers.control);
        assert!(!k1.modifiers.alt);

        let k2 = KeyChord::parse("^R").unwrap();
        assert_eq!(k2.code, KeyCodeDef::Char('r'));
        assert!(k2.modifiers.control);

        let k3 = KeyChord::parse("alt-f").unwrap();
        assert_eq!(k3.code, KeyCodeDef::Char('f'));
        assert!(k3.modifiers.alt);

        let k4 = KeyChord::parse("shift-tab").unwrap();
        assert_eq!(k4.code, KeyCodeDef::BackTab);

        let k5 = KeyChord::parse("f12").unwrap();
        assert_eq!(k5.code, KeyCodeDef::F(12));
    }

    #[test]
    fn test_registry_bind_and_get() {
        let mut reg = KeybindingRegistry::new();
        reg.bind(
            KeyMapMode::Emacs,
            "ctrl-k",
            KeyAction::Widget("kill-line".to_string()),
        )
        .unwrap();
        let chord = KeyChord::parse("ctrl-k").unwrap();
        assert_eq!(
            reg.get_action(KeyMapMode::Emacs, &chord),
            Some(&KeyAction::Widget("kill-line".to_string()))
        );
    }
}
