//! Evaluator ordering, failure boundaries, lexical frames and control transfers.
//! Constructed ASTs keep these contracts independent of parser classification.

use fshell_core::{BinOp, Expr, RwLock, Stmt, Val};
use fshell_engine::eval::{ExprOutcome, eval_expr_flow};
use fshell_engine::{EngineError, Env, Flow, eval_expr, eval_stmt};
use std::sync::Arc;

fn binary(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
    Expr::BinaryOp {
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    }
}

fn assign(name: &str, expr: Expr) -> Stmt {
    Stmt::Assign {
        name: name.into(),
        expr,
    }
}

fn environment() -> Env {
    let env = Env::for_command();
    env.vars.write().insert("visited".into(), Val::Int(0));
    env
}

fn visited(env: &Env) -> Val {
    env.vars
        .read()
        .get("visited")
        .expect("trace binding")
        .clone()
}

fn effect(digit: i64, result: Expr) -> Expr {
    Expr::If {
        condition: Box::new(Expr::Bool(true)),
        then_body: vec![
            assign(
                "visited",
                binary(
                    BinOp::Add,
                    binary(BinOp::Mul, Expr::Variable("visited".into()), Expr::Int(10)),
                    Expr::Int(digit),
                ),
            ),
            Stmt::Expr(result),
        ],
        else_body: None,
    }
}

fn division_by_zero() -> Expr {
    binary(BinOp::Div, Expr::Int(1), Expr::Int(0))
}

fn returning(value: i64) -> Expr {
    Expr::If {
        condition: Box::new(Expr::Bool(true)),
        then_body: vec![Stmt::Return(Expr::Int(value))],
        else_body: None,
    }
}

#[tokio::test]
async fn boolean_short_circuit_skips_failing_and_effectful_right_operands() {
    for (op, lhs) in [(BinOp::And, false), (BinOp::Or, true)] {
        for rhs in [division_by_zero(), effect(1, Expr::Bool(!lhs))] {
            let env = environment();
            assert_eq!(
                eval_expr(&binary(op, Expr::Bool(lhs), rhs), &env)
                    .await
                    .expect("short circuit"),
                Val::Bool(lhs)
            );
            assert_eq!(visited(&env), Val::Int(0));
        }
    }
}

#[tokio::test]
async fn binary_operands_run_left_to_right_exactly_once() {
    let env = environment();
    let expr = binary(BinOp::Sub, effect(1, Expr::Int(8)), effect(2, Expr::Int(3)));
    assert_eq!(
        eval_expr(&expr, &env).await.expect("subtraction"),
        Val::Int(5)
    );
    assert_eq!(visited(&env), Val::Int(12));
}

#[tokio::test]
async fn list_elements_keep_value_order_and_effect_order() {
    let env = environment();
    let expr = Expr::List(vec![effect(2, Expr::Int(7)), effect(1, Expr::Int(4))]);
    assert_eq!(
        eval_expr(&expr, &env).await.expect("list"),
        Val::List(vec![Val::Int(7), Val::Int(4)])
    );
    assert_eq!(visited(&env), Val::Int(21));
}

#[tokio::test]
async fn map_values_keep_insertion_order_and_effect_order() {
    let env = environment();
    let expr = Expr::Map(vec![
        ("second".into(), effect(2, Expr::Int(7))),
        ("first".into(), effect(1, Expr::Int(4))),
    ]);
    let Val::Map(map) = eval_expr(&expr, &env).await.expect("map") else {
        unreachable!("expected typed map")
    };
    assert_eq!(
        map.iter()
            .map(|(key, value)| (key.as_str(), value.clone()))
            .collect::<Vec<_>>(),
        [("second", Val::Int(7)), ("first", Val::Int(4))]
    );
    assert_eq!(visited(&env), Val::Int(21));
}

#[tokio::test]
async fn first_list_error_preserves_prior_effects_and_skips_later_elements() {
    let env = environment();
    let expr = Expr::List(vec![
        effect(1, Expr::Int(7)),
        division_by_zero(),
        effect(3, Expr::Int(4)),
    ]);
    assert!(matches!(
        eval_expr(&expr, &env).await,
        Err(EngineError::DivisionByZero { span: None })
    ));
    assert_eq!(visited(&env), Val::Int(1));
}

