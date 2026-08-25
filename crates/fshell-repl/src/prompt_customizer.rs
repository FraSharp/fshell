// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::prompt::{get_rich_git_status, render_segment_list_to_ratatui_lines};
use crate::prompt_config;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use fshell_core::prompt_config::{
    ColorSpec, PromptConfig, SegmentConfig, SegmentType, SeparatorStyle,
};
use fshell_core::theme::Theme;
use fshell_engine::Env;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Row, Table,
    },
};
use std::io::{self, IsTerminal};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusPane {
    SegmentList,
    Inspector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptSide {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StudioMode {
    Studio,
    AddSegment,
    ChangeType,
    ColorPicker,
    PresetPicker,
    ThemePicker,
    ConfirmDelete,
    ConfirmReset,
    ConfirmQuit,
    ConfirmPresetMerge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InspectorField {
    Type,
    Prefix,
    Suffix,
    Text,
    Fg,
    Bg,
    SeparatorStyle,
    Bold,
    Italic,
    Shorten,
    HideOnZero,
    HideWhenClean,
    ShowOnlyInRepo,
}

impl InspectorField {
    fn label(&self) -> &'static str {
        match self {
            Self::Type => "Segment Type",
            Self::Prefix => "Prefix",
            Self::Suffix => "Suffix",
            Self::Text => "Custom Text",
            Self::Fg => "Foreground",
            Self::Bg => "Background",
            Self::SeparatorStyle => "Powerline Style",
            Self::Bold => "Bold Font",
            Self::Italic => "Italic Font",
            Self::Shorten => "Shorten PWD",
            Self::HideOnZero => "Hide on Success",
            Self::HideWhenClean => "Hide on Clean Git",
            Self::ShowOnlyInRepo => "Repo Only",
        }
    }

    fn description(&self) -> &'static str {
        match self {
            Self::Type => "Built-in data generator or literal widget (Enter to change)",
            Self::Prefix => "String rendered immediately before content (type to edit)",
            Self::Suffix => "String rendered immediately after content (type to edit)",
            Self::Text => "Fixed custom label or command text (type to edit)",
            Self::Fg => "Text foreground color or theme role (Enter to open palette)",
            Self::Bg => "Segment background fill color or theme role (Enter to open palette)",
            Self::SeparatorStyle => {
                "Powerline glyph connecting to adjacent segments (Enter/Space to cycle)"
            }
            Self::Bold => "Render segment text with bold typography (Enter/Space to toggle)",
            Self::Italic => "Render segment text with italic typography (Enter/Space to toggle)",
            Self::Shorten => {
                "Truncate parent directories (~/d/f instead of ~/dev/fshell) (Enter/Space to toggle)"
            }
            Self::HideOnZero => {
                "Only render exit code when previous command failed (Enter/Space to toggle)"
            }
            Self::HideWhenClean => {
                "Hide git dirty counter when repository has no changes (Enter/Space to toggle)"
            }
            Self::ShowOnlyInRepo => {
                "Hide segment when outside of a git repository (Enter/Space to toggle)"
            }
        }
    }

    fn from_name(name: &str) -> Option<InspectorField> {
        match name {
            "type" => Some(InspectorField::Type),
            "prefix" => Some(InspectorField::Prefix),
            "suffix" => Some(InspectorField::Suffix),
            "text" => Some(InspectorField::Text),
            "fg" => Some(InspectorField::Fg),
            "bg" => Some(InspectorField::Bg),
            "separator_style" => Some(InspectorField::SeparatorStyle),
            "bold" => Some(InspectorField::Bold),
            "italic" => Some(InspectorField::Italic),
            "shorten" => Some(InspectorField::Shorten),
            "hide_on_zero" => Some(InspectorField::HideOnZero),
            "hide_when_clean" => Some(InspectorField::HideWhenClean),
            "show_only_in_repo" => Some(InspectorField::ShowOnlyInRepo),
            _ => None,
        }
    }
}

fn fields_for_segment(seg: &SegmentConfig, show_advanced: bool) -> Vec<InspectorField> {
    let allowed = seg.r#type.fields_for_type();
    let all_fields: Vec<InspectorField> = allowed
        .iter()
        .filter_map(|name| InspectorField::from_name(name))
        .collect();

    if show_advanced {
        return all_fields;
    }

    const ESSENTIAL: &[InspectorField] = &[
        InspectorField::Type,
        InspectorField::Fg,
        InspectorField::Bg,
        InspectorField::SeparatorStyle,
        InspectorField::Prefix,
        InspectorField::Suffix,
        InspectorField::Bold,
    ];

    all_fields
        .into_iter()
        .filter(|f| ESSENTIAL.contains(f))
        .collect()
}

pub fn segment_badge(st: &SegmentType) -> &'static str {
    match st {
        SegmentType::User => "(usr)",
        SegmentType::Host => "(host)",
        SegmentType::Pwd => "(pwd)",
        SegmentType::GitBranch => "(git)",
        SegmentType::GitStatus => "(vcs)",
        SegmentType::ExitCode => "(exit)",
        SegmentType::Duration => "(time)",
        SegmentType::Jobs => "(jobs)",
        SegmentType::Char => "(char)",
        SegmentType::Time => "(clock)",
        SegmentType::Date => "(date)",
        SegmentType::Timestamp => "(ts)",
        SegmentType::Shlvl => "(shlvl)",
        SegmentType::Shell => "(shell)",
        SegmentType::Line => "(line)",
        SegmentType::Aws => "(aws)",
        SegmentType::Kube => "(kube)",
        SegmentType::Venv => "(venv)",
        SegmentType::Ssh => "(ssh)",
        SegmentType::CargoRun => "(cargo)",
        SegmentType::Text => "(text)",
        SegmentType::Separator => "(sep)",
        SegmentType::Newline => "(break)",
        SegmentType::Custom => "(custom)",
    }
}

pub const SEMANTIC_PALETTE: &[(&str, &str, &str)] = &[
    (
        "ok",
        "Success / Clean",
        "Green accent for zero exit code and clean git state",
    ),
    (
        "error",
        "Error / Dirty",
        "Red accent for non-zero exit code and modified files",
    ),
    (
        "warning",
        "Warning",
        "Amber accent for pending jobs and alert badges",
    ),
    (
        "info",
        "Information",
        "Sky blue accent for general contextual information",
    ),
    (
        "muted",
        "Muted / Dim",
        "Subtle foreground for secondary annotations",
    ),
    (
        "keyword",
        "Keyword Accent",
        "Primary brand color from active syntax theme",
    ),
    (
        "builtin",
        "Builtin Accent",
        "Secondary syntax color for shell builtins",
    ),
    (
        "string",
        "String / Path",
        "Vibrant accent for filesystem and paths",
    ),
    ("user", "User Segment", "Accent color for username badge"),
    ("host", "Host Segment", "Accent color for hostname badge"),
    (
        "pwd",
        "Directory Segment",
        "Accent color for directory path segment",
    ),
    ("git_branch", "Git Branch", "Branch name badge accent"),
    (
        "duration",
        "Duration Accent",
        "Elapsed command time badge accent",
    ),
    (
        "job_count",
        "Jobs Accent",
        "Background jobs indicator accent",
    ),
    (
        "prompt_symbol",
        "Prompt Symbol",
        "Main prompt character indicator",
    ),
    ("badge_cargo", "Cargo Accent", "Cargo project runner badge"),
];

pub const STANDARD_ANSI_PALETTE: &[(&str, Color)] = &[
    ("black", Color::Rgb(0, 0, 0)),
    ("red", Color::Rgb(205, 49, 49)),
    ("green", Color::Rgb(13, 188, 121)),
    ("yellow", Color::Rgb(229, 229, 16)),
    ("blue", Color::Rgb(36, 114, 200)),
    ("magenta", Color::Rgb(188, 63, 188)),
    ("cyan", Color::Rgb(17, 168, 205)),
    ("white", Color::Rgb(229, 229, 229)),
    ("darkgray", Color::Rgb(102, 102, 102)),
    ("lightred", Color::Rgb(241, 76, 76)),
    ("lightgreen", Color::Rgb(35, 209, 139)),
    ("lightyellow", Color::Rgb(245, 245, 67)),
    ("lightblue", Color::Rgb(59, 142, 234)),
    ("lightmagenta", Color::Rgb(214, 112, 214)),
    ("lightcyan", Color::Rgb(41, 184, 219)),
    ("lightgray", Color::Rgb(204, 204, 204)),
];

