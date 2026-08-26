mod common;
use common::*;
use fshell_core::{Parser, Val};
use fshell_engine::run_script;

// ---------------------------------------------------------------------------
// 1. Deep AST Nesting Stress Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_deeply_nested_parentheses_evaluation() {
    let env = setup_test_env();

    // 100 layers of nested parentheses: (((... (42) ...)))
    let mut expr_str = "42".to_string();
    for _ in 0..100 {
        expr_str = format!("({})", expr_str);
    }
    let script = format!("let deep_val = {expr_str}");

    run_script(&script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(vars.get("deep_val"), Some(&Val::Int(42)));
}

#[tokio::test]
async fn test_deeply_nested_if_else_chain() {
    let env = setup_test_env();

    // Build 20 nested if-else blocks
    let mut script = "let depth_result = 0\n".to_string();
    let depth = 20;
    for i in 0..depth {
        script.push_str(&format!(
            "if $depth_result == {i} {{\n depth_result = ({i} + 1)\n"
        ));
    }
    for _ in 0..depth {
        script.push_str("}\n");
    }

    run_script(&script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(vars.get("depth_result"), Some(&Val::Int(20)));
}

// ---------------------------------------------------------------------------
// 2. Adversarial & Malformed Input Handling
// ---------------------------------------------------------------------------

#[test]
fn test_parser_unclosed_quotes_returns_syntax_error() {
    let inputs = [
        "let unclosed = \"hello world",
        "let unclosed_single = 'unterminated",
        "let unclosed_backtick = `unterminated cmd",
    ];

    for input in inputs {
        let mut parser = Parser::new(input);
        let res = parser.parse_statements();
        assert!(
            res.is_err(),
            "Expected parse error on unclosed quote: {input}"
        );
    }
}

#[test]
fn test_parser_unclosed_braces_and_brackets() {
    let inputs = [
        "let obj = { key: 42",
        "let arr = [1, 2, 3",
        "while true { let x = 1",
    ];

    for input in inputs {
        let mut parser = Parser::new(input);
        let res = parser.parse_statements();
        assert!(
            res.is_err(),
            "Expected parse error on unclosed delimiter: {input}"
        );
    }
}

#[tokio::test]
async fn test_massive_string_literals_and_buffers() {
    let env = setup_test_env();

    // 100KB string payload
    let massive_data = "A".repeat(100_000);
    let script =
        format!("let big_str = \"{massive_data}\"\nlet big_len = (string length $big_str)");

    run_script(&script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(
        vars.get("big_len"),
        Some(&Val::List(vec![Val::Int(100_000)]))
    );
}

// ---------------------------------------------------------------------------
// 3. Unicode Grapheme Clusters, Emojis & Slicing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_unicode_emoji_and_multibyte_string_operations() {
    let env = setup_test_env();

    let script = r#"
let emoji_str = "🦀🚀✨🎉"
let emoji_len = (string length $emoji_str)
let greeting_jp = "こんにちは世界"
let greeting_len = (string length $greeting_jp)
let combined = ($emoji_str + " " + $greeting_jp)
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(vars.get("emoji_len"), Some(&Val::List(vec![Val::Int(4)])));
    assert_eq!(
        vars.get("greeting_len"),
        Some(&Val::List(vec![Val::Int(7)]))
    );
    assert_eq!(
        vars.get("combined"),
        Some(&Val::String("🦀🚀✨🎉 こんにちは世界".to_string()))
    );
}

#[tokio::test]
async fn test_posix_parameter_expansion_unicode_slicing() {
    let env = setup_test_env();

    let script = r#"
sh {
    TEXT="🦀_FERRIS_🦀"
    SLICE="${TEXT:2:6}"
    export RESULT="$SLICE"
}
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(vars.get("RESULT"), Some(&Val::String("FERRIS".to_string())));
}
