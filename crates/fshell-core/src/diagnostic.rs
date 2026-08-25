// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::Val;
use crate::val::FxIndexMap;
use fshell_hash::FxBuildHasher;
use miette::{Diagnostic, LabeledSpan, Severity, SourceSpan};
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use thiserror::Error;
use ustr::ustr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ErrorCode {
    // General
    General, // FSH-GEN-001

    // Parse / Syntax
    ParseError,    // FSH-PARSE-001
    SyntaxError,   // FSH-PARSE-002
    UnexpectedEof, // FSH-PARSE-003
    ExpectedChar,  // FSH-PARSE-004
    ExpectedToken, // FSH-PARSE-005
    UnclosedQuote, // FSH-PARSE-006

    // Scope & Variables
    VariableNotFound,  // FSH-SCOPE-001
    FunctionNotFound,  // FSH-SCOPE-002
    ImmutableVariable, // FSH-SCOPE-003

    // Types & Constraints
    TypeError,           // FSH-TYPE-001
    TypeErrorArgCount,   // FSH-TYPE-002
    TypeConstraintError, // FSH-TYPE-003

    // Arithmetic / Math
    DivisionByZero,  // FSH-MATH-001
    NumericOverflow, // FSH-MATH-002
    InvalidNumber,   // FSH-MATH-003

    // Execution & Commands
    CommandNotFound,  // FSH-EXEC-001
    RuntimeError,     // FSH-EXEC-002
    CapabilityDenied, // FSH-EXEC-003
    ConditionFalse,   // FSH-EXEC-004
    CommandFailed,    // FSH-EXEC-005

    // I/O & Filesystem
    FileNotFound,     // FSH-IO-001
    PermissionDenied, // FSH-IO-002
    IoError,          // FSH-IO-003
    AlreadyExists,    // FSH-IO-004
    IsDirectory,      // FSH-IO-005
    NotDirectory,     // FSH-IO-006

    // Network
    NetworkError,      // FSH-NET-001
    Timeout,           // FSH-NET-002
    ConnectionRefused, // FSH-NET-003

    // Pipelines
    PipelineError,    // FSH-PIPE-001
    SortTooManyItems, // FSH-PIPE-002
    BrokenPipe,       // FSH-PIPE-003

    // Sandbox
    SandboxDenied, // FSH-SBX-001
    SandboxError,  // FSH-SBX-002

    // Builtins
    BuiltinError,    // FSH-BLT-001
    InvalidArgument, // FSH-BLT-002
    MissingArgument, // FSH-BLT-003
    NotFound,        // FSH-BLT-005
    Unsupported,     // FSH-BLT-008

    // Runtime engine
    LockPoisoned,  // FSH-RT-001
    Cancelled,     // FSH-RT-002
    CycleDetected, // FSH-RT-003

    // Lint
    LintParseError,    // FSH-LNT-001
    LintEmptyStage,    // FSH-LNT-002
    LintEmptyPipeline, // FSH-LNT-003

    // Internal
    InternalError, // FSH-INT-001
    Unimplemented, // FSH-INT-002
}

