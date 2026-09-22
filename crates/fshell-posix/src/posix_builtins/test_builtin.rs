// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use brush_parser::ast as brush_ast;

/// Evaluate a POSIX test(1) expression (brush's TestExpr) against Env.
pub fn eval_test_expr(
    expr: &brush_ast::TestExpr,
    env: &fshell_engine::Env,
) -> Result<bool, String> {
    match expr {
        brush_ast::TestExpr::False => Ok(false),
        brush_ast::TestExpr::Literal(s) => Ok(!s.is_empty()),
        brush_ast::TestExpr::And(a, b) => Ok(eval_test_expr(a, env)? && eval_test_expr(b, env)?),
        brush_ast::TestExpr::Or(a, b) => Ok(eval_test_expr(a, env)? || eval_test_expr(b, env)?),
        brush_ast::TestExpr::Not(inner) => Ok(!eval_test_expr(inner, env)?),
        brush_ast::TestExpr::Parenthesized(inner) => eval_test_expr(inner, env),
        brush_ast::TestExpr::UnaryTest(op, val) => Ok(eval_unary_test(op, val, env)),
        brush_ast::TestExpr::BinaryTest(op, left, right) => eval_binary_test(op, left, right, env),
    }
}

fn eval_unary_test(op: &brush_ast::UnaryPredicate, val: &str, env: &fshell_engine::Env) -> bool {
    let path = || env.resolve_path(val);
    match op {
        brush_ast::UnaryPredicate::StringHasNonZeroLength => !val.is_empty(),
        brush_ast::UnaryPredicate::StringHasZeroLength => val.is_empty(),
        brush_ast::UnaryPredicate::FileExists => path().exists(),
        brush_ast::UnaryPredicate::FileExistsAndIsRegularFile => path().is_file(),
        brush_ast::UnaryPredicate::FileExistsAndIsDir => path().is_dir(),
        brush_ast::UnaryPredicate::FileExistsAndIsReadable => std::fs::metadata(path())
            .map(|m| !m.permissions().readonly())
            .unwrap_or(false),
        brush_ast::UnaryPredicate::FileExistsAndIsWritable => {
            std::fs::metadata(path())
                .map(|m| !m.permissions().readonly())
                .unwrap_or(false)
                || !path().exists()
        }
        brush_ast::UnaryPredicate::FileExistsAndIsExecutable => {
            #[cfg(unix)]
            {
                std::fs::metadata(path())
                    .map(|m| {
                        use std::os::unix::fs::PermissionsExt;
                        m.permissions().mode() & 0o111 != 0
                    })
                    .unwrap_or(false)
            }
            #[cfg(not(unix))]
            {
                false
            }
        }
        brush_ast::UnaryPredicate::FileExistsAndIsNotZeroLength => std::fs::metadata(path())
            .map(|m| m.len() > 0)
            .unwrap_or(false),
        brush_ast::UnaryPredicate::FileExistsAndIsSymlink => path().is_symlink(),
        _ => false,
    }
}

fn eval_binary_test(
    op: &brush_ast::BinaryPredicate,
    left: &str,
    right: &str,
    env: &fshell_engine::Env,
) -> Result<bool, String> {
    match op {
        brush_ast::BinaryPredicate::StringExactlyMatchesString
        | brush_ast::BinaryPredicate::StringExactlyMatchesPattern => Ok(left == right),
        brush_ast::BinaryPredicate::StringDoesNotExactlyMatchString
        | brush_ast::BinaryPredicate::StringDoesNotExactlyMatchPattern => Ok(left != right),
        brush_ast::BinaryPredicate::LeftSortsBeforeRight => Ok(left < right),
        brush_ast::BinaryPredicate::LeftSortsAfterRight => Ok(left > right),
        brush_ast::BinaryPredicate::StringMatchesRegex
        | brush_ast::BinaryPredicate::StringContainsSubstring => {
            // Simplify: contains check
            Ok(left.contains(right))
        }
        brush_ast::BinaryPredicate::ArithmeticEqualTo => Ok(parse_int(left)? == parse_int(right)?),
        brush_ast::BinaryPredicate::ArithmeticNotEqualTo => {
            Ok(parse_int(left)? != parse_int(right)?)
        }
        brush_ast::BinaryPredicate::ArithmeticLessThan => Ok(parse_int(left)? < parse_int(right)?),
        brush_ast::BinaryPredicate::ArithmeticLessThanOrEqualTo => {
            Ok(parse_int(left)? <= parse_int(right)?)
        }
        brush_ast::BinaryPredicate::ArithmeticGreaterThan => {
            Ok(parse_int(left)? > parse_int(right)?)
        }
        brush_ast::BinaryPredicate::ArithmeticGreaterThanOrEqualTo => {
            Ok(parse_int(left)? >= parse_int(right)?)
        }
        brush_ast::BinaryPredicate::LeftFileIsNewerOrExistsWhenRightDoesNot => Ok(
            eval_file_binary("-nt", &env.resolve_path(left), &env.resolve_path(right)),
        ),
        brush_ast::BinaryPredicate::LeftFileIsOlderOrDoesNotExistWhenRightDoes => Ok(
            eval_file_binary("-ot", &env.resolve_path(left), &env.resolve_path(right)),
        ),
        brush_ast::BinaryPredicate::FilesReferToSameDeviceAndInodeNumbers => Ok(eval_file_binary(
            "-ef",
            &env.resolve_path(left),
            &env.resolve_path(right),
        )),
    }
}

fn parse_int(s: &str) -> Result<i64, String> {
    s.trim()
        .parse::<i64>()
        .map_err(|error| format!("integer expression expected: {:?} ({})", s.trim(), error))
}

pub(crate) fn eval_file_binary(op: &str, left: &std::path::Path, right: &std::path::Path) -> bool {
    let left_metadata = std::fs::metadata(left).ok();
    let right_metadata = std::fs::metadata(right).ok();

    match op {
        "-nt" => match (left_metadata.as_ref(), right_metadata.as_ref()) {
            (Some(left), Some(right)) => match (left.modified(), right.modified()) {
                (Ok(left), Ok(right)) => left > right,
                _ => false,
            },
            (Some(_), None) => true,
            _ => false,
        },
        "-ot" => match (left_metadata.as_ref(), right_metadata.as_ref()) {
            (Some(left), Some(right)) => match (left.modified(), right.modified()) {
                (Ok(left), Ok(right)) => left < right,
                _ => false,
            },
            (None, Some(_)) => true,
            _ => false,
        },
        "-ef" => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;

                match (left_metadata.as_ref(), right_metadata.as_ref()) {
                    (Some(left), Some(right)) => {
                        left.dev() == right.dev() && left.ino() == right.ino()
                    }
                    _ => false,
                }
            }
            #[cfg(not(unix))]
            {
                false
            }
        }
        _ => false,
    }
}
