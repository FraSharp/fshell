// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Typed error enum for builtin commands.
//!
//! Every variant carries a miette diagnostic annotation so it renders through
//! the unified fshell-render pipeline (graphical / compact / JSON).
//!
//! Builtins return `Result<(), BuiltinError>` internally and the framework
//! converts them to `ShellError` at the handler boundary (the public API
//! still returns `Result<(), ShellError>`).

use fshell_core::ShellError;
use fshell_core::diagnostic::{DiagnosticExt, ErrorCode, FshDiag};
use miette::{Diagnostic, SourceSpan};
use thiserror::Error;

/// A typed, structured error for a builtin command.
///
/// Each variant records:
/// - **cmd**: the command name (e.g. "ff", "config", "http")
/// - **message/arg/path/what**: the specific offending value
/// - **span**: optional source span pointing at the relevant part of user input
/// - **status/stderr**: optional subprocess exit details (for CommandFailed)
/// - **duration**: timeout duration (for Timeout)
#[derive(Error, Diagnostic, Debug)]
pub enum BuiltinError {
    /// An argument value is invalid for its position or flag.
    #[error("{cmd}: invalid argument '{arg}'")]
    #[diagnostic(
        code = "FSH-BLT-001",
        help("Use `help {cmd}` to see valid arguments and syntax.")
    )]
    InvalidArgument {
        cmd: String,
        arg: String,
        #[label("invalid argument")]
        span: Option<SourceSpan>,
    },

    /// An argument or flag was not expected in this position.
    #[error("{cmd}: unexpected argument '{arg}'")]
    #[diagnostic(
        code = "FSH-BLT-002",
        help("Remove the unexpected argument or check the command syntax.")
    )]
    UnexpectedArgument {
        cmd: String,
        arg: String,
        #[label("unexpected argument")]
        span: Option<SourceSpan>,
    },

    /// A required argument was not provided.
    #[error("{cmd}: missing argument: {description}")]
    #[diagnostic(
        code = "FSH-BLT-003",
        help("Provide the required argument. Use `help {cmd}` for syntax.")
    )]
    MissingArgument {
        cmd: String,
        description: String,
        #[label("expected argument here")]
        span: Option<SourceSpan>,
    },

    /// A file or directory referenced by the command was not found.
    #[error("{cmd}: file not found: '{path}'")]
    #[diagnostic(
        code = "FSH-IO-001",
        help("Check that the path exists and is spelled correctly.")
    )]
    FileNotFound {
        cmd: String,
        path: String,
        #[label("no such file or directory")]
        span: Option<SourceSpan>,
    },

    /// An IO error occurred while the builtin was operating.
    #[error("{cmd}: I/O error: {message}")]
    #[diagnostic(
        code = "FSH-IO-003",
        help("Check file permissions, disk space, and that the path is accessible.")
    )]
    IoError {
        cmd: String,
        message: String,
        #[label("I/O error")]
        span: Option<SourceSpan>,
    },

    /// An external command invoked by the builtin failed.
    #[error("{cmd}: command exited with status {status}: {stderr}")]
    #[diagnostic(
        code = "FSH-BLT-004",
        help("Check the external command output for details.")
    )]
    CommandFailed {
        cmd: String,
        status: i32,
        stderr: String,
    },

    /// The builtin could not parse user input as the expected format.
    #[error("{cmd}: failed to parse input: {message}")]
    #[diagnostic(
        code = "FSH-PARSE-001",
        help("Check the input format. Use `help {cmd}` for syntax details.")
    )]
    ParseFailed {
        cmd: String,
        message: String,
        #[label("parse error here")]
        span: Option<SourceSpan>,
    },

    /// A named resource (variable, key, entry) was not found.
    #[error("{cmd}: {what} not found")]
    #[diagnostic(
        code = "FSH-BLT-005",
        help("Check the name or identifier and try again.")
    )]
    NotFound {
        cmd: String,
        what: String,
        #[label("not found")]
        span: Option<SourceSpan>,
    },

    /// Permission denied for a resource.
    #[error("{cmd}: permission denied: '{path}'")]
    #[diagnostic(
        code = "FSH-IO-002",
        help("Check file permissions or run with elevated capabilities.")
    )]
    PermissionDenied {
        cmd: String,
        path: String,
        #[label("permission denied")]
        span: Option<SourceSpan>,
    },

    /// A resource the command tried to create already exists.
    #[error("{cmd}: '{path}' already exists")]
    #[diagnostic(
        code = "FSH-BLT-006",
        help("Use a different name or path, or remove the existing resource first.")
    )]
    AlreadyExists {
        cmd: String,
        path: String,
        #[label("already exists")]
        span: Option<SourceSpan>,
    },

    /// A network operation failed.
    #[error("{cmd}: network error: {message}")]
    #[diagnostic(
        code = "FSH-NET-001",
        help("Check your network connection and the target URL.")
    )]
    NetworkError { cmd: String, message: String },

    /// An operation timed out.
    #[error("{cmd}: timed out after {duration}s")]
    #[diagnostic(
        code = "FSH-BLT-007",
        help("The operation took too long. Try again or increase the timeout.")
    )]
    Timeout { cmd: String, duration: f64 },

    /// The requested feature or operation is not supported.
    #[error("{cmd}: {feature} is not supported")]
    #[diagnostic(
        code = "FSH-BLT-008",
        help("This feature is not available. Check `help {cmd}` for supported operations.")
    )]
    Unsupported { cmd: String, feature: String },

    /// An unexpected internal failure in the builtin.
    #[error("{cmd}: internal error: {message}")]
    #[diagnostic(
        code = "FSH-INT-001",
        help("This is an unexpected error. Please file a bug report.")
    )]
    InternalError {
        cmd: String,
        message: String,
        #[label("internal error")]
        span: Option<SourceSpan>,
    },

    /// The operation was cancelled by the user or system.
    #[error("{cmd}: cancelled")]
    #[diagnostic(
        code = "FSH-RT-002",
        help("The operation was cancelled before it completed.")
    )]
    Cancelled { cmd: String },
}
// ErrorCode mapping
impl BuiltinError {
    pub fn error_code(&self) -> ErrorCode {
        match self {
            BuiltinError::InvalidArgument { .. } => ErrorCode::InvalidArgument,
            BuiltinError::UnexpectedArgument { .. } => ErrorCode::InvalidArgument,
            BuiltinError::MissingArgument { .. } => ErrorCode::InvalidArgument,
            BuiltinError::FileNotFound { .. } => ErrorCode::FileNotFound,
            BuiltinError::IoError { .. } => ErrorCode::IoError,
            BuiltinError::CommandFailed { .. } => ErrorCode::CommandFailed,
            BuiltinError::ParseFailed { .. } => ErrorCode::ParseError,
            BuiltinError::NotFound { .. } => ErrorCode::NotFound,
            BuiltinError::PermissionDenied { .. } => ErrorCode::PermissionDenied,
            BuiltinError::AlreadyExists { .. } => ErrorCode::AlreadyExists,
            BuiltinError::NetworkError { .. } => ErrorCode::NetworkError,
            BuiltinError::Timeout { .. } => ErrorCode::Timeout,
            BuiltinError::Unsupported { .. } => ErrorCode::Unsupported,
            BuiltinError::InternalError { .. } => ErrorCode::InternalError,
            BuiltinError::Cancelled { .. } => ErrorCode::Cancelled,
        }
    }