impl ErrorCode {
    /// Return the canonical uppercase alphanumeric code string (e.g. "FSH-TYPE-001").
    pub fn code_str(&self) -> &'static str {
        match self {
            ErrorCode::General => "FSH-GEN-001",
            ErrorCode::ParseError => "FSH-PARSE-001",
            ErrorCode::SyntaxError => "FSH-PARSE-002",
            ErrorCode::UnexpectedEof => "FSH-PARSE-003",
            ErrorCode::ExpectedChar => "FSH-PARSE-004",
            ErrorCode::ExpectedToken => "FSH-PARSE-005",
            ErrorCode::UnclosedQuote => "FSH-PARSE-006",
            ErrorCode::VariableNotFound => "FSH-SCOPE-001",
            ErrorCode::FunctionNotFound => "FSH-SCOPE-002",
            ErrorCode::ImmutableVariable => "FSH-SCOPE-003",
            ErrorCode::TypeError => "FSH-TYPE-001",
            ErrorCode::TypeErrorArgCount => "FSH-TYPE-002",
            ErrorCode::TypeConstraintError => "FSH-TYPE-003",
            ErrorCode::DivisionByZero => "FSH-MATH-001",
            ErrorCode::NumericOverflow => "FSH-MATH-002",
            ErrorCode::InvalidNumber => "FSH-MATH-003",
            ErrorCode::CommandNotFound => "FSH-EXEC-001",
            ErrorCode::RuntimeError => "FSH-EXEC-002",
            ErrorCode::CapabilityDenied => "FSH-EXEC-003",
            ErrorCode::ConditionFalse => "FSH-EXEC-004",
            ErrorCode::CommandFailed => "FSH-EXEC-005",
            ErrorCode::FileNotFound => "FSH-IO-001",
            ErrorCode::PermissionDenied => "FSH-IO-002",
            ErrorCode::IoError => "FSH-IO-003",
            ErrorCode::AlreadyExists => "FSH-IO-004",
            ErrorCode::IsDirectory => "FSH-IO-005",
            ErrorCode::NotDirectory => "FSH-IO-006",
            ErrorCode::NetworkError => "FSH-NET-001",
            ErrorCode::Timeout => "FSH-NET-002",
            ErrorCode::ConnectionRefused => "FSH-NET-003",
            ErrorCode::PipelineError => "FSH-PIPE-001",
            ErrorCode::SortTooManyItems => "FSH-PIPE-002",
            ErrorCode::BrokenPipe => "FSH-PIPE-003",
            ErrorCode::SandboxDenied => "FSH-SBX-001",
            ErrorCode::SandboxError => "FSH-SBX-002",
            ErrorCode::BuiltinError => "FSH-BLT-001",
            ErrorCode::InvalidArgument => "FSH-BLT-002",
            ErrorCode::MissingArgument => "FSH-BLT-003",
            ErrorCode::NotFound => "FSH-BLT-005",
            ErrorCode::Unsupported => "FSH-BLT-008",
            ErrorCode::LockPoisoned => "FSH-RT-001",
            ErrorCode::Cancelled => "FSH-RT-002",
            ErrorCode::CycleDetected => "FSH-RT-003",
            ErrorCode::LintParseError => "FSH-LNT-001",
            ErrorCode::LintEmptyStage => "FSH-LNT-002",
            ErrorCode::LintEmptyPipeline => "FSH-LNT-003",
            ErrorCode::InternalError => "FSH-INT-001",
            ErrorCode::Unimplemented => "FSH-INT-002",
        }
    }

    /// User-facing PascalCase error name (e.g. "TypeError", "FileNotFound").
    pub fn name(&self) -> &'static str {
        match self {
            ErrorCode::General => "GeneralError",
            ErrorCode::ParseError | ErrorCode::SyntaxError => "SyntaxError",
            ErrorCode::UnexpectedEof => "UnexpectedEof",
            ErrorCode::ExpectedChar => "ExpectedChar",
            ErrorCode::ExpectedToken => "ExpectedToken",
            ErrorCode::UnclosedQuote => "UnclosedQuote",
            ErrorCode::VariableNotFound => "VariableNotFound",
            ErrorCode::FunctionNotFound => "FunctionNotFound",
            ErrorCode::ImmutableVariable => "ImmutableVariable",
            ErrorCode::TypeError => "TypeError",
            ErrorCode::TypeErrorArgCount => "ArgCountMismatch",
            ErrorCode::TypeConstraintError => "TypeConstraintError",
            ErrorCode::DivisionByZero => "DivisionByZero",
            ErrorCode::NumericOverflow => "NumericOverflow",
            ErrorCode::InvalidNumber => "InvalidNumber",
            ErrorCode::CommandNotFound => "CommandNotFound",
            ErrorCode::RuntimeError => "RuntimeError",
            ErrorCode::CapabilityDenied => "CapabilityDenied",
            ErrorCode::ConditionFalse => "ConditionFalse",
            ErrorCode::CommandFailed => "CommandFailed",
            ErrorCode::FileNotFound => "FileNotFound",
            ErrorCode::PermissionDenied => "PermissionDenied",
            ErrorCode::IoError => "IoError",
            ErrorCode::AlreadyExists => "AlreadyExists",
            ErrorCode::IsDirectory => "IsDirectory",
            ErrorCode::NotDirectory => "NotDirectory",
            ErrorCode::NetworkError => "NetworkError",
            ErrorCode::Timeout => "Timeout",
            ErrorCode::ConnectionRefused => "ConnectionRefused",
            ErrorCode::PipelineError => "PipelineError",
            ErrorCode::SortTooManyItems => "SortTooManyItems",
            ErrorCode::BrokenPipe => "BrokenPipe",
            ErrorCode::SandboxDenied => "SandboxDenied",
            ErrorCode::SandboxError => "SandboxError",
            ErrorCode::BuiltinError => "BuiltinError",
            ErrorCode::InvalidArgument => "InvalidArgument",
            ErrorCode::MissingArgument => "MissingArgument",
            ErrorCode::NotFound => "NotFound",
            ErrorCode::Unsupported => "Unsupported",
            ErrorCode::LockPoisoned => "LockPoisoned",
            ErrorCode::Cancelled => "Cancelled",
            ErrorCode::CycleDetected => "CycleDetected",
            ErrorCode::LintParseError => "LintParseError",
            ErrorCode::LintEmptyStage => "LintEmptyStage",
            ErrorCode::LintEmptyPipeline => "LintEmptyPipeline",
            ErrorCode::InternalError => "InternalError",
            ErrorCode::Unimplemented => "Unimplemented",
        }
    }

    /// Semantic error category identifier (e.g. "types", "io", "scope", "syntax").
    pub fn category(&self) -> &'static str {
        match self {
            ErrorCode::General => "general",
            ErrorCode::ParseError
            | ErrorCode::SyntaxError
            | ErrorCode::UnexpectedEof
            | ErrorCode::ExpectedChar
            | ErrorCode::ExpectedToken
            | ErrorCode::UnclosedQuote => "syntax",
            ErrorCode::VariableNotFound
            | ErrorCode::FunctionNotFound
            | ErrorCode::ImmutableVariable => "scope",
            ErrorCode::TypeError
            | ErrorCode::TypeErrorArgCount
            | ErrorCode::TypeConstraintError => "types",
            ErrorCode::DivisionByZero | ErrorCode::NumericOverflow | ErrorCode::InvalidNumber => {
                "arithmetic"
            }
            ErrorCode::CommandNotFound
            | ErrorCode::RuntimeError
            | ErrorCode::ConditionFalse
            | ErrorCode::CommandFailed => "execution",
            ErrorCode::CapabilityDenied | ErrorCode::SandboxDenied | ErrorCode::SandboxError => {
                "security"
            }
            ErrorCode::FileNotFound
            | ErrorCode::PermissionDenied
            | ErrorCode::IoError
            | ErrorCode::AlreadyExists
            | ErrorCode::IsDirectory
            | ErrorCode::NotDirectory => "io",
            ErrorCode::NetworkError | ErrorCode::Timeout | ErrorCode::ConnectionRefused => {
                "network"
            }
            ErrorCode::PipelineError | ErrorCode::SortTooManyItems | ErrorCode::BrokenPipe => {
                "pipeline"
            }
            ErrorCode::BuiltinError
            | ErrorCode::InvalidArgument
            | ErrorCode::MissingArgument
            | ErrorCode::NotFound
            | ErrorCode::Unsupported => "builtin",
            ErrorCode::LockPoisoned | ErrorCode::Cancelled | ErrorCode::CycleDetected => "runtime",
            ErrorCode::LintParseError
            | ErrorCode::LintEmptyStage
            | ErrorCode::LintEmptyPipeline => "lint",
            ErrorCode::InternalError | ErrorCode::Unimplemented => "internal",
        }
    }

    /// Documentation URL for this error code.
    ///
    /// The base is configured at compile-time via `FSHELL_DOCS_BASE`.
    /// When the variable is not set (local development) no URL is produced
    /// and renderers suppress the `↳ docs:` line.
    pub fn docs_url(&self) -> Option<String> {
        let base = option_env!("FSHELL_DOCS_BASE")?;
        Some(format!(
            "{}/{}",
            base.trim_end_matches('/'),
            self.code_str()
        ))
    }

    /// Default human-readable description of what this error code signifies.
    pub fn default_description(&self) -> &'static str {
        match self {
            ErrorCode::General => "A general or uncategorized failure occurred.",
            ErrorCode::ParseError | ErrorCode::SyntaxError => {
                "The input could not be parsed according to fsh grammar rules."
            }
            ErrorCode::UnexpectedEof => "The command ended prematurely while parsing.",
            ErrorCode::ExpectedChar => "A required character was missing at this location.",
            ErrorCode::ExpectedToken => "A required keyword, identifier, or token was missing.",
            ErrorCode::UnclosedQuote => "A quote or escape sequence was not properly terminated.",
            ErrorCode::VariableNotFound => {
                "An identifier was referenced that has not been defined in the current scope."
            }
            ErrorCode::FunctionNotFound => "A function was called that does not exist.",
            ErrorCode::ImmutableVariable => {
                "Attempted to mutate an immutable or reactive cell value."
            }
            ErrorCode::TypeError => "Operands or arguments have incompatible data types.",
            ErrorCode::TypeErrorArgCount => {
                "A command or function received an incorrect number of arguments."
            }
            ErrorCode::TypeConstraintError => {
                "A value did not satisfy the declared type constraint."
            }
            ErrorCode::DivisionByZero => "Attempted to divide or modulo by zero.",
            ErrorCode::NumericOverflow => {
                "An arithmetic operation resulted in an integer or float overflow."
            }
            ErrorCode::InvalidNumber => "A string could not be parsed as a valid numeric value.",
            ErrorCode::CommandNotFound => {
                "The specified command, builtin, or alias could not be found."
            }
            ErrorCode::RuntimeError => "An error occurred during evaluation or script execution.",
            ErrorCode::CapabilityDenied => {
                "An operation was denied because the required capability was not granted."
            }
            ErrorCode::ConditionFalse => "A test or boolean condition evaluated to false.",
            ErrorCode::CommandFailed => {
                "An external process or builtin command exited with a non-zero status."
            }
            ErrorCode::FileNotFound => "The specified file or directory path does not exist.",
            ErrorCode::PermissionDenied => {
                "Access to the requested file or system resource was denied by the OS."
            }
            ErrorCode::IoError => "An underlying I/O system call returned an error.",
            ErrorCode::AlreadyExists => {
                "Cannot create a file or directory because it already exists."
            }
            ErrorCode::IsDirectory => "Expected a file, but the target is a directory.",
            ErrorCode::NotDirectory => "Expected a directory, but the target is a file.",
            ErrorCode::NetworkError => "A network request failed or the host could not be reached.",
            ErrorCode::Timeout => "The operation did not complete within the allotted time limit.",
            ErrorCode::ConnectionRefused => {
                "The remote network host actively refused the connection."
            }
            ErrorCode::PipelineError => {
                "A failure occurred within an active pipeline processing stage."
            }
            ErrorCode::SortTooManyItems => {
                "The sort stage exceeded maximum in-memory buffering capacity."
            }
            ErrorCode::BrokenPipe => {
                "A downstream pipeline stage closed the channel while items were sending."
            }
            ErrorCode::SandboxDenied => "The sandbox security profile blocked this operation.",
            ErrorCode::SandboxError => "The sandbox subsystem encountered an error.",
            ErrorCode::BuiltinError => "A builtin shell command failed.",
            ErrorCode::InvalidArgument => "An argument provided to a builtin was invalid.",
            ErrorCode::MissingArgument => "A mandatory argument was omitted from the command.",
            ErrorCode::NotFound => "The requested item or key could not be located.",
            ErrorCode::Unsupported => {
                "The requested operation is not supported on this platform or data type."
            }
            ErrorCode::LockPoisoned => "An internal concurrency lock was poisoned.",
            ErrorCode::Cancelled => "The active operation or job was cancelled.",
            ErrorCode::CycleDetected => {
                "A circular dependency cycle was detected in reactive cells."
            }
            ErrorCode::LintParseError => "Lint: the source contains a parse error.",
            ErrorCode::LintEmptyStage => "Lint: a trailing `|` has no following command.",
            ErrorCode::LintEmptyPipeline => "Lint: an empty pipeline has no effect.",
            ErrorCode::InternalError => "An internal engine invariant or assertion was violated.",
            ErrorCode::Unimplemented => "This feature has not yet been implemented.",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.code_str())
    }
}

