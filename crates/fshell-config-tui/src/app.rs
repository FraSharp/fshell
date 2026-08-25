// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::theme_ext::ThemeColorRatatui;
use crate::widgets;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use fshell_engine::Env;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use std::io;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    ShellOptions,
    Prompt,
    Aliases,
    Hooks,
    EnvVars,
}

impl Category {
    pub fn all() -> &'static [Category] {
        &[
            Category::ShellOptions,
            Category::Prompt,
            Category::Aliases,
            Category::Hooks,
            Category::EnvVars,
        ]
    }

    pub fn label(&self) -> &str {
        match self {
            Category::ShellOptions => "Shell Options",
            Category::Prompt => "Prompt",
            Category::Aliases => "Aliases",
            Category::Hooks => "Hooks",
            Category::EnvVars => "Env Vars",
        }
    }

    pub fn is_read_only(&self) -> bool {
        matches!(self, Category::EnvVars)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Content,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
    None,
    Editing,
    ConfirmSave,
}

#[derive(Debug, Clone)]
pub enum PendingAction {
    ToggleBool(String),
    SetValue(String, String),
    AddAlias(String, String),
    DeleteAlias(String),
    AddHook(String, String),
}

pub struct App<'a> {
    pub env: &'a Env,
    pub categories: Vec<Category>,
    pub focus: Focus,
    pub sidebar_selected: usize,
    pub content_selected: usize,
    pub content_item_count: usize,
    pub running: bool,
    pub dirty: bool,
    pub input_mode: InputMode,
    pub input_buffer: String,
    pub input_cursor: usize,
    pub input_label: String,
    pub pending_actions: Vec<PendingAction>,
    pub message: Option<String>,
}

impl<'a> App<'a> {
    pub fn new(env: &'a Env) -> Self {
        Self {
            env,
            categories: Category::all().to_vec(),
            focus: Focus::Sidebar,
            sidebar_selected: 0,
            content_selected: 0,
            content_item_count: 0,
            running: true,
            dirty: false,
            input_mode: InputMode::None,
            input_buffer: String::new(),
            input_cursor: 0,
            input_label: String::new(),
            pending_actions: Vec::new(),
            message: None,
        }
    }

    pub fn current_category(&self) -> Category {
        self.categories[self.sidebar_selected]
    }

    pub fn enter_input(&mut self, label: &str, initial: &str) {
        self.input_mode = InputMode::Editing;
        self.input_buffer = initial.to_string();
        self.input_cursor = initial.len();
        self.input_label = label.to_string();
    }

    pub fn confirm_input(&mut self) -> String {
        self.input_mode = InputMode::None;
        std::mem::take(&mut self.input_buffer)
    }

