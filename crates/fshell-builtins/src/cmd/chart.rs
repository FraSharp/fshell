// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Built-in `chart` command: render ASCII/ANSI bar charts, histograms, and summary tables from pipeline data.

use crate::error::BuiltinError;
use fshell_core::theme::{Theme, ThemeColor};
use fshell_core::{ShellError, Val};
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use miette::SourceSpan;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use unicode_width::UnicodeWidthStr;
use ustr::ustr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartType {
    Bar,
    Table,
    Histogram,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Asc,
    Desc,
}

pub fn chart_builtin(
    in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let mut by_field: Option<String> = None;
    let mut val_field: Option<String> = None;
    let mut count_mode = false;
    let mut chart_type = ChartType::Bar;
    let mut sort_order = SortOrder::Desc;
    let mut limit: Option<usize> = None;
    let mut direct_list: Option<Vec<Val>> = None;

    let mut idx = 0;
    while idx < args.len() {
        match &args[idx] {
            Val::String(s) if s == "--by" => {
                if idx + 1 < args.len() {
                    by_field = Some(args[idx + 1].to_text());
                    idx += 1;
                } else {
                    return Err(BuiltinError::MissingArgument {
                        cmd: "chart".into(),
                        description: "field name after --by".into(),
                        span,
                    }
                    .into());
                }
            }
            Val::String(s) if s.starts_with("--by=") => {
                if let Some(rest) = s.strip_prefix("--by=") {
                    by_field = Some(rest.to_string());
                }
            }
            Val::String(s) if s == "--val" => {
                if idx + 1 < args.len() {
                    val_field = Some(args[idx + 1].to_text());
                    idx += 1;
                } else {
                    return Err(BuiltinError::MissingArgument {
                        cmd: "chart".into(),
                        description: "field name after --val".into(),
                        span,
                    }
                    .into());
                }
            }
            Val::String(s) if s.starts_with("--val=") => {
                if let Some(rest) = s.strip_prefix("--val=") {
                    val_field = Some(rest.to_string());
                }
            }
            Val::String(s) if s == "--count" => {
                count_mode = true;
            }
            Val::String(s) if s == "--type" => {
                if idx + 1 < args.len() {
                    let t = args[idx + 1].to_text();
                    chart_type = match t.as_str() {
                        "bar" => ChartType::Bar,
                        "table" => ChartType::Table,
                        "histogram" | "hist" => ChartType::Histogram,
                        _ => {
                            return Err(BuiltinError::InvalidArgument {
                                cmd: "chart".into(),
                                arg: format!(
                                    "unknown chart type '{t}', expected bar, table, or histogram"
                                ),
                                span,
                            }
                            .into());
                        }
                    };
                    idx += 1;
                } else {
                    return Err(BuiltinError::MissingArgument {
                        cmd: "chart".into(),
                        description: "chart type (bar, table, histogram) after --type".into(),
                        span,
                    }
                    .into());
                }
            }
            Val::String(s) if s.starts_with("--type=") => {
                if let Some(t) = s.strip_prefix("--type=") {
                    chart_type = match t {
                        "bar" => ChartType::Bar,
                        "table" => ChartType::Table,
                        "histogram" | "hist" => ChartType::Histogram,
                        _ => {
                            return Err(BuiltinError::InvalidArgument {
                                cmd: "chart".into(),
                                arg: format!(
                                    "unknown chart type '{t}', expected bar, table, or histogram"
                                ),
                                span,
                            }
                            .into());
                        }
                    };
                }
            }
            Val::String(s) if s == "--sort" => {
                if idx + 1 < args.len() {
                    let s_val = args[idx + 1].to_text();
                    sort_order = match s_val.as_str() {
                        "asc" => SortOrder::Asc,
                        "desc" => SortOrder::Desc,
                        _ => {
                            return Err(BuiltinError::InvalidArgument {
                                cmd: "chart".into(),
                                arg: format!(
                                    "unknown sort direction '{s_val}', expected asc or desc"
                                ),
                                span,
                            }
                            .into());
                        }
                    };
                    idx += 1;
                } else {
                    return Err(BuiltinError::MissingArgument {
                        cmd: "chart".into(),
                        description: "sort direction (asc, desc) after --sort".into(),
                        span,
                    }
                    .into());
                }
            }
            Val::String(s) if s.starts_with("--sort=") => {
                if let Some(s_val) = s.strip_prefix("--sort=") {
                    sort_order = match s_val {
                        "asc" => SortOrder::Asc,
                        "desc" => SortOrder::Desc,
                        _ => {
                            return Err(BuiltinError::InvalidArgument {
                                cmd: "chart".into(),
                                arg: format!(
                                    "unknown sort direction '{s_val}', expected asc or desc"
                                ),
                                span,
                            }
                            .into());
                        }
                    };
                }
            }
            Val::String(s) if s == "-n" || s == "--limit" => {
                if idx + 1 < args.len() {
                    let n_str = args[idx + 1].to_text();
                    match n_str.parse::<usize>() {
                        Ok(n) => limit = Some(n),
                        Err(_) => {
                            return Err(BuiltinError::InvalidArgument {
                                cmd: "chart".into(),
                                arg: format!("invalid limit number '{n_str}'"),
                                span,
                            }
                            .into());
                        }
                    }
                    idx += 1;
                } else {
                    return Err(BuiltinError::MissingArgument {
                        cmd: "chart".into(),
                        description: "number after -n/--limit".into(),
                        span,
                    }
                    .into());
                }
            }
            Val::String(s) if s.starts_with("-n=") || s.starts_with("--limit=") => {
                let num_str = s
                    .strip_prefix("-n=")
                    .or_else(|| s.strip_prefix("--limit="))
                    .unwrap_or("");
                match num_str.parse::<usize>() {
                    Ok(n) => limit = Some(n),
                    Err(_) => {
                        return Err(BuiltinError::InvalidArgument {
                            cmd: "chart".into(),
                            arg: format!("invalid limit number '{num_str}'"),
                            span,
                        }
                        .into());
                    }
                }
            }
            Val::List(l) => {
                if direct_list.is_some() {
                    return Err(BuiltinError::InvalidArgument {
                        cmd: "chart".into(),
                        arg: "multiple list arguments provided".into(),
                        span,
                    }
                    .into());
                }
                direct_list = Some(l.clone());
            }
            Val::String(s) if s.starts_with('-') => {
                return Err(BuiltinError::UnexpectedArgument {
                    cmd: "chart".into(),
                    arg: s.clone(),
                    span,
                }
                .into());
            }
            other => {
                return Err(BuiltinError::UnexpectedArgument {
                    cmd: "chart".into(),
                    arg: other.to_text(),
                    span,
                }
                .into());
            }
        }
        idx += 1;
    }

    let by = match by_field {
        Some(b) => b,
        None => {
            return Err(BuiltinError::MissingArgument {
                cmd: "chart".into(),
                description: "--by <field> is required".into(),
                span,
            }
            .into());
        }
    };

    let env_clone = env.clone();
    let theme = env.active_theme();

    tokio::spawn(async move {
        let mut items = Vec::new();

        if let Some(list) = direct_list {
            for item in list {
                items.push(Arc::new(item));
            }
        } else if let Some(mut rx) = in_rx {
            while let Some(payload) = rx.recv().await {
                if env_clone.job_control.cancellation.load(Ordering::Relaxed) {
                    return;
                }
                match payload {
                    PipelinePayload::Data(val_arc) => {
                        items.push(val_arc);
                    }
                    PipelinePayload::Structured(data) => {
                        let _ = tx.send(PipelinePayload::Structured(data)).await;
                    }
                    PipelinePayload::Bytes(_) => {}
                }
            }
        }

        if items.is_empty() {
            let _ = tx
                .send(PipelinePayload::Data(Arc::new(Val::String(
                    "(no data)\n".to_string(),
                ))))
                .await;
            return;
        }

        let term_width = crossterm::terminal::size()
            .map(|(w, _)| w as usize)
            .unwrap_or(80);

        let rendered = render_chart_data(
            &items,
            &by,
            val_field.as_deref(),
            count_mode,
            chart_type,
            sort_order,
            limit,
            term_width,
            &theme,
        );

        let _ = tx
            .send(PipelinePayload::Data(Arc::new(Val::String(rendered))))
            .await;
    });

    Ok(())
}