impl FromStr for ErrorCode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "FSH-GEN-001" => Ok(ErrorCode::General),
            "FSH-PARSE-001" => Ok(ErrorCode::ParseError),
            "FSH-PARSE-002" => Ok(ErrorCode::SyntaxError),
            "FSH-PARSE-003" => Ok(ErrorCode::UnexpectedEof),
            "FSH-PARSE-004" => Ok(ErrorCode::ExpectedChar),
            "FSH-PARSE-005" => Ok(ErrorCode::ExpectedToken),
            "FSH-PARSE-006" => Ok(ErrorCode::UnclosedQuote),
            "FSH-SCOPE-001" => Ok(ErrorCode::VariableNotFound),
            "FSH-SCOPE-002" => Ok(ErrorCode::FunctionNotFound),
            "FSH-SCOPE-003" => Ok(ErrorCode::ImmutableVariable),
            "FSH-TYPE-001" => Ok(ErrorCode::TypeError),
            "FSH-TYPE-002" => Ok(ErrorCode::TypeErrorArgCount),
            "FSH-TYPE-003" => Ok(ErrorCode::TypeConstraintError),
            "FSH-MATH-001" => Ok(ErrorCode::DivisionByZero),
            "FSH-MATH-002" => Ok(ErrorCode::NumericOverflow),
            "FSH-MATH-003" => Ok(ErrorCode::InvalidNumber),
            "FSH-EXEC-001" => Ok(ErrorCode::CommandNotFound),
            "FSH-EXEC-002" => Ok(ErrorCode::RuntimeError),
            "FSH-EXEC-003" => Ok(ErrorCode::CapabilityDenied),
            "FSH-EXEC-004" => Ok(ErrorCode::ConditionFalse),
            "FSH-EXEC-005" | "FSH-BLT-004" => Ok(ErrorCode::CommandFailed),
            "FSH-IO-001" => Ok(ErrorCode::FileNotFound),
            "FSH-IO-002" => Ok(ErrorCode::PermissionDenied),
            "FSH-IO-003" => Ok(ErrorCode::IoError),
            "FSH-IO-004" => Ok(ErrorCode::AlreadyExists),
            "FSH-IO-005" => Ok(ErrorCode::IsDirectory),
            "FSH-IO-006" => Ok(ErrorCode::NotDirectory),
            "FSH-NET-001" => Ok(ErrorCode::NetworkError),
            "FSH-NET-002" | "FSH-BLT-007" => Ok(ErrorCode::Timeout),
            "FSH-NET-003" => Ok(ErrorCode::ConnectionRefused),
            "FSH-PIPE-001" => Ok(ErrorCode::PipelineError),
            "FSH-PIPE-002" => Ok(ErrorCode::SortTooManyItems),
            "FSH-PIPE-003" => Ok(ErrorCode::BrokenPipe),
            "FSH-SBX-001" => Ok(ErrorCode::SandboxDenied),
            "FSH-SBX-002" => Ok(ErrorCode::SandboxError),
            "FSH-BLT-001" => Ok(ErrorCode::BuiltinError),
            "FSH-BLT-002" => Ok(ErrorCode::InvalidArgument),
            "FSH-BLT-003" => Ok(ErrorCode::MissingArgument),
            "FSH-BLT-005" => Ok(ErrorCode::NotFound),
            "FSH-BLT-006" => Ok(ErrorCode::AlreadyExists),
            "FSH-BLT-008" => Ok(ErrorCode::Unsupported),
            "FSH-RT-001" => Ok(ErrorCode::LockPoisoned),
            "FSH-RT-002" => Ok(ErrorCode::Cancelled),
            "FSH-RT-003" => Ok(ErrorCode::CycleDetected),
            "FSH-LNT-001" => Ok(ErrorCode::LintParseError),
            "FSH-LNT-002" => Ok(ErrorCode::LintEmptyStage),
            "FSH-LNT-003" => Ok(ErrorCode::LintEmptyPipeline),
            "FSH-INT-001" => Ok(ErrorCode::InternalError),
            "FSH-INT-002" => Ok(ErrorCode::Unimplemented),
            _ => Err(format!("Unknown error code: {s}")),
        }
    }
}

