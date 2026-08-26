// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::eval::{eval_binop, is_query_stage, matches_pattern, pad_truncate, parse_csv_field};
use crate::glob::expand_braces;
use crate::suggestions::levenshtein_distance;
use crate::*;
use fshell_core::ast::{BinOp, Expr, Stmt};
use fshell_core::env_utils::{remove_var, set_var};
use fshell_core::{
    FxIndexMap, LiteralPattern, MatchArm, MatchPattern, Param, Parser, Pipeline, PipelineStage,
    ResourceHandle, SerializationFormat, StringPart, TimeUnit, TypeConstraint, Val,
};
use std::sync::atomic::Ordering;
use ustr::ustr;

#[cfg(test)]
mod tests {
    #![allow(clippy::module_inception, clippy::approx_constant)]
    use super::BuiltinHandler;
    use super::*;

    #[tokio::test]
    async fn test_exit_stmt() {
        let env = Env::new();
        let mut p = Parser::new("exit 42");
        let stmts = p.parse_statements().unwrap();
        let result = eval_stmt(&stmts[0], &env, false).await;
        assert!(matches!(result, Ok(Flow::Exit(42))));
    }

    #[test]
    fn finalize_no_failures_keeps_last_exit_code() {
        let (ec, err) = pipeline_finalize(Vec::new(), 7, false);
        assert_eq!(ec, 7);
        assert!(err.is_none());
    }

    #[test]
    fn finalize_all_condition_false_yields_exit_1_and_no_hard_error() {
        let failures = vec![
            PipelineFailure::ConditionFalse,
            PipelineFailure::ConditionFalse,
        ];
        let (ec, err) = pipeline_finalize(failures, 0, false);
        assert_eq!(ec, 1);
        assert!(matches!(err, Some(PipelineFailure::ConditionFalse)));
    }

    #[test]
    fn finalize_condition_false_respects_nonzero_last_exit_code() {
        let (ec, err) = pipeline_finalize(vec![PipelineFailure::ConditionFalse], 3, false);
        assert_eq!(ec, 1);
        assert!(matches!(err, Some(PipelineFailure::ConditionFalse)));
    }

    #[test]
    fn finalize_mixed_takes_last_hard_error_message() {
        let hard_a = FshDiag::new(EngineError::Generic {
            message: "first failure".to_string(),
            span: None,
        });
        let hard_b = FshDiag::new(EngineError::Generic {
            message: "last failure".to_string(),
            span: None,
        });
        let failures = vec![
            PipelineFailure::Hard(hard_a),
            PipelineFailure::ConditionFalse,
            PipelineFailure::Hard(hard_b),
        ];
        let (ec, err) = pipeline_finalize(failures, 2, false);
        assert_eq!(ec, 2);
        match err {
            Some(PipelineFailure::Hard(diag)) => {
                assert!(diag.report.to_string().contains("last failure"));
            }
            other => panic!("expected PipelineFailure::Hard, got {other:?}"),
        }
    }

    #[test]
    fn finalize_pipefail_synthesizes_exit_1_when_last_stage_succeeded() {
        let failures = vec![PipelineFailure::Hard(FshDiag::new(EngineError::Generic {
            message: "boom".to_string(),
            span: None,
        }))];
        let (ec, err) = pipeline_finalize(failures, 0, true);
        assert_eq!(ec, 1);
        assert!(err.is_some());
    }

    #[test]
    fn finalize_pipefail_prefers_nonzero_last_exit_code() {
        let failures = vec![PipelineFailure::Hard(FshDiag::new(EngineError::Generic {
            message: "boom".to_string(),
            span: None,
        }))];
        let (ec, _) = pipeline_finalize(failures, 5, true);
        assert_eq!(ec, 5);
    }

    #[tokio::test]
    async fn test_exit_stmt_bare_exit() {
        let env = Env::new();
        let mut p = Parser::new("exit");
        let stmts = p.parse_statements().unwrap();
        let result = eval_stmt(&stmts[0], &env, false).await;
        assert!(matches!(result, Ok(Flow::Exit(0))));
    }

    #[test]
    fn test_levenshtein_and_command_suggestions() {
        assert_eq!(levenshtein_distance("git", "gitt"), 1);
        assert_eq!(levenshtein_distance("ls", "la"), 1);
        assert_eq!(levenshtein_distance("bash", "zsh"), 2);
        assert_eq!(levenshtein_distance("hello", "world"), 4);

        let env = Env::new();
        env.register_builtin("cd", Arc::new(|_, _, _, _| Ok(())));
        let suggestion = get_suggested_command("cdd", &env, None);
        assert_eq!(suggestion, Some("cd".to_string()));

        // Test user function suggestion
        {
            let mut fns = env.fns.write();
            fns.insert("my_custom_func".to_string(), (vec![], None, vec![]));
        }
        let suggestion_fn = get_suggested_command("my_custom_funcc", &env, None);
        assert_eq!(suggestion_fn, Some("my_custom_func".to_string()));
    }