fn nu_color(c: &ThemeColor) -> nu_ansi_term::Color {
    let (r, g, b) = c.to_rgb();
    nu_ansi_term::Color::Rgb(r, g, b)
}

#[derive(Debug, Clone)]
struct DataGroup {
    label: String,
    value: f64,
    count: usize,
}

#[allow(clippy::too_many_arguments)]
fn render_chart_data(
    items: &[Arc<Val>],
    by_field: &str,
    val_field: Option<&str>,
    count_mode: bool,
    chart_type: ChartType,
    sort_order: SortOrder,
    limit: Option<usize>,
    term_width: usize,
    theme: &Theme,
) -> String {
    if chart_type == ChartType::Histogram {
        return render_histogram(items, by_field, val_field, term_width, theme);
    }

    // Aggregate by categorical group
    let mut groups_map: indexmap::IndexMap<String, (f64, usize)> = indexmap::IndexMap::new();
    let by_key = ustr(by_field);
    let val_key = val_field.map(ustr);

    for item in items {
        if let Val::Map(map) = item.as_ref() {
            let label = map
                .get(&by_key)
                .map(|v| v.to_text())
                .unwrap_or_else(|| "(null)".to_string());

            let val = if count_mode || val_key.is_none() {
                1.0
            } else if let Some(vk) = val_key {
                match map.get(&vk) {
                    Some(Val::Int(i)) => *i as f64,
                    Some(Val::Float(f)) => *f,
                    _ => 0.0,
                }
            } else {
                1.0
            };

            let entry = groups_map.entry(label).or_insert((0.0, 0));
            entry.0 += val;
            entry.1 += 1;
        }
    }

    if groups_map.is_empty() {
        return "(no data matching group field)\n".to_string();
    }

    let mut groups: Vec<DataGroup> = groups_map
        .into_iter()
        .map(|(label, (value, count))| DataGroup {
            label,
            value,
            count,
        })
        .collect();

    // Sort groups
    groups.sort_by(|a, b| {
        let ord = a
            .value
            .partial_cmp(&b.value)
            .unwrap_or(std::cmp::Ordering::Equal);
        if sort_order == SortOrder::Desc {
            ord.reverse()
        } else {
            ord
        }
    });

    let total_value: f64 = groups.iter().map(|g| g.value).sum();

    // Limit groups if requested
    let display_groups = if let Some(n) = limit {
        if n < groups.len() {
            let mut top: Vec<DataGroup> = groups[..n].to_vec();
            let other_val: f64 = groups[n..].iter().map(|g| g.value).sum();
            let other_cnt: usize = groups[n..].iter().map(|g| g.count).sum();
            if other_cnt > 0 {
                top.push(DataGroup {
                    label: "(other)".to_string(),
                    value: other_val,
                    count: other_cnt,
                });
            }
            top
        } else {
            groups
        }
    } else {
        groups
    };

    match chart_type {
        ChartType::Bar => render_bar_groups(&display_groups, total_value, term_width, theme),
        ChartType::Table => {
            render_table_groups(&display_groups, total_value, val_field.is_some(), theme)
        }
        ChartType::Histogram => unreachable!(),
    }
}