pub trait DiagnosticExt {
    fn category(&self) -> &'static str;
    fn code_enum(&self) -> Option<ErrorCode> {
        None
    }
    fn fix(&self) -> Option<String> {
        None
    }
    fn suggestions(&self) -> Vec<String> {
        Vec::new()
    }
    fn docs_url(&self) -> Option<String> {
        self.code_enum().and_then(|c| c.docs_url())
    }
    fn to_val(&self) -> Val {
        let mut m = FxIndexMap::with_hasher(FxBuildHasher::default());
        if let Some(code) = self.code_enum() {
            m.insert(ustr("code"), Val::String(code.code_str().to_string()));
            m.insert(ustr("name"), Val::String(code.name().to_string()));
            m.insert(ustr("category"), Val::String(code.category().to_string()));
            if let Some(url) = code.docs_url() {
                m.insert(ustr("docs_url"), Val::String(url));
            } else {
                m.insert(ustr("docs_url"), Val::Null);
            }
        } else {
            m.insert(ustr("category"), Val::String(self.category().to_string()));
            m.insert(ustr("docs_url"), Val::Null);
        }
        if let Some(fix) = self.fix() {
            m.insert(ustr("fix"), Val::String(fix));
        }
        let suggs: Vec<Val> = self.suggestions().into_iter().map(Val::String).collect();
        if !suggs.is_empty() {
            m.insert(ustr("suggestions"), Val::List(suggs));
        }
        Val::Map(m)
    }
}