    pub fn span(&self) -> Option<SourceSpan> {
        match self {
            BuiltinError::InvalidArgument { span, .. } => *span,
            BuiltinError::UnexpectedArgument { span, .. } => *span,
            BuiltinError::MissingArgument { span, .. } => *span,
            BuiltinError::FileNotFound { span, .. } => *span,
            BuiltinError::IoError { span, .. } => *span,
            BuiltinError::CommandFailed { .. } => None,
            BuiltinError::ParseFailed { span, .. } => *span,
            BuiltinError::NotFound { span, .. } => *span,
            BuiltinError::PermissionDenied { span, .. } => *span,
            BuiltinError::AlreadyExists { span, .. } => *span,
            BuiltinError::NetworkError { .. } => None,
            BuiltinError::Timeout { .. } => None,
            BuiltinError::Unsupported { .. } => None,
            BuiltinError::InternalError { span, .. } => *span,
            BuiltinError::Cancelled { .. } => None,
        }
    }

    /// The command name associated with this error.
    pub fn cmd_name(&self) -> &str {
        match self {
            BuiltinError::InvalidArgument { cmd, .. } => cmd,
            BuiltinError::UnexpectedArgument { cmd, .. } => cmd,
            BuiltinError::MissingArgument { cmd, .. } => cmd,
            BuiltinError::FileNotFound { cmd, .. } => cmd,
            BuiltinError::IoError { cmd, .. } => cmd,
            BuiltinError::CommandFailed { cmd, .. } => cmd,
            BuiltinError::ParseFailed { cmd, .. } => cmd,
            BuiltinError::NotFound { cmd, .. } => cmd,
            BuiltinError::PermissionDenied { cmd, .. } => cmd,
            BuiltinError::AlreadyExists { cmd, .. } => cmd,
            BuiltinError::NetworkError { cmd, .. } => cmd,
            BuiltinError::Timeout { cmd, .. } => cmd,
            BuiltinError::Unsupported { cmd, .. } => cmd,
            BuiltinError::InternalError { cmd, .. } => cmd,
            BuiltinError::Cancelled { cmd, .. } => cmd,
        }
    }
}
// DiagnosticExt — required by the rendering pipeline
impl DiagnosticExt for BuiltinError {
    fn category(&self) -> &'static str {
        "builtins"
    }

    fn suggestions(&self) -> Vec<String> {
        match self {
            BuiltinError::InvalidArgument { cmd, .. } => {
                vec![format!("Use `help {cmd}` to see valid arguments.")]
            }
            BuiltinError::UnexpectedArgument { cmd, .. } => {
                vec![format!("Use `help {cmd}` for the correct argument syntax.")]
            }
            BuiltinError::MissingArgument { cmd, .. } => {
                vec![format!("Use `help {cmd}` to see required arguments.")]
            }
            BuiltinError::FileNotFound { .. } => {
                vec![
                    "Check the spelling of the path.".into(),
                    "Use an absolute path if the file is outside the current directory.".into(),
                ]
            }
            BuiltinError::CommandFailed { cmd, stderr, .. } => {
                let mut s = Vec::new();
                if !stderr.is_empty() {
                    s.push("Review the stderr output above.".to_string());
                }
                s.push(format!("Run `{cmd}` with --help for usage."));
                s
            }
            BuiltinError::ParseFailed { cmd, .. } => {
                vec![format!("Use `help {cmd}` for syntax and format details.")]
            }
            BuiltinError::NotFound { cmd, .. } => {
                vec![format!("Use `help {cmd}` to see available resources.")]
            }
            BuiltinError::PermissionDenied { .. } => {
                vec!["Use `caps(...)` to grant the required capability.".into()]
            }
            BuiltinError::NetworkError { .. } => {
                vec![
                    "Check your internet connection.".into(),
                    "Verify the URL is correct.".into(),
                ]
            }
            BuiltinError::Timeout { .. } => {
                vec!["The operation is taking too long — consider retrying.".into()]
            }
            BuiltinError::InternalError { .. } => {
                vec!["Please file a bug report with steps to reproduce.".into()]
            }
            _ => vec![],
        }
    }
}
// Conversions to the public error types
impl From<BuiltinError> for ShellError {
    fn from(err: BuiltinError) -> Self {
        let message = err.to_string();
        let code = err.error_code();
        let suggestions = err.suggestions();
        let span = err.span();
        // Extract help text from miette's diagnostic infrastructure.
        let help = miette_help(&err);
        ShellError {
            code,
            message,
            span,
            help,
            fix: None,
            suggestions,
            secondary_spans: vec![],
            severity: None,
        }
    }
}