struct App {
    env: Env,
    config: PromptConfig,
    left_segments: Vec<SegmentConfig>,
    right_segments: Vec<SegmentConfig>,
    side: PromptSide,
    selected_segment: usize,
    pane: FocusPane,
    mode: StudioMode,
    inspector_field_idx: usize,
    modal_index: usize,
    modal_filter: String,
    color_target_bg: bool,
    color_category: usize, // 0: Semantic, 1: ANSI, 2: Hex
    hex_buffer: String,
    dirty: bool,
    status_toast: Option<(String, bool)>, // (message, is_error)
    show_advanced: bool,
    undo_stack: Vec<(Vec<SegmentConfig>, Vec<SegmentConfig>)>,
    redo_stack: Vec<(Vec<SegmentConfig>, Vec<SegmentConfig>)>,
    pending_preset: Option<String>,
}

impl App {
    fn new(env: Env) -> Self {
        let config = env.prompt_config.read().clone();
        Self {
            env,
            left_segments: config.left.clone(),
            right_segments: config.right.clone(),
            config: config.clone(),
            side: PromptSide::Left,
            selected_segment: 0,
            pane: FocusPane::SegmentList,
            mode: StudioMode::Studio,
            inspector_field_idx: 0,
            modal_index: 0,
            modal_filter: String::new(),
            color_target_bg: false,
            color_category: 0,
            hex_buffer: String::new(),
            dirty: false,
            status_toast: None,
            show_advanced: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            pending_preset: None,
        }
    }

    fn cur_segments(&self) -> &Vec<SegmentConfig> {
        match self.side {
            PromptSide::Left => &self.left_segments,
            PromptSide::Right => &self.right_segments,
        }
    }

    fn cur_segments_mut(&mut self) -> &mut Vec<SegmentConfig> {
        match self.side {
            PromptSide::Left => &mut self.left_segments,
            PromptSide::Right => &mut self.right_segments,
        }
    }

    fn cur_len(&self) -> usize {
        self.cur_segments().len()
    }

    fn clamp_selection(&mut self) {
        let len = self.cur_len();
        if len == 0 {
            self.selected_segment = 0;
        } else if self.selected_segment >= len {
            self.selected_segment = len - 1;
        }
    }

    fn push_undo(&mut self) {
        self.undo_stack
            .push((self.left_segments.clone(), self.right_segments.clone()));
        if self.undo_stack.len() > 50 {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    fn undo(&mut self) {
        if let Some((left, right)) = self.undo_stack.pop() {
            self.redo_stack
                .push((self.left_segments.clone(), self.right_segments.clone()));
            self.left_segments = left;
            self.right_segments = right;
            self.clamp_selection();
            self.dirty = true;
            self.status_toast = Some(("Reverted last action (Undo)".into(), false));
        }
    }

    fn redo(&mut self) {
        if let Some((left, right)) = self.redo_stack.pop() {
            self.undo_stack
                .push((self.left_segments.clone(), self.right_segments.clone()));
            self.left_segments = left;
            self.right_segments = right;
            self.clamp_selection();
            self.dirty = true;
            self.status_toast = Some(("Re-applied action (Redo)".into(), false));
        }
    }

    fn move_selected_up(&mut self) {
        if self.selected_segment > 0 && self.cur_len() > 1 {
            self.push_undo();
            let idx = self.selected_segment;
            self.cur_segments_mut().swap(idx, idx - 1);
            self.selected_segment -= 1;
            self.dirty = true;
            self.status_toast = Some(("Moved segment earlier in prompt".into(), false));
        }
    }

    fn move_selected_down(&mut self) {
        let len = self.cur_len();
        if len > 1 && self.selected_segment + 1 < len {
            self.push_undo();
            let idx = self.selected_segment;
            self.cur_segments_mut().swap(idx, idx + 1);
            self.selected_segment += 1;
            self.dirty = true;
            self.status_toast = Some(("Moved segment later in prompt".into(), false));
        }
    }
}

struct TerminalGuard;

impl TerminalGuard {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

fn spec_display(spec: &Option<ColorSpec>) -> String {
    match spec {
        Some(ColorSpec::Named(n)) => n.clone(),
        Some(ColorSpec::Hex(h)) => {
            if h.starts_with('#') {
                h.clone()
            } else {
                format!("#{}", h)
            }
        }
        Some(ColorSpec::Conditional { ok, err }) => format!("{}|{}", ok, err),
        None => "default".to_string(),
    }
}

fn segment_type_from_name(name: &str) -> Option<SegmentType> {
    SegmentType::all()
        .into_iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, _, _)| match name {
            "user" => SegmentType::User,
            "host" => SegmentType::Host,
            "pwd" => SegmentType::Pwd,
            "git_branch" => SegmentType::GitBranch,
            "git_status" => SegmentType::GitStatus,
            "exit_code" => SegmentType::ExitCode,
            "duration" => SegmentType::Duration,
            "jobs" => SegmentType::Jobs,
            "char" => SegmentType::Char,
            "time" => SegmentType::Time,
            "date" => SegmentType::Date,
            "timestamp" => SegmentType::Timestamp,
            "shlvl" => SegmentType::Shlvl,
            "shell" => SegmentType::Shell,
            "line" => SegmentType::Line,
            "aws" => SegmentType::Aws,
            "kube" => SegmentType::Kube,
            "venv" => SegmentType::Venv,
            "ssh" => SegmentType::Ssh,
            "cargo_run" => SegmentType::CargoRun,
            "text" => SegmentType::Text,
            "separator" => SegmentType::Separator,
            "newline" => SegmentType::Newline,
            "custom" => SegmentType::Custom,
            _ => SegmentType::Text,
        })
}

fn next_separator(current: &SeparatorStyle) -> SeparatorStyle {
    match current {
        SeparatorStyle::None => SeparatorStyle::Arrow,
        SeparatorStyle::Arrow => SeparatorStyle::Chevron,
        SeparatorStyle::Chevron => SeparatorStyle::Flame,
        SeparatorStyle::Flame => SeparatorStyle::Pipe,
        SeparatorStyle::Pipe => SeparatorStyle::Slash,
        SeparatorStyle::Slash => SeparatorStyle::Dots,
        SeparatorStyle::Dots => SeparatorStyle::Custom(" ".into()),
        SeparatorStyle::Custom(_) => SeparatorStyle::None,
    }
}

pub fn run_prompt_customizer(env: &Env) -> Result<(), String> {
    if fshell_engine::is_test_mode() {
        return Err("not a terminal (test mode)".to_string());
    }
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        return Err("not a terminal".to_string());
    }

    let _guard = TerminalGuard::new().map_err(|e| format!("terminal setup: {}", e))?;
    let mut stdout = io::stdout();
    let backend = CrosstermBackend::new(&mut stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| format!("terminal: {}", e))?;

    let mut app = App::new(env.clone());

    loop {
        let ok = terminal.draw(|f| draw_studio(f, &app)).is_ok();
        if !ok {
            break;
        }

        match handle_studio_input(&mut app) {
            Ok(true) => {}
            _ => break,
        }
    }

    Ok(())
}

