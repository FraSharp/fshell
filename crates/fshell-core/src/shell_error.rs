// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::diagnostic::{DiagnosticExt, ErrorCode, FshDiag};
use miette::{Diagnostic, LabeledSpan, Severity, SourceSpan};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellError {
    pub code: ErrorCode,
    pub message: String,
    pub span: Option<SourceSpan>,
    pub help: Option<String>,
    pub fix: Option<String>,
    pub suggestions: Vec<String>,
    pub secondary_spans: Vec<(SourceSpan, String)>,
    pub severity: Option<Severity>,
}

impl fmt::Display for ShellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ShellError {}

impl Diagnostic for ShellError {
    fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(self.code))
    }

    fn severity(&self) -> Option<Severity> {
        self.severity
    }

    fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        self.help
            .as_deref()
            .map(|h| Box::new(h) as Box<dyn fmt::Display + 'a>)
    }

    fn labels<'a>(&'a self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + 'a>> {
        let mut labels: Vec<LabeledSpan> = Vec::new();
        if let Some(span) = self.span {
            labels.push(LabeledSpan::new_with_span(None, span));
        }
        for (span, label) in &self.secondary_spans {
            labels.push(LabeledSpan::new_with_span(Some(label.clone()), *span));
        }
        if labels.is_empty() {
            None
        } else {
            Some(Box::new(labels.into_iter()))
        }
    }
}

impl DiagnosticExt for ShellError {
    fn category(&self) -> &'static str {
        self.code.category()
    }

    fn code_enum(&self) -> Option<ErrorCode> {
        Some(self.code)
    }

    fn fix(&self) -> Option<String> {
        self.fix.clone()
    }

    fn suggestions(&self) -> Vec<String> {
        self.suggestions.clone()
    }
}

