mod common;
use common::*;
use fshell_core::{Parser, Val};
use fshell_engine::{eval_expr, run_script};

// ---------------------------------------------------------------------------
// 1. Math Evaluation & Edge Cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_math_integer_arithmetic_precedence() {
    let env = setup_test_env();

    // Standard precedence: multiplication before addition
    let mut p = Parser::new("2 + 3 * 4");
    let expr = p.parse_expr().unwrap();
    let res = eval_expr(&expr, &env).await.unwrap();
    assert_eq!(res, Val::Int(14));

    // Parenthesized precedence
    let mut p = Parser::new("(2 + 3) * 4");
    let expr = p.parse_expr().unwrap();
    let res = eval_expr(&expr, &env).await.unwrap();
    assert_eq!(res, Val::Int(20));

    // Negative numbers
    let mut p = Parser::new("-5 + 12");
    let expr = p.parse_expr().unwrap();
    let res = eval_expr(&expr, &env).await.unwrap();
    assert_eq!(res, Val::Int(7));
}

#[tokio::test]
async fn test_math_division_by_zero_fails_gracefully() {
    let env = setup_test_env();

    let mut p = Parser::new("100 / 0");
    let expr = p.parse_expr().unwrap();
    let res = eval_expr(&expr, &env).await;
    assert!(res.is_err(), "Division by zero should return an error");
}

#[tokio::test]
async fn test_boolean_conditional_chain_evaluation() {
    let env = setup_test_env();

    // Logical OR short-circuits on true (exit 0)
    let res = run_script("true || false", &env).await;
    assert!(res.is_ok());
    assert_eq!(env.vars.read().get("?"), Some(&Val::Int(0)));

    // Logical AND short-circuits on false (exit 1)
    let res = run_script("false && true", &env).await;
    assert!(res.is_ok());
    assert_eq!(env.vars.read().get("?"), Some(&Val::Int(1)));
}

// ---------------------------------------------------------------------------
// 2. Type System & Evaluation Errors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_type_error_on_incompatible_operations() {
    let env = setup_test_env();

    // Trying to do math with a string and a map
    let script = r#"
let bad_math = ("hello" * 5)
"#;
    let res = run_script(script, &env).await;
    assert!(res.is_err(), "String multiplication should fail");
}

#[tokio::test]
async fn test_map_and_list_access_error_handling() {
    let env = setup_test_env();

    let script = r#"
let user = { name: "Alice" }
let missing = $user.non_existent_key
"#;
    let res = run_script(script, &env).await;
    assert!(
        res.is_err(),
        "Accessing non-existent map field should return error"
    );
    let err = res.unwrap_err();
    assert!(err.contains("Map has no field 'non_existent_key'"));
}

#[tokio::test]
async fn test_function_arity_and_call_validation() {
    let env = setup_test_env();

    let script = r#"
fn add(a, b) {
    $a + $b
}
let sum = (add 10 20)
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(vars.get("sum"), Some(&Val::List(vec![Val::Int(30)])));
}