    #[test]
    fn test_deferred_dym_stores_pending() {
        let env = Env::new();
        env.register_builtin("cd", Arc::new(|_, _, _, _| Ok(())));
        // get_suggested_command should still find suggestions
        let suggestion = get_suggested_command("cdd", &env, None);
        assert_eq!(suggestion, Some("cd".to_string()));
        // Pending should be empty initially
        assert!(env.prompt.pending_suggestion.read().is_none());
        assert!(!env.prompt.suggestion_deferred.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn test_cryptographic_trust_profiles() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();

        let script_file = tmp.path().join("my_trusted_script.fsh");
        let script_content = "let x = 100\n";
        std::fs::write(&script_file, script_content).unwrap();

        // Calculate fhash256
        let digest = fshell_hash::fhash256(script_content.as_bytes());
        let mut hash_hex = String::with_capacity(64);
        for b in digest {
            hash_hex.push_str(&format!("{:02x}", b));
        }

        // Canonical path
        let canonical_path_str = script_file
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .to_string();

        // Create trust_profiles.json
        let trust_profiles_content = serde_json::json!({
            "trusted_hashes": {
                canonical_path_str: hash_hex
            }
        });
        let trust_profiles_path = config_dir.join("trust_profiles.json");
        std::fs::write(
            &trust_profiles_path,
            serde_json::to_string(&trust_profiles_content).unwrap(),
        )
        .unwrap();

        // Set config dir env
        let old_config_dir = std::env::var("FSH_CONFIG_DIR").ok();
        set_var("FSH_CONFIG_DIR", &config_dir.to_string_lossy());

        // Test is_script_trusted
        assert!(is_script_trusted(
            &script_file.to_string_lossy(),
            script_content
        ));
        // Untrusted file/content should fail
        assert!(!is_script_trusted(
            &script_file.to_string_lossy(),
            "different content"
        ));
        assert!(!is_script_trusted("/tmp/other_path.fsh", script_content));

        // Restore env
        if let Some(d) = old_config_dir {
            set_var("FSH_CONFIG_DIR", &d);
        } else {
            remove_var("FSH_CONFIG_DIR");
        }
    }
    #[tokio::test]
    async fn test_eval_expr_binop() {
        let env = Env::new();
        let expr = Expr::BinaryOp {
            op: BinOp::Add,
            lhs: Box::new(Expr::Int(10)),
            rhs: Box::new(Expr::Int(20)),
        };
        let res = eval_expr(&expr, &env).await.unwrap();
        assert_eq!(res, Val::Int(30));
    }

    #[tokio::test]
    async fn test_eval_statement_let() {
        let env = Env::new();
        let stmt = Stmt::Let {
            name: "y".to_string(),
            expr: Expr::Int(100),
        };
        eval_stmt(&stmt, &env, false).await.unwrap();
        let vars = env.vars.read();
        assert_eq!(vars.get("y"), Some(&Val::Int(100)));
    }
    // eval_binop tests
    #[test]
    fn test_binop_add_int() {
        assert_eq!(
            eval_binop(BinOp::Add, Val::Int(3), Val::Int(7)).unwrap(),
            Val::Int(10)
        );
    }

    #[test]
    fn test_binop_add_float() {
        assert_eq!(
            eval_binop(BinOp::Add, Val::Float(1.5), Val::Float(2.5)).unwrap(),
            Val::Float(4.0)
        );
    }

    #[test]
    fn test_binop_add_string() {
        assert_eq!(
            eval_binop(
                BinOp::Add,
                Val::String("hello ".into()),
                Val::String("world".into())
            )
            .unwrap(),
            Val::String("hello world".into())
        );
    }

    #[test]
    fn test_binop_add_type_error() {
        let err = eval_binop(BinOp::Add, Val::Int(1), Val::Bool(true)).unwrap_err();
        assert!(matches!(err, EngineError::TypeMismatch { .. }));
    }

    #[test]
    fn test_binop_sub_int() {
        assert_eq!(
            eval_binop(BinOp::Sub, Val::Int(10), Val::Int(3)).unwrap(),
            Val::Int(7)
        );
    }

    #[test]
    fn test_binop_sub_float() {
        assert_eq!(
            eval_binop(BinOp::Sub, Val::Float(5.5), Val::Float(1.2)).unwrap(),
            Val::Float(4.3)
        );
    }

    #[test]
    fn test_binop_sub_type_error() {
        assert!(eval_binop(BinOp::Sub, Val::Int(1), Val::String("a".into())).is_err());
    }

    #[test]
    fn test_binop_mul_int() {
        assert_eq!(
            eval_binop(BinOp::Mul, Val::Int(6), Val::Int(7)).unwrap(),
            Val::Int(42)
        );
    }

    #[test]
    fn test_binop_mul_float() {
        assert_eq!(
            eval_binop(BinOp::Mul, Val::Float(2.5), Val::Float(4.0)).unwrap(),
            Val::Float(10.0)
        );
    }

    #[test]
    fn test_binop_mul_type_error() {
        assert!(eval_binop(BinOp::Mul, Val::Int(2), Val::Null).is_err());
    }

    #[test]
    fn test_binop_div_int() {
        assert_eq!(
            eval_binop(BinOp::Div, Val::Int(10), Val::Int(3)).unwrap(),
            Val::Int(3)
        );
    }

    #[test]
    fn test_binop_div_float() {
        assert_eq!(
            eval_binop(BinOp::Div, Val::Float(7.0), Val::Float(2.0)).unwrap(),
            Val::Float(3.5)
        );
    }

    #[test]
    fn test_binop_div_int_by_zero() {
        let err = eval_binop(BinOp::Div, Val::Int(5), Val::Int(0)).unwrap_err();
        assert!(matches!(err, EngineError::DivisionByZero { .. }));
    }

    #[test]
    fn test_binop_div_float_by_zero() {
        let err = eval_binop(BinOp::Div, Val::Float(5.0), Val::Float(0.0)).unwrap_err();
        assert!(matches!(err, EngineError::DivisionByZero { .. }));
    }

    #[test]
    fn test_binop_div_type_error() {
        assert!(eval_binop(BinOp::Div, Val::Int(1), Val::Bool(false)).is_err());
    }

    #[test]
    fn test_binop_eq_true() {
        assert_eq!(
            eval_binop(BinOp::Eq, Val::Int(42), Val::Int(42)).unwrap(),
            Val::Bool(true)
        );
    }

    #[test]
    fn test_binop_eq_false() {
        assert_eq!(
            eval_binop(BinOp::Eq, Val::Int(1), Val::Int(2)).unwrap(),
            Val::Bool(false)
        );
    }

    #[test]
    fn test_binop_eq_cross_type() {
        assert_eq!(
            eval_binop(BinOp::Eq, Val::Int(1), Val::Bool(true)).unwrap(),
            Val::Bool(false)
        );
    }

    #[test]
    fn test_binop_neq_true() {
        assert_eq!(
            eval_binop(BinOp::Neq, Val::Int(1), Val::Int(2)).unwrap(),
            Val::Bool(true)
        );
    }

    #[test]
    fn test_binop_neq_false() {
        assert_eq!(
            eval_binop(BinOp::Neq, Val::Int(42), Val::Int(42)).unwrap(),
            Val::Bool(false)
        );
    }

    #[test]
    fn test_binop_lt_int() {
        assert_eq!(
            eval_binop(BinOp::Lt, Val::Int(3), Val::Int(7)).unwrap(),
            Val::Bool(true)
        );
        assert_eq!(
            eval_binop(BinOp::Lt, Val::Int(7), Val::Int(3)).unwrap(),
            Val::Bool(false)
        );
    }

    #[test]
    fn test_binop_lt_float() {
        assert_eq!(
            eval_binop(BinOp::Lt, Val::Float(1.5), Val::Float(2.5)).unwrap(),
            Val::Bool(true)
        );
    }

    #[test]
    fn test_binop_lt_string() {
        assert_eq!(
            eval_binop(BinOp::Lt, Val::String("a".into()), Val::String("b".into())).unwrap(),
            Val::Bool(true)
        );
    }

    #[test]
    fn test_binop_lte() {
        assert_eq!(
            eval_binop(BinOp::Lte, Val::Int(5), Val::Int(5)).unwrap(),
            Val::Bool(true)
        );
        assert_eq!(
            eval_binop(BinOp::Lte, Val::Int(3), Val::Int(5)).unwrap(),
            Val::Bool(true)
        );
        assert_eq!(
            eval_binop(BinOp::Lte, Val::Int(7), Val::Int(5)).unwrap(),
            Val::Bool(false)
        );
    }

    #[test]
    fn test_binop_gt() {
        assert_eq!(
            eval_binop(BinOp::Gt, Val::Int(7), Val::Int(3)).unwrap(),
            Val::Bool(true)
        );
        assert_eq!(
            eval_binop(BinOp::Gt, Val::Int(3), Val::Int(7)).unwrap(),
            Val::Bool(false)
        );
    }

    #[test]
    fn test_binop_gte() {
        assert_eq!(
            eval_binop(BinOp::Gte, Val::Int(5), Val::Int(5)).unwrap(),
            Val::Bool(true)
        );
        assert_eq!(
            eval_binop(BinOp::Gte, Val::Int(7), Val::Int(5)).unwrap(),
            Val::Bool(true)
        );
        assert_eq!(
            eval_binop(BinOp::Gte, Val::Int(3), Val::Int(5)).unwrap(),
            Val::Bool(false)
        );
    }

    #[test]
    fn test_binop_compare_type_error() {
        let err = eval_binop(BinOp::Lt, Val::Int(1), Val::Bool(true)).unwrap_err();
        assert!(matches!(err, EngineError::TypeMismatch { .. }));
        assert_eq!(
            eval_binop(BinOp::Lte, Val::Float(1.0), Val::Int(2)).unwrap(),
            Val::Bool(true)
        );
        assert!(eval_binop(BinOp::Gt, Val::String("a".into()), Val::Int(1)).is_err());
        assert!(eval_binop(BinOp::Gte, Val::Bool(true), Val::Bool(false)).is_err());
    }

    #[test]
    fn test_binop_and() {
        assert_eq!(
            eval_binop(BinOp::And, Val::Bool(true), Val::Bool(true)).unwrap(),
            Val::Bool(true)
        );
        assert_eq!(
            eval_binop(BinOp::And, Val::Bool(true), Val::Bool(false)).unwrap(),
            Val::Bool(false)
        );
        assert_eq!(
            eval_binop(BinOp::And, Val::Bool(false), Val::Bool(true)).unwrap(),
            Val::Bool(false)
        );
        assert_eq!(
            eval_binop(BinOp::And, Val::Bool(false), Val::Bool(false)).unwrap(),
            Val::Bool(false)
        );
    }

    #[test]
    fn test_binop_or() {
        assert_eq!(
            eval_binop(BinOp::Or, Val::Bool(true), Val::Bool(false)).unwrap(),
            Val::Bool(true)
        );
        assert_eq!(
            eval_binop(BinOp::Or, Val::Bool(true), Val::Bool(true)).unwrap(),
            Val::Bool(true)
        );
        assert_eq!(
            eval_binop(BinOp::Or, Val::Bool(false), Val::Bool(false)).unwrap(),
            Val::Bool(false)
        );
    }

    #[test]
    fn test_binop_logical_type_error() {
        let err = eval_binop(BinOp::And, Val::Bool(true), Val::Int(1)).unwrap_err();
        assert!(matches!(err, EngineError::TypeMismatch { .. }));
        let err = eval_binop(BinOp::Or, Val::Int(0), Val::Bool(false)).unwrap_err();
        assert!(matches!(err, EngineError::TypeMismatch { .. }));
    }

    #[test]
    fn test_binop_int_overflow_add() {
        let err = eval_binop(BinOp::Add, Val::Int(i64::MAX), Val::Int(1)).unwrap_err();
        assert!(matches!(err, EngineError::Generic { .. }));
    }

    #[test]
    fn test_binop_int_overflow_sub() {
        let err = eval_binop(BinOp::Sub, Val::Int(i64::MIN), Val::Int(1)).unwrap_err();
        assert!(matches!(err, EngineError::Generic { .. }));
    }

    #[test]
    fn test_binop_int_overflow_mul() {
        let err = eval_binop(BinOp::Mul, Val::Int(i64::MAX), Val::Int(2)).unwrap_err();
        assert!(matches!(err, EngineError::Generic { .. }));
    }

    #[test]
    fn test_binop_int_overflow_div() {
        let err = eval_binop(BinOp::Div, Val::Int(i64::MIN), Val::Int(-1)).unwrap_err();
        assert!(matches!(err, EngineError::Generic { .. }));
    }
    // matches_pattern tests
    #[test]
    fn test_matches_pattern_wildcard() {
        assert!(matches_pattern(&Val::Null, &MatchPattern::Wildcard));
        assert!(matches_pattern(&Val::Int(42), &MatchPattern::Wildcard));
        assert!(matches_pattern(
            &Val::String("x".into()),
            &MatchPattern::Wildcard
        ));
    }

    #[test]
    fn test_matches_pattern_literal_null() {
        assert!(matches_pattern(
            &Val::Null,
            &MatchPattern::Literal(LiteralPattern::Null)
        ));
        assert!(!matches_pattern(
            &Val::Int(0),
            &MatchPattern::Literal(LiteralPattern::Null)
        ));
    }

    #[test]
    fn test_matches_pattern_literal_bool() {
        assert!(matches_pattern(
            &Val::Bool(true),
            &MatchPattern::Literal(LiteralPattern::Bool(true))
        ));
        assert!(!matches_pattern(
            &Val::Bool(true),
            &MatchPattern::Literal(LiteralPattern::Bool(false))
        ));
        assert!(!matches_pattern(
            &Val::Int(1),
            &MatchPattern::Literal(LiteralPattern::Bool(true))
        ));
    }

    #[test]
    fn test_matches_pattern_literal_int() {
        assert!(matches_pattern(
            &Val::Int(42),
            &MatchPattern::Literal(LiteralPattern::Int(42))
        ));
        assert!(!matches_pattern(
            &Val::Int(42),
            &MatchPattern::Literal(LiteralPattern::Int(0))
        ));
        assert!(!matches_pattern(
            &Val::Float(42.0),
            &MatchPattern::Literal(LiteralPattern::Int(42))
        ));
    }

    #[test]
    fn test_matches_pattern_literal_float() {
        assert!(matches_pattern(
            &Val::Float(3.14),
            &MatchPattern::Literal(LiteralPattern::Float(3.14))
        ));
        assert!(!matches_pattern(
            &Val::Float(3.14),
            &MatchPattern::Literal(LiteralPattern::Float(0.0))
        ));
    }

    #[test]
    fn test_matches_pattern_literal_string() {
        assert!(matches_pattern(
            &Val::String("hello".into()),
            &MatchPattern::Literal(LiteralPattern::String("hello".into()))
        ));
        assert!(!matches_pattern(
            &Val::String("hello".into()),
            &MatchPattern::Literal(LiteralPattern::String("world".into()))
        ));
    }

    #[test]
    fn test_matches_pattern_map_exact() {
        let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        m.insert(ustr::ustr("a"), Val::Int(1));
        m.insert(ustr::ustr("b"), Val::String("x".into()));
        let val = Val::Map(m);
        assert!(matches_pattern(
            &val,
            &MatchPattern::Map {
                fields: vec![
                    ("a".into(), MatchPattern::Literal(LiteralPattern::Int(1))),
                    (
                        "b".into(),
                        MatchPattern::Literal(LiteralPattern::String("x".into()))
                    ),
                ],
                rest: false,
            }
        ));
    }

    #[test]
    fn test_matches_pattern_map_with_rest() {
        let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        m.insert(ustr::ustr("a"), Val::Int(1));
        m.insert(ustr::ustr("b"), Val::Int(2));
        let val = Val::Map(m);
        assert!(matches_pattern(
            &val,
            &MatchPattern::Map {
                fields: vec![("a".into(), MatchPattern::Literal(LiteralPattern::Int(1)))],
                rest: true,
            }
        ));
    }

    #[test]
    fn test_matches_pattern_map_extra_field_no_rest() {
        let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        m.insert(ustr::ustr("a"), Val::Int(1));
        m.insert(ustr::ustr("b"), Val::Int(2));
        let val = Val::Map(m);
        assert!(!matches_pattern(
            &val,
            &MatchPattern::Map {
                fields: vec![("a".into(), MatchPattern::Literal(LiteralPattern::Int(1)))],
                rest: false,
            }
        ));
    }

    #[test]
    fn test_matches_pattern_map_missing_field() {
        let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        m.insert(ustr::ustr("a"), Val::Int(1));
        let val = Val::Map(m);
        assert!(!matches_pattern(
            &val,
            &MatchPattern::Map {
                fields: vec![("b".into(), MatchPattern::Wildcard)],
                rest: false,
            }
        ));
    }

    #[test]
    fn test_matches_pattern_map_not_map() {
        assert!(!matches_pattern(
            &Val::Int(1),
            &MatchPattern::Map {
                fields: vec![],
                rest: true,
            }
        ));
    }
    // is_query_stage tests
    #[test]
    fn test_is_query_stage_dollar_prefix() {
        let stage = PipelineStage::CommandCall {
            name: "$myvar".into(),
            args: vec![],
            env: vec![],
        };
        assert!(is_query_stage(&stage, &FxHashMap::default()));
    }

    #[test]
    fn test_is_query_stage_ls_command() {
        let stage = PipelineStage::CommandCall {
            name: "ls".into(),
            args: vec![],
            env: vec![],
        };
        assert!(is_query_stage(&stage, &FxHashMap::default()));
    }

    #[test]
    fn test_is_query_stage_registered_builtin() {
        let stage = PipelineStage::CommandCall {
            name: "some_builtin".into(),
            args: vec![],
            env: vec![],
        };
        let mut registry = FxHashMap::default();
        let handler: BuiltinHandler =
            Arc::new(|_: Option<PipeStream>, _: Vec<Val>, _: &Env, _: PipeSender| Ok(()));
        registry.insert("some_builtin".into(), handler);
        assert!(!is_query_stage(&stage, &registry));
    }

    #[test]
    fn test_is_query_stage_unknown_not_query() {
        let stage = PipelineStage::CommandCall {
            name: "unknown_cmd".into(),
            args: vec![],
            env: vec![],
        };
        assert!(!is_query_stage(&stage, &FxHashMap::default()));
    }

    #[test]
    fn test_is_query_stage_pipeline_operators() {
        assert!(is_query_stage(
            &PipelineStage::Filter {
                condition: Expr::Null
            },
            &FxHashMap::default()
        ));
        assert!(is_query_stage(
            &PipelineStage::Map {
                projections: vec![]
            },
            &FxHashMap::default()
        ));
        assert!(is_query_stage(
            &PipelineStage::Sort {
                column: "x".into(),
                descending: false
            },
            &FxHashMap::default()
        ));
        assert!(is_query_stage(
            &PipelineStage::Grep {
                pattern: Expr::Null
            },
            &FxHashMap::default()
        ));
        assert!(is_query_stage(&PipelineStage::Count, &FxHashMap::default()));
        assert!(is_query_stage(
            &PipelineStage::Limit { amount: Expr::Null },
            &FxHashMap::default()
        ));
        assert!(is_query_stage(
            &PipelineStage::BoundaryOperator {
                format: SerializationFormat::Json
            },
            &FxHashMap::default()
        ));
    }
    // eval_expr tests
    #[tokio::test]
    async fn test_eval_expr_null() {
        let env = Env::new();
        assert_eq!(eval_expr(&Expr::Null, &env).await.unwrap(), Val::Null);
    }

    #[tokio::test]
    async fn test_eval_expr_bool() {
        let env = Env::new();
        assert_eq!(
            eval_expr(&Expr::Bool(true), &env).await.unwrap(),
            Val::Bool(true)
        );
        assert_eq!(
            eval_expr(&Expr::Bool(false), &env).await.unwrap(),
            Val::Bool(false)
        );
    }

    #[tokio::test]
    async fn test_eval_expr_int() {
        let env = Env::new();
        assert_eq!(eval_expr(&Expr::Int(42), &env).await.unwrap(), Val::Int(42));
        assert_eq!(eval_expr(&Expr::Int(-5), &env).await.unwrap(), Val::Int(-5));
    }

    #[tokio::test]
    async fn test_eval_expr_float() {
        let env = Env::new();
        assert_eq!(
            eval_expr(&Expr::Float(3.14), &env).await.unwrap(),
            Val::Float(3.14)
        );
    }

    #[tokio::test]
    async fn test_eval_expr_ident_resolved() {
        let env = Env::new();
        env.vars.write().insert("myvar".into(), Val::Int(99));
        assert_eq!(
            eval_expr(&Expr::Ident("myvar".into()), &env).await.unwrap(),
            Val::Int(99)
        );
    }

    #[tokio::test]
    async fn test_eval_expr_ident_unresolved() {
        let env = Env::new();
        assert_eq!(
            eval_expr(&Expr::Ident("foobar".into()), &env)
                .await
                .unwrap(),
            Val::String("foobar".into())
        );
    }

    #[tokio::test]
    async fn test_eval_expr_string_literal() {
        let env = Env::new();
        assert_eq!(
            eval_expr(
                &Expr::String(vec![StringPart::Lit("hello world".into())]),
                &env
            )
            .await
            .unwrap(),
            Val::String("hello world".into())
        );
    }

    #[tokio::test]
    async fn test_eval_expr_string_interpolation() {
        let env = Env::new();
        env.vars
            .write()
            .insert("name".into(), Val::String("Alice".into()));
        let expr = Expr::String(vec![
            StringPart::Lit("Hello, ".into()),
            StringPart::Expr(Box::new(Expr::Ident("name".into()))),
            StringPart::Lit("!".into()),
        ]);
        assert_eq!(
            eval_expr(&expr, &env).await.unwrap(),
            Val::String("Hello, Alice!".into())
        );
    }

    #[tokio::test]
    async fn test_eval_expr_string_interpolation_non_string() {
        let env = Env::new();
        env.vars.write().insert("val".into(), Val::Int(42));
        let expr = Expr::String(vec![
            StringPart::Lit("value=".into()),
            StringPart::Expr(Box::new(Expr::Variable("val".into()))),
        ]);
        assert_eq!(
            eval_expr(&expr, &env).await.unwrap(),
            Val::String("value=42".into())
        );
    }

    #[tokio::test]
    async fn test_eval_expr_list() {
        let env = Env::new();
        let expr = Expr::List(vec![Expr::Int(1), Expr::Int(2), Expr::Int(3)]);
        assert_eq!(
            eval_expr(&expr, &env).await.unwrap(),
            Val::List(vec![Val::Int(1), Val::Int(2), Val::Int(3)])
        );
    }

    #[tokio::test]
    async fn test_eval_expr_not_negates_bool() {
        let env = Env::new();
        assert_eq!(
            eval_expr(&Expr::Not(Box::new(Expr::Bool(true))), &env)
                .await
                .unwrap(),
            Val::Bool(false)
        );
        assert_eq!(
            eval_expr(&Expr::Not(Box::new(Expr::Bool(false))), &env)
                .await
                .unwrap(),
            Val::Bool(true)
        );
    }

    #[tokio::test]
    async fn test_eval_expr_double_not() {
        let env = Env::new();
        assert_eq!(
            eval_expr(
                &Expr::Not(Box::new(Expr::Not(Box::new(Expr::Bool(true))))),
                &env
            )
            .await
            .unwrap(),
            Val::Bool(true)
        );
    }

    #[tokio::test]
    async fn test_eval_expr_not_type_error() {
        let env = Env::new();
        let result = eval_expr(&Expr::Not(Box::new(Expr::Int(42))), &env).await;
        assert!(result.is_err(), "Expected error for ! on non-bool");
    }

    #[tokio::test]
    async fn test_eval_expr_map() {
        let env = Env::new();
        let expr = Expr::Map(vec![
            ("a".into(), Expr::Int(10)),
            (
                "b".into(),
                Expr::String(vec![StringPart::Lit("hello".into())]),
            ),
        ]);
        let val = eval_expr(&expr, &env).await.unwrap();
        match val {
            Val::Map(m) => {
                assert_eq!(m.get(&ustr::ustr("a")), Some(&Val::Int(10)));
                assert_eq!(m.get(&ustr::ustr("b")), Some(&Val::String("hello".into())));
            }
            _ => panic!("Expected Map"),
        }
    }

    #[tokio::test]
    async fn test_eval_expr_variable_found() {
        let env = Env::new();
        env.vars.write().insert("x".into(), Val::Bool(true));
        assert_eq!(
            eval_expr(&Expr::Variable("x".into()), &env).await.unwrap(),
            Val::Bool(true)
        );
    }

    #[tokio::test]
    async fn test_eval_expr_variable_not_found() {
        let env = Env::new();
        let err = eval_expr(&Expr::Variable("undefined".into()), &env)
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::VariableNotFound { .. }));
    }

    #[tokio::test]
    async fn test_eval_expr_member_access_ok() {
        let env = Env::new();
        let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        m.insert(ustr::ustr("field"), Val::String("value".into()));
        env.vars.write().insert("obj".into(), Val::Map(m));
        let expr = Expr::MemberAccess {
            expr: Box::new(Expr::Variable("obj".into())),
            member: "field".into(),
        };
        assert_eq!(
            eval_expr(&expr, &env).await.unwrap(),
            Val::String("value".into())
        );
    }

    #[tokio::test]
    async fn test_eval_expr_member_access_not_found() {
        let env = Env::new();
        let m = Val::empty_map();
        env.vars.write().insert("obj".into(), m);
        let expr = Expr::MemberAccess {
            expr: Box::new(Expr::Variable("obj".into())),
            member: "nonexistent".into(),
        };
        let err = eval_expr(&expr, &env).await.unwrap_err();
        assert_eq!(err.to_string(), "Map has no field 'nonexistent'");
    }

    #[tokio::test]
    async fn test_eval_expr_member_access_not_map() {
        let env = Env::new();
        env.vars.write().insert("x".into(), Val::Int(42));
        let expr = Expr::MemberAccess {
            expr: Box::new(Expr::Variable("x".into())),
            member: "field".into(),
        };
        let err = eval_expr(&expr, &env).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "Member access is only supported on Map, ObjectGraph, or capability modules"
        );
    }

    #[tokio::test]
    async fn test_eval_expr_pipeline_single_count() {
        let env = Env::new();
        let pipeline = Pipeline {
            stages: vec![PipelineStage::Count],
        };
        let expr = Expr::Pipeline(pipeline);
        let res = eval_expr(&expr, &env).await.unwrap();
        assert_eq!(res, Val::List(vec![]));
    }

    #[tokio::test]
    async fn test_eval_expr_pipeline_dollar_variable() {
        let env = Env::new();
        env.vars.write().insert(
            "items".into(),
            Val::List(vec![Val::Int(1), Val::Int(2), Val::Int(3)]),
        );
        let pipeline = Pipeline {
            stages: vec![PipelineStage::CommandCall {
                name: "$items".into(),
                args: vec![],
                env: vec![],
            }],
        };
        let expr = Expr::Pipeline(pipeline);
        let res = eval_expr(&expr, &env).await.unwrap();
        assert_eq!(res, Val::List(vec![Val::Int(1), Val::Int(2), Val::Int(3)]));
    }

    #[tokio::test]
    async fn test_eval_expr_binary_op_type_error() {
        let env = Env::new();
        let expr = Expr::BinaryOp {
            op: BinOp::Add,
            lhs: Box::new(Expr::Int(1)),
            rhs: Box::new(Expr::Bool(true)),
        };
        let err = eval_expr(&expr, &env).await.unwrap_err();
        assert!(matches!(err, EngineError::TypeMismatch { .. }));
    }
    // eval_stmt tests
    #[tokio::test]
    async fn test_eval_stmt_fn_def() {
        let env = Env::new();
        let stmt = Stmt::FnDef {
            name: "foo".into(),
            params: vec![Param {
                name: "x".into(),
                constraint: TypeConstraint::Any,
            }],
            ret_type: None,
            body: vec![Stmt::Let {
                name: "result".into(),
                expr: Expr::Variable("x".into()),
            }],
        };
        eval_stmt(&stmt, &env, false).await.unwrap();
        let fns = env.fns.read();
        assert!(fns.contains_key("foo"));
        let (params, _ret_type, body) = fns.get("foo").unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "x");
        assert_eq!(body.len(), 1);
    }

    #[tokio::test]
    async fn test_eval_stmt_expr_expression() {
        let env = Env::new();
        let stmt = Stmt::Expr(Expr::Int(42));
        eval_stmt(&stmt, &env, false).await.unwrap();
    }

    #[tokio::test]
    async fn test_eval_stmt_try_no_error() {
        let env = Env::new();
        let stmt = Stmt::TryCatch {
            try_body: vec![Stmt::Let {
                name: "x".into(),
                expr: Expr::Int(10),
            }],
            catch_var: "e".into(),
            catch_body: vec![Stmt::Let {
                name: "caught".into(),
                expr: Expr::Bool(true),
            }],
        };
        eval_stmt(&stmt, &env, false).await.unwrap();
        let vars = env.vars.read();
        assert_eq!(vars.get("x"), Some(&Val::Int(10)));
        assert!(
            !vars.contains_key("caught"),
            "Catch block should not execute on success"
        );
    }

    #[tokio::test]
    async fn test_eval_stmt_try_with_catch() {
        let env = Env::new();
        let stmt = Stmt::TryCatch {
            try_body: vec![Stmt::Expr(Expr::Variable("nonexistent".into()))],
            catch_var: "err".into(),
            catch_body: vec![Stmt::Let {
                name: "handled".into(),
                expr: Expr::Bool(true),
            }],
        };
        eval_stmt(&stmt, &env, false).await.unwrap();
        let vars = env.vars.read();
        assert_eq!(vars.get("handled"), Some(&Val::Bool(true)));
        if let Some(Val::Map(m)) = vars.get("err") {
            assert_eq!(
                m.get(&ustr("code")),
                Some(&Val::String("FSH-SCOPE-001".into()))
            );
            assert!(
                matches!(m.get(&ustr("message")), Some(Val::String(s)) if s.contains("nonexistent"))
            );
        } else {
            panic!(
                "Expected Val::Map for catch variable, got {:?}",
                vars.get("err")
            );
        }
    }

    #[tokio::test]
    async fn test_eval_stmt_try_catch_binds_var() {
        let env = Env::new();
        let stmt = Stmt::TryCatch {
            try_body: vec![Stmt::Expr(Expr::Variable("bad".into()))],
            catch_var: "err_msg".into(),
            catch_body: vec![Stmt::Let {
                name: "logged".into(),
                expr: Expr::Ident("err_msg".into()),
            }],
        };
        eval_stmt(&stmt, &env, false).await.unwrap();
        let vars = env.vars.read();
        if let Some(Val::Map(m)) = vars.get("err_msg") {
            assert_eq!(
                m.get(&ustr("code")),
                Some(&Val::String("FSH-SCOPE-001".into()))
            );
            assert!(matches!(m.get(&ustr("message")), Some(Val::String(s)) if s.contains("bad")));
        } else {
            panic!(
                "Expected Val::Map for err_msg, got {:?}",
                vars.get("err_msg")
            );
        }
        assert_eq!(vars.get("logged"), vars.get("err_msg"));
    }

    #[tokio::test]
    async fn test_eval_stmt_try_catch_nested() {
        let env = Env::new();
        let stmt = Stmt::TryCatch {
            try_body: vec![
                Stmt::TryCatch {
                    try_body: vec![Stmt::Expr(Expr::Variable("inner_bad".into()))],
                    catch_var: "inner_err".into(),
                    catch_body: vec![Stmt::Let {
                        name: "inner_handled".into(),
                        expr: Expr::Bool(true),
                    }],
                },
                Stmt::Expr(Expr::Variable("outer_bad".into())),
            ],
            catch_var: "outer_err".into(),
            catch_body: vec![Stmt::Let {
                name: "outer_handled".into(),
                expr: Expr::Bool(true),
            }],
        };
        eval_stmt(&stmt, &env, false).await.unwrap();
        let vars = env.vars.read();
        assert_eq!(vars.get("inner_handled"), Some(&Val::Bool(true)));
        assert!(vars.contains_key("inner_err"));
        assert_eq!(vars.get("outer_handled"), Some(&Val::Bool(true)));
        assert!(vars.contains_key("outer_err"));
    }

    #[tokio::test]
    async fn test_eval_stmt_with_caps_grants_then_restores() {
        let env = Env::new();
        assert!(!env.caps.caps.read().check_env_read("TEST_VAR"));
        env.vars.write().insert(
            "test_cap".into(),
            Val::Capability(ResourceHandle::ReadEnv("TEST_VAR".into())),
        );
        let stmt = Stmt::WithCaps {
            caps: vec![Expr::Variable("test_cap".into())],
            body: vec![Stmt::Let {
                name: "inside".into(),
                expr: Expr::Bool(true),
            }],
        };
        eval_stmt(&stmt, &env, false).await.unwrap();
        let vars = env.vars.read();
        assert_eq!(vars.get("inside"), Some(&Val::Bool(true)));
        assert!(!env.caps.caps.read().check_env_read("TEST_VAR"));
    }

    #[tokio::test]
    async fn test_eval_stmt_with_caps_error_does_not_leak() {
        let env = Env::new();
        env.vars.write().insert(
            "test_cap".into(),
            Val::Capability(ResourceHandle::ReadEnv("LEAK".into())),
        );
        let stmt = Stmt::WithCaps {
            caps: vec![Expr::Variable("test_cap".into())],
            body: vec![Stmt::Expr(Expr::Variable("undefined".into()))],
        };
        assert!(eval_stmt(&stmt, &env, false).await.is_err());
        assert!(!env.caps.caps.read().check_env_read("LEAK"));
    }

    #[tokio::test]
    async fn test_eval_stmt_reactive_cell_rejects_mutation() {
        let env = Env::new();
        let stmt = Stmt::ReactiveCell {
            name: "bad".into(),
            pipeline: Pipeline {
                stages: vec![PipelineStage::CommandCall {
                    name: "rm".into(),
                    args: vec![],
                    env: vec![],
                }],
            },
        };
        let err = eval_stmt(&stmt, &env, false).await.unwrap_err();
        assert!(
            err.to_string().contains("Mutation"),
            "Expected mutation rejection, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_eval_stmt_reactive_cell_allows_query() {
        let env = Env::new();
        let stmt = Stmt::ReactiveCell {
            name: "live".into(),
            pipeline: Pipeline {
                stages: vec![PipelineStage::Count],
            },
        };
        let res = eval_stmt(&stmt, &env, false).await;
        assert!(
            res.is_ok(),
            "Expected query to be allowed, got: {:?}",
            res.err()
        );
        let cells = env.reactive.cells.read();
        assert!(cells.contains_key("live"));
    }

    #[tokio::test]
    async fn test_eval_stmt_reactive_cell_allowed_in_unsafe() {
        let env = Env::new();
        let stmt = Stmt::Unsafe {
            body: vec![Stmt::ReactiveCell {
                name: "x".into(),
                pipeline: Pipeline {
                    stages: vec![PipelineStage::CommandCall {
                        name: "rm".into(),
                        args: vec![],
                        env: vec![],
                    }],
                },
            }],
        };
        let res = eval_stmt(&stmt, &env, false).await;
        assert!(
            res.is_ok(),
            "Expected unsafe to allow mutation, got: {:?}",
            res.err()
        );
        let cells = env.reactive.cells.read();
        assert!(cells.contains_key("x"));
    }

    #[tokio::test]
    async fn test_eval_stmt_match_wildcard() {
        let env = Env::new();
        env.vars
            .write()
            .insert("val".into(), Val::String("anything".into()));
        let stmt = Stmt::Match {
            expr: Expr::Variable("val".into()),
            arms: vec![MatchArm {
                pattern: MatchPattern::Wildcard,
                body: vec![Stmt::Let {
                    name: "matched".into(),
                    expr: Expr::Bool(true),
                }],
            }],
        };
        eval_stmt(&stmt, &env, false).await.unwrap();
        assert_eq!(env.vars.read().get("matched"), Some(&Val::Bool(true)));
    }

    #[tokio::test]
    async fn test_eval_stmt_match_literal_int() {
        let env = Env::new();
        env.vars.write().insert("val".into(), Val::Int(42));
        let stmt = Stmt::Match {
            expr: Expr::Variable("val".into()),
            arms: vec![
                MatchArm {
                    pattern: MatchPattern::Literal(LiteralPattern::Int(0)),
                    body: vec![Stmt::Let {
                        name: "matched".into(),
                        expr: Expr::String(vec![StringPart::Lit("zero".into())]),
                    }],
                },
                MatchArm {
                    pattern: MatchPattern::Literal(LiteralPattern::Int(42)),
                    body: vec![Stmt::Let {
                        name: "matched".into(),
                        expr: Expr::String(vec![StringPart::Lit("forty-two".into())]),
                    }],
                },
            ],
        };
        eval_stmt(&stmt, &env, false).await.unwrap();
        assert_eq!(
            env.vars.read().get("matched"),
            Some(&Val::String("forty-two".into()))
        );
    }

    #[tokio::test]
    async fn test_eval_stmt_match_literal_string() {
        let env = Env::new();
        env.vars
            .write()
            .insert("val".into(), Val::String("hello".into()));
        let stmt = Stmt::Match {
            expr: Expr::Variable("val".into()),
            arms: vec![MatchArm {
                pattern: MatchPattern::Literal(LiteralPattern::String("hello".into())),
                body: vec![Stmt::Let {
                    name: "found".into(),
                    expr: Expr::Bool(true),
                }],
            }],
        };
        eval_stmt(&stmt, &env, false).await.unwrap();
        assert_eq!(env.vars.read().get("found"), Some(&Val::Bool(true)));
    }

    #[tokio::test]
    async fn test_eval_stmt_match_map_pattern() {
        let env = Env::new();
        let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        m.insert(ustr::ustr("key"), Val::Int(42));
        env.vars.write().insert("obj".into(), Val::Map(m));
        let stmt = Stmt::Match {
            expr: Expr::Variable("obj".into()),
            arms: vec![MatchArm {
                pattern: MatchPattern::Map {
                    fields: vec![("key".into(), MatchPattern::Literal(LiteralPattern::Int(42)))],
                    rest: true,
                },
                body: vec![Stmt::Let {
                    name: "matched".into(),
                    expr: Expr::Bool(true),
                }],
            }],
        };
        eval_stmt(&stmt, &env, false).await.unwrap();
        assert_eq!(env.vars.read().get("matched"), Some(&Val::Bool(true)));
    }

    #[tokio::test]
    async fn test_eval_stmt_match_no_match() {
        let env = Env::new();
        env.vars.write().insert("val".into(), Val::Int(99));
        let stmt = Stmt::Match {
            expr: Expr::Variable("val".into()),
            arms: vec![MatchArm {
                pattern: MatchPattern::Literal(LiteralPattern::Int(0)),
                body: vec![Stmt::Let {
                    name: "matched".into(),
                    expr: Expr::Bool(true),
                }],
            }],
        };
        let err = eval_stmt(&stmt, &env, false).await.unwrap_err();
        assert!(matches!(err, EngineError::MatchNonExhaustive { .. }));
    }

    #[tokio::test]
    async fn test_eval_stmt_unsafe_no_context_propagation_to_try() {
        let env = Env::new();
        let stmt = Stmt::Unsafe {
            body: vec![Stmt::TryCatch {
                try_body: vec![Stmt::ReactiveCell {
                    name: "leaked".into(),
                    pipeline: Pipeline {
                        stages: vec![PipelineStage::CommandCall {
                            name: "write".into(),
                            args: vec![],
                            env: vec![],
                        }],
                    },
                }],
                catch_var: "e".into(),
                catch_body: vec![Stmt::Let {
                    name: "caught".into(),
                    expr: Expr::Bool(true),
                }],
            }],
        };
        let res = eval_stmt(&stmt, &env, false).await;
        assert!(res.is_ok(), "Unsafe+Try should be ok: {:?}", res.err());
        let vars = env.vars.read();
        assert_eq!(vars.get("caught"), Some(&Val::Bool(true)));
        assert!(!vars.contains_key("leaked"));
    }

    #[test]
    fn test_native_glob_and_brace_expansion() {
        // 1. Brace expansion list
        let res = expand_braces("file.{rs,md}");
        assert_eq!(res, vec!["file.rs", "file.md"]);

        // 2. Brace expansion range
        let res = expand_braces("{1..3}");
        assert_eq!(res, vec!["1", "2", "3"]);

        // 3. Zero-padded numeric range
        let res = expand_braces("{01..03}");
        assert_eq!(res, vec!["01", "02", "03"]);

        // 4. Nested brace expansion
        let res = expand_braces("{a,b}/{1,2}");
        assert_eq!(res, vec!["a/1", "a/2", "b/1", "b/2"]);

        // 5. Native glob expansion (should find Cargo.toml since it exists in workspace)
        let env = Env::new();
        let args = vec![Val::String("Cargo.to*".to_string())];
        let expanded = expand_globs(args, &env).unwrap();
        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0], Val::String("Cargo.toml".to_string()));
    }

    #[tokio::test]
    async fn test_eval_stmt_every() {
        let env = Env::new();
        let stmt = Stmt::Every {
            duration: 1,
            unit: TimeUnit::Second,
            body: vec![
                Stmt::Let {
                    name: "triggered".into(),
                    expr: Expr::Bool(true),
                },
                Stmt::Expr(Expr::Null),
            ],
        };

        let trigger_flag = env.job_control.sigint_pending.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            trigger_flag.store(true, Ordering::SeqCst);
        });

        let res = eval_stmt(&stmt, &env, false).await;
        assert!(res.is_err());
        assert_eq!(env.vars.read().get("triggered"), Some(&Val::Bool(true)));
    }

    #[tokio::test]
    async fn test_eval_stmt_reactive_cell_every() {
        let env = Env::new();
        let stmt = Stmt::ReactiveCellEvery {
            name: "live_time".into(),
            duration: 1,
            unit: TimeUnit::Second,
            body: vec![Stmt::Expr(Expr::Int(100))],
        };

        eval_stmt(&stmt, &env, false).await.unwrap();

        let rx = {
            let cells = env.reactive.cells.read();
            assert!(cells.contains_key("live_time"));
            cells.get("live_time").unwrap().clone()
        };

        let rx_clone = rx.clone();
        tokio::time::timeout(std::time::Duration::from_millis(1500), async move {
            while rx_clone.borrow().is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("Timeout waiting for reactive cell every update");

        let val: Arc<Vec<Val>> = rx.borrow().clone();
        assert_eq!(val.as_ref(), &vec![Val::Int(100)]);
    }

    #[tokio::test]
    async fn test_eval_stmt_source_bash() {
        let env = Env::new();
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("test_script.sh");
        crate::register_posix_handler(
            |content: String, _args: Vec<String>, env: crate::Env, _capture: bool| async move {
                for line in content.lines() {
                    let line = line.trim();
                    if line.starts_with("FOO=") {
                        env.vars
                            .write()
                            .insert("FOO".to_string(), Val::String("bar".into()));
                    } else if line.starts_with("export BAZ=") {
                        env.vars
                            .write()
                            .insert("BAZ".to_string(), Val::String("qux".into()));
                    } else if line.starts_with("alias ll=") {
                        env.register_alias("ll", "ls -la");
                    } else if line.starts_with("VIRTUAL_ENV=") {
                        env.vars
                            .write()
                            .insert("VIRTUAL_ENV".to_string(), Val::String("/opt/venv".into()));
                    } else if line.starts_with("export PATH=") {
                        env.vars.write().insert(
                            "PATH".to_string(),
                            Val::String("/opt/venv/bin:/usr/bin".into()),
                        );
                    }
                }
                Ok((0, None))
            },
        );

        std::fs::write(
            &file_path,
            b"# test script\nFOO=bar\nexport BAZ=\"qux\"\nalias ll='ls -la'\n",
        )
        .unwrap();

        let stmt = Stmt::Source {
            path: Expr::String(vec![StringPart::Lit(
                file_path.to_string_lossy().to_string(),
            )]),
            bash: true,
        };

        eval_stmt(&stmt, &env, false).await.unwrap();

        let vars = env.vars.read();
        assert_eq!(vars.get("FOO"), Some(&Val::String("bar".into())));
        assert_eq!(vars.get("BAZ"), Some(&Val::String("qux".into())));
        assert_eq!(env.get_alias("ll"), Some("ls -la".to_string()));

        remove_var("FOO");
        remove_var("BAZ");
        env.remove_alias("ll");
    }

    #[tokio::test]
    async fn test_eval_stmt_source_autodetects_bash() {
        // Plain `source` of a bash script (like a venv `activate`) must
        // delegate to the bash shim instead of surfacing a parse error.
        let env = Env::new();
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("activate");
        std::fs::write(
            &file_path,
            b"# venv activate script\nVIRTUAL_ENV=\"/opt/venv\"\nexport VIRTUAL_ENV\nexport PATH=\"/opt/venv/bin:$PATH\"\ndeactivate () {\n    unset VIRTUAL_ENV\n}\n",
        )
        .unwrap();

        let stmt = Stmt::Source {
            path: Expr::String(vec![StringPart::Lit(
                file_path.to_string_lossy().to_string(),
            )]),
            bash: false,
        };

        eval_stmt(&stmt, &env, false).await.unwrap();

        let vars = env.vars.read();
        assert_eq!(
            vars.get("VIRTUAL_ENV"),
            Some(&Val::String("/opt/venv".into()))
        );
        assert!(
            vars.get("PATH")
                .unwrap()
                .to_text()
                .contains("/opt/venv/bin")
        );

        remove_var("VIRTUAL_ENV");
    }

    #[tokio::test]
    async fn test_eval_stmt_source_invalid_fshell_still_errors() {
        // A non-bash file that fails to parse should still report a parse
        // error rather than being silently delegated to the bash shim.
        let env = Env::new();
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("broken.fsh");
        std::fs::write(&file_path, b"let x = | invalid fshell syntax !!!\n").unwrap();

        let stmt = Stmt::Source {
            path: Expr::String(vec![StringPart::Lit(
                file_path.to_string_lossy().to_string(),
            )]),
            bash: false,
        };

        let err = eval_stmt(&stmt, &env, false).await.unwrap_err();
        assert!(matches!(err, EngineError::Parse(_)));
    }
    // CSV / table / bar format tests
    #[test]
    fn test_coerce_csv_value_int() {
        assert_eq!(super::parse_csv_field("42"), Val::Int(42));
        assert_eq!(super::parse_csv_field("-7"), Val::Int(-7));
        assert_eq!(super::parse_csv_field("0"), Val::Int(0));
    }

    #[test]
    fn test_coerce_csv_value_float() {
        assert_eq!(super::parse_csv_field("3.14"), Val::Float(3.14));
        assert_eq!(super::parse_csv_field("0.5"), Val::Float(0.5));
    }

    #[test]
    fn test_coerce_csv_value_string() {
        assert_eq!(super::parse_csv_field("hello"), Val::String("hello".into()));
        assert_eq!(
            super::parse_csv_field("abc123"),
            Val::String("abc123".into())
        );
    }

    #[test]
    fn test_coerce_csv_value_empty() {
        assert_eq!(super::parse_csv_field(""), Val::Null);
        assert_eq!(super::parse_csv_field("   "), Val::Null);
    }

    #[test]
    fn test_coerce_csv_value_trimmed() {
        assert_eq!(super::parse_csv_field("  42  "), Val::Int(42));
    }

    #[test]
    fn test_decode_csv_input_basic() {
        let csv = "name,size\nmain.rs,14230\nlib.rs,89201\n";
        let result = super::decode_csv_input(csv).unwrap();
        if let Val::List(items) = result {
            assert_eq!(items.len(), 2);
            if let Val::Map(map) = &items[0] {
                assert_eq!(map.get(&ustr("name")), Some(&Val::String("main.rs".into())));
                assert_eq!(map.get(&ustr("size")), Some(&Val::Int(14230)));
            } else {
                panic!("Expected Map");
            }
        } else {
            panic!("Expected List");
        }
    }

    #[test]
    fn test_decode_csv_input_headers_only() {
        let csv = "name,size\n";
        let result = super::decode_csv_input(csv).unwrap();
        if let Val::List(items) = result {
            assert!(items.is_empty());
        } else {
            panic!("Expected List");
        }
    }

    #[test]
    fn test_decode_csv_input_quoted_fields() {
        let csv = "name,desc\nfoo,\"hello, world\"\n";
        let result = super::decode_csv_input(csv).unwrap();
        if let Val::List(items) = result {
            assert_eq!(items.len(), 1);
            if let Val::Map(map) = &items[0] {
                assert_eq!(
                    map.get(&ustr("desc")),
                    Some(&Val::String("hello, world".into()))
                );
            } else {
                panic!("Expected Map");
            }
        } else {
            panic!("Expected List");
        }
    }

    #[test]
    fn test_render_table_basic() {
        let items = vec![Val::Map({
            let mut m = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            m.insert(ustr("name"), Val::String("foo".into()));
            m.insert(ustr("size"), Val::Int(100));
            m
        })];
        let table = super::render_table(&items);
        assert!(table.contains("name"));
        assert!(table.contains("foo"));
        assert!(table.contains("100"));
        assert!(table.starts_with("|"));
    }

    #[test]
    fn test_render_table_empty() {
        assert_eq!(super::render_table(&[]), "(no results)");
    }

    #[test]
    fn test_render_table_multiple_rows() {
        let items = vec![
            Val::Map({
                let mut m = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
                m.insert(ustr("x"), Val::Int(1));
                m
            }),
            Val::Map({
                let mut m = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
                m.insert(ustr("x"), Val::Int(2));
                m
            }),
        ];
        let table = super::render_table(&items);
        assert_eq!(table.lines().count(), 4); // header + separator + 2 data rows
    }

    #[test]
    fn test_render_bar_chart_basic() {
        let items = vec![Val::Map({
            let mut m = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            m.insert(ustr("ext"), Val::String("rs".into()));
            m.insert(ustr("count"), Val::Int(42));
            m
        })];
        let chart = super::render_bar_chart(&items);
        assert!(chart.contains("rs"));
        assert!(chart.contains("42"));
    }

    #[test]
    fn test_render_bar_chart_empty() {
        assert_eq!(super::render_bar_chart(&[]), "(no data)");
    }

    #[test]
    fn test_render_bar_chart_no_numeric() {
        let items = vec![Val::Map({
            let mut m = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            m.insert(ustr("name"), Val::String("foo".into()));
            m
        })];
        assert_eq!(super::render_bar_chart(&items), "(no numeric data found)");
    }

    #[test]
    fn test_render_bar_chart_sort_order() {
        let items = vec![
            Val::Map({
                let mut m = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
                m.insert(ustr("name"), Val::String("small".into()));
                m.insert(ustr("val"), Val::Int(10));
                m
            }),
            Val::Map({
                let mut m = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
                m.insert(ustr("name"), Val::String("big".into()));
                m.insert(ustr("val"), Val::Int(100));
                m
            }),
        ];
        let chart = super::render_bar_chart(&items);
        // big should come before small (descending sort)
        let big_pos = chart.find("big").unwrap();
        let small_pos = chart.find("small").unwrap();
        assert!(big_pos < small_pos, "bars should be sorted descending");
    }

    #[test]
    fn test_pad_or_truncate_short() {
        let result = super::pad_truncate("hello", 10);
        assert_eq!(result, "hello     ");
    }

    #[test]
    fn test_pad_or_truncate_exact() {
        let result = super::pad_truncate("hello", 5);
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_pad_or_truncate_long() {
        let result = super::pad_truncate("hello world", 5);
        assert_eq!(result, "hell…");
    }

    #[test]
    fn test_pad_or_truncate_unicode() {
        // Unicode chars must not be split at byte boundary
        let result = super::pad_truncate("héllo", 3);
        assert_eq!(result, "hé…");
    }

    #[tokio::test]
    async fn test_exit_code_variables_and_shadowing() {
        let env = Env::new();
        // Set last exit code to 42
        *env.prompt.last_exit_code.write() = 42;

        // Check eval_expr resolves status and ?
        assert_eq!(
            eval_expr(&Expr::Variable("status".into()), &env)
                .await
                .unwrap(),
            Val::Int(42)
        );
        assert_eq!(
            eval_expr(&Expr::Variable("?".into()), &env).await.unwrap(),
            Val::Int(42)
        );

        // Check try_eval_sync resolves status and ?
        assert_eq!(
            crate::eval::try_eval_sync(&Expr::Variable("status".into()), &env)
                .unwrap()
                .unwrap(),
            Val::Int(42)
        );
        assert_eq!(
            crate::eval::try_eval_sync(&Expr::Variable("?".into()), &env)
                .unwrap()
                .unwrap(),
            Val::Int(42)
        );

        // Shadow status with local variable
        {
            let mut locals = FxHashMap::default();
            locals.insert("status".to_string(), Val::Int(123));
            let env_with_locals = env.push_scope(Arc::new(fshell_core::RwLock::new(locals)));

            // Check both resolve status to 123 (shadowed)
            assert_eq!(
                eval_expr(&Expr::Variable("status".into()), &env_with_locals)
                    .await
                    .unwrap(),
                Val::Int(123)
            );
            assert_eq!(
                crate::eval::try_eval_sync(&Expr::Variable("status".into()), &env_with_locals)
                    .unwrap()
                    .unwrap(),
                Val::Int(123)
            );
        }
    }

    #[tokio::test]
    async fn test_loop_brace_expansion() {
        let env = Env::new();
        let script = "let sum = 0; for i in {1..5} { sum = $sum + $i }";
        let mut parser = fshell_core::Parser::new(script);
        let stmts = parser.parse_statements().unwrap();
        for stmt in &stmts {
            eval_stmt(stmt, &env, false).await.unwrap();
        }
        let vars = env.vars.read();
        assert_eq!(vars.get("sum"), Some(&Val::Int(15)));
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use crate::eval::eval_binop;
    use fshell_core::BinOp;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn test_add_does_not_panic(a: i64, b: i64) {
            let _result = eval_binop(BinOp::Add, Val::Int(a), Val::Int(b));
        }

        #[test]
        fn test_sub_does_not_panic(a: i64, b: i64) {
            let _result = eval_binop(BinOp::Sub, Val::Int(a), Val::Int(b));
        }

        #[test]
        fn test_mul_does_not_panic(a: i64, b: i64) {
            let _result = eval_binop(BinOp::Mul, Val::Int(a), Val::Int(b));
        }

        #[test]
        fn test_div_with_zero_returns_err(a: i64) {
            let result = eval_binop(BinOp::Div, Val::Int(a), Val::Int(0));
            assert!(result.is_err());
        }

        #[test]
        fn test_float_add_does_not_panic(a: f64, b: f64) {
            let _result = eval_binop(BinOp::Add, Val::Float(a), Val::Float(b));
        }

        #[test]
        fn test_float_sub_does_not_panic(a: f64, b: f64) {
            let _result = eval_binop(BinOp::Sub, Val::Float(a), Val::Float(b));
        }
    }
}
