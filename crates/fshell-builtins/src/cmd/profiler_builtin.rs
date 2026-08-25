// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::Val;
use fshell_core::diagnostic::StringError;
use fshell_engine::profiler::{ProfilerCategory, ProfilerEdge, ProfilerEntry};
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use std::sync::Arc;
use std::time::Duration;

pub fn profile_builtin(
    _input: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), StringError> {
    let strings: Vec<String> = args
        .iter()
        .filter_map(|v| {
            if let Val::String(s) = v {
                Some(s.clone())
            } else {
                None
            }
        })
        .collect();

    let show_tree = strings.iter().any(|s| s == "--tree" || s == "-t");
    let subcommands: Vec<&str> = strings
        .iter()
        .map(|s| s.as_str())
        .filter(|s| *s != "--tree" && *s != "-t")
        .collect();

    if let Some(cmd) = subcommands.first() {
        match *cmd {
            "on" => {
                env.profiler.write().set_enabled(true);
                send_msg(&tx, "Profiling enabled.");
            }
            "off" => {
                env.profiler.write().set_enabled(false);
                send_msg(&tx, "Profiling disabled.");
            }
            "reset" => {
                env.profiler.write().reset();
                send_msg(&tx, "Profiling data reset.");
            }
            other => {
                let msg = format!("profile: unknown subcommand '{other}'. Use: on, off, reset");
                return Err(StringError::from(msg));
            }
        }
        return Ok(());
    }

    if show_tree {
        print_tree_report(env, &tx)
    } else {
        print_flat_report(env, &tx)
    }
}

fn send_msg(tx: &PipeSender, msg: &str) {
    let tx = tx.clone();
    let msg = msg.to_string();
    tokio::spawn(async move {
        let _ = tx
            .send(PipelinePayload::Data(Arc::new(Val::String(msg))))
            .await;
    });
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs >= 1.0 {
        format!("{:.2}s", secs)
    } else if secs >= 0.001 {
        format!("{:.2}ms", secs * 1000.0)
    } else {
        format!("{:.2}µs", secs * 1_000_000.0)
    }
}

struct EntryData {
    label: String,
    category: ProfilerCategory,
    total_time: Duration,
    self_time: Duration,
    call_count: u64,
}

fn compute_load_pct(self_time: Duration, total: Duration) -> f64 {
    if total == Duration::ZERO {
        return 0.0;
    }
    self_time.as_secs_f64() / total.as_secs_f64() * 100.0
}

fn print_flat_report(env: &Env, tx: &PipeSender) -> Result<(), StringError> {
    let state = env.profiler.read();

    if !state.is_enabled() {
        drop(state);
        env.profiler.write().set_enabled(true);
        send_msg(tx, "Profiling enabled. Run 'profile' again to see results.");
        return Ok(());
    }

    let raw: Vec<EntryData> = state
        .entries()
        .iter()
        .map(|e| EntryData {
            label: e.label.clone(),
            category: e.category,
            total_time: e.total_time,
            self_time: e.self_time(),
            call_count: e.call_count,
        })
        .collect();
    drop(state);

    if raw.is_empty() {
        send_msg(tx, "No profiling data captured yet.");
        return Ok(());
    }

    let top_total: Duration = raw.iter().fold(Duration::ZERO, |acc, e| acc + e.self_time);

    let mut sorted = raw;
    sorted.sort_unstable_by_key(|b| std::cmp::Reverse(b.self_time));

    let mut lines = Vec::new();
    lines.push(format!(
        "{:<8} {:<15} {:<15} {:<8} {:<10} {}",
        "calls", "self_time", "total_time", "load%", "category", "label"
    ));
    lines.push("-".repeat(75));

    for e in &sorted {
        let load = compute_load_pct(e.self_time, top_total);
        lines.push(format!(
            "{:<8} {:<15} {:<15} {:<8.1} {:<10} {}",
            e.call_count,
            format_duration(e.self_time),
            format_duration(e.total_time),
            load,
            e.category.as_str(),
            e.label,
        ));
    }

    let output = lines.join("\n");
    let tx_clone = tx.clone();
    tokio::spawn(async move {
        let _ = tx_clone
            .send(PipelinePayload::Data(Arc::new(Val::String(output))))
            .await;
    });
    Ok(())
}

// --- tree view ---

struct DisplayNode {
    label: String,
    category: ProfilerCategory,
    self_time: Duration,
    total_time: Duration,
    call_count: u64,
    load_pct: f64,
    children: Vec<DisplayNode>,
}

