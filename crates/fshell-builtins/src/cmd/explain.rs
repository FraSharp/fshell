// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::Val;
use fshell_core::diagnostic::{ErrorCode, FshDiag, StringError};
use fshell_core::val::FxIndexMap;
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use fshell_hash::FxBuildHasher;
use fshell_render::{RenderConfig, RenderFormat};
use std::str::FromStr;
use std::sync::Arc;
use ustr::ustr;

pub fn explain_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), StringError> {
    let opts = env.options.read();
    let color = opts.error_color;
    let config = RenderConfig {
        format: RenderFormat::Explain,
        color,
        is_interactive: false,
    };

    let arg_strings: Vec<String> = args
        .iter()
        .map(|v| match v {
            Val::String(s) => s.clone(),
            other => other.to_text(),
        })
        .collect();

    if arg_strings.iter().any(|a| a == "--help" || a == "-h") {
        let help_text = "\
explain - Inspect diagnostics, error codes, and failure details

USAGE:
    explain              Explain the most recent error in this session
    explain <CODE>       Explain a specific error code (e.g. FSH-TYPE-001)
    explain --list       List all registered error codes and categories

EXAMPLES:
    explain
    explain FSH-IO-001
    explain --list
";
        let payload = Arc::new(Val::String(help_text.to_string()));
        tokio::spawn(async move {
            let _ = tx.send(PipelinePayload::Data(payload)).await;
        });
        return Ok(());
    }

    if arg_strings.iter().any(|a| a == "--list" || a == "-l") {
        let all_codes = [
            ErrorCode::General,
            ErrorCode::ParseError,
            ErrorCode::SyntaxError,
            ErrorCode::UnexpectedEof,
            ErrorCode::ExpectedChar,
            ErrorCode::ExpectedToken,
            ErrorCode::UnclosedQuote,
            ErrorCode::VariableNotFound,
            ErrorCode::FunctionNotFound,
            ErrorCode::ImmutableVariable,
            ErrorCode::TypeError,
            ErrorCode::TypeErrorArgCount,
            ErrorCode::TypeConstraintError,
            ErrorCode::DivisionByZero,
            ErrorCode::NumericOverflow,
            ErrorCode::InvalidNumber,
            ErrorCode::CommandNotFound,
            ErrorCode::RuntimeError,
            ErrorCode::CapabilityDenied,
            ErrorCode::ConditionFalse,
            ErrorCode::CommandFailed,
            ErrorCode::FileNotFound,
            ErrorCode::PermissionDenied,
            ErrorCode::IoError,
            ErrorCode::AlreadyExists,
            ErrorCode::IsDirectory,
            ErrorCode::NotDirectory,
            ErrorCode::NetworkError,
            ErrorCode::Timeout,
            ErrorCode::ConnectionRefused,
            ErrorCode::PipelineError,
            ErrorCode::SortTooManyItems,
            ErrorCode::BrokenPipe,
            ErrorCode::SandboxDenied,
            ErrorCode::SandboxError,
            ErrorCode::BuiltinError,
            ErrorCode::InvalidArgument,
            ErrorCode::MissingArgument,
            ErrorCode::NotFound,
            ErrorCode::Unsupported,
            ErrorCode::LockPoisoned,
            ErrorCode::Cancelled,
            ErrorCode::CycleDetected,
            ErrorCode::LintParseError,
            ErrorCode::LintEmptyStage,
            ErrorCode::LintEmptyPipeline,
            ErrorCode::InternalError,
            ErrorCode::Unimplemented,
        ];

        let list: Vec<Val> = all_codes
            .iter()
            .map(|c| {
                let mut m = FxIndexMap::with_hasher(FxBuildHasher::default());
                m.insert(ustr("code"), Val::String(c.code_str().to_string()));
                m.insert(ustr("name"), Val::String(c.name().to_string()));
                m.insert(ustr("category"), Val::String(c.category().to_string()));
                m.insert(
                    ustr("description"),
                    Val::String(c.default_description().to_string()),
                );
                if let Some(url) = c.docs_url() {
                    m.insert(ustr("docs_url"), Val::String(url));
                }
                Val::Map(m)
            })
            .collect();

        let payload = Arc::new(Val::List(list));
        tokio::spawn(async move {
            let _ = tx.send(PipelinePayload::Data(payload)).await;
        });
        return Ok(());
    }

    if let Some(target_code_str) = arg_strings.first() {
        let code_res = ErrorCode::from_str(target_code_str);
        match code_res {
            Ok(code) => {
                let help = if let Some(url) = code.docs_url() {
                    format!("Refer to the documentation at {url}")
                } else {
                    code.default_description().to_string()
                };
                let diag = FshDiag::from(
                    StringError::new(code, code.default_description()).with_help(help),
                );
                let rendered = fshell_render::render(diag, None, "explain", &config);
                let payload = Arc::new(Val::String(rendered));
                tokio::spawn(async move {
                    let _ = tx.send(PipelinePayload::Data(payload)).await;
                });
                return Ok(());
            }
            Err(_) => {
                return Err(
                    StringError::invalid_argument("explain", target_code_str).with_help(
                        "Provide a valid error code (e.g. FSH-TYPE-001) or run `explain --list`.",
                    ),
                );
            }
        }
    }

    // No args: explain last error
    if let Some(last_err) = env.get_last_error() {
        let rendered = fshell_render::render(last_err, None, "explain", &config);
        let payload = Arc::new(Val::String(rendered));
        tokio::spawn(async move {
            let _ = tx.send(PipelinePayload::Data(payload)).await;
        });
        Ok(())
    } else {
        let msg = "No error has occurred in this session.";
        let payload = Arc::new(Val::String(msg.to_string()));
        tokio::spawn(async move {
            let _ = tx.send(PipelinePayload::Data(payload)).await;
        });
        Ok(())
    }
}