    pub fn cancel_input(&mut self) {
        self.input_mode = InputMode::None;
        self.input_buffer.clear();
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        match &self.input_mode {
            InputMode::Editing => self.handle_input_key(key),
            InputMode::ConfirmSave => self.handle_confirm_key(key),
            InputMode::None => self.handle_normal_key(key),
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => {
                if self.dirty {
                    self.input_mode = InputMode::ConfirmSave;
                    self.input_buffer = "y".to_string();
                } else {
                    self.running = false;
                }
            }
            KeyCode::Esc => {
                if self.dirty {
                    self.input_mode = InputMode::ConfirmSave;
                    self.input_buffer = "y".to_string();
                } else {
                    self.running = false;
                }
            }
            KeyCode::Tab => match self.focus {
                Focus::Sidebar => {
                    self.focus = Focus::Content;
                    self.content_selected = 0;
                }
                Focus::Content => {
                    self.focus = Focus::Sidebar;
                }
            },
            KeyCode::Left | KeyCode::Char('h') => {
                if matches!(self.focus, Focus::Content) {
                    self.focus = Focus::Sidebar;
                }
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if matches!(self.focus, Focus::Sidebar) {
                    self.focus = Focus::Content;
                    self.content_selected = 0;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => match self.focus {
                Focus::Sidebar => {
                    if self.sidebar_selected < self.categories.len() - 1 {
                        self.sidebar_selected += 1;
                        self.content_selected = 0;
                    }
                }
                Focus::Content => {
                    if self.content_selected < self.content_item_count.saturating_sub(1) {
                        self.content_selected += 1;
                    }
                }
            },
            KeyCode::Up | KeyCode::Char('k') => match self.focus {
                Focus::Sidebar => {
                    if self.sidebar_selected > 0 {
                        self.sidebar_selected -= 1;
                        self.content_selected = 0;
                    }
                }
                Focus::Content => {
                    if self.content_selected > 0 {
                        self.content_selected -= 1;
                    }
                }
            },
            KeyCode::Enter => {
                if matches!(self.focus, Focus::Content) {
                    self.handle_enter();
                } else if matches!(self.focus, Focus::Sidebar) {
                    self.focus = Focus::Content;
                    self.content_selected = 0;
                }
            }
            KeyCode::Char('a') if matches!(self.focus, Focus::Content) => {
                self.handle_add();
            }
            KeyCode::Char('d') if matches!(self.focus, Focus::Content) => {
                self.handle_delete();
            }
            _ => {}
        }
    }

    fn handle_enter(&mut self) {
        let cat = self.current_category();
        match cat {
            Category::ShellOptions => {
                let opts = self.env.options.read();
                let bool_names = Self::bool_option_names();
                if self.content_selected < bool_names.len() {
                    let name = &bool_names[self.content_selected];
                    let current = opts.get_bool(name).unwrap_or(false);
                    drop(opts);
                    self.pending_actions
                        .push(PendingAction::ToggleBool(name.to_string()));
                    // Apply immediately to env
                    {
                        let mut opts = self.env.options.write();
                        let _ = opts.set_bool(name, !current);
                    }
                    self.dirty = true;
                    self.message = Some(format!("Toggled {} = {}", name, !current));
                } else {
                    let value_idx = self.content_selected - bool_names.len();
                    let value_names = [
                        "sandbox_mode",
                        "pipeline_channel_size",
                        "clear_on_reload",
                        "error_format",
                        "suggestion_mode",
                        "notify_threshold",
                        "stderr_max_bytes",
                        "sort_max_items",
                    ];
                    if value_idx < value_names.len() {
                        let name = value_names[value_idx];
                        let current = match name {
                            "sandbox_mode" => opts.sandbox_mode.clone(),
                            "pipeline_channel_size" => opts.pipeline_channel_size.to_string(),
                            "clear_on_reload" => opts.clear_on_reload.clone(),
                            "error_format" => match opts.error_format {
                                fshell_render::RenderFormat::Auto => "auto".to_string(),
                                fshell_render::RenderFormat::Graphical => "graphical".to_string(),
                                fshell_render::RenderFormat::Compact => "compact".to_string(),
                                fshell_render::RenderFormat::Explain => "explain".to_string(),
                                fshell_render::RenderFormat::Json => "json".to_string(),
                            },
                            "suggestion_mode" => match opts.suggestion_mode {
                                fshell_engine::SuggestionMode::Blocking => "blocking".to_string(),
                                fshell_engine::SuggestionMode::Deferred => "deferred".to_string(),
                            },
                            "notify_threshold" => opts.notify_threshold.to_string(),
                            "stderr_max_bytes" => opts.stderr_max_bytes.to_string(),
                            "sort_max_items" => opts.sort_max_items.to_string(),
                            _ => String::new(),
                        };
                        drop(opts);
                        self.enter_input(&format!("{}:", name), &current);
                    }
                }
            }
            Category::Aliases | Category::Hooks => {
                // Aliases/Hooks editing handled by 'a' (add) and 'd' (delete)
                // Enter on an item could edit it
            }
            Category::Prompt | Category::EnvVars => {
                // Read-only
            }
        }
    }

    fn handle_add(&mut self) {
        let cat = self.current_category();
        match cat {
            Category::Aliases => {
                self.enter_input("alias name:", "");
                // After name is entered, we'll need to ask for expansion
                // For now, use format "name = expansion"
            }
            Category::Hooks => {
                self.enter_input("hook (event fn_name):", "");
            }
            _ => {}
        }
    }

    fn handle_delete(&mut self) {
        let cat = self.current_category();
        match cat {
            Category::Aliases => {
                let aliases = self.env.scope.aliases.read();
                if self.content_selected < aliases.len() {
                    let name = aliases.keys().nth(self.content_selected).cloned();
                    drop(aliases);
                    if let Some(name) = name {
                        self.pending_actions
                            .push(PendingAction::DeleteAlias(name.clone()));
                        self.dirty = true;
                        self.message = Some(format!("Deleted alias: {}", name));
                    }
                }
            }
            Category::Hooks => {
                // Hooks deletion would need hook registry API
            }
            _ => {}
        }
    }

    fn handle_input_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                let result = self.confirm_input();
                self.apply_input_result(&result);
            }
            KeyCode::Esc => {
                self.cancel_input();
            }
            KeyCode::Char(c) => {
                self.input_buffer.insert(self.input_cursor, c);
                self.input_cursor += 1;
            }
            KeyCode::Backspace if self.input_cursor > 0 => {
                self.input_cursor -= 1;
                self.input_buffer.remove(self.input_cursor);
            }
            KeyCode::Delete if self.input_cursor < self.input_buffer.len() => {
                self.input_buffer.remove(self.input_cursor);
            }
            KeyCode::Left => {
                self.input_cursor = self.input_cursor.saturating_sub(1);
            }
            KeyCode::Right if self.input_cursor < self.input_buffer.len() => {
                self.input_cursor += 1;
            }
            KeyCode::Home => {
                self.input_cursor = 0;
            }
            KeyCode::End => {
                self.input_cursor = self.input_buffer.len();
            }
            _ => {}
        }
    }

    fn apply_input_result(&mut self, result: &str) {
        let cat = self.current_category();
        match cat {
            Category::ShellOptions => {
                // Parsing value option input
                let value_names = [
                    "sandbox_mode",
                    "pipeline_channel_size",
                    "clear_on_reload",
                    "error_format",
                    "suggestion_mode",
                    "notify_threshold",
                    "stderr_max_bytes",
                    "sort_max_items",
                ];
                let bool_names = Self::bool_option_names();
                let value_idx = self.content_selected - bool_names.len();
                if value_idx < value_names.len() {
                    let name = value_names[value_idx];
                    match name {
                        "sandbox_mode" => {
                            let valid = ["prompt", "deny-all", "monitor", "off"];
                            if valid.contains(&result) {
                                self.pending_actions.push(PendingAction::SetValue(
                                    name.to_string(),
                                    result.to_string(),
                                ));
                                let mut opts = self.env.options.write();
                                opts.sandbox_mode = result.to_string();
                                self.dirty = true;
                                self.message = Some(format!("Set {} = {}", name, result));
                            } else {
                                self.message = Some(format!(
                                    "Invalid value. Must be one of: {}",
                                    valid.join(", ")
                                ));
                            }
                        }
                        "clear_on_reload" => {
                            let valid = ["ask", "always", "never"];
                            if valid.contains(&result) {
                                self.pending_actions.push(PendingAction::SetValue(
                                    name.to_string(),
                                    result.to_string(),
                                ));
                                let mut opts = self.env.options.write();
                                opts.clear_on_reload = result.to_string();
                                self.dirty = true;
                                self.message = Some(format!("Set {} = {}", name, result));
                            } else {
                                self.message = Some(format!(
                                    "Invalid value. Must be one of: {}",
                                    valid.join(", ")
                                ));
                            }
                        }
                        "error_format" => {
                            let valid = ["graphical", "compact", "json"];
                            if valid.contains(&result) {
                                self.pending_actions.push(PendingAction::SetValue(
                                    name.to_string(),
                                    result.to_string(),
                                ));
                                let mut opts = self.env.options.write();
                                opts.error_format = match result {
                                    "compact" => fshell_render::RenderFormat::Compact,
                                    "json" => fshell_render::RenderFormat::Json,
                                    _ => fshell_render::RenderFormat::Graphical,
                                };
                                self.dirty = true;
                                self.message = Some(format!("Set {} = {}", name, result));
                            } else {
                                self.message = Some(format!(
                                    "Invalid value. Must be one of: {}",
                                    valid.join(", ")
                                ));
                            }
                        }
                        "suggestion_mode" => {
                            let valid = ["blocking", "deferred"];
                            if valid.contains(&result) {
                                self.pending_actions.push(PendingAction::SetValue(
                                    name.to_string(),
                                    result.to_string(),
                                ));
                                let mut opts = self.env.options.write();
                                opts.suggestion_mode = match result {
                                    "blocking" => fshell_engine::SuggestionMode::Blocking,
                                    _ => fshell_engine::SuggestionMode::Deferred,
                                };
                                self.dirty = true;
                                self.message = Some(format!("Set {} = {}", name, result));
                            } else {
                                self.message = Some(format!(
                                    "Invalid value. Must be one of: {}",
                                    valid.join(", ")
                                ));
                            }
                        }
                        "pipeline_channel_size" => {
                            if let Ok(v) = result.parse::<usize>() {
                                self.pending_actions.push(PendingAction::SetValue(
                                    name.to_string(),
                                    result.to_string(),
                                ));
                                let mut opts = self.env.options.write();
                                opts.pipeline_channel_size = v;
                                self.dirty = true;
                                self.message = Some(format!("Set {} = {}", name, result));
                            } else {
                                self.message = Some("Invalid number".into());
                            }
                        }
                        "notify_threshold" => {
                            if let Ok(v) = result.parse::<u64>() {
                                self.pending_actions.push(PendingAction::SetValue(
                                    name.to_string(),
                                    result.to_string(),
                                ));
                                let mut opts = self.env.options.write();
                                opts.notify_threshold = v;
                                self.dirty = true;
                                self.message = Some(format!("Set {} = {}", name, result));
                            } else {
                                self.message = Some("Invalid number".into());
                            }
                        }
                        "stderr_max_bytes" => {
                            if let Ok(v) = result.parse::<usize>() {
                                self.pending_actions.push(PendingAction::SetValue(
                                    name.to_string(),
                                    result.to_string(),
                                ));
                                let mut opts = self.env.options.write();
                                opts.stderr_max_bytes = v;
                                self.dirty = true;
                                self.message = Some(format!("Set {} = {}", name, result));
                            } else {
                                self.message = Some("Invalid number".into());
                            }
                        }
                        "sort_max_items" => {
                            if let Ok(v) = result.parse::<usize>() {
                                self.pending_actions.push(PendingAction::SetValue(
                                    name.to_string(),
                                    result.to_string(),
                                ));
                                let mut opts = self.env.options.write();
                                opts.sort_max_items = v;
                                self.dirty = true;
                                self.message = Some(format!("Set {} = {}", name, result));
                            } else {
                                self.message = Some("Invalid number".into());
                            }
                        }
                        _ => {}
                    }
                }
            }
            Category::Aliases => {
                // Input format: "name = expansion" or just "name" for delete hint
                if let Some((name, expansion)) = result.split_once('=') {
                    let name = name.trim().to_string();
                    let expansion = expansion.trim().to_string();
                    if !name.is_empty() && !expansion.is_empty() {
                        self.pending_actions
                            .push(PendingAction::AddAlias(name, expansion));
                        self.dirty = true;
                        self.message = Some("Alias added".into());
                    }
                } else {
                    self.message = Some("Format: name = expansion".into());
                }
            }
            Category::Hooks => {
                // Input format: "event fn_name"
                let parts: Vec<&str> = result.split_whitespace().collect();
                if parts.len() == 2 {
                    let event = parts[0].to_string();
                    let fn_name = parts[1].to_string();
                    self.pending_actions
                        .push(PendingAction::AddHook(event, fn_name));
                    self.dirty = true;
                    self.message = Some("Hook added".into());
                } else {
                    self.message = Some("Format: event fn_name (e.g. precmd my_fn)".into());
                }
            }
            _ => {}
        }
    }

    fn handle_confirm_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                self.persist_all();
                self.running = false;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.running = false;
            }
            _ => {}
        }
    }

    fn persist_all(&mut self) {
        // Persist shell options to init.fsh via managed block
        if let Err(e) = self.persist_shell_options() {
            self.message = Some(format!("Failed to persist options: {}", e));
        }
        // Persist aliases and hooks
        for action in &self.pending_actions {
            match action {
                PendingAction::AddAlias(name, expansion) => {
                    let _ = fshell_engine::config::persist_alias(name, expansion);
                }
                PendingAction::DeleteAlias(name) => {
                    let _ = fshell_engine::config::remove_alias(name);
                }
                PendingAction::AddHook(event, fn_name) => {
                    let _ = fshell_engine::config::persist_hook(event, fn_name);
                }
                _ => {}
            }
        }
    }

    fn persist_shell_options(&self) -> Result<(), String> {
        let opts = self.env.options.read();
        let vars = self.env.vars.read();
        let prompt = match vars.get("FSH_PROMPT") {
            Some(fshell_core::Val::String(s)) => s.clone(),
            _ => String::new(),
        };
        let prompt_right = match vars.get("FSH_PROMPT_RIGHT") {
            Some(fshell_core::Val::String(s)) => s.clone(),
            _ => String::new(),
        };
        let keybinding = match vars.get("FSH_KEYBINDING_MODE") {
            Some(fshell_core::Val::String(s)) => s.clone(),
            _ => String::new(),
        };
        drop(vars);
        let snapshot = fshell_engine::config::SettingsSnapshot {
            autocd: opts.autocd,
            pipefail: opts.pipefail,
            notify: opts.notify,
            json_auto_parse: opts.json_auto_parse,
            did_you_mean: opts.did_you_mean,
            sandbox_mode: opts.sandbox_mode.clone(),
            pipeline_channel_size: opts.pipeline_channel_size,
            prompt,
            prompt_right,
            keybinding,
            errexit: opts.errexit,
            nounset: opts.nounset,
            nullglob: opts.nullglob,
            nocaseglob: opts.nocaseglob,
            noclobber: opts.noclobber,
            noexec: opts.noexec,
            xtrace: opts.xtrace,
            verbose: opts.verbose,
            ignoreeof: opts.ignoreeof,
            autopushd: opts.autopushd,
            histignoredups: opts.histignoredups,
            cdable_vars: opts.cdable_vars,
            quiet_aliases: opts.quiet_aliases,
            clear_on_reload: opts.clear_on_reload.clone(),
            session_restore: opts.session_restore.clone(),
            theme: opts.theme.clone(),
            disabled_builtins: opts.disabled_builtins.clone(),
            command_binaries: opts.command_binaries.clone(),
            confirm_destructive: opts.confirm_destructive,
            sandbox_all: opts.sandbox_all,
        };
        drop(opts);

        let lines = fshell_engine::config::collect_settings_lines(&snapshot);
        fshell_engine::config::update_managed_settings(&lines)
            .map_err(|e| format!("Failed to persist settings: {}", e))
    }

    pub fn bool_option_names() -> Vec<&'static str> {
        vec![
            "autocd",
            "pipefail",
            "notify",
            "json_auto_parse",
            "error_color",
            "did_you_mean",
            "errexit",
            "nounset",
            "nullglob",
            "nocaseglob",
            "noclobber",
            "noexec",
            "xtrace",
            "verbose",
            "ignoreeof",
            "autopushd",
            "histignoredups",
            "cdable_vars",
            "quiet_aliases",
        ]
    }
}

