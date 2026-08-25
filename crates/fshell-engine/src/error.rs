// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::diagnostic::{DiagnosticExt, ErrorCode, FshDiag};
use miette::{Diagnostic, SourceSpan};
use thiserror::Error;

#[derive(Error, Diagnostic, Debug, Clone, PartialEq)]
pub enum EngineError {
    #[error("Variable '{name}' is not defined. Use `let` to declare variables before assignment.")]
    #[diagnostic(
        code = "FSH-SCOPE-001",
        help("Declare with `let {name} = <value>` or check the spelling.")
    )]
    VariableNotFound {
        name: String,
        #[label("variable not found here")]
        span: Option<SourceSpan>,
    },

    #[error("Type mismatch: expected {expected}, found {found}")]
    #[diagnostic(
        code = "FSH-TYPE-001",
        help("Use a type conversion function like `to_text()` or check the operand types.")
    )]
    TypeMismatch {
        expected: String,
        found: String,
        #[label("type mismatch here")]
        span: Option<SourceSpan>,
    },

    #[error("Capability denied: {cmd_name} requested {action}")]
    #[diagnostic(
        code = "FSH-EXEC-003",
        help("Grant the capability with `with caps({action}) {{ ... }}` or run without --strict.")
    )]
    CapabilityDenied {
        cmd_name: String,
        action: String,
        #[label("permission denied here")]
        span: Option<SourceSpan>,
    },

    #[error("Division by zero")]
    #[diagnostic(
        code = "FSH-MATH-001",
        help("Ensure the divisor is not zero before dividing.")
    )]
    DivisionByZero {
        #[label("division by zero here")]
        span: Option<SourceSpan>,
    },

    /// Logical condition evaluated to false (test(1) / false builtin).
    /// Not a hard failure — consumed by `&&`/`||`, `if`, and `$?` plumbing.
    /// Never shown to the user as an error line.
    #[error("false")]
    #[diagnostic(code = "FSH-EXEC-004")]
    ConditionFalse {
        #[label("condition was false here")]
        span: Option<SourceSpan>,
    },

    #[error("{message}")]
    #[diagnostic(
        code = "FSH-PIPE-001",
        help("Check the pipeline arguments and structure.")
    )]
    PipelineError {
        message: String,
        #[label("error in pipeline stage")]
        span: Option<SourceSpan>,
    },

    #[error("Mutation not allowed: {message}")]
    #[diagnostic(
        code = "FSH-SCOPE-003",
        help("Reactive cells are read-only. Use an `unsafe` block to mutate them.")
    )]
    MutationNotAllowed {
        message: String,
        #[label("unauthorized mutation here")]
        span: Option<SourceSpan>,
    },

    #[error("{message}")]
    #[diagnostic(
        code = "FSH-IO-003",
        help("Check that the file path exists and is accessible.")
    )]
    IoError {
        message: String,
        #[label("io error here")]
        span: Option<SourceSpan>,
    },

    #[error("{message}")]
    #[diagnostic(
        code = "FSH-GEN-001",
        help("Review the command syntax and parameters.")
    )]
    Generic {
        message: String,
        #[label("error here")]
        span: Option<SourceSpan>,
    },

    #[error("{0}")]
    #[diagnostic(transparent)]
    Parse(#[from] fshell_core::ParseError),

    /// A cycle was detected in the reactive cell dependency graph.
    #[error("Cycle detected in reactive cell dependencies")]
    #[diagnostic(
        code = "FSH-RT-003",
        help("Remove the circular dependency between reactive cells.")
    )]
    CycleDetected {
        #[label("cycle detected here")]
        span: Option<SourceSpan>,
    },

    #[error("Match expression is non-exhaustive — no pattern matched the value")]
    #[diagnostic(
        code = "FSH-EXEC-002",
        help("Add a catch-all arm (`_ =>`) to handle unmatched values.")
    )]
    MatchNonExhaustive {
        #[label("non-exhaustive match here")]
        span: Option<SourceSpan>,
    },

    /// Internal control-flow signal for `break` statements.
    /// Not a real error — caught by loop evaluators.
    #[error("break")]
    #[diagnostic(code = "FSH-CTRL-001")]
    BreakSignal,

    /// Internal control-flow signal for `continue` statements.
    /// Not a real error — caught by loop evaluators.
    #[error("continue")]
    #[diagnostic(code = "FSH-CTRL-002")]
    ContinueSignal,

    /// Internal control-flow signal for `return <val>` statements.
    /// Not a real error — caught by function body evaluators.
    #[error("return")]
    #[diagnostic(code = "FSH-CTRL-003")]
    ReturnSignal(fshell_core::Val),

    /// Internal control-flow signal for `exit <code>` statements.
    /// Caught by the REPL loop to terminate.
    #[error("exit")]
    #[diagnostic(code = "FSH-CTRL-004")]
    ExitSignal(i32),
}

impl From<String> for EngineError {
    fn from(message: String) -> Self {
        EngineError::Generic {
            message,
            span: None,
        }
    }
}

impl From<&str> for EngineError {
    fn from(message: &str) -> Self {
        EngineError::Generic {
            message: message.to_string(),
            span: None,
        }
    }
}

impl From<EngineError> for String {
    fn from(err: EngineError) -> Self {
        err.to_string()
    }
}