fn format_metric_value(val: f64) -> String {
    let abs_val = val.abs();
    if abs_val >= 1_000_000_000.0 {
        format!("{:.1}G", val / 1_000_000_000.0)
    } else if abs_val >= 1_000_000.0 {
        format!("{:.1}M", val / 1_000_000.0)
    } else if abs_val >= 1_000.0 {
        format!("{:.1}K", val / 1_000.0)
    } else if (val.fract() - 0.0).abs() < 1e-4 {
        format!("{:.0}", val)
    } else {
        format!("{:.2}", val)
    }
}

fn render_bar_groups(
    groups: &[DataGroup],
    total_value: f64,
    term_width: usize,
    theme: &Theme,
) -> String {
    if groups.is_empty() {
        return "(no data)\n".to_string();
    }

    let max_val = groups.iter().map(|g| g.value).fold(0.0_f64, f64::max);
    let max_label_len = groups
        .iter()
        .map(|g| g.label.width())
        .max()
        .unwrap_or(4)
        .min(term_width / 3)
        .max(4);

    let max_val_len = groups
        .iter()
        .map(|g| format_metric_value(g.value).len())
        .max()
        .unwrap_or(4)
        .max(4);

    let label_style = nu_color(&theme.syntax.keyword).bold();
    let bar_style = nu_color(&theme.status.ok);
    let val_style = nu_color(&theme.widgets.foreground);
    let pct_style = nu_color(&theme.status.muted);

    // Reserved: label + 2 spaces + bar + 2 spaces + value (max_val_len) + 2 spaces + percentage "(100%)" (6 chars)
    let non_bar_width = max_label_len + 2 + 2 + max_val_len + 2 + 6;
    let bar_area_width = term_width.saturating_sub(non_bar_width).clamp(10, 80);

    const FRACTIONAL_BLOCKS: &[char] = &[' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

    let mut out = String::new();
    for g in groups {
        // Truncate or pad label
        let label_w = g.label.width();
        let display_label = if label_w > max_label_len {
            let mut s = String::new();
            let mut w = 0;
            for c in g.label.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
                if w + cw + 1 > max_label_len {
                    break;
                }
                s.push(c);
                w += cw;
            }
            s.push('…');
            let pad = max_label_len.saturating_sub(s.width());
            format!("{}{}", s, " ".repeat(pad))
        } else {
            format!("{}{}", g.label, " ".repeat(max_label_len - label_w))
        };

        let ratio = if max_val > 0.0 {
            (g.value / max_val).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let exact_blocks = ratio * (bar_area_width as f64);
        let full_blocks = exact_blocks.floor() as usize;
        let remainder = exact_blocks - (full_blocks as f64);
        let frac_idx = (remainder * 8.0).round() as usize;

        let mut bar = "█".repeat(full_blocks);
        if full_blocks < bar_area_width && frac_idx > 0 && frac_idx < FRACTIONAL_BLOCKS.len() {
            bar.push(FRACTIONAL_BLOCKS[frac_idx]);
        }
        let bar_pad = bar_area_width.saturating_sub(bar.width());
        let bar_padded = format!("{}{}", bar, " ".repeat(bar_pad));

        let formatted_val = format_metric_value(g.value);
        let val_pad = max_val_len.saturating_sub(formatted_val.len());
        let val_aligned = format!("{}{}", " ".repeat(val_pad), formatted_val);

        let pct = if total_value > 0.0 {
            (g.value / total_value * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };

        out.push_str(&format!(
            "{}  {}  {}  {}\n",
            label_style.paint(&display_label),
            bar_style.paint(&bar_padded),
            val_style.paint(&val_aligned),
            pct_style.paint(format!("{:>4.0}%", pct))
        ));
    }

    out
}

fn render_table_groups(
    groups: &[DataGroup],
    total_value: f64,
    has_custom_val: bool,
    theme: &Theme,
) -> String {
    let header_style = nu_color(&theme.widgets.title).bold().underline();
    let border_style = nu_color(&theme.status.muted);
    let val_style = nu_color(&theme.widgets.foreground);
    let bar_style = nu_color(&theme.status.ok);

    let val_header = if has_custom_val { "VALUE" } else { "COUNT" };
    let mut out = String::new();

    let max_label_len = groups
        .iter()
        .map(|g| g.label.width())
        .max()
        .unwrap_or(8)
        .max(8);

    out.push_str(&format!(
        "{:<lbl$}   {:>10}   {:>8}   {}\n",
        header_style.paint("CATEGORY"),
        header_style.paint(val_header),
        header_style.paint("PERCENT"),
        header_style.paint("DISTRIBUTION"),
        lbl = max_label_len
    ));

    out.push_str(
        &border_style
            .paint("─".repeat(max_label_len + 45))
            .to_string(),
    );
    out.push('\n');

    let max_val = groups.iter().map(|g| g.value).fold(0.0_f64, f64::max);

    for g in groups {
        let label_pad = max_label_len.saturating_sub(g.label.width());
        let padded_label = format!("{}{}", g.label, " ".repeat(label_pad));

        let formatted_val = format_metric_value(g.value);
        let pct = if total_value > 0.0 {
            g.value / total_value * 100.0
        } else {
            0.0
        };

        let ratio = if max_val > 0.0 {
            g.value / max_val
        } else {
            0.0
        };
        let bar_len = (ratio * 16.0).round() as usize;
        let bar_str = "█".repeat(bar_len);

        out.push_str(&format!(
            "{:<lbl$}   {:>10}   {:>7.1}%   {}\n",
            padded_label,
            val_style.paint(&formatted_val),
            pct,
            bar_style.paint(&bar_str),
            lbl = max_label_len
        ));
    }

    out
}

fn render_histogram(
    items: &[Arc<Val>],
    by_field: &str,
    val_field: Option<&str>,
    term_width: usize,
    theme: &Theme,
) -> String {
    let key = ustr(val_field.unwrap_or(by_field));
    let mut numbers: Vec<f64> = Vec::new();

    for item in items {
        if let Val::Map(map) = item.as_ref() {
            match map.get(&key) {
                Some(Val::Int(i)) => numbers.push(*i as f64),
                Some(Val::Float(f)) => numbers.push(*f),
                _ => {}
            }
        }
    }

    if numbers.is_empty() {
        return "(no numeric data found for histogram)\n".to_string();
    }

    let min = numbers.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = numbers.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

    let bin_count = 8.min(numbers.len().max(1));
    let range = max - min;

    let mut bins = vec![0usize; bin_count];
    let bin_width = if range > 0.0 {
        range / (bin_count as f64)
    } else {
        1.0
    };

    for n in &numbers {
        let idx = if range > 0.0 {
            ((*n - min) / bin_width).floor() as usize
        } else {
            0
        };
        let clamped_idx = idx.min(bin_count - 1);
        bins[clamped_idx] += 1;
    }

    let total_samples = numbers.len() as f64;
    let max_bin_count = bins.iter().cloned().max().unwrap_or(1);

    let label_style = nu_color(&theme.syntax.type_name).bold();
    let bar_style = nu_color(&theme.status.ok);
    let val_style = nu_color(&theme.widgets.foreground);
    let pct_style = nu_color(&theme.status.muted);

    let bar_width = term_width.saturating_sub(40).clamp(10, 50);

    let mut out = String::new();
    out.push_str(&format!(
        "Histogram of {} ({} samples, min: {}, max: {}):\n\n",
        key.as_str(),
        numbers.len(),
        format_metric_value(min),
        format_metric_value(max)
    ));

    for (i, count) in bins.iter().enumerate() {
        let bin_start = min + (i as f64) * bin_width;
        let bin_end = if i + 1 == bin_count {
            max
        } else {
            min + ((i + 1) as f64) * bin_width
        };

        let range_label = format!(
            "[{:>6} - {:>6})",
            format_metric_value(bin_start),
            format_metric_value(bin_end)
        );
        let ratio = if max_bin_count > 0 {
            (*count as f64) / (max_bin_count as f64)
        } else {
            0.0
        };

        let bar_len = (ratio * (bar_width as f64)).round() as usize;
        let bar_str = "█".repeat(bar_len);
        let bar_padded = format!(
            "{}{}",
            bar_str,
            " ".repeat(bar_width.saturating_sub(bar_len))
        );

        let pct = if total_samples > 0.0 {
            (*count as f64) / total_samples * 100.0
        } else {
            0.0
        };

        out.push_str(&format!(
            "{}  {}  {}  {}\n",
            label_style.paint(&range_label),
            bar_style.paint(&bar_padded),
            val_style.paint(format!("{:>5}", count)),
            pct_style.paint(format!("{:>4.0}%", pct))
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use fshell_core::FxIndexMap;

    fn make_test_record(user: &str, cpu: f64) -> Val {
        let mut m = FxIndexMap::default();
        m.insert(ustr("user"), Val::String(user.to_string()));
        m.insert(ustr("cpu"), Val::Float(cpu));
        Val::Map(m)
    }

    #[test]
    fn test_chart_bar_aggregation() {
        let theme = Theme::default_theme();
        let items = vec![
            Arc::new(make_test_record("alice", 20.0)),
            Arc::new(make_test_record("bob", 50.0)),
            Arc::new(make_test_record("alice", 30.0)),
        ];

        let out = render_chart_data(
            &items,
            "user",
            Some("cpu"),
            false,
            ChartType::Bar,
            SortOrder::Desc,
            None,
            80,
            &theme,
        );

        assert!(out.contains("bob"));
        assert!(out.contains("alice"));
        assert!(out.contains("50"));
    }

    #[test]
    fn test_chart_count_aggregation() {
        let theme = Theme::default_theme();
        let items = vec![
            Arc::new(make_test_record("alice", 1.0)),
            Arc::new(make_test_record("alice", 2.0)),
            Arc::new(make_test_record("bob", 1.0)),
        ];

        let out = render_chart_data(
            &items,
            "user",
            None,
            true,
            ChartType::Bar,
            SortOrder::Desc,
            None,
            80,
            &theme,
        );

        assert!(out.contains("alice"));
        assert!(out.contains("bob"));
        assert!(out.contains("67%"));
    }

    #[test]
    fn test_chart_histogram() {
        let theme = Theme::default_theme();
        let items = vec![
            Arc::new(make_test_record("a", 5.0)),
            Arc::new(make_test_record("b", 15.0)),
            Arc::new(make_test_record("c", 25.0)),
        ];

        let out = render_chart_data(
            &items,
            "cpu",
            None,
            false,
            ChartType::Histogram,
            SortOrder::Desc,
            None,
            80,
            &theme,
        );

        assert!(out.contains("Histogram"));
        assert!(out.contains("min:"));
    }
}