#[tokio::test]
async fn return_unwinds_aggregate_and_binary_expressions_without_later_effects() {
    for expr in [
        Expr::List(vec![returning(9), effect(1, Expr::Int(3))]),
        Expr::Map(vec![
            ("first".into(), returning(9)),
            ("second".into(), effect(1, Expr::Int(3))),
        ]),
        binary(BinOp::Add, returning(9), effect(1, Expr::Int(3))),
    ] {
        let env = environment();
        assert_eq!(
            eval_expr_flow(&expr, &env)
                .await
                .expect("return propagation"),
            ExprOutcome::Return(Val::Int(9))
        );
        assert_eq!(visited(&env), Val::Int(0));
        assert!(env.get_last_error().is_none());
    }
}

#[tokio::test]
async fn failed_assignment_does_not_replace_the_previous_binding() {
    let env = environment();
    env.vars.write().insert("answer".into(), Val::Int(42));
    assert!(matches!(
        eval_stmt(&assign("answer", division_by_zero()), &env, false).await,
        Err(EngineError::DivisionByZero { span: None })
    ));
    assert_eq!(env.vars.read().get("answer"), Some(&Val::Int(42)));
}

#[tokio::test]
async fn statement_boolean_chains_suppress_and_restore_errexit() {
    for old_errexit in [false, true] {
        for is_and in [false, true] {
            let env = environment();
            env.options.write().errexit = old_errexit;
            let lhs = Box::new(Stmt::Expr(Expr::Bool(false)));
            let rhs = Box::new(Stmt::Expr(effect(1, Expr::Bool(true))));
            let stmt = if is_and {
                Stmt::And(lhs, rhs)
            } else {
                Stmt::Or(lhs, rhs)
            };
            assert_eq!(
                eval_stmt(&stmt, &env, false).await.expect("status chain"),
                Flow::Normal
            );
            assert_eq!(visited(&env), Val::Int(if is_and { 0 } else { 1 }));
            assert_eq!(env.exit_code(), if is_and { 1 } else { 0 });
            assert_eq!(env.options.read().errexit, old_errexit);
        }
    }
}

#[tokio::test]
async fn statement_boolean_chains_do_not_swallow_errors_or_return_signals() {
    for is_and in [false, true] {
        for fails in [false, true] {
            let env = environment();
            env.options.write().errexit = true;
            let lhs = Box::new(if fails {
                Stmt::Expr(division_by_zero())
            } else {
                Stmt::Return(Expr::Int(9))
            });
            let rhs = Box::new(Stmt::Expr(effect(1, Expr::Int(3))));
            let stmt = if is_and {
                Stmt::And(lhs, rhs)
            } else {
                Stmt::Or(lhs, rhs)
            };
            let result = eval_stmt(&stmt, &env, false).await;
            if fails {
                assert!(matches!(
                    result,
                    Err(EngineError::DivisionByZero { span: None })
                ));
            } else {
                assert_eq!(result.expect("return signal"), Flow::Return(Val::Int(9)));
            }
            assert_eq!(visited(&env), Val::Int(0));
            assert!(env.options.read().errexit);
        }
    }
}

#[tokio::test]
async fn negation_restores_errexit_on_errors_and_control_transfers() {
    for fails in [false, true] {
        let env = environment();
        env.options.write().errexit = true;
        let expr = Expr::Not(Box::new(if fails {
            division_by_zero()
        } else {
            returning(9)
        }));
        let result = eval_expr_flow(&expr, &env).await;
        if fails {
            assert!(matches!(
                result,
                Err(EngineError::DivisionByZero { span: None })
            ));
        } else {
            assert_eq!(
                result.expect("return signal"),
                ExprOutcome::Return(Val::Int(9))
            );
        }
        assert!(env.options.read().errexit);
    }
}

