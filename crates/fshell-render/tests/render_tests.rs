// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::diagnostic::{DiagnosticExt, ErrorCode, FshDiag};
use fshell_render::{RenderConfig, RenderFormat, render};
use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
#[error("test error occurred")]
#[diagnostic(code(test::error))]
struct TestError {
    #[source_code]
    src: NamedSource<String>,

    #[label("here")]
    bad_bit: SourceSpan,
}

impl DiagnosticExt for TestError {
    fn category(&self) -> &'static str {
        "E001"
    }

    fn code_enum(&self) -> Option<ErrorCode> {
        Some(ErrorCode::DivisionByZero)
    }

    fn fix(&self) -> Option<String> {
        Some("let x = 1 / 1".to_string())
    }

    fn suggestions(&self) -> Vec<String> {
        vec!["check your math".into()]
    }
}

fn make_diag() -> FshDiag {
    let err = TestError {
        src: NamedSource::new("test.fsh", "let x = 1 / 0".to_string()),
        bad_bit: (10, 5).into(),
    };
    FshDiag::new(err)
}

#[test]
fn test_render_graphical() {
    let diag = make_diag();
    let config = RenderConfig {
        format: RenderFormat::Graphical,
        color: false,
        is_interactive: false,
    };
    let output = render(diag, Some("let x = 1 / 0"), "test.fsh", &config);
    assert!(output.contains("test error occurred"), "output: {output}");
    assert!(output.contains("fix: let x = 1 / 1"), "output: {output}");
    assert!(output.len() > 50, "graphical output too short: {output}");
}

#[test]
fn test_render_compact() {
    let diag = make_diag();
    let config = RenderConfig {
        format: RenderFormat::Compact,
        color: false,
        is_interactive: false,
    };
    let output = render(diag, Some("let x = 1 / 0"), "test.fsh", &config);
    assert!(output.contains("test error occurred"));
    assert!(output.contains("FSH-MATH-001"));
    assert!(output.contains("fix: let x = 1 / 1"));
}

#[test]
fn test_render_explain() {
    let diag = make_diag();
    let config = RenderConfig {
        format: RenderFormat::Explain,
        color: false,
        is_interactive: false,
    };
    let output = render(diag, Some("let x = 1 / 0"), "test.fsh", &config);
    assert!(output.contains("DivisionByZero: FSH-MATH-001"));
    assert!(output.contains("test error occurred"));
    // docs line is suppressed when FSHELL_DOCS_BASE is not set (local dev)
    if option_env!("FSHELL_DOCS_BASE").is_some() {
        assert!(output.contains("FSH-MATH-001") && output.contains("docs:"));
    } else {
        assert!(!output.contains("↳ docs:"));
    }
}

#[test]
fn test_render_auto() {
    let diag_interactive = make_diag();
    let config_interactive = RenderConfig {
        format: RenderFormat::Auto,
        color: false,
        is_interactive: true,
    };
    let output_compact = render(
        diag_interactive,
        Some("let x = 1 / 0"),
        "test.fsh",
        &config_interactive,
    );
    assert!(output_compact.contains("✖ [DivisionByZero]"));

    let diag_script = make_diag();
    let config_script = RenderConfig {
        format: RenderFormat::Auto,
        color: false,
        is_interactive: false,
    };
    let output_graphical = render(
        diag_script,
        Some("let x = 1 / 0"),
        "test.fsh",
        &config_script,
    );
    assert!(output_graphical.contains("test error occurred"));
    assert!(output_graphical.contains("fix: let x = 1 / 1"));
}

#[test]
fn test_render_json() {
    let diag = make_diag();
    let config = RenderConfig {
        format: RenderFormat::Json,
        color: false,
        is_interactive: false,
    };
    let output = render(diag, None, "", &config);
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["code"], "FSH-MATH-001");
    assert_eq!(parsed["name"], "DivisionByZero");
    assert!(
        parsed["message"]
            .as_str()
            .unwrap()
            .contains("test error occurred")
    );
    assert_eq!(parsed["fix"], "let x = 1 / 1");
}
