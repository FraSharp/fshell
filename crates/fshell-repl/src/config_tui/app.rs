// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Application state machine, input handling, and event loop for config TUI.

use super::schema::{OptionItem, OptionKind};
use super::widgets;
use crate::theme_ext::ThemeColorRatatui;
use crossterm::event::{self, Event, KeyCode, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use fshell_core::Val;
use fshell_core::theme::Theme;
use fshell_engine::Env;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use std::io;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    ShellOptions,
    Themes,
    Aliases,
    Hooks,
    EnvVars,
    Prompt,
}

impl Category {
    pub fn all() -> &'static [Category] {
        &[
            Category::ShellOptions,
            Category::Themes,
            Category::Aliases,
            Category::Hooks,
            Category::EnvVars,
            Category::Prompt,
        ]
    }

    pub fn label(&self) -> &str {
        match self {
            Category::ShellOptions => "Shell Options",
            Category::Themes => "Themes & Colors",
            Category::Aliases => "Aliases",
            Category::Hooks => "Hooks",
            Category::EnvVars => "Environment Vars",
            Category::Prompt => "Prompt & Studio",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Content,
    Search,
    Modal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModalType {
    None,
    TextInput {
        title: String,
        label: String,
        value: String,
        details: Vec<String>,
        cursor: usize,
        target_opt_key: Option<String>,
        error: Option<String>,
    },
    TwoFieldAlias {
        title: String,
        name: String,
        expansion: String,
        active_field: usize, // 0 = name, 1 = expansion
        cursor: usize,
        is_editing: bool,
        old_name: Option<String>,
        error: Option<String>,
    },
    TwoFieldHook {
        title: String,
        event: String,
        fn_name: String,
        active_field: usize,
        cursor: usize,
        error: Option<String>,
    },
    ConfirmDiscard {
        title: String,
        message: String,
    },
    Help,
}

#[derive(Debug, Clone)]
pub enum PendingAction {
    OptionChanged(String, String),
    AliasAdded(String, String),
    AliasDeleted(String),
    HookAdded(String, String),
    HookDeleted(String, String),
}

pub struct App<'a> {
    pub env: &'a Env,
    pub categories: Vec<Category>,
    pub focus: Focus,
    pub sidebar_selected: usize,
    pub content_selected: usize,
    pub scroll_offset: usize,
    pub running: bool,
    pub dirty: bool,
    pub search_query: String,
    pub options: Vec<OptionItem>,
    pub themes: Vec<String>,
    pub modal: ModalType,
    pub pending_actions: Vec<PendingAction>,
    pub status_message: Option<(String, bool)>, // (msg, is_error)
    pub launch_prompt_studio: bool,
    pub original_theme: String,
}

impl<'a> App<'a> {
    pub fn new(env: &'a Env) -> Self {
        let config_dir =
            fshell_engine::config_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        let mut themes = Theme::available(&config_dir);
        themes.sort();

        let options = OptionItem::load_all(env);
        let original_theme = env.options.read().theme.clone();

        Self {
            env,
            categories: Category::all().to_vec(),
            focus: Focus::Sidebar,
            sidebar_selected: 0,
            content_selected: 0,
            scroll_offset: 0,
            running: true,
            dirty: false,
            search_query: String::new(),
            options,
            themes,
            modal: ModalType::None,
            pending_actions: Vec::new(),
            status_message: None,
            launch_prompt_studio: false,
            original_theme,
        }
    }

    pub fn current_category(&self) -> Category {
        self.categories[self.sidebar_selected]
    }

    pub fn filtered_option_indices(&self) -> Vec<usize> {
        if self.search_query.is_empty() {
            return (0..self.options.len()).collect();
        }
        let q = self.search_query.to_lowercase();
        self.options
            .iter()
            .enumerate()
            .filter(|(_, opt)| {
                opt.key.to_lowercase().contains(&q)
                    || opt.label.to_lowercase().contains(&q)
                    || opt.description.to_lowercase().contains(&q)
                    || opt.section.to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn filtered_aliases(&self) -> Vec<(String, String)> {
        let aliases = self.env.scope.aliases.read();
        let mut list: Vec<(String, String)> = aliases
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        drop(aliases);

        if !self.search_query.is_empty() {
            let q = self.search_query.to_lowercase();
            list.retain(|(k, v)| k.to_lowercase().contains(&q) || v.to_lowercase().contains(&q));
        }
        list
    }

    pub fn flat_hooks_list(&self) -> Vec<(String, String)> {
        let reg = self.env.hooks.registry.read();
        let mut list = Vec::new();
        for &event in &["precmd", "preexec", "chpwd"] {
            if let Some(fns) = reg.get(event) {
                for f in fns {
                    list.push((event.to_string(), f.clone()));
                }
            }
        }
        list
    }

    pub fn filtered_vars(&self) -> Vec<(String, String, String)> {
        let vars = self.env.vars.read();
        let mut res: Vec<(String, String, String)> = Vec::new();

        for (k, v) in vars.iter() {
            let (val_str, kind) = match v {
                Val::String(s) => (s.clone(), "String".into()),
                Val::Int(i) => (i.to_string(), "Int".into()),
                Val::Float(f) => (f.to_string(), "Float".into()),
                Val::Bool(b) => (b.to_string(), "Bool".into()),
                Val::List(l) => (format!("[list of {} items]", l.len()), "List".into()),
                Val::Map(m) => (format!("{{map of {} entries}}", m.len()), "Map".into()),
                _ => (format!("{v:?}"), "Other".into()),
            };
            res.push((k.to_string(), val_str, kind));
        }
        drop(vars);

        res.sort_by(|a, b| a.0.cmp(&b.0));

        if !self.search_query.is_empty() {
            let q = self.search_query.to_lowercase();
            res.retain(|(k, v, kind)| {
                k.to_lowercase().contains(&q)
                    || v.to_lowercase().contains(&q)
                    || kind.to_lowercase().contains(&q)
            });
        }
        res
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.modal != ModalType::None {
            self.handle_modal_key(key);
            return;
        }

        if matches!(self.focus, Focus::Search) {
            self.handle_search_key(key);
            return;
        }

        self.handle_normal_key(key);
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                if self.dirty {
                    self.modal = ModalType::ConfirmDiscard {
                        title: "Unsaved Changes".into(),
                        message: "Save changes to init.fsh before exiting?".into(),
                    };
                } else {
                    self.running = false;
                }
            }
            KeyCode::Char('?') => {
                self.modal = ModalType::Help;
            }
            KeyCode::Char('/') => {
                self.focus = Focus::Search;
                self.search_query.clear();
            }
            KeyCode::Char('s') => {
                self.save_and_persist();
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Content,
                    _ => Focus::Sidebar,
                };
                self.content_selected = 0;
            }
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h')
                if matches!(self.focus, Focus::Content) =>
            {
                self.focus = Focus::Sidebar;
            }
            KeyCode::Right | KeyCode::Char('l') if matches!(self.focus, Focus::Sidebar) => {
                self.focus = Focus::Content;
                self.content_selected = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => match self.focus {
                Focus::Sidebar => {
                    if self.sidebar_selected < self.categories.len() - 1 {
                        self.sidebar_selected += 1;
                        self.content_selected = 0;
                        self.scroll_offset = 0;
                        self.search_query.clear();
                    }
                }
                Focus::Content => {
                    let max_idx = self.current_category_item_count().saturating_sub(1);
                    if self.content_selected < max_idx {
                        self.content_selected += 1;
                    }
                }
                _ => {}
            },
            KeyCode::Up | KeyCode::Char('k') => match self.focus {
                Focus::Sidebar => {
                    if self.sidebar_selected > 0 {
                        self.sidebar_selected -= 1;
                        self.content_selected = 0;
                        self.scroll_offset = 0;
                        self.search_query.clear();
                    }
                }
                Focus::Content if self.content_selected > 0 => {
                    self.content_selected -= 1;
                }
                _ => {}
            },
            KeyCode::Home => {
                if matches!(self.focus, Focus::Content) {
                    self.content_selected = 0;
                    self.scroll_offset = 0;
                }
            }
            KeyCode::End => {
                if matches!(self.focus, Focus::Content) {
                    let max_idx = self.current_category_item_count().saturating_sub(1);
                    self.content_selected = max_idx;
                }
            }
            KeyCode::Char(' ') => {
                self.handle_space();
            }
            KeyCode::Enter => {
                if matches!(self.focus, Focus::Sidebar) {
                    self.focus = Focus::Content;
                    self.content_selected = 0;
                } else if matches!(self.focus, Focus::Content) {
                    self.handle_enter();
                }
            }
            KeyCode::Char('e') if matches!(self.focus, Focus::Content) => {
                self.handle_edit();
            }
            KeyCode::Char('a') if matches!(self.focus, Focus::Content) => {
                self.handle_add();
            }
            KeyCode::Char('d') if matches!(self.focus, Focus::Content) => {
                self.handle_delete();
            }
            KeyCode::Char('p') => {
                if matches!(self.current_category(), Category::Prompt) {
                    self.launch_prompt_studio = true;
                    self.running = false;
                }
            }
            _ => {}
        }
    }

    fn current_category_item_count(&self) -> usize {
        match self.current_category() {
            Category::ShellOptions => self.filtered_option_indices().len(),
            Category::Themes => self.themes.len(),
            Category::Aliases => self.filtered_aliases().len(),
            Category::Hooks => self.flat_hooks_list().len(),
            Category::EnvVars => self.filtered_vars().len(),
            Category::Prompt => 1,
        }
    }

    fn handle_space(&mut self) {
        match self.current_category() {
            Category::ShellOptions => {
                let filtered = self.filtered_option_indices();
                if let Some(&opt_idx) = filtered.get(self.content_selected) {
                    let opt = &mut self.options[opt_idx];
                    match opt.cycle(self.env) {
                        Ok(msg) => {
                            self.dirty = true;
                            self.status_message = Some((msg, false));
                        }
                        Err(err) => {
                            self.status_message = Some((err, true));
                        }
                    }
                }
            }
            Category::Themes => {
                self.apply_selected_theme();
            }
            _ => {}
        }
    }

    fn handle_enter(&mut self) {
        match self.current_category() {
            Category::ShellOptions => {
                let filtered = self.filtered_option_indices();
                if let Some(&opt_idx) = filtered.get(self.content_selected) {
                    let opt = &self.options[opt_idx];
                    match &opt.kind {
                        OptionKind::Bool(_)
                        | OptionKind::Choice { .. }
                        | OptionKind::Theme { .. }
                        | OptionKind::Keybinding { .. } => {
                            self.handle_space();
                        }
                        OptionKind::Integer {
                            current,
                            min,
                            max,
                            unit,
                            examples,
                            higher_meaning,
                            lower_meaning,
                        } => {
                            let details = vec![
                                format!("Range: [{} .. {}] {}", min, max, unit),
                                format!("Examples: {}", examples.join("  |  ")),
                                format!("[+] Higher: {}", higher_meaning),
                                format!("[-] Lower:  {}", lower_meaning),
                            ];
                            self.modal = ModalType::TextInput {
                                title: format!("Edit {}", opt.label),
                                label: format!(
                                    "Enter numeric value for {} (in {}):",
                                    opt.key, unit
                                ),
                                value: current.to_string(),
                                details,
                                cursor: current.to_string().len(),
                                target_opt_key: Some(opt.key.to_string()),
                                error: None,
                            };
                        }
                    }
                }
            }
            Category::Themes => {
                self.apply_selected_theme();
            }
            Category::Aliases => {
                self.handle_edit();
            }
            Category::Hooks => {
                self.handle_edit();
            }
            Category::Prompt => {
                self.launch_prompt_studio = true;
                self.running = false;
            }
            Category::EnvVars => {}
        }
    }

    fn apply_selected_theme(&mut self) {
        if let Some(theme_name) = self.themes.get(self.content_selected).cloned() {
            let config_dir =
                fshell_engine::config_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
            match Theme::load(&theme_name, &config_dir) {
                Ok(theme) => {
                    self.env.set_theme(Arc::new(theme));
                    let mut opts = self.env.options.write();
                    opts.theme = theme_name.clone();
                    drop(opts);
                    let _ = self.env.sync_options_map();
                    self.dirty = true;
                    self.status_message = Some((format!("Applied theme: {theme_name}"), false));
                }
                Err(e) => {
                    self.status_message = Some((format!("Error loading theme: {e}"), true));
                }
            }
        }
    }

    fn handle_edit(&mut self) {
        match self.current_category() {
            Category::Aliases => {
                let aliases = self.filtered_aliases();
                if let Some((name, exp)) = aliases.get(self.content_selected) {
                    self.modal = ModalType::TwoFieldAlias {
                        title: "Edit Alias".into(),
                        name: name.clone(),
                        expansion: exp.clone(),
                        active_field: 1, // Focus expansion directly for fast editing
                        cursor: exp.len(),
                        is_editing: true,
                        old_name: Some(name.clone()),
                        error: None,
                    };
                }
            }
            Category::ShellOptions => {
                self.handle_enter();
            }
            _ => {}
        }
    }

    fn handle_add(&mut self) {
        match self.current_category() {
            Category::Aliases => {
                self.modal = ModalType::TwoFieldAlias {
                    title: "Add New Alias".into(),
                    name: String::new(),
                    expansion: String::new(),
                    active_field: 0,
                    cursor: 0,
                    is_editing: false,
                    old_name: None,
                    error: None,
                };
            }
            Category::Hooks => {
                self.modal = ModalType::TwoFieldHook {
                    title: "Add Hook".into(),
                    event: "precmd".into(),
                    fn_name: String::new(),
                    active_field: 1,
                    cursor: 0,
                    error: None,
                };
            }
            _ => {}
        }
    }

    fn handle_delete(&mut self) {
        match self.current_category() {
            Category::Aliases => {
                let aliases = self.filtered_aliases();
                if let Some((name, _)) = aliases.get(self.content_selected) {
                    let name_clone = name.clone();
                    let mut env_aliases = self.env.scope.aliases.write();
                    env_aliases.shift_remove(&name_clone);
                    drop(env_aliases);

                    self.pending_actions
                        .push(PendingAction::AliasDeleted(name_clone.clone()));
                    self.dirty = true;
                    self.status_message = Some((format!("Deleted alias: {name_clone}"), false));

                    if self.content_selected > 0
                        && self.content_selected >= self.filtered_aliases().len()
                    {
                        self.content_selected -= 1;
                    }
                }
            }
            Category::Hooks => {
                let hooks = self.flat_hooks_list();
                if let Some((event, fn_name)) = hooks.get(self.content_selected) {
                    let ev_clone = event.clone();
                    let fn_clone = fn_name.clone();

                    let mut reg = self.env.hooks.registry.write();
                    if let Some(list) = reg.get_mut(&ev_clone) {
                        list.retain(|f| f != &fn_clone);
                    }
                    drop(reg);

                    self.pending_actions.push(PendingAction::HookDeleted(
                        ev_clone.clone(),
                        fn_clone.clone(),
                    ));
                    self.dirty = true;
                    self.status_message =
                        Some((format!("Deleted hook: {ev_clone} -> {fn_clone}"), false));

                    if self.content_selected > 0
                        && self.content_selected >= self.flat_hooks_list().len()
                    {
                        self.content_selected -= 1;
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.focus = Focus::Content;
                self.search_query.clear();
                self.content_selected = 0;
            }
            KeyCode::Enter => {
                self.focus = Focus::Content;
                self.content_selected = 0;
            }
            KeyCode::Backspace => {
                self.search_query.pop();
                self.content_selected = 0;
            }
            KeyCode::Char(c) => {
                self.search_query.push(c);
                self.content_selected = 0;
            }
            _ => {}
        }
    }

    fn handle_modal_key(&mut self, key: KeyEvent) {
        match &mut self.modal {
            ModalType::Help => {
                self.modal = ModalType::None;
            }
            ModalType::ConfirmDiscard { .. } => {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                        self.save_and_persist();
                        self.running = false;
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') => {
                        // Revert theme if changed
                        let config_dir = fshell_engine::config_dir()
                            .unwrap_or_else(|| std::path::PathBuf::from("."));
                        if let Ok(theme) = Theme::load(&self.original_theme, &config_dir) {
                            self.env.set_theme(Arc::new(theme));
                        }
                        self.running = false;
                    }
                    KeyCode::Esc => {
                        self.modal = ModalType::None;
                    }
                    _ => {}
                }
            }
            ModalType::TextInput {
                value,
                cursor,
                target_opt_key,
                error,
                ..
            } => {
                match key.code {
                    KeyCode::Esc => {
                        self.modal = ModalType::None;
                    }
                    KeyCode::Enter => {
                        if let Some(key_name) = target_opt_key.take() {
                            let val_str = value.clone();
                            // Validate & apply
                            let opt_item = self.options.iter().find(|o| o.key == key_name);
                            if let Some(opt) = opt_item {
                                match opt.apply_value(self.env, &val_str) {
                                    Ok(()) => {
                                        // Reload options state
                                        self.options = OptionItem::load_all(self.env);
                                        self.dirty = true;
                                        self.status_message =
                                            Some((format!("Set {key_name} = {val_str}"), false));
                                        self.modal = ModalType::None;
                                    }
                                    Err(e) => {
                                        *error = Some(e);
                                    }
                                }
                            }
                        }
                    }
                    KeyCode::Char(c) => {
                        value.insert(*cursor, c);
                        *cursor += 1;
                        *error = None;
                    }
                    KeyCode::Backspace if *cursor > 0 => {
                        *cursor -= 1;
                        value.remove(*cursor);
                        *error = None;
                    }
                    KeyCode::Delete if *cursor < value.len() => {
                        value.remove(*cursor);
                        *error = None;
                    }
                    KeyCode::Left => {
                        *cursor = cursor.saturating_sub(1);
                    }
                    KeyCode::Right if *cursor < value.len() => {
                        *cursor += 1;
                    }
                    _ => {}
                }
            }
            ModalType::TwoFieldAlias {
                name,
                expansion,
                active_field,
                cursor,
                old_name,
                error,
                ..
            } => match key.code {
                KeyCode::Esc => {
                    self.modal = ModalType::None;
                }
                KeyCode::Tab | KeyCode::Down | KeyCode::Up => {
                    *active_field = if *active_field == 0 { 1 } else { 0 };
                    *cursor = if *active_field == 0 {
                        name.len()
                    } else {
                        expansion.len()
                    };
                }
                KeyCode::Enter => {
                    if name.trim().is_empty() {
                        *error = Some("Alias name cannot be empty".into());
                        return;
                    }
                    if expansion.trim().is_empty() {
                        *error = Some("Expansion cannot be empty".into());
                        return;
                    }

                    let clean_name = name.trim().to_string();
                    let clean_exp = expansion.trim().to_string();

                    let mut aliases = self.env.scope.aliases.write();
                    if let Some(old) = old_name.take()
                        && old != clean_name
                    {
                        aliases.shift_remove(&old);
                        self.pending_actions.push(PendingAction::AliasDeleted(old));
                    }
                    aliases.insert(clean_name.clone(), clean_exp.clone());
                    drop(aliases);

                    self.pending_actions.push(PendingAction::AliasAdded(
                        clean_name.clone(),
                        clean_exp.clone(),
                    ));
                    self.dirty = true;
                    self.status_message = Some((format!("Saved alias: {clean_name}"), false));
                    self.modal = ModalType::None;
                }
                KeyCode::Char(c) => {
                    let target_str = if *active_field == 0 { name } else { expansion };
                    target_str.insert(*cursor, c);
                    *cursor += 1;
                    *error = None;
                }
                KeyCode::Backspace if *cursor > 0 => {
                    let target_str = if *active_field == 0 { name } else { expansion };
                    *cursor -= 1;
                    target_str.remove(*cursor);
                    *error = None;
                }
                KeyCode::Delete => {
                    let target_str = if *active_field == 0 { name } else { expansion };
                    if *cursor < target_str.len() {
                        target_str.remove(*cursor);
                        *error = None;
                    }
                }
                KeyCode::Left => {
                    *cursor = cursor.saturating_sub(1);
                }
                KeyCode::Right => {
                    let len = if *active_field == 0 {
                        name.len()
                    } else {
                        expansion.len()
                    };
                    if *cursor < len {
                        *cursor += 1;
                    }
                }
                _ => {}
            },
            ModalType::TwoFieldHook {
                event,
                fn_name,
                active_field,
                cursor,
                error,
                ..
            } => {
                match key.code {
                    KeyCode::Esc => {
                        self.modal = ModalType::None;
                    }
                    KeyCode::Tab => {
                        if *active_field == 0 {
                            // Cycle event
                            *event = match event.as_str() {
                                "precmd" => "preexec".into(),
                                "preexec" => "chpwd".into(),
                                _ => "precmd".into(),
                            };
                        } else {
                            *active_field = 0;
                        }
                    }
                    KeyCode::Enter => {
                        if fn_name.trim().is_empty() {
                            *error = Some("Function name cannot be empty".into());
                            return;
                        }

                        let ev = event.trim().to_string();
                        let func = fn_name.trim().to_string();

                        let mut reg = self.env.hooks.registry.write();
                        reg.entry(ev.clone()).or_default().push(func.clone());
                        drop(reg);

                        self.pending_actions
                            .push(PendingAction::HookAdded(ev.clone(), func.clone()));
                        self.dirty = true;
                        self.status_message = Some((format!("Added hook: {ev} -> {func}"), false));
                        self.modal = ModalType::None;
                    }
                    KeyCode::Char(c) => {
                        if *active_field == 1 {
                            fn_name.insert(*cursor, c);
                            *cursor += 1;
                            *error = None;
                        }
                    }
                    KeyCode::Backspace if *active_field == 1 && *cursor > 0 => {
                        *cursor -= 1;
                        fn_name.remove(*cursor);
                        *error = None;
                    }
                    _ => {}
                }
            }
            ModalType::None => {}
        }
    }

    pub fn save_and_persist(&mut self) {
        let opts = self.env.options.read().clone();
        let prompt_val = self
            .env
            .vars
            .read()
            .get("FSH_PROMPT")
            .and_then(|v| match v {
                Val::String(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default();

        let prompt_right_val = self
            .env
            .vars
            .read()
            .get("FSH_PROMPT_RIGHT")
            .and_then(|v| match v {
                Val::String(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default();

        let keybinding_val = self
            .env
            .vars
            .read()
            .get("FSH_KEYBINDING_MODE")
            .and_then(|v| match v {
                Val::String(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "emacs".into());

        let snapshot = fshell_engine::config::SettingsSnapshot {
            autocd: opts.autocd,
            autopushd: opts.autopushd,
            cdable_vars: opts.cdable_vars,
            quiet_aliases: opts.quiet_aliases,
            pipefail: opts.pipefail,
            errexit: opts.errexit,
            nounset: opts.nounset,
            nullglob: opts.nullglob,
            nocaseglob: opts.nocaseglob,
            noclobber: opts.noclobber,
            noexec: opts.noexec,
            xtrace: opts.xtrace,
            verbose: opts.verbose,
            ignoreeof: opts.ignoreeof,
            histignoredups: opts.histignoredups,
            did_you_mean: opts.did_you_mean,
            notify: opts.notify,
            json_auto_parse: opts.json_auto_parse,
            sandbox_mode: opts.sandbox_mode.clone(),
            clear_on_reload: opts.clear_on_reload.clone(),
            session_restore: opts.session_restore.clone(),
            theme: opts.theme.clone(),
            pipeline_channel_size: opts.pipeline_channel_size,
            prompt: prompt_val,
            prompt_right: prompt_right_val,
            keybinding: keybinding_val,
            disabled_builtins: opts.disabled_builtins.clone(),
            command_binaries: opts.command_binaries.clone(),
            confirm_destructive: opts.confirm_destructive,
            sandbox_all: opts.sandbox_all,
        };

        let set_lines = fshell_engine::config::collect_settings_lines(&snapshot);
        if let Err(e) = fshell_engine::config::update_managed_settings(&set_lines) {
            self.status_message = Some((format!("Failed to save settings: {e}"), true));
            return;
        }

        // Persist aliases and hooks
        for act in &self.pending_actions {
            match act {
                PendingAction::AliasAdded(name, exp) => {
                    let _ = fshell_engine::config::persist_alias(name, exp);
                }
                PendingAction::AliasDeleted(name) => {
                    let _ = fshell_engine::config::remove_alias(name);
                }
                PendingAction::HookAdded(event, fn_name) => {
                    let _ = fshell_engine::config::persist_hook(event, fn_name);
                }
                PendingAction::HookDeleted(event, fn_name) => {
                    let _ = fshell_engine::config::remove_hook(event, fn_name);
                }
                _ => {}
            }
        }

        self.pending_actions.clear();
        self.dirty = false;
        self.original_theme = opts.theme;
        self.status_message = Some(("All changes saved to ~/.config/fsh/init.fsh".into(), false));
    }
}

pub fn run(env: &Env) -> Result<(), String> {
    enable_raw_mode().map_err(|e| format!("Failed to enable raw mode: {e}"))?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)
        .map_err(|e| format!("Failed to enter alternate screen: {e}"))?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal =
        Terminal::new(backend).map_err(|e| format!("Failed to initialize terminal: {e}"))?;

    let mut app = App::new(env);
    let res = run_loop(&mut terminal, &mut app);

    // Teardown
    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    res?;

    if app.launch_prompt_studio {
        return crate::prompt_customizer::run_prompt_customizer(env);
    }

    Ok(())
}

fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<(), String> {
    while app.running {
        terminal
            .draw(|f| {
                let size = f.area();
                let theme = app.env.active_theme();

                let root_layout = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1), // Top Header Bar
                        Constraint::Min(10),   // Main Viewport
                        Constraint::Length(1), // Bottom Status/Keybindings Bar
                    ])
                    .split(size);

                // 1. Header Bar
                let header_title = Span::styled(
                    " fsh config ── Shell Configuration & Customization",
                    theme.widgets.title.to_style().add_modifier(Modifier::BOLD),
                );
                let header_hints = Span::styled(
                    "[?] Help  [s] Save  [q] Quit ",
                    theme.status.muted.to_style(),
                );
                let header_line = Line::from(vec![header_title, Span::raw("  "), header_hints]);
                Paragraph::new(header_line).render(root_layout[0], f.buffer_mut());

                // 2. Main Viewport (Sidebar 25% | Content 75%)
                let main_layout = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(24), Constraint::Percentage(76)])
                    .split(root_layout[1]);

                widgets::sidebar::render(main_layout[0], f.buffer_mut(), app);

                match app.current_category() {
                    Category::ShellOptions => {
                        widgets::shell_options::render(main_layout[1], f.buffer_mut(), app)
                    }
                    Category::Themes => {
                        widgets::theme_browser::render(main_layout[1], f.buffer_mut(), app)
                    }
                    Category::Aliases => {
                        widgets::aliases::render(main_layout[1], f.buffer_mut(), app)
                    }
                    Category::Hooks => widgets::hooks::render(main_layout[1], f.buffer_mut(), app),
                    Category::EnvVars => {
                        widgets::env_vars::render(main_layout[1], f.buffer_mut(), app)
                    }
                    Category::Prompt => {
                        widgets::prompt_config::render(main_layout[1], f.buffer_mut(), app)
                    }
                }

                // 3. Bottom Status Bar
                let status_line = if let Some((msg, is_err)) = &app.status_message {
                    let st = if *is_err {
                        theme.status.error.to_style().add_modifier(Modifier::BOLD)
                    } else {
                        theme.status.ok.to_style().add_modifier(Modifier::BOLD)
                    };
                    Line::from(vec![Span::styled(format!("  {msg}"), st)])
                } else if matches!(app.focus, Focus::Search) {
                    Line::from(vec![
                        Span::styled(
                            "  Search: ",
                            theme.status.info.to_style().add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            app.search_query.as_str(),
                            Style::default().add_modifier(Modifier::UNDERLINED),
                        ),
                        Span::styled(
                            " [Enter to lock, Esc to clear]",
                            theme.status.muted.to_style(),
                        ),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled("  [Tab]", theme.status.info.to_style()),
                        Span::raw(" Switch Pane  "),
                        Span::styled("[Space/Enter]", theme.status.info.to_style()),
                        Span::raw(" Toggle/Cycle  "),
                        Span::styled("[e]", theme.status.info.to_style()),
                        Span::raw(" Edit  "),
                        Span::styled("[/]", theme.status.info.to_style()),
                        Span::raw(" Search  "),
                        Span::styled("[s]", theme.status.info.to_style()),
                        Span::raw(" Save  "),
                        Span::styled("[?]", theme.status.info.to_style()),
                        Span::raw(" Help"),
                    ])
                };

                Paragraph::new(status_line).render(root_layout[2], f.buffer_mut());

                // 4. Modals / Overlays
                match &app.modal {
                    ModalType::TextInput {
                        title,
                        label,
                        value,
                        details,
                        error,
                        ..
                    } => {
                        widgets::modal::render_text_input(
                            size,
                            f.buffer_mut(),
                            &theme,
                            widgets::modal::TextInputProps {
                                title,
                                label,
                                value,
                                details: if details.is_empty() {
                                    None
                                } else {
                                    Some(details.as_slice())
                                },
                                error: error.as_deref(),
                            },
                        );
                    }
                    ModalType::TwoFieldAlias {
                        title,
                        name,
                        expansion,
                        active_field,
                        error,
                        ..
                    } => {
                        widgets::modal::render_two_field_modal(
                            size,
                            f.buffer_mut(),
                            &theme,
                            widgets::modal::TwoFieldModalProps {
                                title,
                                field1_label: "Alias Name",
                                field1_val: name,
                                field2_label: "Expansion",
                                field2_val: expansion,
                                active_field: *active_field,
                                error: error.as_deref(),
                            },
                        );
                    }
                    ModalType::TwoFieldHook {
                        title,
                        event,
                        fn_name,
                        active_field,
                        error,
                        ..
                    } => {
                        widgets::modal::render_two_field_modal(
                            size,
                            f.buffer_mut(),
                            &theme,
                            widgets::modal::TwoFieldModalProps {
                                title,
                                field1_label: "Event (Tab: precmd/preexec/chpwd)",
                                field1_val: event,
                                field2_label: "Handler Function Name",
                                field2_val: fn_name,
                                active_field: *active_field,
                                error: error.as_deref(),
                            },
                        );
                    }
                    ModalType::ConfirmDiscard { title, message } => {
                        widgets::modal::render_confirm_dialog(
                            size,
                            f.buffer_mut(),
                            &theme,
                            title,
                            message,
                        );
                    }
                    ModalType::Help => {
                        widgets::modal::render_help_modal(size, f.buffer_mut(), &theme);
                    }
                    ModalType::None => {}
                }
            })
            .map_err(|e| format!("Failed to draw frame: {e}"))?;

        if event::poll(std::time::Duration::from_millis(50))
            .map_err(|e| format!("Poll failed: {e}"))?
        {
            match event::read().map_err(|e| format!("Read failed: {e}"))? {
                Event::Key(key) => {
                    app.handle_key(key);
                }
                Event::Resize(_, _) => {
                    // Handled automatically on next draw loop iteration
                }
                _ => {}
            }
        }
    }

    Ok(())
}
