// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use brush_parser::ast as brush_ast;

/// Evaluate a POSIX test(1) expression (brush's TestExpr) against Env.
pub fn eval_test_expr(expr: &brush_ast::TestExpr, _env: &fshell_engine::Env) -> bool {
    match expr {
        brush_ast::TestExpr::False => false,
        brush_ast::TestExpr::Literal(s) => !s.is_empty(),
        brush_ast::TestExpr::And(a, b) => eval_test_expr(a, _env) && eval_test_expr(b, _env),
        brush_ast::TestExpr::Or(a, b) => eval_test_expr(a, _env) || eval_test_expr(b, _env),
        brush_ast::TestExpr::Not(inner) => !eval_test_expr(inner, _env),
        brush_ast::TestExpr::Parenthesized(inner) => eval_test_expr(inner, _env),
        brush_ast::TestExpr::UnaryTest(op, val) => eval_unary_test(op, val),
        brush_ast::TestExpr::BinaryTest(op, left, right) => eval_binary_test(op, left, right),
    }
}

fn eval_unary_test(op: &brush_ast::UnaryPredicate, val: &str) -> bool {
    match op {
        brush_ast::UnaryPredicate::StringHasNonZeroLength => !val.is_empty(),
        brush_ast::UnaryPredicate::StringHasZeroLength => val.is_empty(),
        brush_ast::UnaryPredicate::FileExists => std::path::Path::new(val).exists(),
        brush_ast::UnaryPredicate::FileExistsAndIsRegularFile => {
            std::path::Path::new(val).is_file()
        }
        brush_ast::UnaryPredicate::FileExistsAndIsDir => std::path::Path::new(val).is_dir(),
        brush_ast::UnaryPredicate::FileExistsAndIsReadable => std::fs::metadata(val)
            .map(|m| !m.permissions().readonly())
            .unwrap_or(false),
        brush_ast::UnaryPredicate::FileExistsAndIsWritable => {
            std::fs::metadata(val)
                .map(|m| !m.permissions().readonly())
                .unwrap_or(false)
                || !std::path::Path::new(val).exists()
        }
        brush_ast::UnaryPredicate::FileExistsAndIsExecutable => {
            #[cfg(unix)]
            {
                std::fs::metadata(val)
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
        brush_ast::UnaryPredicate::FileExistsAndIsNotZeroLength => {
            std::fs::metadata(val).map(|m| m.len() > 0).unwrap_or(false)
        }
        brush_ast::UnaryPredicate::FileExistsAndIsSymlink => std::path::Path::new(val).is_symlink(),
        _ => false,
    }
}

fn eval_binary_test(op: &brush_ast::BinaryPredicate, left: &str, right: &str) -> bool {
    match op {
        brush_ast::BinaryPredicate::StringExactlyMatchesString
        | brush_ast::BinaryPredicate::StringExactlyMatchesPattern => left == right,
        brush_ast::BinaryPredicate::StringDoesNotExactlyMatchString
        | brush_ast::BinaryPredicate::StringDoesNotExactlyMatchPattern => left != right,
        brush_ast::BinaryPredicate::LeftSortsBeforeRight => left < right,
        brush_ast::BinaryPredicate::LeftSortsAfterRight => left > right,
        brush_ast::BinaryPredicate::StringMatchesRegex
        | brush_ast::BinaryPredicate::StringContainsSubstring => {
            // Simplify: contains check
            left.contains(right)
        }
        brush_ast::BinaryPredicate::ArithmeticEqualTo => parse_int(left) == parse_int(right),
        brush_ast::BinaryPredicate::ArithmeticNotEqualTo => parse_int(left) != parse_int(right),
        brush_ast::BinaryPredicate::ArithmeticLessThan => parse_int(left) < parse_int(right),
        brush_ast::BinaryPredicate::ArithmeticLessThanOrEqualTo => {
            parse_int(left) <= parse_int(right)
        }
        brush_ast::BinaryPredicate::ArithmeticGreaterThan => parse_int(left) > parse_int(right),
        brush_ast::BinaryPredicate::ArithmeticGreaterThanOrEqualTo => {
            parse_int(left) >= parse_int(right)
        }
        brush_ast::BinaryPredicate::LeftFileIsNewerOrExistsWhenRightDoesNot => {
            file_mtime(left) > file_mtime(right)
        }
        brush_ast::BinaryPredicate::LeftFileIsOlderOrDoesNotExistWhenRightDoes => {
            file_mtime(left) < file_mtime(right)
        }
        brush_ast::BinaryPredicate::FilesReferToSameDeviceAndInodeNumbers => {
            file_mtime(left) == file_mtime(right)
        }
    }
}

fn parse_int(s: &str) -> i64 {
    s.trim().parse::<i64>().unwrap_or(0)
}

fn file_mtime(path: &str) -> std::time::SystemTime {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::UNIX_EPOCH)
}