pub fn run(env: &Env) -> Result<(), String> {
    enable_raw_mode().map_err(|e| e.to_string())?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(|e| e.to_string())?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;

    let mut app = App::new(env);

    while app.running {
        terminal
            .draw(|f| {
                let chunks = Layout::default()
                    .direction(ratatui::layout::Direction::Horizontal)
                    .constraints([Constraint::Length(22), Constraint::Min(40)])
                    .split(f.area());

                widgets::sidebar::render(chunks[0], f.buffer_mut(), &app);

                let category = app.current_category();
                match category {
                    Category::ShellOptions => {
                        let count = widgets::shell_options::item_count(&app);
                        app.content_item_count = count;
                        widgets::shell_options::render(chunks[1], f.buffer_mut(), &app);
                    }
                    Category::Prompt => {
                        app.content_item_count = 0;
                        widgets::prompt_config::render(chunks[1], f.buffer_mut(), &app);
                    }
                    Category::Aliases => {
                        let count = widgets::aliases::item_count(&app);
                        app.content_item_count = count;
                        widgets::aliases::render(chunks[1], f.buffer_mut(), &app);
                    }
                    Category::Hooks => {
                        let count = widgets::hooks::item_count(&app);
                        app.content_item_count = count;
                        widgets::hooks::render(chunks[1], f.buffer_mut(), &app);
                    }
                    Category::EnvVars => {
                        app.content_item_count = 0;
                        widgets::env_vars::render(chunks[1], f.buffer_mut(), &app);
                    }
                }

                // Bottom bar
                let bottom = Layout::default()
                    .direction(ratatui::layout::Direction::Vertical)
                    .constraints([Constraint::Min(0), Constraint::Length(1)])
                    .split(f.area());

                let theme = app.env.active_theme();
                let hints = match &app.input_mode {
                    InputMode::Editing => Line::from(vec![
                        Span::styled(
                            " Enter Confirm ",
                            theme
                                .widgets
                                .item_selected_fg
                                .to_style()
                                .bg(theme.status.ok.to_ratatui_color()),
                        ),
                        Span::raw(" "),
                        Span::styled(
                            " Esc Cancel ",
                            theme
                                .widgets
                                .item_selected_fg
                                .to_style()
                                .bg(theme.status.muted.to_ratatui_color()),
                        ),
                    ]),
                    InputMode::ConfirmSave => Line::from(vec![Span::styled(
                        " Save changes? (y/n) ",
                        theme
                            .widgets
                            .item_selected_fg
                            .to_style()
                            .bg(theme.status.warning.to_ratatui_color()),
                    )]),
                    InputMode::None => {
                        let mut spans = vec![
                            Span::styled(
                                " q Quit ",
                                theme
                                    .widgets
                                    .item_selected_fg
                                    .to_style()
                                    .bg(theme.status.muted.to_ratatui_color()),
                            ),
                            Span::raw(" "),
                            Span::styled(
                                " Tab/h/l Switch ",
                                theme
                                    .widgets
                                    .item_selected_fg
                                    .to_style()
                                    .bg(theme.status.muted.to_ratatui_color()),
                            ),
                            Span::raw(" "),
                            Span::styled(
                                " j/k Nav ",
                                theme
                                    .widgets
                                    .item_selected_fg
                                    .to_style()
                                    .bg(theme.status.muted.to_ratatui_color()),
                            ),
                            Span::raw(" "),
                            Span::styled(
                                " Enter Edit ",
                                theme
                                    .widgets
                                    .item_selected_fg
                                    .to_style()
                                    .bg(theme.status.muted.to_ratatui_color()),
                            ),
                        ];
                        if matches!(app.focus, Focus::Content) {
                            let cat = app.current_category();
                            if !cat.is_read_only() {
                                spans.push(Span::raw(" "));
                                spans.push(Span::styled(
                                    " a Add ",
                                    theme
                                        .widgets
                                        .item_selected_fg
                                        .to_style()
                                        .bg(theme.status.info.to_ratatui_color()),
                                ));
                                spans.push(Span::raw(" "));
                                spans.push(Span::styled(
                                    " d Delete ",
                                    theme
                                        .widgets
                                        .item_selected_fg
                                        .to_style()
                                        .bg(theme.status.error.to_ratatui_color()),
                                ));
                            }
                        }
                        Line::from(spans)
                    }
                };

                f.render_widget(Paragraph::new(hints), bottom[1]);

                // Input popup
                if matches!(app.input_mode, InputMode::Editing) {
                    let popup_area = ratatui::layout::Rect {
                        x: chunks[1].x + 2,
                        y: chunks[1].y + chunks[1].height.saturating_sub(5),
                        width: chunks[1].width.saturating_sub(4).min(60),
                        height: 3,
                    };
                    f.render_widget(Clear, popup_area);
                    let input_text = format!("{} {}", app.input_label, app.input_buffer);
                    let input_widget = Block::default()
                        .borders(Borders::ALL)
                        .style(theme.status.muted.to_style());
                    let inner = input_widget.inner(popup_area);
                    f.render_widget(input_widget, popup_area);
                    f.render_widget(Paragraph::new(input_text), inner);
                    // Show cursor
                    f.set_cursor_position((
                        inner.x + app.input_label.len() as u16 + 1 + app.input_cursor as u16,
                        inner.y,
                    ));
                }

                // Message bar
                if let Some(ref msg) = app.message {
                    let msg_area = ratatui::layout::Rect {
                        x: chunks[1].x + 2,
                        y: chunks[1].y,
                        width: chunks[1].width.saturating_sub(4).min(60),
                        height: 1,
                    };
                    f.render_widget(
                        Paragraph::new(Span::styled(msg.as_str(), theme.widgets.title.to_style())),
                        msg_area,
                    );
                }
            })
            .map_err(|e| e.to_string())?;

        // Clear message after render
        app.message = None;

        if event::poll(std::time::Duration::from_millis(100)).map_err(|e| e.to_string())?
            && let Event::Key(key) = event::read().map_err(|e| e.to_string())?
        {
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                if app.dirty {
                    app.input_mode = InputMode::ConfirmSave;
                    app.input_buffer = "y".to_string();
                } else {
                    app.running = false;
                }
            } else {
                app.handle_key(key);
            }
        }
    }

    disable_raw_mode().map_err(|e| e.to_string())?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen).map_err(|e| e.to_string())?;
    terminal.show_cursor().map_err(|e| e.to_string())?;

    Ok(())
}
