// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::app::{App, Focus};
use crate::theme_ext::ThemeColorRatatui;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

pub fn item_count(_app: &App) -> usize {
    let bool_names = super::super::app::App::bool_option_names();
    bool_names.len() + 8 // 8 value options
}

const BOOL_DESCS: &[(&str, &str)] = &[
    ("autocd", "Typing a directory name cd's into it"),
    ("pipefail", "Pipeline fails if any stage fails"),
    ("notify", "Report background job status changes"),
    ("json_auto_parse", "Parse external process stdout as JSON"),
    ("error_color", "Enable colored error output"),
    ("did_you_mean", "Suggest corrections for mistyped commands"),
    ("errexit", "Exit on error"),
    ("nounset", "Error on unset variables"),
    ("nullglob", "Glob with no matches expands to empty"),
    ("nocaseglob", "Case-insensitive glob matching"),
    ("noclobber", "Prevent overwriting files with redirection"),
    ("noexec", "Parse but don't execute commands"),
    ("xtrace", "Print commands before execution"),
    ("verbose", "Verbose output"),
    ("ignoreeof", "Ignore EOF (Ctrl-D)"),
    ("autopushd", "Auto-push dirs onto stack on cd"),
    ("histignoredups", "Don't record duplicate history entries"),
    ("cdable_vars", "Treat variable names as dirs for cd"),
    ("quiet_aliases", "Suppress alias shadowing warnings"),
];

const VALUE_DESCS: &[(&str, &str)] = &[
    ("sandbox_mode", "Sandbox mode: prompt|deny-all|monitor|off"),
    ("pipeline_channel_size", "Pipeline channel buffer size"),
    ("clear_on_reload", "Reload behavior: ask|always|never"),
    ("error_format", "Error format: graphical|compact|json"),
    ("suggestion_mode", "Did-you-mean mode: blocking|deferred"),
    ("notify_threshold", "Notification threshold"),
    ("stderr_max_bytes", "Max bytes for stderr capture"),
    ("sort_max_items", "Max items for sorting"),
];

pub fn render(area: Rect, buf: &mut Buffer, app: &App) {
    let theme = app.env.active_theme();
    let opts = app.env.options.read();

    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(Span::styled(
        "── Boolean Options ──",
        theme.status.info.to_style(),
    )));
    lines.push(Line::from(""));

    let bool_options = [
        ("autocd", opts.autocd),
        ("pipefail", opts.pipefail),
        ("notify", opts.notify),
        ("json_auto_parse", opts.json_auto_parse),
        ("error_color", opts.error_color),
        ("did_you_mean", opts.did_you_mean),
        ("errexit", opts.errexit),
        ("nounset", opts.nounset),
        ("nullglob", opts.nullglob),
        ("nocaseglob", opts.nocaseglob),
        ("noclobber", opts.noclobber),
        ("noexec", opts.noexec),
        ("xtrace", opts.xtrace),
        ("verbose", opts.verbose),
        ("ignoreeof", opts.ignoreeof),
        ("autopushd", opts.autopushd),
        ("histignoredups", opts.histignoredups),
        ("cdable_vars", opts.cdable_vars),
        ("quiet_aliases", opts.quiet_aliases),
    ];

    let is_content_focused = matches!(app.focus, Focus::Content);

    for (idx, (name, value)) in bool_options.iter().enumerate() {
        let is_highlighted = is_content_focused && idx == app.content_selected;
        let desc = BOOL_DESCS
            .iter()
            .find(|(n, _)| *n == *name)
            .map(|(_, d)| *d)
            .unwrap_or("");

        let indicator = if *value {
            Span::styled(
                " [ON] ",
                theme.status.ok.to_style().add_modifier(if is_highlighted {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            )
        } else {
            Span::styled(
                " [OFF] ",
                theme
                    .status
                    .error
                    .to_style()
                    .add_modifier(if is_highlighted {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            )
        };
        let name_style = if is_highlighted {
            theme.widgets.title.to_style().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<20}", name), name_style),
            indicator,
            Span::styled(desc, theme.status.muted.to_style()),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "── Value Options ──",
        theme.status.info.to_style(),
    )));
    lines.push(Line::from(""));

    let error_format_str = match opts.error_format {
        fshell_render::RenderFormat::Auto => "auto",
        fshell_render::RenderFormat::Graphical => "graphical",
        fshell_render::RenderFormat::Compact => "compact",
        fshell_render::RenderFormat::Explain => "explain",
        fshell_render::RenderFormat::Json => "json",
    };
    let suggestion_mode_str = match opts.suggestion_mode {
        fshell_engine::SuggestionMode::Blocking => "blocking",
        fshell_engine::SuggestionMode::Deferred => "deferred",
    };

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
    let value_values = [
        opts.sandbox_mode.clone(),
        opts.pipeline_channel_size.to_string(),
        opts.clear_on_reload.clone(),
        error_format_str.to_string(),
        suggestion_mode_str.to_string(),
        opts.notify_threshold.to_string(),
        opts.stderr_max_bytes.to_string(),
        opts.sort_max_items.to_string(),
    ];

    for (idx, (name, val)) in value_names.iter().zip(value_values.iter()).enumerate() {
        let item_idx = bool_options.len() + idx;
        let is_highlighted = is_content_focused && item_idx == app.content_selected;
        let desc = VALUE_DESCS
            .iter()
            .find(|(n, _)| *n == *name)
            .map(|(_, d)| *d)
            .unwrap_or("");
        let style = if is_highlighted {
            theme.widgets.title.to_style().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<23} {:<20}", name, val), style),
            Span::styled(desc, theme.status.muted.to_style()),
        ]));
    }

    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .title(" Shell Options ")
            .borders(Borders::ALL),
    );

    paragraph.render(area, buf);
}
