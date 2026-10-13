//! Native parser/evaluator boundary: typed values, errors and control transfers.

use fshell_core::{Expr, Parser, Stmt, Val};
use fshell_engine::eval::{ExprOutcome, eval_expr_flow};
use fshell_engine::{EngineError, Env, Flow, eval_expr, eval_stmt};

fn expression(source: &str) -> Expr {
    let statements = Parser::new(source)
        .parse_statements()
        .expect("valid native expression");
    assert_eq!(
        statements.len(),
        1,
        "expression fixture must be a single statement"
    );
    let Stmt::Expr(expr) = statements[0].unpack() else {
        unreachable!("expression fixture parsed as a non-expression")
    };
    expr.clone()
}

#[tokio::test]
async fn arithmetic_preserves_integer_type_and_operator_precedence() {
    let env = Env::for_command();
    assert_eq!(
        eval_expr(&expression("7 + 3 * 4"), &env)
            .await
            .expect("arithmetic"),
        Val::Int(19)
    );
    assert_eq!(
        eval_expr(&expression("(7 + 3) * 4"), &env)
            .await
            .expect("grouped arithmetic"),
        Val::Int(40)
    );
}

#[tokio::test]
async fn signed_addition_matches_integer_arithmetic_for_every_selected_pair() {
    let env = Env::for_command();
    for a in [-997_i64, -1, 0, 1, 991] {
        for b in [-991_i64, -1, 0, 1, 997] {
            let source = format!("{a} + {b}");
            let actual = eval_expr(&expression(&source), &env)
                .await
                .expect("every generated expression must evaluate");
            assert_eq!(actual, Val::Int(a + b), "{source}");
        }
    }
}

#[tokio::test]
async fn zero_division_retains_error_identity_and_source_span() {
    let env = Env::for_command();
    let error = eval_expr(&expression("9 / 0"), &env)
        .await
        .expect_err("division by zero");
    assert!(
        matches!(error, EngineError::DivisionByZero { span: Some(_) }),
        "{error:?}"
    );
    let span = error.span().expect("native source span");
    assert!(span.offset() + span.len() <= "9 / 0".len());
}

#[tokio::test]
async fn missing_variable_is_not_an_untyped_generic_error() {
    let env = Env::for_command();
    let error = eval_expr(&expression("$undefined_binding"), &env)
        .await
        .expect_err("undefined binding");
    assert!(
        matches!(error, EngineError::VariableNotFound { ref name, span: Some(_) } if name == "undefined_binding"),
        "{error:?}"
    );
}

#[tokio::test]
async fn control_transfer_inside_a_value_if_stays_in_the_success_channel() {
    let env = Env::for_command();
    let expr = expression("if true { return 9 }");
    assert_eq!(
        eval_expr_flow(&expr, &env)
            .await
            .expect("return is flow, not error"),
        ExprOutcome::Return(Val::Int(9))
    );
    assert!(
        eval_expr(&expr, &env).await.is_err(),
        "value-only entrypoint must reject a destinationless return"
    );
}

#[tokio::test]
async fn declaration_and_assignment_update_the_same_runtime_binding() {
    let env = Env::for_command();
    let statements = Parser::new("let counter = 4; counter = $counter + 3")
        .parse_statements()
        .expect("binding script");
    assert_eq!(statements.len(), 2);
    for stmt in &statements {
        assert_eq!(
            eval_stmt(stmt, &env, false)
                .await
                .expect("binding statement"),
            Flow::Normal
        );
    }
    assert_eq!(env.vars.read().get("counter"), Some(&Val::Int(7)));
}