pub trait ShellDiagnostic: Diagnostic + DiagnosticExt + Send + Sync {}
impl<T: Diagnostic + DiagnosticExt + Send + Sync> ShellDiagnostic for T {}

#[derive(Debug, Clone)]
pub struct FshDiag {
    pub report: std::sync::Arc<miette::Report>,
    pub category: &'static str,
    pub code: Option<ErrorCode>,
    pub fix: Option<String>,
    pub suggestions: Vec<String>,
}

impl DiagnosticExt for FshDiag {
    fn category(&self) -> &'static str {
        self.category
    }
    fn code_enum(&self) -> Option<ErrorCode> {
        self.code
    }
    fn fix(&self) -> Option<String> {
        self.fix.clone()
    }
    fn suggestions(&self) -> Vec<String> {
        self.suggestions.clone()
    }
}

impl FshDiag {
    pub fn new(err: impl Diagnostic + DiagnosticExt + Send + Sync + 'static) -> Self {
        let category = err.category();
        let code = err.code_enum();
        let fix = err.fix();
        let suggestions = err.suggestions();
        FshDiag {
            report: Arc::new(miette::Report::new(err)),
            category,
            code,
            fix,
            suggestions,
        }
    }

    pub fn into_inner(self) -> miette::Report {
        match Arc::try_unwrap(self.report) {
            Ok(report) => report,
            Err(arc) => {
                let msg = format!("{arc}");
                StringError::from(msg).into()
            }
        }
    }