impl From<BuiltinError> for FshDiag {
    fn from(err: BuiltinError) -> Self {
        FshDiag::new(err)
    }
}
// Helper: extract the `help` text from a miette Diagnostic
fn miette_help(diag: &dyn miette::Diagnostic) -> Option<String> {
    diag.help().map(|h| h.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fshell_render::{RenderConfig, RenderFormat, render as render_diag};

    fn mk_span() -> Option<SourceSpan> {
        Some(SourceSpan::new(0.into(), 5))
    }

    #[test]
    fn test_invalid_argument_roundtrip() {
        let err = BuiltinError::InvalidArgument {
            cmd: "test".into(),
            arg: "--bad-flag".into(),
            span: mk_span(),
        };
        let se: ShellError = err.into();
        assert!(se.message.contains("invalid argument"));
        assert!(se.message.contains("--bad-flag"));
        assert_eq!(se.code, ErrorCode::InvalidArgument);
        assert_eq!(se.category(), "builtin");
        assert!(se.help.is_some());
        assert!(se.span.is_some());
    }

    #[test]
    fn test_unexpected_argument() {
        let err = BuiltinError::UnexpectedArgument {
            cmd: "ff".into(),
            arg: "--bogus".into(),
            span: None,
        };
        let se: ShellError = err.into();
        assert!(se.message.contains("unexpected argument"));
        assert_eq!(se.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn test_missing_argument() {
        let err = BuiltinError::MissingArgument {
            cmd: "http".into(),
            description: "URL".into(),
            span: None,
        };
        let se: ShellError = err.into();
        assert!(se.message.contains("missing argument"));
        assert!(se.message.contains("URL"));
        assert_eq!(se.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn test_file_not_found() {
        let err = BuiltinError::FileNotFound {
            cmd: "ff".into(),
            path: "/nonexistent".into(),
            span: None,
        };
        let se: ShellError = err.into();
        assert!(se.message.contains("file not found"));
        assert!(se.message.contains("/nonexistent"));
        assert_eq!(se.code, ErrorCode::FileNotFound);
        assert_eq!(format!("{}", se.code), "FSH-IO-001");
    }

    #[test]
    fn test_command_failed() {
        let err = BuiltinError::CommandFailed {
            cmd: "git".into(),
            status: 128,
            stderr: "fatal: not a git repository".into(),
        };
        let se: ShellError = err.into();
        assert!(se.message.contains("command exited with status"));
        assert!(se.message.contains("128"));
        assert_eq!(se.code, ErrorCode::CommandFailed);
        assert_eq!(format!("{}", se.code), "FSH-EXEC-005");
    }

    #[test]
    fn test_not_found() {
        let err = BuiltinError::NotFound {
            cmd: "which".into(),
            what: "foobar".into(),
            span: None,
        };
        let se: ShellError = err.into();
        assert!(se.message.contains("not found"));
        assert_eq!(se.code, ErrorCode::NotFound);
    }

    #[test]
    fn test_network_error() {
        let err = BuiltinError::NetworkError {
            cmd: "http".into(),
            message: "connection refused".into(),
        };
        let se: ShellError = err.into();
        assert!(se.message.contains("network error"));
        assert_eq!(se.code, ErrorCode::NetworkError);
        assert_eq!(format!("{}", se.code), "FSH-NET-001");
    }

    #[test]
    fn test_internal_error() {
        let err = BuiltinError::InternalError {
            cmd: "reload".into(),
            message: "unexpected state".into(),
            span: None,
        };
        let se: ShellError = err.into();
        assert!(se.message.contains("internal error"));
        assert_eq!(se.code, ErrorCode::InternalError);
        assert!(se.help.is_some_and(|h| h.contains("bug report")));
    }

    #[test]
    fn test_cancelled() {
        let err = BuiltinError::Cancelled { cmd: "caps".into() };
        let se: ShellError = err.into();
        assert!(se.message.contains("cancelled"));
        assert_eq!(se.code, ErrorCode::Cancelled);
    }

    #[test]
    fn test_unsupported() {
        let err = BuiltinError::Unsupported {
            cmd: "http".into(),
            feature: "method 'DELETE'".into(),
        };
        let se: ShellError = err.into();
        assert!(se.message.contains("not supported"));
        assert_eq!(se.code, ErrorCode::Unsupported);
    }

    #[test]
    fn test_timeout() {
        let err = BuiltinError::Timeout {
            cmd: "http".into(),
            duration: 30.0,
        };
        let se: ShellError = err.into();
        assert!(se.message.contains("timed out"));
        assert!(se.message.contains("30"));
        assert_eq!(se.code, ErrorCode::Timeout);
    }

    #[test]
    fn test_already_exists() {
        let err = BuiltinError::AlreadyExists {
            cmd: "mkdir".into(),
            path: "/tmp/foo".into(),
            span: None,
        };
        let se: ShellError = err.into();
        assert!(se.message.contains("already exists"));
        assert_eq!(se.code, ErrorCode::AlreadyExists);
    }

    #[test]
    fn test_encapsulated_diagnostic_conversion() {
        let err = BuiltinError::InvalidArgument {
            cmd: "test".into(),
            arg: "x".into(),
            span: None,
        };
        let ed: FshDiag = err.into();
        assert_eq!(ed.category, "builtins");
        assert!(!ed.suggestions.is_empty());
    }

    #[test]
    fn test_string_error_conversion_preserves_suggestions() {
        let err = BuiltinError::FileNotFound {
            cmd: "cat".into(),
            path: "/missing".into(),
            span: mk_span(),
        };
        let se: ShellError = err.into();
        assert!(!se.suggestions.is_empty(), "should have suggestions");
        assert!(se.suggestions[0].contains("spelling"));
    }

    #[test]
    fn test_render_graphical() {
        let err = BuiltinError::InvalidArgument {
            cmd: "test".into(),
            arg: "--bad".into(),
            span: mk_span(),
        };
        let ed: FshDiag = err.into();
        let config = RenderConfig {
            format: RenderFormat::Graphical,
            color: false,
            is_interactive: false,
        };
        let output = render_diag(ed, None, "", &config);
        assert!(!output.is_empty(), "graphical render should produce output");
        // Should mention the error message
        assert!(output.contains("invalid argument") || output.contains("--bad"));
    }

    #[test]
    fn test_render_compact() {
        let err = BuiltinError::FileNotFound {
            cmd: "ff".into(),
            path: "/x".into(),
            span: None,
        };
        let se: ShellError = err.into();
        let ed: FshDiag = se.into();
        let config = RenderConfig {
            format: RenderFormat::Compact,
            color: false,
            is_interactive: false,
        };
        let output = render_diag(ed, None, "", &config);
        assert!(
            output.contains("ff: file not found"),
            "expected message in compact output, got: {output}"
        );
    }

    #[test]
    fn test_render_json() {
        let err = BuiltinError::InternalError {
            cmd: "x".into(),
            message: "out of memory".into(),
            span: None,
        };
        let ed: FshDiag = err.into();
        let config = RenderConfig {
            format: RenderFormat::Json,
            color: false,
            is_interactive: false,
        };
        let output = render_diag(ed, None, "", &config);
        assert!(output.contains("out of memory"));
        assert!(output.contains("FSH-INT-001"));
    }

    #[test]
    fn test_cmd_name() {
        let err = BuiltinError::InvalidArgument {
            cmd: "mycmd".into(),
            arg: "x".into(),
            span: None,
        };
        assert_eq!(err.cmd_name(), "mycmd");
    }

    #[test]
    fn test_error_code_maps() {
        let cases: Vec<(BuiltinError, ErrorCode)> = vec![
            (
                BuiltinError::InvalidArgument {
                    cmd: "".into(),
                    arg: "".into(),
                    span: None,
                },
                ErrorCode::InvalidArgument,
            ),
            (
                BuiltinError::PermissionDenied {
                    cmd: "".into(),
                    path: "".into(),
                    span: None,
                },
                ErrorCode::PermissionDenied,
            ),
            (
                BuiltinError::IoError {
                    cmd: "".into(),
                    message: "".into(),
                    span: None,
                },
                ErrorCode::IoError,
            ),
            (
                BuiltinError::ParseFailed {
                    cmd: "".into(),
                    message: "".into(),
                    span: None,
                },
                ErrorCode::ParseError,
            ),
        ];
        for (err, expected_code) in cases {
            assert_eq!(err.error_code(), expected_code, "mismatch for {err:?}");
        }
    }

    #[test]
    fn test_all_variants_no_panic_on_convert() {
        // Smoke test: every variant converts to ShellError without panic
        let variants: Vec<BuiltinError> = vec![
            BuiltinError::InvalidArgument {
                cmd: "a".into(),
                arg: "b".into(),
                span: None,
            },
            BuiltinError::UnexpectedArgument {
                cmd: "a".into(),
                arg: "b".into(),
                span: None,
            },
            BuiltinError::MissingArgument {
                cmd: "a".into(),
                description: "b".into(),
                span: None,
            },
            BuiltinError::FileNotFound {
                cmd: "a".into(),
                path: "b".into(),
                span: None,
            },
            BuiltinError::IoError {
                cmd: "a".into(),
                message: "b".into(),
                span: None,
            },
            BuiltinError::CommandFailed {
                cmd: "a".into(),
                status: 1,
                stderr: "b".into(),
            },
            BuiltinError::ParseFailed {
                cmd: "a".into(),
                message: "b".into(),
                span: None,
            },
            BuiltinError::NotFound {
                cmd: "a".into(),
                what: "b".into(),
                span: None,
            },
            BuiltinError::PermissionDenied {
                cmd: "a".into(),
                path: "b".into(),
                span: None,
            },
            BuiltinError::AlreadyExists {
                cmd: "a".into(),
                path: "b".into(),
                span: None,
            },
            BuiltinError::NetworkError {
                cmd: "a".into(),
                message: "b".into(),
            },
            BuiltinError::Timeout {
                cmd: "a".into(),
                duration: 1.0,
            },
            BuiltinError::Unsupported {
                cmd: "a".into(),
                feature: "b".into(),
            },
            BuiltinError::InternalError {
                cmd: "a".into(),
                message: "b".into(),
                span: None,
            },
            BuiltinError::Cancelled { cmd: "a".into() },
        ];
        for v in variants {
            let _: ShellError = v.into();
        }
    }
}