impl ShellError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            span: None,
            help: None,
            fix: None,
            suggestions: Vec::new(),
            secondary_spans: Vec::new(),
            severity: None,
        }
    }

    pub fn with_span(mut self, span: SourceSpan) -> Self {
        self.span = Some(span);
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }

    pub fn with_suggestion(mut self, s: impl Into<String>) -> Self {
        self.suggestions.push(s.into());
        self
    }

    pub fn with_suggestions(mut self, v: Vec<String>) -> Self {
        self.suggestions = v;
        self
    }

    pub fn with_secondary_span(mut self, span: SourceSpan, label: impl Into<String>) -> Self {
        self.secondary_spans.push((span, label.into()));
        self
    }

    pub fn with_severity(mut self, severity: Severity) -> Self {
        self.severity = Some(severity);
        self
    }

    pub fn is_condition_false(&self) -> bool {
        self.code == ErrorCode::ConditionFalse
    }

    pub fn condition_false() -> Self {
        Self {
            code: ErrorCode::ConditionFalse,
            message: "false".to_string(),
            span: None,
            help: None,
            fix: None,
            suggestions: Vec::new(),
            secondary_spans: Vec::new(),
            severity: None,
        }
    }

    pub fn variable_not_found(name: &str, span: Option<SourceSpan>) -> Self {
        Self::new(
            ErrorCode::VariableNotFound,
            format!("Variable '{name}' is not defined. Use `let` to declare variables before assignment."),
        )
        .with_help(format!(
            "Declare with `let {name} = <value>` or check the spelling."
        ))
        .with_fix(format!("let {name} = <value>"))
        .with_suggestion(format!(
            "Declare with `let {name} = <value>` or check the spelling."
        ))
        .maybe_with_span(span)
    }

    pub fn function_not_found(name: &str, span: Option<SourceSpan>) -> Self {
        Self::new(
            ErrorCode::FunctionNotFound,
            format!("Function '{name}' is not defined."),
        )
        .with_help(format!(
            "Define with `fn {name}(...) {{ ... }}` or check the spelling."
        ))
        .maybe_with_span(span)
    }

    pub fn type_mismatch(expected: &str, found: &str, span: Option<SourceSpan>) -> Self {
        Self::new(
            ErrorCode::TypeError,
            format!("Type mismatch: expected {expected}, found {found}"),
        )
        .with_help("Use a type conversion function like `to_text()` or check the operand types.")
        .with_fix(format!("try conversion to {expected}"))
        .with_suggestion("Try a type conversion like `to_text()` or `to_int()`.".to_string())
        .maybe_with_span(span)
    }

    pub fn capability_denied(cmd_name: &str, action: &str, span: Option<SourceSpan>) -> Self {
        Self::new(
            ErrorCode::CapabilityDenied,
            format!("Capability denied: {cmd_name} requested {action}"),
        )
        .with_help(format!(
            "Grant the capability with `with caps({action}) {{ ... }}` or run without --strict."
        ))
        .with_suggestion(format!(
            "Wrap with `with caps({action}) {{ ... }}` or run without --strict."
        ))
        .maybe_with_span(span)
    }

    pub fn division_by_zero(span: Option<SourceSpan>) -> Self {
        Self::new(ErrorCode::DivisionByZero, "Division by zero")
            .with_help("Ensure the divisor is not zero before dividing.")
            .with_fix("ensure divisor != 0".to_string())
            .with_suggestion("Check the divisor before dividing.".to_string())
            .maybe_with_span(span)
    }

    pub fn pipeline_error(message: impl Into<String>, span: Option<SourceSpan>) -> Self {
        Self::new(ErrorCode::PipelineError, message)
            .with_help("Check the pipeline arguments and structure.")
            .maybe_with_span(span)
    }

    pub fn mutation_not_allowed(message: impl Into<String>, span: Option<SourceSpan>) -> Self {
        Self::new(ErrorCode::ImmutableVariable, message)
            .with_help("Reactive cells are read-only. Use an `unsafe` block to mutate them.")
            .maybe_with_span(span)
    }

    pub fn io_error(message: impl Into<String>, span: Option<SourceSpan>) -> Self {
        Self::new(ErrorCode::IoError, message)
            .with_help("Check that the file path exists and is accessible.")
            .maybe_with_span(span)
    }

    pub fn generic(message: impl Into<String>, span: Option<SourceSpan>) -> Self {
        Self::new(ErrorCode::General, message)
            .with_help("Review the command syntax and parameters.")
            .maybe_with_span(span)
    }

    pub fn match_non_exhaustive(span: Option<SourceSpan>) -> Self {
        Self::new(
            ErrorCode::RuntimeError,
            "Match expression is non-exhaustive — no pattern matched the value",
        )
        .with_help("Add a catch-all arm (`_ =>`) to handle unmatched values.")
        .with_fix("_ => <default_value>".to_string())
        .maybe_with_span(span)
    }

    pub fn cycle_detected(span: Option<SourceSpan>) -> Self {
        Self::new(
            ErrorCode::CycleDetected,
            "Cycle detected in reactive cell dependencies",
        )
        .with_help("Remove the circular dependency between reactive cells.")
        .maybe_with_span(span)
    }

    pub fn command_not_found(cmd: &str, span: Option<SourceSpan>) -> Self {
        Self::new(
            ErrorCode::CommandNotFound,
            format!("command not found: {cmd}"),
        )
        .with_help("Check the command name or path.")
        .with_suggestion("Check the command name or path.".to_string())
        .maybe_with_span(span)
    }

    pub fn file_not_found(path: &str, span: Option<SourceSpan>) -> Self {
        Self::new(ErrorCode::FileNotFound, format!("file not found: {path}"))
            .with_help("Check that the target path exists and is spelled correctly.")
            .maybe_with_span(span)
    }

    pub fn permission_denied(path: &str, span: Option<SourceSpan>) -> Self {
        Self::new(
            ErrorCode::PermissionDenied,
            format!("permission denied for '{path}'"),
        )
        .with_help("Verify read/write permissions or grant required shell capabilities.")
        .maybe_with_span(span)
    }

    pub fn invalid_argument(cmd: &str, arg: &str, span: Option<SourceSpan>) -> Self {
        Self::new(
            ErrorCode::InvalidArgument,
            format!("{cmd}: invalid argument '{arg}'"),
        )
        .with_help(format!("Use `help {cmd}` for syntax and allowed options."))
        .maybe_with_span(span)
    }

    pub fn missing_argument(cmd: &str, desc: &str, span: Option<SourceSpan>) -> Self {
        Self::new(
            ErrorCode::MissingArgument,
            format!("{cmd}: missing argument: {desc}"),
        )
        .with_help(format!("Provide the required argument. See `help {cmd}`."))
        .maybe_with_span(span)
    }

    pub fn numeric_overflow(message: impl Into<String>, span: Option<SourceSpan>) -> Self {
        Self::new(ErrorCode::NumericOverflow, message)
            .with_help("An arithmetic operation overflowed. Check operand sizes.")
            .maybe_with_span(span)
    }

    fn maybe_with_span(mut self, span: Option<SourceSpan>) -> Self {
        self.span = span;
        self
    }
}

impl From<String> for ShellError {
    fn from(message: String) -> Self {
        Self::new(ErrorCode::General, message)
    }
}

impl From<&str> for ShellError {
    fn from(message: &str) -> Self {
        Self::new(ErrorCode::General, message.to_string())
    }
}

impl From<ShellError> for FshDiag {
    fn from(err: ShellError) -> Self {
        FshDiag::new(err)
    }
}