fn draw_studio(f: &mut ratatui::Frame, app: &App) {
    let area = f.area();
    let theme = app.env.active_theme();
    let (tr, tg, tb) = theme.widgets.title.to_rgb();
    let (br, bg, bb) = theme.widgets.border.to_rgb();
    let (kr, kg, kb) = theme.syntax.keyword.to_rgb();

    let max_w = area.width.saturating_sub(2).max(80);
    let max_h = area.height.saturating_sub(2).max(24);
    let x = area.width.saturating_sub(max_w) / 2;
    let y = area.height.saturating_sub(max_h) / 2;
    let container = Rect::new(x, y, max_w, max_h);

    let mut title_spans = vec![
        Span::styled(
            " :: ",
            Style::default()
                .fg(Color::Rgb(kr, kg, kb))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "fshell Prompt & Theme Studio",
            Style::default()
                .fg(Color::Rgb(tr, tg, tb))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" [Theme: {}] ", theme.name),
            Style::default().fg(Color::DarkGray),
        ),
    ];

    if let Some((ref msg, is_err)) = app.status_toast {
        let toast_style = if is_err {
            Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::LightGreen)
                .add_modifier(Modifier::BOLD)
        };
        title_spans.push(Span::styled(format!(" -- {} ", msg), toast_style));
    } else if app.dirty {
        title_spans.push(Span::styled(
            " -- [Modified, press 's' to save] ",
            Style::default().fg(Color::Yellow),
        ));
    }

    let main_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(br, bg, bb)))
        .title(Line::from(title_spans));

    f.render_widget(Clear, container);
    f.render_widget(main_block, container);

    let inner = Rect {
        x: container.x + 1,
        y: container.y + 1,
        width: container.width.saturating_sub(2),
        height: container.height.saturating_sub(2),
    };

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5), // Top Live Canvas
            Constraint::Min(12),   // Dual Pane Workspace
            Constraint::Length(1), // Footer / Help Bar
        ])
        .split(inner);

    draw_live_canvas(f, layout[0], app);
    draw_workspace(f, layout[1], app);
    draw_footer_bar(f, layout[2], app);

    // Overlays / Modals
    match app.mode {
        StudioMode::AddSegment => draw_catalog_modal(f, container, app),
        StudioMode::ChangeType => draw_catalog_modal(f, container, app),
        StudioMode::ColorPicker => draw_color_studio_modal(f, container, app),
        StudioMode::PresetPicker => draw_preset_modal(f, container, app),
        StudioMode::ThemePicker => draw_theme_modal(f, container, app),
        StudioMode::ConfirmDelete | StudioMode::ConfirmReset | StudioMode::ConfirmQuit => {
            draw_confirm_modal(f, container, app)
        }
        StudioMode::ConfirmPresetMerge => draw_preset_merge_modal(f, container, app),
        StudioMode::Studio => {}
    }
}

fn draw_live_canvas(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let theme = app.env.active_theme();
    let (br, bg, bb) = theme.widgets.border.to_rgb();
    let pwd = std::env::var("PWD").unwrap_or_else(|_| "~/dev/fshell".to_string());
    let git = get_rich_git_status(&pwd);

    let preview_config = PromptConfig {
        left: app.left_segments.clone(),
        right: app.right_segments.clone(),
        separator_style: app.config.separator_style.clone(),
        ..PromptConfig::default()
    };

    let left_lines = render_segment_list_to_ratatui_lines(
        &app.left_segments,
        &preview_config,
        &pwd,
        &git,
        0,
        std::time::Duration::from_millis(42),
        0,
        true,
        &theme,
    );

    let right_lines = render_segment_list_to_ratatui_lines(
        &app.right_segments,
        &preview_config,
        &pwd,
        &git,
        0,
        std::time::Duration::from_millis(42),
        0,
        false,
        &theme,
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(br, bg, bb)))
        .title(Line::from(vec![
            Span::styled(
                " LIVE SHELL PROMPT ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " (Real-time AST Rendering) ",
                Style::default().fg(Color::DarkGray),
            ),
        ]));

    let canvas_inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    f.render_widget(block, area);

    let left_line = left_lines.into_iter().next().unwrap_or_default();
    let right_line = right_lines.into_iter().next().unwrap_or_default();

    let cmd_line = Line::from(vec![
        Span::styled(
            "╰─❯ ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("cargo test --all ", Style::default().fg(Color::White)),
        Span::styled(
            "_",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ]);

    let canvas_lines = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(canvas_inner);

    let split_top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(canvas_lines[0]);

    f.render_widget(Paragraph::new(left_line), split_top[0]);
    f.render_widget(
        Paragraph::new(right_line).alignment(Alignment::Right),
        split_top[1],
    );
    if canvas_lines.len() > 1 {
        f.render_widget(Paragraph::new(cmd_line), canvas_lines[1]);
    }
}

fn draw_workspace(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let workspace_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(45), // Left: Segment Pipeline Manager
            Constraint::Percentage(55), // Right: Interactive Inspector Card
        ])
        .split(area);

    draw_segment_pipeline_pane(f, workspace_cols[0], app);
    draw_inspector_card_pane(f, workspace_cols[1], app);
}