#[tokio::test]
async fn for_continue_skips_the_tail_and_loop_bindings_do_not_escape() {
    let env = environment().push_scope(Arc::new(RwLock::new(Default::default())));
    env.local_vars
        .as_ref()
        .expect("outer frame")
        .declare("item", Val::Int(99));
    let stmt = Stmt::For {
        var: "item".into(),
        iter: Expr::List(vec![Expr::Int(1), Expr::Int(2), Expr::Int(3)]),
        body: vec![
            Stmt::Expr(Expr::If {
                condition: Box::new(binary(
                    BinOp::Eq,
                    Expr::Variable("item".into()),
                    Expr::Int(2),
                )),
                then_body: vec![Stmt::Continue],
                else_body: None,
            }),
            assign(
                "visited",
                binary(
                    BinOp::Add,
                    binary(BinOp::Mul, Expr::Variable("visited".into()), Expr::Int(10)),
                    Expr::Variable("item".into()),
                ),
            ),
        ],
    };
    assert_eq!(
        eval_stmt(&stmt, &env, false).await.expect("loop"),
        Flow::Normal
    );
    assert_eq!(visited(&env), Val::Int(13));
    assert_eq!(
        env.local_vars.as_ref().expect("outer frame").get("item"),
        Some(Val::Int(99))
    );
    assert!(!env.vars.read().contains_key("item"));
}

#[tokio::test]
async fn for_break_stops_iteration_and_return_is_not_consumed_as_break() {
    for returns in [false, true] {
        let env = environment();
        let stmt = Stmt::For {
            var: "item".into(),
            iter: Expr::List(vec![Expr::Int(1), Expr::Int(2)]),
            body: vec![
                Stmt::Expr(effect(1, Expr::Int(0))),
                if returns {
                    Stmt::Return(Expr::Int(9))
                } else {
                    Stmt::Break
                },
                Stmt::Expr(effect(2, Expr::Int(0))),
            ],
        };
        assert_eq!(
            eval_stmt(&stmt, &env, false).await.expect("loop control"),
            if returns {
                Flow::Return(Val::Int(9))
            } else {
                Flow::Normal
            }
        );
        assert_eq!(visited(&env), Val::Int(1));
        assert!(!env.vars.read().contains_key("item"));
    }
}

#[tokio::test]
async fn catch_binding_is_lexical_and_preserves_structured_error_identity() {
    let env = environment();
    env.vars.write().insert("failure".into(), Val::Int(42));
    env.vars.write().insert("caught".into(), Val::Null);
    let stmt = Stmt::TryCatch {
        try_body: vec![
            Stmt::Expr(division_by_zero()),
            Stmt::Expr(effect(1, Expr::Int(0))),
        ],
        catch_var: "failure".into(),
        catch_body: vec![assign("caught", Expr::Variable("failure".into()))],
    };
    assert_eq!(
        eval_stmt(&stmt, &env, false)
            .await
            .expect("handled failure"),
        Flow::Normal
    );
    assert_eq!(visited(&env), Val::Int(0));
    assert_eq!(env.vars.read().get("failure"), Some(&Val::Int(42)));
    let caught = env
        .vars
        .read()
        .get("caught")
        .expect("caught diagnostic")
        .clone();
    let Val::Map(map) = caught else {
        unreachable!("expected diagnostic record")
    };
    assert_eq!(
        map.iter()
            .find(|(key, _)| key.as_str() == "code")
            .map(|(_, value)| value),
        Some(&Val::String("FSH-MATH-001".into()))
    );
    assert_eq!(
        env.get_last_error()
            .expect("recorded error")
            .code()
            .map(|code| code.to_string()),
        Some("FSH-MATH-001".into())
    );
    assert_eq!(env.exit_code(), 0);
}

#[tokio::test]
async fn try_catch_propagates_control_transfers_without_running_the_handler() {
    for signal in [
        Stmt::Break,
        Stmt::Continue,
        Stmt::Return(Expr::Int(9)),
        Stmt::Exit(Some(Expr::Int(23))),
    ] {
        let env = environment();
        let expected = match &signal {
            Stmt::Break => Flow::Break,
            Stmt::Continue => Flow::Continue,
            Stmt::Return(_) => Flow::Return(Val::Int(9)),
            Stmt::Exit(_) => Flow::Exit(23),
            _ => unreachable!(),
        };
        let stmt = Stmt::TryCatch {
            try_body: vec![signal, Stmt::Expr(effect(1, Expr::Int(0)))],
            catch_var: "failure".into(),
            catch_body: vec![Stmt::Expr(effect(2, Expr::Int(0)))],
        };
        assert_eq!(
            eval_stmt(&stmt, &env, false)
                .await
                .expect("uncaught control transfer"),
            expected
        );
        assert_eq!(visited(&env), Val::Int(0));
        assert!(env.get_last_error().is_none());
        assert!(!env.vars.read().contains_key("failure"));
    }
}
