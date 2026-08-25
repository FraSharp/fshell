// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Status bar widget for the bottom of the screen.
//!
//! Renders a single-line bar showing window list, session info, pane count, and clock.
//! When prefix mode is active, the entire bar shifts to a purple tint.

use std::collections::HashMap;
use std::time::SystemTime;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::app::focus::FocusController;
use crate::daemon::window::{PaneState, Window};

use super::theme;

/// A window tab in the status bar.
struct VisibleTab {
    #[allow(dead_code)]
    index: usize,
    label: String,
    is_active: bool,
}

/// Compute which window tabs fit in the available width.
///
/// Always shows the active window. Expands outward left/right
/// greedily until no more tabs fit.
fn compute_visible_windows(
    windows: &[Window],
    active_idx: usize,
    mut available_width: u16,
) -> Vec<VisibleTab> {
    if windows.is_empty() {
        return Vec::new();
    }
    let active_idx = active_idx.min(windows.len() - 1);

    // Format all tabs and compute display widths.
    let formatted: Vec<(usize, String, u16)> = windows
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let suffix = if i == active_idx { "*" } else { "-" };
            let label = format!(" {}:{}{} ", i, w.name, suffix);
            let width = UnicodeWidthStr::width(label.as_str()) as u16;
            (i, label, width)
        })
        .collect();

    let mut visible = Vec::new();

    // Always include the active window.
    let (ai, ref al, aw) = formatted[active_idx];
    if aw > available_width {
        // Terminal too narrow — show truncated active tab.
        let truncated = format!(" {}.. ", ai);
        visible.push(VisibleTab {
            index: ai,
            label: truncated,
            is_active: true,
        });
        return visible;
    }
    visible.push(VisibleTab {
        index: ai,
        label: al.clone(),
        is_active: true,
    });
    available_width -= aw;

    // Greedy outward expansion, alternating sides.
    let mut left = ai.wrapping_sub(1);
    let mut right = ai + 1;
    let mut try_left = true;

    while available_width > 0 {
        let added = if try_left {
            if left < formatted.len() {
                let (i, l, w) = &formatted[left];
                if *w <= available_width {
                    visible.insert(
                        0,
                        VisibleTab {
                            index: *i,
                            label: l.clone(),
                            is_active: false,
                        },
                    );
                    available_width -= w;
                    left = left.wrapping_sub(1);
                    true
                } else {
                    left = left.wrapping_sub(1);
                    false
                }
            } else {
                false
            }
        } else {
            if right < formatted.len() {
                let (i, l, w) = &formatted[right];
                if *w <= available_width {
                    visible.push(VisibleTab {
                        index: *i,
                        label: l.clone(),
                        is_active: false,
                    });
                    available_width -= w;
                    right += 1;
                    true
                } else {
                    right += 1;
                    false
                }
            } else {
                false
            }
        };

        if !added && (left >= formatted.len() || left == usize::MAX) && right >= formatted.len() {
            break;
        }

        try_left = !try_left;
    }

    visible
}

/// Context for rendering the status bar.
pub struct StatusBarContext<'a> {
    pub session_name: &'a str,
    pub windows: &'a [Window],
    pub active_window_idx: usize,
    pub prefix_active: bool,
    pub panes: &'a HashMap<u32, PaneState>,
    pub focus: &'a FocusController,
}

/// Render the status bar at the bottom of the terminal.
pub fn render_status_bar(frame: &mut Frame, area: Rect, ctx: &StatusBarContext<'_>) {
    let time_str = current_time_hhmm();
    let session_name = ctx.session_name;
    let windows = ctx.windows;
    let active_window_idx = ctx.active_window_idx;
    let prefix_active = ctx.prefix_active;
    let panes = ctx.panes;
    let focus = ctx.focus;
    let pane_count = panes.len();
    let pane_str = if pane_count == 1 {
        "1 pane".to_string()
    } else {
        format!("{} panes", pane_count)
    };

    // Choose background style based on prefix state.
    let bg_style = if prefix_active {
        theme::statusbar_bg_prefix()
    } else {
        theme::statusbar_bg()
    };

    let sep_style = if prefix_active {
        theme::statusbar_sep_prefix()
    } else {
        theme::statusbar_sep()
    };

    let session_style = if prefix_active {
        theme::statusbar_session_prefix()
    } else {
        theme::statusbar_session()
    };

    // Compute window tabs.
    // Reserve space for: " session │ pane_info │ panes │ time " (roughly 40 chars).
    let fixed_overhead =
        session_name.len() as u16 + pane_str.len() as u16 + time_str.len() as u16 + 20;
    let tab_budget = area.width.saturating_sub(fixed_overhead);
    let tabs = compute_visible_windows(windows, active_window_idx, tab_budget);

    // Build left side spans.
    let mut left_spans = Vec::new();

    // Window tabs.
    for tab in &tabs {
        let tab_style = if prefix_active {
            if tab.is_active {
                theme::statusbar_window_active_prefix()
            } else {
                theme::statusbar_window_inactive_prefix()
            }
        } else if tab.is_active {
            theme::statusbar_window_active()
        } else {
            theme::statusbar_window_inactive()
        };
        left_spans.push(Span::styled(tab.label.clone(), tab_style));
    }

    // Separator after tabs.
    if !tabs.is_empty() {
        left_spans.push(Span::styled("│", sep_style));
    }

    // Session name.
    left_spans.push(Span::styled(format!(" {} ", session_name), session_style));
    left_spans.push(Span::styled("│", sep_style));

    // Pane info.
    let focused_title = panes
        .get(&focus.focused_pane)
        .map(|p| p.label.as_deref().unwrap_or(&p.shell_name));
    if let Some(title) = focused_title {
        let pane_info_style = if prefix_active {
            theme::statusbar_pane_info_prefix()
        } else {
            theme::statusbar_pane_info()
        };
        let focused_id = focus.focused_pane;
        left_spans.push(Span::styled(
            format!(" p{} {} ", focused_id, title),
            pane_info_style,
        ));
        left_spans.push(Span::styled("│", sep_style));
    }

    left_spans.push(Span::styled(format!(" {} ", pane_str), bg_style));
    left_spans.push(Span::styled("│", sep_style));
    left_spans.push(Span::styled(format!(" {} ", time_str), bg_style));

    // Fill the rest of the bar.
    let left_text: String = left_spans.iter().map(|s| s.content.as_ref()).collect();
    let padding = area
        .width
        .saturating_sub(UnicodeWidthStr::width(left_text.as_str()) as u16);

    let mut all_spans = left_spans;
    if padding > 0 {
        all_spans.push(Span::styled(" ".repeat(padding as usize), bg_style));
    }

    let line = Line::from(all_spans);
    let bar = Paragraph::new(line);
    frame.render_widget(bar, area);
}

/// Get the current time as "HH:MM" in 24-hour format.
pub fn current_time_hhmm() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = now.as_secs();
    let hours = (total_secs / 3600) % 24;
    let minutes = (total_secs / 60) % 60;
    format!("{:02}:{:02}", hours, minutes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_format() {
        let t = current_time_hhmm();
        assert_eq!(t.len(), 5);
        assert_eq!(t.chars().nth(2), Some(':'));
    }

    #[test]
    fn visible_windows_empty() {
        let windows = vec![];
        let visible = compute_visible_windows(&windows, 0, 80);
        assert!(visible.is_empty());
    }
}