fn draw_segment_pipeline_pane(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let theme = app.env.active_theme();
    let (tr, tg, tb) = theme.widgets.title.to_rgb();
    let (br, bg, bb) = theme.widgets.border.to_rgb();
    let (sr, sg, sb) = theme.widgets.item_selected_bg.to_rgb();

    let is_focused = app.pane == FocusPane::SegmentList;
    let border_color = if is_focused {
        Color::Rgb(tr, tg, tb)
    } else {
        Color::Rgb(br, bg, bb)
    };

    let segments = app.cur_segments();
    let side_label = match app.side {
        PromptSide::Left => "Left Prompt Pipeline",
        PromptSide::Right => "Right Prompt Pipeline",
    };

    let title_line = Line::from(vec![
        Span::styled(
            format!(" {} ({}) ", side_label, segments.len()),
            Style::default()
                .fg(if is_focused {
                    Color::Rgb(tr, tg, tb)
                } else {
                    Color::White
                })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("[Tab: Switch L/R]", Style::default().fg(Color::DarkGray)),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(title_line);

    let inner_area = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    f.render_widget(block, area);

    // Segment items
    let mut list_items: Vec<ListItem> = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        let is_selected = i == app.selected_segment;
        let badge = segment_badge(&seg.r#type);
        let tag = if is_selected { "▸ " } else { "  " };

        let fg_desc = spec_display(&seg.fg);
        let (fr, fg, fb) = theme.resolve_color(&fg_desc);

        let spans = vec![
            Span::styled(
                tag,
                if is_selected {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                },
            ),
            Span::styled(
                format!("{:<8} ", badge),
                Style::default()
                    .fg(Color::Rgb(fr, fg, fb))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:<12}", seg.r#type.name()),
                if is_selected {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                },
            ),
            Span::styled(
                format!(" [fg:{} bg:{}]", fg_desc, spec_display(&seg.bg)),
                Style::default().fg(Color::DarkGray),
            ),
        ];

        list_items.push(ListItem::new(Line::from(spans)));
    }

    if list_items.is_empty() {
        list_items.push(ListItem::new(Line::from(Span::styled(
            " (Pipeline empty. Press 'a' to add segments) ",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ))));
    }

    let list = List::new(list_items).highlight_style(Style::default().bg(Color::Rgb(sr, sg, sb)));

    let mut state = ListState::default();
    if !segments.is_empty() {
        state.select(Some(app.selected_segment.min(segments.len() - 1)));
    }
    f.render_stateful_widget(list, inner_area, &mut state);
}

fn draw_inspector_card_pane(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let theme = app.env.active_theme();
    let (tr, tg, tb) = theme.widgets.title.to_rgb();
    let (br, bg, bb) = theme.widgets.border.to_rgb();

    let is_focused = app.pane == FocusPane::Inspector;
    let border_color = if is_focused {
        Color::Rgb(tr, tg, tb)
    } else {
        Color::Rgb(br, bg, bb)
    };

    let segments = app.cur_segments();
    let seg_opt = segments.get(app.selected_segment);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(vec![
            Span::styled(
                " Segment Properties Inspector ",
                Style::default()
                    .fg(if is_focused {
                        Color::Rgb(tr, tg, tb)
                    } else {
                        Color::White
                    })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if app.show_advanced {
                    "[All Fields - 'e' for essential]"
                } else {
                    "[Essential - 'e' for all]"
                },
                Style::default().fg(Color::DarkGray),
            ),
        ]));

    let inner_area = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    f.render_widget(block, area);

    let Some(seg) = seg_opt else {
        let msg = Paragraph::new(Line::from(Span::styled(
            " Select a segment from the left pipeline to inspect and edit its properties ",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )));
        f.render_widget(msg, inner_area);
        return;
    };

    let inspector_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(6),    // Table of properties
            Constraint::Length(2), // Context Description & Hint box
        ])
        .split(inner_area);

    let visible_fields = fields_for_segment(seg, app.show_advanced);
    let mut rows: Vec<Row> = Vec::new();

    for (i, field) in visible_fields.iter().enumerate() {
        let is_field_selected = is_focused && i == app.inspector_field_idx;
        let tag = if is_field_selected { "▸ " } else { "  " };

        let label_style = if is_field_selected {
            Style::default()
                .fg(Color::LightYellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let val_repr = field_value(seg, field);
        let is_bool = matches!(
            field,
            InspectorField::Bold
                | InspectorField::Italic
                | InspectorField::Shorten
                | InspectorField::HideOnZero
                | InspectorField::HideWhenClean
                | InspectorField::ShowOnlyInRepo
        );
        let is_color = matches!(field, InspectorField::Fg | InspectorField::Bg);

        let value_span = if is_bool {
            if val_repr == "+" {
                Span::styled(
                    " [ON] ",
                    Style::default()
                        .fg(Color::LightGreen)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(" [OFF] ", Style::default().fg(Color::DarkGray))
            }
        } else if is_color {
            let (r, g, b) = theme.resolve_color(&val_repr);
            Span::styled(
                format!(" ■ {} ", val_repr),
                Style::default()
                    .fg(Color::Rgb(r, g, b))
                    .add_modifier(Modifier::BOLD),
            )
        } else if *field == InspectorField::SeparatorStyle {
            let style = seg.separator_style.clone().unwrap_or(SeparatorStyle::None);
            Span::styled(
                format!(" {} ({:?}) ", style.glyph(), style),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(
                format!(" \"{}\" ", val_repr),
                if is_field_selected {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::LightCyan)
                },
            )
        };

        rows.push(Row::new(vec![
            Line::from(vec![
                Span::styled(tag, label_style),
                Span::styled(field.label(), label_style),
            ]),
            Line::from(vec![value_span]),
        ]));
    }

    let col_widths = [Constraint::Length(22), Constraint::Min(24)];
    let table = Table::new(rows, col_widths).column_spacing(2);
    f.render_widget(table, inspector_layout[0]);

    // Bottom description and hint box
    let cur_field_desc = if !visible_fields.is_empty() {
        let cur_idx = app.inspector_field_idx.min(visible_fields.len() - 1);
        visible_fields[cur_idx].description()
    } else {
        ""
    };

    let desc_lines = vec![Line::from(vec![
        Span::styled(
            " Description: ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(cur_field_desc, Style::default().fg(Color::White)),
    ])];
    f.render_widget(Paragraph::new(desc_lines), inspector_layout[1]);
}

fn draw_footer_bar(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let theme = app.env.active_theme();
    let (kr, kg, kb) = theme.syntax.keyword.to_rgb();

    let help_line = match app.pane {
        FocusPane::SegmentList => Line::from(vec![
            Span::styled(
                "Tab",
                Style::default()
                    .fg(Color::Rgb(kr, kg, kb))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": L/R   "),
            Span::styled(
                "Enter/->",
                Style::default()
                    .fg(Color::Rgb(kr, kg, kb))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Inspect   "),
            Span::styled(
                "a",
                Style::default()
                    .fg(Color::Rgb(kr, kg, kb))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Add   "),
            Span::styled(
                "d",
                Style::default()
                    .fg(Color::Rgb(kr, kg, kb))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Delete   "),
            Span::styled(
                "J/K",
                Style::default()
                    .fg(Color::Rgb(kr, kg, kb))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Reorder   "),
            Span::styled(
                "p",
                Style::default()
                    .fg(Color::Rgb(kr, kg, kb))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Presets   "),
            Span::styled(
                "t",
                Style::default()
                    .fg(Color::Rgb(kr, kg, kb))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Themes   "),
            Span::styled(
                "s",
                Style::default()
                    .fg(Color::Rgb(kr, kg, kb))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Save   "),
            Span::styled(
                "u",
                Style::default()
                    .fg(Color::Rgb(kr, kg, kb))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Undo   "),
            Span::styled(
                "q",
                Style::default()
                    .fg(Color::Rgb(kr, kg, kb))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Exit"),
        ]),
        FocusPane::Inspector => Line::from(vec![
            Span::styled(
                "Esc/<-",
                Style::default()
                    .fg(Color::Rgb(kr, kg, kb))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Back to Pipeline   "),
            Span::styled(
                "Enter/Space",
                Style::default()
                    .fg(Color::Rgb(kr, kg, kb))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Toggle / Edit   "),
            Span::styled(
                "e",
                Style::default()
                    .fg(Color::Rgb(kr, kg, kb))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Toggle All Fields   "),
            Span::styled(
                "s",
                Style::default()
                    .fg(Color::Rgb(kr, kg, kb))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Save Config"),
        ]),
    };

    f.render_widget(Paragraph::new(help_line), area);
}

fn draw_catalog_modal(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let pop_w = 80u16.min(area.width.saturating_sub(4));
    let pop_h = 24u16.min(area.height.saturating_sub(4));
    let pop_x = area.x + (area.width.saturating_sub(pop_w)) / 2;
    let pop_y = area.y + (area.height.saturating_sub(pop_h)) / 2;
    let pop_area = Rect::new(pop_x, pop_y, pop_w, pop_h);

    let all_types = SegmentType::all();
    let filtered: Vec<_> = all_types
        .iter()
        .filter(|(name, desc, _)| {
            let q = app.modal_filter.to_lowercase();
            q.is_empty() || name.contains(&q) || desc.to_lowercase().contains(&q)
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Line::from(vec![
            Span::styled(
                " [+] ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if app.mode == StudioMode::AddSegment {
                    "Add Segment to Pipeline"
                } else {
                    "Change Segment Type"
                },
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

    f.render_widget(Clear, pop_area);
    f.render_widget(block, pop_area);

    let inner = Rect {
        x: pop_area.x + 1,
        y: pop_area.y + 1,
        width: pop_area.width.saturating_sub(2),
        height: pop_area.height.saturating_sub(2),
    };

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(inner);

    let filter_bar = Line::from(vec![
        Span::styled(
            " Search: ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if app.modal_filter.is_empty() {
                "Type to filter catalog..."
            } else {
                &app.modal_filter
            },
            if app.modal_filter.is_empty() {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            },
        ),
    ]);
    f.render_widget(Paragraph::new(filter_bar), layout[0]);

    let mut items: Vec<ListItem> = Vec::new();
    for (name, desc, sample) in &filtered {
        let st = segment_type_from_name(name).unwrap_or(SegmentType::Text);
        let badge = segment_badge(&st);

        items.push(ListItem::new(Line::from(vec![
            Span::styled(
                format!(" {:<8} ", badge),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:<14}", name),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {:<30}", desc),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(format!(" [{}]", sample), Style::default().fg(Color::Yellow)),
        ])));
    }

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(40, 44, 52))
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut state = ListState::default();
    if !filtered.is_empty() {
        state.select(Some(app.modal_index.min(filtered.len() - 1)));
    }
    f.render_stateful_widget(list, layout[1], &mut state);

    let hint = Line::from(Span::styled(
        " Up/Down: Navigate   Enter: Choose   Esc: Cancel ",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    ));
    f.render_widget(Paragraph::new(hint).alignment(Alignment::Center), layout[2]);
}

fn draw_color_studio_modal(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let theme = app.env.active_theme();
    let pop_w = 78u16.min(area.width.saturating_sub(4));
    let pop_h = 22u16.min(area.height.saturating_sub(4));
    let pop_x = area.x + (area.width.saturating_sub(pop_w)) / 2;
    let pop_y = area.y + (area.height.saturating_sub(pop_h)) / 2;
    let pop_area = Rect::new(pop_x, pop_y, pop_w, pop_h);

    let target = if app.color_target_bg {
        "Background"
    } else {
        "Foreground"
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow))
        .title(Line::from(vec![
            Span::styled(
                " [Color] ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("Color Studio ({})", target),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

    f.render_widget(Clear, pop_area);
    f.render_widget(block, pop_area);

    let inner = Rect {
        x: pop_area.x + 1,
        y: pop_area.y + 1,
        width: pop_area.width.saturating_sub(2),
        height: pop_area.height.saturating_sub(2),
    };

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(12),
            Constraint::Length(1),
        ])
        .split(inner);

    // Tab buttons
    let tab_titles = [
        "1. Semantic Theme Roles",
        "2. Standard ANSI Colors",
        "3. Custom RGB Hex Code",
    ];
    let mut tab_spans = Vec::new();
    for (i, title) in tab_titles.iter().enumerate() {
        let is_tab_sel = i == app.color_category;
        if i > 0 {
            tab_spans.push(Span::raw("   "));
        }
        tab_spans.push(Span::styled(
            format!(" [{}] ", title),
            if is_tab_sel {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ));
    }
    f.render_widget(Paragraph::new(Line::from(tab_spans)), layout[0]);

    if app.color_category == 0 {
        // Semantic Theme Roles Grid
        let mut rows = Vec::new();
        let max_rows = SEMANTIC_PALETTE.len().div_ceil(2);
        for r in 0..max_rows {
            let mut spans = Vec::new();
            for c in 0..2 {
                let idx = r * 2 + c;
                if idx >= SEMANTIC_PALETTE.len() {
                    break;
                }
                let (role, name, _) = SEMANTIC_PALETTE[idx];
                let (cr, cg, cb) = theme.resolve_color(role);
                let is_sel = idx == app.modal_index;

                if c > 0 {
                    spans.push(Span::raw("   "));
                }
                spans.push(Span::styled(
                    if is_sel { "▸ " } else { "  " },
                    Style::default().fg(Color::Yellow),
                ));
                spans.push(Span::styled(
                    "■■",
                    Style::default().fg(Color::Rgb(cr, cg, cb)),
                ));
                spans.push(Span::styled(
                    format!(" {:<14}", role),
                    if is_sel {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    },
                ));
                spans.push(Span::styled(
                    format!("({})", name),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            rows.push(Line::from(spans));
        }
        f.render_widget(Paragraph::new(rows), layout[1]);
    } else if app.color_category == 1 {
        // ANSI 16 Grid
        let mut rows = Vec::new();
        let max_rows = STANDARD_ANSI_PALETTE.len().div_ceil(4);
        for r in 0..max_rows {
            let mut spans = Vec::new();
            for c in 0..4 {
                let idx = r * 4 + c;
                if idx >= STANDARD_ANSI_PALETTE.len() {
                    break;
                }
                let (name, color) = STANDARD_ANSI_PALETTE[idx];
                let is_sel = idx == app.modal_index;

                if c > 0 {
                    spans.push(Span::raw(" "));
                }
                spans.push(Span::styled(
                    if is_sel { "▸" } else { " " },
                    Style::default().fg(Color::Yellow),
                ));
                spans.push(Span::styled("■■", Style::default().fg(color)));
                spans.push(Span::styled(
                    format!(" {:<11}", name),
                    if is_sel {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    },
                ));
            }
            rows.push(Line::from(spans));
        }
        f.render_widget(Paragraph::new(rows), layout[1]);
    } else {
        // Custom Hex Input
        let mut hex_lines = vec![
            Line::from(vec![
                Span::styled(" Enter Hex (#RRGGBB): ", Style::default().fg(Color::Cyan)),
                Span::styled(
                    format!("#{}", app.hex_buffer),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(Span::raw("")),
        ];

        let hex_clean = app.hex_buffer.trim_start_matches('#');
        if hex_clean.len() == 6 && hex_clean.chars().all(|c| c.is_ascii_hexdigit()) {
            let r = u8::from_str_radix(&hex_clean[0..2], 16).unwrap_or(0);
            let g = u8::from_str_radix(&hex_clean[2..4], 16).unwrap_or(0);
            let b = u8::from_str_radix(&hex_clean[4..6], 16).unwrap_or(0);
            hex_lines.push(Line::from(vec![
                Span::styled(" Live Color Swatch: ", Style::default().fg(Color::DarkGray)),
                Span::styled("■■■■■■■■■■■■■■", Style::default().fg(Color::Rgb(r, g, b))),
                Span::styled(
                    format!(" (RGB: {}, {}, {})", r, g, b),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
        f.render_widget(Paragraph::new(hex_lines), layout[1]);
    }

    let footer_hints = Line::from(Span::styled(
        " Tab: Category   Up/Down/Left/Right: Select   Enter: Apply   c: Clear Color   Esc: Cancel ",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    ));
    f.render_widget(
        Paragraph::new(footer_hints).alignment(Alignment::Center),
        layout[2],
    );
}

fn draw_preset_modal(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let pop_w = 76u16.min(area.width.saturating_sub(4));
    let pop_h = 18u16.min(area.height.saturating_sub(4));
    let pop_x = area.x + (area.width.saturating_sub(pop_w)) / 2;
    let pop_y = area.y + (area.height.saturating_sub(pop_h)) / 2;
    let pop_area = Rect::new(pop_x, pop_y, pop_w, pop_h);

    let presets = fshell_core::presets::available();
    let mut items: Vec<ListItem> = Vec::new();

    for name in presets {
        let cfg = match fshell_core::presets::by_name(name) {
            Some(c) => c,
            None => continue,
        };
        let seg_names: Vec<&str> = cfg.left.iter().map(|s| s.r#type.name()).collect();
        let preview = seg_names.join(" > ");
        items.push(ListItem::new(Line::from(vec![
            Span::styled(
                format!(" {:<14}", name),
                Style::default()
                    .fg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("[{:?}]  {}", cfg.separator_style, preview),
                Style::default().fg(Color::DarkGray),
            ),
        ])));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            " Select Built-In Prompt Preset ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));

    f.render_widget(Clear, pop_area);
    f.render_widget(block, pop_area);

    let inner = Rect {
        x: pop_area.x + 1,
        y: pop_area.y + 1,
        width: pop_area.width.saturating_sub(2),
        height: pop_area.height.saturating_sub(2),
    };

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(40, 44, 52))
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut state = ListState::default();
    state.select(Some(app.modal_index.min(presets.len().saturating_sub(1))));
    f.render_stateful_widget(list, inner, &mut state);
}

fn draw_theme_modal(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let pop_w = 76u16.min(area.width.saturating_sub(4));
    let pop_h = 20u16.min(area.height.saturating_sub(4));
    let pop_x = area.x + (area.width.saturating_sub(pop_w)) / 2;
    let pop_y = area.y + (area.height.saturating_sub(pop_h)) / 2;
    let pop_area = Rect::new(pop_x, pop_y, pop_w, pop_h);

    let config_dir = fshell_engine::resolve_config_dir().unwrap_or_default();
    let themes = Theme::available(&config_dir);
    let active_theme_name = app.env.active_theme().name.clone();

    let mut items: Vec<ListItem> = Vec::new();
    for name in &themes {
        let is_current = *name == active_theme_name;
        items.push(ListItem::new(Line::from(vec![
            Span::styled(
                format!(" {:<24}", name),
                if is_current {
                    Style::default()
                        .fg(Color::LightGreen)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                },
            ),
            Span::styled(
                if is_current { " [Active Theme]" } else { "" },
                Style::default().fg(Color::Green),
            ),
        ])));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Magenta))
        .title(Span::styled(
            " Select Theme (Real-Time Studio Preview) ",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ));

    f.render_widget(Clear, pop_area);
    f.render_widget(block, pop_area);

    let inner = Rect {
        x: pop_area.x + 1,
        y: pop_area.y + 1,
        width: pop_area.width.saturating_sub(2),
        height: pop_area.height.saturating_sub(2),
    };

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(40, 44, 52))
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut state = ListState::default();
    state.select(Some(app.modal_index.min(themes.len().saturating_sub(1))));
    f.render_stateful_widget(list, inner, &mut state);
}

fn draw_confirm_modal(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let pop_w = 56u16.min(area.width.saturating_sub(4));
    let pop_h = 7u16;
    let pop_x = area.x + (area.width.saturating_sub(pop_w)) / 2;
    let pop_y = area.y + (area.height.saturating_sub(pop_h)) / 2;
    let pop_area = Rect::new(pop_x, pop_y, pop_w, pop_h);

    let (title, message) = match app.mode {
        StudioMode::ConfirmDelete => (" Confirm Delete ", "Delete the selected prompt segment?"),
        StudioMode::ConfirmReset => (
            " Confirm Reset ",
            "Reset entire prompt to default configuration?",
        ),
        StudioMode::ConfirmQuit => (
            " Unsaved Changes ",
            "You have unsaved changes. Quit without saving?",
        ),
        _ => ("", ""),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::LightRed))
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
        ));

    f.render_widget(Clear, pop_area);
    f.render_widget(block, pop_area);

    let inner = Rect {
        x: pop_area.x + 2,
        y: pop_area.y + 1,
        width: pop_area.width.saturating_sub(4),
        height: pop_area.height.saturating_sub(2),
    };

    let lines = vec![
        Line::from(Span::styled(message, Style::default().fg(Color::White))),
        Line::from(Span::raw("")),
        Line::from(vec![
            Span::styled(
                " [y] Confirm   ",
                Style::default()
                    .fg(Color::LightRed)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" [n / Esc] Cancel ", Style::default().fg(Color::DarkGray)),
        ]),
    ];

    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_preset_merge_modal(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let pop_w = 64u16.min(area.width.saturating_sub(4));
    let pop_h = 8u16;
    let pop_x = area.x + (area.width.saturating_sub(pop_w)) / 2;
    let pop_y = area.y + (area.height.saturating_sub(pop_h)) / 2;
    let pop_area = Rect::new(pop_x, pop_y, pop_w, pop_h);

    let name = app.pending_preset.as_deref().unwrap_or("preset");

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            " Apply Preset ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));

    f.render_widget(Clear, pop_area);
    f.render_widget(block, pop_area);

    let inner = Rect {
        x: pop_area.x + 2,
        y: pop_area.y + 1,
        width: pop_area.width.saturating_sub(4),
        height: pop_area.height.saturating_sub(2),
    };

    let lines = vec![
        Line::from(vec![
            Span::raw("Apply prompt preset '"),
            Span::styled(
                name,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("' to your current setup?"),
        ]),
        Line::from(Span::raw("")),
        Line::from(vec![
            Span::styled(
                " [r] Replace All   ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " [m] Append/Merge   ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" [Esc] Cancel ", Style::default().fg(Color::DarkGray)),
        ]),
    ];

    f.render_widget(Paragraph::new(lines), inner);
}

fn field_value(seg: &SegmentConfig, field: &InspectorField) -> String {
    match field {
        InspectorField::Type => seg.r#type.name().to_string(),
        InspectorField::Prefix => seg.prefix.clone(),
        InspectorField::Suffix => seg.suffix.clone(),
        InspectorField::Text => seg.text.clone().unwrap_or_default(),
        InspectorField::Fg => spec_display(&seg.fg),
        InspectorField::Bg => spec_display(&seg.bg),
        InspectorField::SeparatorStyle => {
            let style = seg.separator_style.clone().unwrap_or(SeparatorStyle::None);
            format!("{:?}", style)
        }
        InspectorField::Bold => if seg.bold { "+" } else { " " }.to_string(),
        InspectorField::Italic => if seg.italic { "+" } else { " " }.to_string(),
        InspectorField::Shorten => if seg.shorten { "+" } else { " " }.to_string(),
        InspectorField::HideOnZero => if seg.hide_on_zero { "+" } else { " " }.to_string(),
        InspectorField::HideWhenClean => if seg.hide_when_clean { "+" } else { " " }.to_string(),
        InspectorField::ShowOnlyInRepo => if seg.show_only_in_repo { "+" } else { " " }.to_string(),
    }
}

fn handle_studio_input(app: &mut App) -> Result<bool, io::Error> {
    if !event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
        return Ok(true);
    }
    let Ok(Event::Key(key)) = event::read() else {
        return Ok(true);
    };
    if key.kind != event::KeyEventKind::Press {
        return Ok(true);
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Ok(false);
    }

    // Global Undo / Redo
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('z') => {
                app.undo();
                return Ok(true);
            }
            KeyCode::Char('y') => {
                app.redo();
                return Ok(true);
            }
            _ => {}
        }
    }

    match app.mode {
        StudioMode::Studio => match app.pane {
            FocusPane::SegmentList => {
                if !handle_segment_list_input(app, key) {
                    return Ok(false);
                }
            }
            FocusPane::Inspector => handle_inspector_input(app, key),
        },
        StudioMode::AddSegment | StudioMode::ChangeType => handle_catalog_input(app, key),
        StudioMode::ColorPicker => handle_color_picker_input(app, key),
        StudioMode::PresetPicker => handle_preset_picker_input(app, key),
        StudioMode::ThemePicker => handle_theme_picker_input(app, key),
        StudioMode::ConfirmDelete => handle_confirm_action(app, key, |a| {
            let sel = a.selected_segment;
            if sel < a.cur_len() {
                a.push_undo();
                a.cur_segments_mut().remove(sel);
                a.clamp_selection();
                a.dirty = true;
                a.status_toast = Some(("Segment removed from pipeline".into(), false));
            }
            a.mode = StudioMode::Studio;
        }),
        StudioMode::ConfirmReset => handle_confirm_action(app, key, |a| {
            let default = PromptConfig::default();
            a.left_segments = default.left.clone();
            a.right_segments = default.right.clone();
            a.config.separator_style = default.separator_style;
            a.selected_segment = 0;
            a.side = PromptSide::Left;
            a.dirty = true;
            a.mode = StudioMode::Studio;
            a.status_toast = Some(("Prompt reset to standard defaults".into(), false));
        }),
        StudioMode::ConfirmQuit => match key.code {
            KeyCode::Char('y' | 'Y') => return Ok(false),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                app.mode = StudioMode::Studio;
            }
            _ => {}
        },
        StudioMode::ConfirmPresetMerge => match key.code {
            KeyCode::Char('r' | 'R') => {
                if let Some(name) = app.pending_preset.take()
                    && let Some(preset_cfg) = fshell_core::presets::by_name(&name)
                {
                    app.push_undo();
                    app.left_segments = preset_cfg.left.clone();
                    app.right_segments = preset_cfg.right.clone();
                    app.config.separator_style = preset_cfg.separator_style.clone();
                    app.config.preset = Some(name);
                    app.selected_segment = 0;
                    app.side = PromptSide::Left;
                    app.dirty = true;
                    app.status_toast = Some(("Preset replaced prompt configuration".into(), false));
                }
                app.mode = StudioMode::Studio;
            }
            KeyCode::Char('m' | 'M') => {
                if let Some(name) = app.pending_preset.take()
                    && let Some(preset_cfg) = fshell_core::presets::by_name(&name)
                {
                    app.push_undo();
                    app.left_segments.extend(preset_cfg.left.clone());
                    app.right_segments.extend(preset_cfg.right.clone());
                    app.dirty = true;
                    app.status_toast = Some(("Preset merged into pipeline".into(), false));
                }
                app.mode = StudioMode::Studio;
            }
            KeyCode::Esc | KeyCode::Char('n' | 'N') => {
                app.pending_preset = None;
                app.mode = StudioMode::Studio;
            }
            _ => {}
        },
    }

    Ok(true)
}

fn handle_segment_list_input(app: &mut App, key: crossterm::event::KeyEvent) -> bool {
    let len = app.cur_len();
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            if app.selected_segment > 0 {
                app.selected_segment -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.selected_segment + 1 < len {
                app.selected_segment += 1;
            }
        }
        KeyCode::Char('K') => {
            app.move_selected_up();
        }
        KeyCode::Char('J') => {
            app.move_selected_down();
        }
        KeyCode::Tab => {
            app.side = match app.side {
                PromptSide::Left => PromptSide::Right,
                PromptSide::Right => PromptSide::Left,
            };
            app.clamp_selection();
        }
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter if len > 0 => {
            app.pane = FocusPane::Inspector;
            app.inspector_field_idx = 0;
        }
        KeyCode::Char('a') | KeyCode::Char('+') => {
            app.mode = StudioMode::AddSegment;
            app.modal_index = 0;
            app.modal_filter.clear();
        }
        KeyCode::Char('d') | KeyCode::Char('x') | KeyCode::Delete if len > 0 => {
            app.mode = StudioMode::ConfirmDelete;
        }
        KeyCode::Char('s') => {
            let new_config = PromptConfig {
                left: app.left_segments.clone(),
                right: app.right_segments.clone(),
                ..app.config.clone()
            };
            match prompt_config::save_config(&new_config) {
                Ok(()) => {
                    {
                        let mut cfg = app.env.prompt_config.write();
                        *cfg = new_config;
                    }
                    app.dirty = false;
                    app.status_toast =
                        Some(("Prompt saved to ~/.config/fshell/prompt.toml".into(), false));
                }
                Err(e) => {
                    app.status_toast = Some((format!("Save failed: {}", e), true));
                }
            }
        }
        KeyCode::Char('p') => {
            app.mode = StudioMode::PresetPicker;
            app.modal_index = 0;
        }
        KeyCode::Char('t') => {
            app.mode = StudioMode::ThemePicker;
            app.modal_index = 0;
        }
        KeyCode::Char('r') => {
            app.mode = StudioMode::ConfirmReset;
        }
        KeyCode::Char('u') => {
            app.undo();
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            if app.dirty {
                app.mode = StudioMode::ConfirmQuit;
            } else {
                return false;
            }
        }
        _ => {}
    }
    true
}

fn handle_inspector_input(app: &mut App, key: crossterm::event::KeyEvent) {
    let sel = app.selected_segment;
    let fields: Vec<InspectorField> = app
        .cur_segments()
        .get(sel)
        .map(|s| fields_for_segment(s, app.show_advanced))
        .unwrap_or_default();

    if fields.is_empty() {
        app.pane = FocusPane::SegmentList;
        return;
    }

    let cur_field = fields[app.inspector_field_idx.min(fields.len().saturating_sub(1))];

    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            if app.inspector_field_idx > 0 {
                app.inspector_field_idx -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.inspector_field_idx + 1 < fields.len() {
                app.inspector_field_idx += 1;
            }
        }
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc => {
            app.pane = FocusPane::SegmentList;
        }
        KeyCode::Char('e') => {
            app.show_advanced = !app.show_advanced;
            let new_fields = app
                .cur_segments()
                .get(sel)
                .map(|s| fields_for_segment(s, app.show_advanced))
                .unwrap_or_default();
            if app.inspector_field_idx >= new_fields.len() {
                app.inspector_field_idx = new_fields.len().saturating_sub(1);
            }
        }
        KeyCode::Enter | KeyCode::Char(' ') => match cur_field {
            InspectorField::Type => {
                app.mode = StudioMode::ChangeType;
                app.modal_index = 0;
                app.modal_filter.clear();
            }
            InspectorField::Bold => toggle_segment_bool(app, |s| &mut s.bold),
            InspectorField::Italic => toggle_segment_bool(app, |s| &mut s.italic),
            InspectorField::Shorten => toggle_segment_bool(app, |s| &mut s.shorten),
            InspectorField::HideOnZero => toggle_segment_bool(app, |s| &mut s.hide_on_zero),
            InspectorField::HideWhenClean => toggle_segment_bool(app, |s| &mut s.hide_when_clean),
            InspectorField::ShowOnlyInRepo => {
                toggle_segment_bool(app, |s| &mut s.show_only_in_repo)
            }
            InspectorField::Fg | InspectorField::Bg => {
                app.color_target_bg = cur_field == InspectorField::Bg;
                app.hex_buffer.clear();
                app.modal_index = 0;
                app.color_category = 0;
                app.mode = StudioMode::ColorPicker;
            }
            InspectorField::SeparatorStyle => {
                app.push_undo();
                if let Some(seg) = app.cur_segments_mut().get_mut(sel) {
                    let current = seg.separator_style.clone().unwrap_or(SeparatorStyle::None);
                    seg.separator_style = Some(next_separator(&current));
                    app.dirty = true;
                }
            }
            InspectorField::Prefix | InspectorField::Suffix | InspectorField::Text => {}
        },
        KeyCode::Backspace => {
            app.push_undo();
            if let Some(seg) = app.cur_segments_mut().get_mut(sel) {
                match cur_field {
                    InspectorField::Prefix => {
                        seg.prefix.pop();
                        app.dirty = true;
                    }
                    InspectorField::Suffix => {
                        seg.suffix.pop();
                        app.dirty = true;
                    }
                    InspectorField::Text => {
                        if let Some(ref mut t) = seg.text {
                            t.pop();
                            if t.is_empty() {
                                seg.text = None;
                            }
                            app.dirty = true;
                        }
                    }
                    _ => {}
                }
            }
        }
        KeyCode::Char(c)
            if c != 'q' && c != 's' && c != 'e' && c != 'j' && c != 'k' && c != 'h' && c != 'l' =>
        {
            app.push_undo();
            if let Some(seg) = app.cur_segments_mut().get_mut(sel) {
                match cur_field {
                    InspectorField::Prefix => {
                        seg.prefix.push(c);
                        app.dirty = true;
                    }
                    InspectorField::Suffix => {
                        seg.suffix.push(c);
                        app.dirty = true;
                    }
                    InspectorField::Text => {
                        let t = seg.text.get_or_insert_with(String::new);
                        t.push(c);
                        app.dirty = true;
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn toggle_segment_bool(app: &mut App, accessor: fn(&mut SegmentConfig) -> &mut bool) {
    let sel = app.selected_segment;
    app.push_undo();
    if let Some(seg) = app.cur_segments_mut().get_mut(sel) {
        *accessor(seg) = !*accessor(seg);
        app.dirty = true;
    }
}

fn handle_catalog_input(app: &mut App, key: crossterm::event::KeyEvent) {
    let all_types = SegmentType::all();
    let filtered: Vec<_> = all_types
        .iter()
        .filter(|(name, desc, _)| {
            let q = app.modal_filter.to_lowercase();
            q.is_empty() || name.contains(&q) || desc.to_lowercase().contains(&q)
        })
        .collect();

    match key.code {
        KeyCode::Up if app.modal_index > 0 => app.modal_index -= 1,
        KeyCode::Down if app.modal_index + 1 < filtered.len() => app.modal_index += 1,
        KeyCode::Enter => {
            if let Some((name, _, _)) = filtered.get(app.modal_index)
                && let Some(st) = segment_type_from_name(name)
            {
                app.push_undo();
                let sel = app.selected_segment;
                if app.mode == StudioMode::AddSegment {
                    let new_seg = SegmentConfig {
                        r#type: st,
                        ..Default::default()
                    };
                    let insert_at = if sel < app.cur_len() {
                        sel + 1
                    } else {
                        app.cur_len()
                    };
                    app.cur_segments_mut().insert(insert_at, new_seg);
                    app.selected_segment = insert_at;
                    app.dirty = true;
                    app.status_toast = Some((format!("Added {} segment", name), false));
                } else if let Some(seg) = app.cur_segments_mut().get_mut(sel) {
                    seg.r#type = st;
                    app.dirty = true;
                    app.status_toast = Some((format!("Type changed to {}", name), false));
                }
            }
            app.mode = StudioMode::Studio;
            app.modal_filter.clear();
            app.modal_index = 0;
        }
        KeyCode::Char(c) if c.is_ascii_graphic() || c == ' ' => {
            app.modal_filter.push(c);
            app.modal_index = 0;
        }
        KeyCode::Backspace => {
            app.modal_filter.pop();
            app.modal_index = 0;
        }
        KeyCode::Esc => {
            app.mode = StudioMode::Studio;
            app.modal_filter.clear();
            app.modal_index = 0;
        }
        _ => {}
    }
}

fn handle_color_picker_input(app: &mut App, key: crossterm::event::KeyEvent) {
    let sel = app.selected_segment;
    match key.code {
        KeyCode::Tab => {
            app.color_category = (app.color_category + 1) % 3;
            app.modal_index = 0;
        }
        KeyCode::Char('c') => {
            let target_bg = app.color_target_bg;
            app.push_undo();
            if let Some(seg) = app.cur_segments_mut().get_mut(sel) {
                if target_bg {
                    seg.bg = None;
                } else {
                    seg.fg = None;
                }
                app.dirty = true;
                app.status_toast = Some(("Cleared segment color".into(), false));
            }
            app.mode = StudioMode::Studio;
            app.hex_buffer.clear();
        }
        KeyCode::Left => {
            if app.modal_index > 0 {
                app.modal_index -= 1;
            }
        }
        KeyCode::Right => {
            let max_len = if app.color_category == 0 {
                SEMANTIC_PALETTE.len()
            } else if app.color_category == 1 {
                STANDARD_ANSI_PALETTE.len()
            } else {
                0
            };
            if app.modal_index + 1 < max_len {
                app.modal_index += 1;
            }
        }
        KeyCode::Up => {
            let step = if app.color_category == 0 { 2 } else { 4 };
            if app.modal_index >= step {
                app.modal_index -= step;
            }
        }
        KeyCode::Down => {
            let step = if app.color_category == 0 { 2 } else { 4 };
            let max_len = if app.color_category == 0 {
                SEMANTIC_PALETTE.len()
            } else if app.color_category == 1 {
                STANDARD_ANSI_PALETTE.len()
            } else {
                0
            };
            if app.modal_index + step < max_len {
                app.modal_index += step;
            }
        }
        KeyCode::Enter => {
            let spec = if app.color_category == 0 {
                if let Some((role, _, _)) = SEMANTIC_PALETTE.get(app.modal_index) {
                    Some(ColorSpec::Named((*role).to_string()))
                } else {
                    None
                }
            } else if app.color_category == 1 {
                if let Some((name, _)) = STANDARD_ANSI_PALETTE.get(app.modal_index) {
                    Some(ColorSpec::Named((*name).to_string()))
                } else {
                    None
                }
            } else if !app.hex_buffer.is_empty() {
                let hex_clean = app.hex_buffer.trim_start_matches('#');
                Some(ColorSpec::Hex(format!("#{}", hex_clean.to_lowercase())))
            } else {
                None
            };

            let target_bg = app.color_target_bg;
            app.push_undo();
            if let Some(seg) = app.cur_segments_mut().get_mut(sel) {
                if target_bg {
                    seg.bg = spec;
                } else {
                    seg.fg = spec;
                }
                app.dirty = true;
                app.status_toast = Some(("Applied color to segment".into(), false));
            }
            app.mode = StudioMode::Studio;
            app.hex_buffer.clear();
        }
        KeyCode::Backspace => {
            if app.color_category == 2 {
                app.hex_buffer.pop();
            }
        }
        KeyCode::Char(c) => {
            if app.color_category == 2 && c.is_ascii_hexdigit() && app.hex_buffer.len() < 6 {
                app.hex_buffer.push(c);
            }
        }
        KeyCode::Esc => {
            app.mode = StudioMode::Studio;
            app.hex_buffer.clear();
        }
        _ => {}
    }
}

fn handle_preset_picker_input(app: &mut App, key: crossterm::event::KeyEvent) {
    let presets = fshell_core::presets::available();
    match key.code {
        KeyCode::Up if app.modal_index > 0 => app.modal_index -= 1,
        KeyCode::Down if app.modal_index + 1 < presets.len() => app.modal_index += 1,
        KeyCode::Enter => {
            if let Some(name) = presets.get(app.modal_index) {
                app.pending_preset = Some(name.to_string());
                app.mode = StudioMode::ConfirmPresetMerge;
            }
        }
        KeyCode::Esc => app.mode = StudioMode::Studio,
        _ => {}
    }
}

fn handle_theme_picker_input(app: &mut App, key: crossterm::event::KeyEvent) {
    let config_dir = fshell_engine::resolve_config_dir().unwrap_or_default();
    let themes = Theme::available(&config_dir);

    match key.code {
        KeyCode::Up if app.modal_index > 0 => {
            app.modal_index -= 1;
            if let Some(theme_name) = themes.get(app.modal_index)
                && let Ok(t) = Theme::load(theme_name, &config_dir)
            {
                app.env.set_theme(Arc::new(t));
            }
        }
        KeyCode::Down if app.modal_index + 1 < themes.len() => {
            app.modal_index += 1;
            if let Some(theme_name) = themes.get(app.modal_index)
                && let Ok(t) = Theme::load(theme_name, &config_dir)
            {
                app.env.set_theme(Arc::new(t));
            }
        }
        KeyCode::Enter => {
            if let Some(theme_name) = themes.get(app.modal_index)
                && let Ok(t) = Theme::load(theme_name, &config_dir)
            {
                app.env.set_theme(Arc::new(t));
                app.status_toast = Some((format!("Theme '{}' applied", theme_name), false));
            }
            app.mode = StudioMode::Studio;
        }
        KeyCode::Esc => {
            app.mode = StudioMode::Studio;
        }
        _ => {}
    }
}

fn handle_confirm_action(
    app: &mut App,
    key: crossterm::event::KeyEvent,
    on_yes: impl FnOnce(&mut App),
) {
    match key.code {
        KeyCode::Char('y' | 'Y') => on_yes(app),
        KeyCode::Char('n' | 'N') | KeyCode::Esc => {
            app.mode = StudioMode::Studio;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_app() -> App {
        let env = Env::new();
        App::new(env)
    }

    #[test]
    fn test_app_initial_state() {
        let app = create_test_app();
        assert_eq!(app.mode, StudioMode::Studio);
        assert_eq!(app.side, PromptSide::Left);
        assert_eq!(app.selected_segment, 0);
        assert_eq!(app.pane, FocusPane::SegmentList);
        assert!(!app.left_segments.is_empty());
        assert!(!app.right_segments.is_empty());
    }

    #[test]
    fn test_app_undo_redo() {
        let mut app = create_test_app();
        let initial_left_len = app.left_segments.len();

        app.push_undo();
        app.left_segments.push(SegmentConfig {
            r#type: SegmentType::Aws,
            ..Default::default()
        });
        assert_eq!(app.left_segments.len(), initial_left_len + 1);

        app.undo();
        assert_eq!(app.left_segments.len(), initial_left_len);

        app.redo();
        assert_eq!(app.left_segments.len(), initial_left_len + 1);
    }

    #[test]
    fn test_move_selected_segments() {
        let mut app = create_test_app();
        assert!(app.left_segments.len() >= 2);
        let first_type = app.left_segments[0].r#type.clone();
        let second_type = app.left_segments[1].r#type.clone();

        app.selected_segment = 0;
        app.move_selected_down();
        assert_eq!(app.selected_segment, 1);
        assert_eq!(app.left_segments[0].r#type, second_type);
        assert_eq!(app.left_segments[1].r#type, first_type);

        app.move_selected_up();
        assert_eq!(app.selected_segment, 0);
        assert_eq!(app.left_segments[0].r#type, first_type);
        assert_eq!(app.left_segments[1].r#type, second_type);
    }

    #[test]
    fn test_toggle_segment_bool_and_separator() {
        let mut app = create_test_app();
        app.selected_segment = 0;
        let initial_bold = app.left_segments[0].bold;

        toggle_segment_bool(&mut app, |s| &mut s.bold);
        assert_eq!(app.left_segments[0].bold, !initial_bold);
        assert!(app.dirty);

        let next = next_separator(&SeparatorStyle::None);
        assert_eq!(next, SeparatorStyle::Arrow);
        let next = next_separator(&SeparatorStyle::Arrow);
        assert_eq!(next, SeparatorStyle::Chevron);
    }

    #[test]
    fn test_semantic_palette_colors_resolve() {
        let env = Env::new();
        let theme = env.active_theme();
        for (key, _, _) in SEMANTIC_PALETTE {
            let (r, g, b) = theme.resolve_color(key);
            assert!(r > 0 || g > 0 || b > 0 || *key == "black");
        }
    }
}