    /// Converts this diagnostic into a structured `Val::Map` suitable for `$last_error` or `catch |err|`.
    pub fn to_val(&self) -> Val {
        let mut m = FxIndexMap::with_hasher(FxBuildHasher::default());
        let diag_ref = self.report.as_ref();

        let code_str = self
            .code
            .map(|c| c.code_str().to_string())
            .or_else(|| diag_ref.code().map(|c| c.to_string()))
            .unwrap_or_else(|| "FSH-GEN-001".to_string());

        let name_str = self
            .code
            .map(|c| c.name().to_string())
            .unwrap_or_else(|| "Error".to_string());

        m.insert(ustr("code"), Val::String(code_str.clone()));
        m.insert(ustr("name"), Val::String(name_str));
        m.insert(ustr("category"), Val::String(self.category.to_string()));
        m.insert(ustr("message"), Val::String(diag_ref.to_string()));

        if let Some(help) = diag_ref.help() {
            m.insert(ustr("help"), Val::String(help.to_string()));
        }
        if let Some(ref fix) = self.fix {
            m.insert(ustr("fix"), Val::String(fix.clone()));
        }
        if !self.suggestions.is_empty() {
            let suggs: Vec<Val> = self.suggestions.iter().cloned().map(Val::String).collect();
            m.insert(ustr("suggestions"), Val::List(suggs));
        }
        if let Some(url) = self.code.and_then(|c| c.docs_url()) {
            m.insert(ustr("docs_url"), Val::String(url));
        } else {
            m.insert(ustr("docs_url"), Val::Null);
        }
        m.insert(ustr("exit_code"), Val::Int(1));

        Val::Map(m)
    }

    /// True iff the wrapped diagnostic is a logical `false` condition.
    pub fn is_condition_false(&self) -> bool {
        if self.code == Some(ErrorCode::ConditionFalse) {
            return true;
        }
        self.report
            .downcast_ref::<StringError>()
            .is_some_and(|e| e.is_condition_false())
            || self.code().is_some_and(|c| {
                let s = c.to_string();
                s == "FSH-EXEC-004" || s == "fshell::engine::E014"
            })
    }
}

impl std::ops::Deref for FshDiag {
    type Target = dyn Diagnostic;
    fn deref(&self) -> &Self::Target {
        &**self.report
    }
}

