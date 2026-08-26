// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::panic))]
use fshell_core::diagnostic::{DiagnosticExt, FshDiag};
use miette::GraphicalReportHandler;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderFormat {
    #[default]
    Auto,
    Graphical,
    Compact,
    Explain,
    Json,
}

#[derive(Clone, Debug)]
pub struct RenderConfig {
    pub format: RenderFormat,
    pub color: bool,
    pub is_interactive: bool,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            format: RenderFormat::Auto,
            color: true,
            is_interactive: false,
        }
    }
}

pub fn render(
    diag: FshDiag,
    source: Option<&str>,
    source_name: &str,
    config: &RenderConfig,
) -> String {
    let effective_format = match config.format {
        RenderFormat::Auto => {
            if config.is_interactive {
                RenderFormat::Compact
            } else {
                RenderFormat::Graphical
            }
        }
        other => other,
    };

    match effective_format {
        RenderFormat::Auto => unreachable!(),
        RenderFormat::Graphical => render_graphical(diag, source, source_name, config.color),
        RenderFormat::Compact => render_compact(diag, source, source_name, config.color),
        RenderFormat::Explain => render_explain(diag, source, source_name, config.color),
        RenderFormat::Json => render_json(diag),
    }
}

fn render_graphical(diag: FshDiag, source: Option<&str>, source_name: &str, color: bool) -> String {
    let fix = diag.fix.clone();
    let suggestions = diag.suggestions.clone();
    let docs_url = diag.docs_url();
    let stored_source = diag.source.clone();
    let stored_name = diag.source_name.clone();

    let mut report = diag.into_inner();
    if let Some(src) = source {
        report = report.with_source_code(miette::NamedSource::new(source_name, src.to_string()));
    } else if let (Some(src), Some(name)) = (stored_source, stored_name) {
        report = report.with_source_code(miette::NamedSource::new(name, (*src).clone()));
    }
    let mut output = String::new();
    let theme = if color {
        miette::GraphicalTheme::unicode()
    } else {
        miette::GraphicalTheme::unicode_nocolor()
    };
    let handler = GraphicalReportHandler::new()
        .with_theme(theme)
        .with_cause_chain();
    let _ = handler.render_report(&mut output, report.as_ref());

    let (c_yellow, c_green, c_cyan, c_reset) = if color {
        ("\x1b[1;33m", "\x1b[1;32m", "\x1b[1;36m", "\x1b[0m")
    } else {
        ("", "", "", "")
    };

    if let Some(fix_str) = fix {
        output.push_str(&format!("  {c_green}↳ fix:{c_reset} {fix_str}\n"));
    }
    if !suggestions.is_empty() {
        output.push_str(&format!(
            "  {c_yellow}↳ did you mean:{c_reset} {}\n",
            suggestions.join(", ")
        ));
    }
    if let Some(url) = docs_url {
        output.push_str(&format!("  {c_cyan}↳ docs:{c_reset} {url}\n"));
    }

    output
}