impl EngineError {
    pub fn span(&self) -> Option<SourceSpan> {
        match self {
            EngineError::VariableNotFound { span, .. } => *span,
            EngineError::TypeMismatch { span, .. } => *span,
            EngineError::CapabilityDenied { span, .. } => *span,
            EngineError::DivisionByZero { span, .. } => *span,
            EngineError::ConditionFalse { span, .. } => *span,
            EngineError::PipelineError { span, .. } => *span,
            EngineError::MutationNotAllowed { span, .. } => *span,
            EngineError::IoError { span, .. } => *span,
            EngineError::Generic { span, .. } => *span,
            EngineError::Parse(_) => None,
            EngineError::MatchNonExhaustive { span, .. } => *span,
            EngineError::BreakSignal => None,
            EngineError::ContinueSignal => None,
            EngineError::ReturnSignal(_) => None,
            EngineError::ExitSignal(_) => None,
            EngineError::CycleDetected { span, .. } => *span,
        }
    }

    pub fn set_span(&mut self, new_span: SourceSpan) {
        match self {
            EngineError::VariableNotFound { span, .. } => *span = Some(new_span),
            EngineError::TypeMismatch { span, .. } => *span = Some(new_span),
            EngineError::CapabilityDenied { span, .. } => *span = Some(new_span),
            EngineError::DivisionByZero { span, .. } => *span = Some(new_span),
            EngineError::ConditionFalse { span, .. } => *span = Some(new_span),
            EngineError::PipelineError { span, .. } => *span = Some(new_span),
            EngineError::MutationNotAllowed { span, .. } => *span = Some(new_span),
            EngineError::IoError { span, .. } => *span = Some(new_span),
            EngineError::Generic { span, .. } => *span = Some(new_span),
            EngineError::Parse(_) => {}
            EngineError::MatchNonExhaustive { span, .. } => *span = Some(new_span),
            // Control-flow signals carry no span.
            EngineError::BreakSignal => {}
            EngineError::ContinueSignal => {}
            EngineError::ReturnSignal(_) => {}
            EngineError::ExitSignal(_) => {}
            EngineError::CycleDetected { span, .. } => *span = Some(new_span),
        }
    }

    pub fn contains(&self, pat: &str) -> bool {
        self.to_string().contains(pat)
    }

    pub fn is_condition_false(&self) -> bool {
        matches!(self, EngineError::ConditionFalse { .. })
    }
}

impl DiagnosticExt for EngineError {
    fn category(&self) -> &'static str {
        match self {
            EngineError::VariableNotFound { .. } => "scope",
            EngineError::TypeMismatch { .. } => "types",
            EngineError::CapabilityDenied { .. } => "security",
            EngineError::DivisionByZero { .. } => "arithmetic",
            EngineError::ConditionFalse { .. } => "condition",
            EngineError::PipelineError { .. } => "pipeline",
            EngineError::MutationNotAllowed { .. } => "security",
            EngineError::IoError { .. } => "io",
            EngineError::Generic { .. } => "general",
            EngineError::Parse(p) => p.category(),
            EngineError::MatchNonExhaustive { .. } => "pattern",
            EngineError::CycleDetected { .. } => "reactive",
            EngineError::BreakSignal
            | EngineError::ContinueSignal
            | EngineError::ReturnSignal(_)
            | EngineError::ExitSignal(_) => "control",
        }
    }

    fn code_enum(&self) -> Option<ErrorCode> {
        match self {
            EngineError::VariableNotFound { .. } => Some(ErrorCode::VariableNotFound),
            EngineError::TypeMismatch { .. } => Some(ErrorCode::TypeError),
            EngineError::CapabilityDenied { .. } => Some(ErrorCode::CapabilityDenied),
            EngineError::DivisionByZero { .. } => Some(ErrorCode::DivisionByZero),
            EngineError::ConditionFalse { .. } => Some(ErrorCode::ConditionFalse),
            EngineError::PipelineError { .. } => Some(ErrorCode::PipelineError),
            EngineError::MutationNotAllowed { .. } => Some(ErrorCode::ImmutableVariable),
            EngineError::IoError { .. } => Some(ErrorCode::IoError),
            EngineError::Generic { message, .. } if message.contains("not found") => {
                Some(ErrorCode::CommandNotFound)
            }
            EngineError::Generic { .. } => Some(ErrorCode::General),
            EngineError::Parse(p) => p.code_enum(),
            EngineError::MatchNonExhaustive { .. } => Some(ErrorCode::RuntimeError),
            EngineError::CycleDetected { .. } => Some(ErrorCode::CycleDetected),
            EngineError::BreakSignal
            | EngineError::ContinueSignal
            | EngineError::ReturnSignal(_)
            | EngineError::ExitSignal(_) => None,
        }
    }

    fn fix(&self) -> Option<String> {
        match self {
            EngineError::VariableNotFound { name, .. } => Some(format!("let {name} = <value>")),
            EngineError::DivisionByZero { .. } => Some("ensure divisor != 0".into()),
            EngineError::MatchNonExhaustive { .. } => Some("_ => <default_value>".into()),
            EngineError::TypeMismatch { expected, .. } => {
                Some(format!("try conversion to {expected}"))
            }
            _ => None,
        }
    }

    fn suggestions(&self) -> Vec<String> {
        match self {
            EngineError::VariableNotFound { name, .. } => {
                vec![format!(
                    "Declare with `let {name} = <value>` or check the spelling."
                )]
            }
            EngineError::TypeMismatch { .. } => {
                vec!["Try a type conversion like `to_text()` or `to_int()`.".into()]
            }
            EngineError::CapabilityDenied { action, .. } => {
                vec![format!(
                    "Wrap with `with caps({action}) {{ ... }}` or run without --strict."
                )]
            }
            EngineError::DivisionByZero { .. } => {
                vec!["Check the divisor before dividing.".into()]
            }
            EngineError::Generic { message, .. } if message.contains("not found") => {
                vec!["Check the command name or path.".into()]
            }
            _ => Vec::new(),
        }
    }
}

impl From<EngineError> for FshDiag {
    fn from(err: EngineError) -> Self {
        FshDiag::new(err)
    }
}