impl From<FshDiag> for miette::Report {
    fn from(val: FshDiag) -> Self {
        val.into_inner()
    }
}

impl From<StringError> for FshDiag {
    fn from(err: StringError) -> Self {
        let category = err.category;
        let code = Some(err.code);
        let fix = err.fix.clone();
        let suggestions = err.suggestions.clone();
        FshDiag {
            report: Arc::new(miette::Report::new(err)),
            category,
            code,
            fix,
            suggestions,
        }
    }
}

impl From<String> for FshDiag {
    fn from(msg: String) -> Self {
        StringError::from(msg).into()
    }
}

impl From<&str> for FshDiag {
    fn from(msg: &str) -> Self {
        StringError::from(msg).into()
    }
}

#[derive(Debug, Error)]
#[error("{message}")]
#[allow(clippy::result_large_err)]
pub struct StringError {
    pub message: String,
    pub code: ErrorCode,
    pub category: &'static str,
    pub help: Option<String>,
    pub fix: Option<String>,
    pub suggestions: Vec<String>,
    pub span: Option<SourceSpan>,
    pub secondary_spans: Vec<(SourceSpan, String)>,
}

impl Diagnostic for StringError {
    fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(self.code))
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
    fn severity(&self) -> Option<Severity> {
        None
    }
}

impl DiagnosticExt for StringError {
    fn category(&self) -> &'static str {
        self.category
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

impl From<String> for StringError {
    fn from(message: String) -> Self {
        StringError {
            message,
            code: ErrorCode::General,
            category: "general",
            help: None,
            fix: None,
            suggestions: Vec::new(),
            span: None,
            secondary_spans: Vec::new(),
        }
    }
}

impl From<&str> for StringError {
    fn from(message: &str) -> Self {
        String::from(message).into()
    }
}

impl StringError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        let category = code.category();
        StringError {
            message: message.into(),
            code,
            category,
            help: None,
            fix: None,
            suggestions: Vec::new(),
            span: None,
            secondary_spans: Vec::new(),
        }
    }

    pub fn not_found(cmd: &str, target: &str) -> Self {
        Self::new(
            ErrorCode::FileNotFound,
            format!("{cmd}: '{target}' not found"),
        )
        .with_help("Check that the target path or item exists and is spelled correctly.")
    }

    pub fn permission_denied(cmd: &str, target: &str) -> Self {
        Self::new(
            ErrorCode::PermissionDenied,
            format!("{cmd}: permission denied for '{target}'"),
        )
        .with_help("Verify read/write permissions or grant required shell capabilities.")
    }

    pub fn invalid_argument(cmd: &str, arg: &str) -> Self {
        Self::new(
            ErrorCode::InvalidArgument,
            format!("{cmd}: invalid argument '{arg}'"),
        )
        .with_help(format!("Use `help {cmd}` for syntax and allowed options."))
    }

    pub fn missing_argument(cmd: &str, desc: &str) -> Self {
        Self::new(
            ErrorCode::MissingArgument,
            format!("{cmd}: missing argument: {desc}"),
        )
        .with_help(format!("Provide the required argument. See `help {cmd}`."))
    }

    pub fn type_error(expected: &str, found: &str) -> Self {
        Self::new(
            ErrorCode::TypeError,
            format!("Type mismatch: expected {expected}, found {found}"),
        )
        .with_help("Use a type conversion function like `to_text()` or check operand types.")
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }

    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestions.push(suggestion.into());
        self
    }

    pub fn with_suggestions(mut self, suggestions: Vec<String>) -> Self {
        self.suggestions = suggestions;
        self
    }

    pub fn with_span(mut self, span: SourceSpan) -> Self {
        self.span = Some(span);
        self
    }

    pub fn with_secondary_span(mut self, span: SourceSpan, label: impl Into<String>) -> Self {
        self.secondary_spans.push((span, label.into()));
        self
    }

    /// True iff this error represents a logical `false` condition, not a hard failure.
    pub fn is_condition_false(&self) -> bool {
        self.code == ErrorCode::ConditionFalse
    }

    /// Construct a `ConditionFalse` sentinel (exit code 1, no error line).
    pub fn condition_false() -> Self {
        StringError {
            message: "false".to_string(),
            code: ErrorCode::ConditionFalse,
            category: "condition",
            help: None,
            fix: None,
            suggestions: Vec::new(),
            span: None,
            secondary_spans: Vec::new(),
        }
    }
}