fn calculate_line_col(source: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (idx, ch) in source.char_indices() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

fn render_compact(diag: FshDiag, source: Option<&str>, source_name: &str, color: bool) -> String {
    let diag_ref = diag.report.as_ref();
    let code = diag
        .code
        .map(|c| c.code_str().to_string())
        .or_else(|| diag_ref.code().map(|c| c.to_string()))
        .unwrap_or_else(|| "FSH-GEN-001".into());

    let category = diag.code.map(|c| c.name()).unwrap_or(diag.category);

    let effective_source = source
        .map(|s| s.to_string())
        .or_else(|| diag.source.as_ref().map(|s| (**s).clone()));
    let _effective_name = source
        .map(|_| source_name.to_string())
        .or_else(|| diag.source_name.clone());
    let location =
        if let (Some(src), Some(labels)) = (effective_source.as_deref(), diag_ref.labels()) {
            labels
                .into_iter()
                .next()
                .map(|l| {
                    let (line, col) = calculate_line_col(src, l.offset());
                    format!(" at {line}:{col}")
                })
                .unwrap_or_default()
        } else {
            String::new()
        };

    let (c_red, c_cyan, c_dim, c_bold, c_yellow, c_green, c_reset) = if color {
        (
            "\x1b[1;31m",
            "\x1b[1;36m",
            "\x1b[2m",
            "\x1b[1m",
            "\x1b[1;33m",
            "\x1b[1;32m",
            "\x1b[0m",
        )
    } else {
        ("", "", "", "", "", "", "")
    };

    let mut out = format!(
        "{c_red}✖{c_reset} {c_cyan}[{category}]{c_reset} {c_bold}{diag_ref}{c_reset} {c_dim}({code}){location}{c_reset}"
    );

    if let Some(ref fix) = diag.fix {
        out.push_str(&format!("\n  {c_green}↳ fix:{c_reset} {fix}"));
    } else if !diag.suggestions.is_empty() {
        out.push_str(&format!(
            "\n  {c_yellow}↳ did you mean:{c_reset} {}",
            diag.suggestions.join(", ")
        ));
    } else if let Some(help) = diag_ref.help() {
        out.push_str(&format!("\n  {c_dim}↳ help:{c_reset} {help}"));
    }

    out
}

fn render_explain(diag: FshDiag, source: Option<&str>, source_name: &str, color: bool) -> String {
    let help = diag.report.as_ref().help().map(|h| h.to_string());
    let report_code = diag.report.as_ref().code().map(|c| c.to_string());
    let diag_message = diag.report.as_ref().to_string();
    let code_str = diag
        .code
        .map(|c| c.code_str().to_string())
        .or_else(|| report_code.clone())
        .unwrap_or_else(|| "FSH-GEN-001".into());

    let name = diag.code.map(|c| c.name()).unwrap_or(diag.category);

    let desc = diag
        .code
        .map(|c| c.default_description())
        .unwrap_or("A shell evaluation or execution failure occurred.");

    let docs_url = diag.code.and_then(|c| c.docs_url());
    let fix = diag.fix.clone();
    let suggestions = diag.suggestions.clone();

    let (c_cyan, c_dim, c_bold, c_yellow, c_green, c_reset) = if color {
        (
            "\x1b[1;36m",
            "\x1b[2m",
            "\x1b[1m",
            "\x1b[1;33m",
            "\x1b[1;32m",
            "\x1b[0m",
        )
    } else {
        ("", "", "", "", "", "")
    };

    let mut out = String::new();
    let header = format!("── [{name}: {code_str}] ");
    let pad_len = 70usize.saturating_sub(header.len()).max(4);
    out.push_str(&format!(
        "{c_cyan}{header}{pad}{c_reset}\n",
        pad = "─".repeat(pad_len)
    ));
    out.push_str(&format!("{c_dim}{desc}{c_reset}\n\n"));
    out.push_str(&format!("{c_bold}Message:{c_reset} {diag_message}\n"));

    let effective_source = source
        .map(|s| (s.to_string(), source_name.to_string()))
        .or_else(|| {
            diag.source
                .as_ref()
                .zip(diag.source_name.as_ref())
                .map(|(s, n)| ((**s).clone(), n.clone()))
        });
    let has_snippet = effective_source.is_some();
    if let Some((src, name)) = effective_source {
        let mut snippet_report = diag.into_inner();
        snippet_report = snippet_report.with_source_code(miette::NamedSource::new(name, src));
        let mut snippet_out = String::new();
        let theme = if color {
            miette::GraphicalTheme::unicode()
        } else {
            miette::GraphicalTheme::unicode_nocolor()
        };
        let handler = GraphicalReportHandler::new().with_theme(theme);
        let _ = handler.render_report(&mut snippet_out, snippet_report.as_ref());
        out.push('\n');
        for line in snippet_out.lines() {
            out.push_str(&format!("  {line}\n"));
        }
    } else {
        // diag was not consumed above when effective_source is None; drop it
        drop(diag);
    }

    if let Some(help) = help.filter(|_| !has_snippet) {
        out.push_str(&format!("\n  {c_dim}↳ help:{c_reset} {help}\n"));
    }
    if let Some(fix) = fix {
        out.push_str(&format!("  {c_green}↳ fix:{c_reset}  {fix}\n"));
    }
    if !suggestions.is_empty() {
        out.push_str(&format!(
            "  {c_yellow}↳ did you mean:{c_reset} {}\n",
            suggestions.join(", ")
        ));
    }
    if let Some(url) = docs_url {
        out.push_str(&format!("  {c_cyan}↳ docs:{c_reset} {url}\n"));
    }

    out
}

#[derive(Serialize)]
struct JsonError {
    code: String,
    name: String,
    category: String,
    message: String,
    severity: Option<String>,
    help: Option<String>,
    fix: Option<String>,
    suggestions: Vec<String>,
    labels: Vec<JsonLabel>,
    cause_chain: Vec<String>,
    docs_url: Option<String>,
}

#[derive(Serialize)]
struct JsonLabel {
    message: Option<String>,
    offset: usize,
    length: usize,
}

fn render_json(diag: FshDiag) -> String {
    let diag_ref = diag.report.as_ref();
    let severities = [
        (miette::Severity::Error, "error"),
        (miette::Severity::Warning, "warning"),
        (miette::Severity::Advice, "advice"),
    ];
    let sev_str = diag_ref.severity().and_then(|s| {
        severities
            .iter()
            .find(|(k, _)| *k == s)
            .map(|(_, v)| v.to_string())
    });

    let labels = diag_ref
        .labels()
        .map(|labels| {
            labels
                .map(|l| JsonLabel {
                    message: l.label().map(|s| s.to_string()),
                    offset: l.offset(),
                    length: l.len(),
                })
                .collect()
        })
        .unwrap_or_default();

    let mut cause_chain = Vec::new();
    let mut current = diag_ref.source();
    while let Some(src) = current {
        cause_chain.push(src.to_string());
        current = src.source();
    }

    let code_str = diag
        .code
        .map(|c| c.code_str().to_string())
        .or_else(|| diag_ref.code().map(|c| c.to_string()))
        .unwrap_or_else(|| "FSH-GEN-001".into());

    let name = diag
        .code
        .map(|c| c.name().to_string())
        .unwrap_or_else(|| diag.category.to_string());

    let docs_url = diag.code.and_then(|c| c.docs_url());

    let json_err = JsonError {
        code: code_str,
        name,
        category: diag.category.to_string(),
        message: diag_ref.to_string(),
        severity: sev_str,
        help: diag_ref.help().map(|h| h.to_string()),
        fix: diag.fix,
        suggestions: diag.suggestions,
        labels,
        cause_chain,
        docs_url,
    };

    serde_json::to_string_pretty(&json_err).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}
