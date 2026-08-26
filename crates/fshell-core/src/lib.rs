// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::panic))]
#![allow(
    clippy::collapsible_if,
    clippy::approx_constant,
    clippy::items_after_test_module
)]
pub mod ast;
pub mod completion;
pub mod diagnostic;
pub mod env_utils;
pub mod extended_glob;
pub mod glob_utils;
pub mod linter;
pub mod lock_utils;
pub mod parser;
pub mod presets;
pub mod prompt_config;
pub mod shell_error;
pub mod theme;
pub mod val;
pub mod validator;

pub use ast::{
    BinOp, DedentMode, Expr, HashMode, LiteralPattern, MatchArm, MatchPattern, OnHandler, Param,
    ParamModifier, Pipeline, PipelineStage, ProcessSubstDirection, SerializationFormat, Stmt,
    StringPart, TimeUnit, TypeConstraint,
};
pub use completion::{CommandCompletion, DynamicProvider, FlagCompletion, SubcmdCompletion};
pub use diagnostic::{DiagnosticExt, ErrorCode, FshDiag, ShellDiagnostic, StringError};
pub use env_utils::{ThreadSafeEnv, expand_tilde, expand_tilde_str, get_var, remove_var, set_var};
pub use glob_utils::{
    determine_base_dir, expand_brace_range, expand_braces, expand_glob, expand_glob_with_options,
};
pub use lock_utils::RwLock;
pub use parser::{ParseError, Parser};
pub use prompt_config::{ColorSpec, PromptConfig, SegmentConfig, SegmentType, SeparatorStyle};
pub use shell_error::ShellError;
pub use val::{EdgeData, FxIndexMap, GraphStorage, NodeData, NodeId, ResourceHandle, Val};
pub use validator::{
    ValidationResult, compute_indent_depth, compute_indent_depth_at, validate_input,
};

/// Initialize core module.
pub fn init() {}

/// Emit a debug log line to stderr if the `FSH_DEBUG` environment variable is set.
/// Useful for tracing initialization and command execution hangs.
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        if std::env::var("FSH_DEBUG").is_ok() {
            let _ = std::io::Write::write(
                &mut std::io::stderr(),
                format!("[fsh_debug] {}:{}: {}\n", file!(), line!(), format!($($arg)*)).as_bytes(),
            );
        }
    };
}