fn build_tree(
    entries: &[ProfilerEntry],
    edges: &[ProfilerEdge],
    top_total: Duration,
) -> Vec<DisplayNode> {
    let has_incoming: Vec<bool> = (0..entries.len())
        .map(|i| edges.iter().any(|e| e.callee_idx == i))
        .collect();

    fn edge_call_count(edges: &[ProfilerEdge], caller_idx: usize, callee_idx: usize) -> u64 {
        edges
            .iter()
            .find(|e| e.caller_idx == caller_idx && e.callee_idx == callee_idx)
            .map(|e| e.call_count)
            .unwrap_or(0)
    }

    fn build_subtree(
        entries: &[ProfilerEntry],
        edges: &[ProfilerEdge],
        idx: usize,
        display_calls: u64,
        top_total: Duration,
    ) -> DisplayNode {
        let e = &entries[idx];
        let self_time = e.self_time();
        let children: Vec<DisplayNode> = {
            let mut kids: Vec<usize> = edges
                .iter()
                .filter(|e| e.caller_idx == idx)
                .map(|e| e.callee_idx)
                .collect();
            kids.sort_unstable();
            kids.dedup();
            kids.into_iter()
                .filter_map(|child_idx| {
                    if child_idx == idx {
                        return None;
                    }
                    let child_calls = edge_call_count(edges, idx, child_idx);
                    Some(build_subtree(
                        entries,
                        edges,
                        child_idx,
                        child_calls,
                        top_total,
                    ))
                })
                .collect()
        };
        DisplayNode {
            label: e.label.clone(),
            category: e.category,
            self_time,
            total_time: e.total_time,
            call_count: display_calls,
            load_pct: compute_load_pct(self_time, top_total),
            children,
        }
    }

    let mut nodes: Vec<DisplayNode> = (0..entries.len())
        .filter(|i| !has_incoming[*i])
        .map(|idx| build_subtree(entries, edges, idx, entries[idx].call_count, top_total))
        .collect();
    nodes.sort_unstable_by_key(|b| std::cmp::Reverse(b.self_time));
    nodes
}

fn render_tree_line(node: &DisplayNode, prefix: &str, is_last: bool, lines: &mut Vec<String>) {
    let connector = if is_last { "└─ " } else { "├─ " };
    let label_text = format!("{} {}", node.category.as_str(), node.label);
    let line = format!(
        "{prefix:<5} {call_count:<8} {self_time:<15} {total_time:<15} {load_pct:<8.1} {label}",
        prefix = format!("{}{}", prefix, connector),
        call_count = node.call_count,
        self_time = format_duration(node.self_time),
        total_time = format_duration(node.total_time),
        load_pct = node.load_pct,
        label = label_text,
    );
    lines.push(line);

    let child_prefix = format!("{}{}   ", prefix, if is_last { " " } else { "│" });
    for (i, child) in node.children.iter().enumerate() {
        let last = i == node.children.len() - 1;
        render_tree_line(child, &child_prefix, last, lines);
    }
}

fn print_tree_report(env: &Env, tx: &PipeSender) -> Result<(), StringError> {
    let state = env.profiler.read();

    if !state.is_enabled() {
        drop(state);
        env.profiler.write().set_enabled(true);
        send_msg(tx, "Profiling enabled. Run 'profile' again to see results.");
        return Ok(());
    }

    let entries: Vec<ProfilerEntry> = state.entries().to_vec();
    let edges: Vec<ProfilerEdge> = state.edges.clone();
    drop(state);

    if entries.is_empty() {
        send_msg(tx, "No profiling data captured yet.");
        return Ok(());
    }

    let top_total: Duration = entries
        .iter()
        .fold(Duration::ZERO, |acc, e| acc + e.self_time());
    let tree = build_tree(&entries, &edges, top_total);

    let mut lines = Vec::new();
    lines.push(format!(
        "{:<5} {:<8} {:<15} {:<15} {:<8} {}",
        "", "calls", "self_time", "total_time", "load%", "label"
    ));
    lines.push("-".repeat(80));

    for (i, root) in tree.iter().enumerate() {
        let last = i == tree.len() - 1;
        let connector = if last { "└─ " } else { "├─ " };
        let label_text = format!("{} {}", root.category.as_str(), root.label);
        let line = format!(
            "{:<5} {:<8} {:<15} {:<15} {:<8.1} {}",
            connector,
            root.call_count,
            format_duration(root.self_time),
            format_duration(root.total_time),
            root.load_pct,
            label_text,
        );
        lines.push(line);

        let child_prefix = if last { "     " } else { "│    " };
        for (ci, child) in root.children.iter().enumerate() {
            let last_child = ci == root.children.len() - 1;
            render_tree_line(child, child_prefix, last_child, &mut lines);
        }
    }

    let output = lines.join("\n");
    let tx_clone = tx.clone();
    tokio::spawn(async move {
        let _ = tx_clone
            .send(PipelinePayload::Data(Arc::new(Val::String(output))))
            .await;
    });
    Ok(())
}
