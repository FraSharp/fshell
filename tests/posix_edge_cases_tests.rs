//! Comprehensive POSIX compliance, edge cases, expansions, and robustness tests.

mod common;
use common::*;
use fshell_posix::eval::{EvalConfig, eval_source, eval_source_stream};
use fshell_posix::expand::split_ifs;
use fshell_posix::parser::parse_posix_script;

fn setup_posix_env() -> Env {
    let env = setup_test_env();
    fshell_engine::populate_env_from_host(&env);
    env
}

async fn run_posix(script: &str, env: &Env) -> i32 {
    let parsed = parse_posix_script(script).expect("failed to parse posix script");
    eval_source(&parsed, env, &EvalConfig::default())
        .await
        .expect("posix evaluation failed")
}

async fn run_posix_capture(script: &str, env: &Env) -> (i32, String) {
    let parsed = parse_posix_script(script).expect("failed to parse posix script");
    let (code, bytes) = eval_source_stream(&parsed, env, &EvalConfig::default(), true)
        .await
        .expect("posix evaluation failed");
    let out = bytes
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    (code, out)
}

// ---------------------------------------------------------------------------
// 1. POSIX Parameter Expansion Edge Cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_posix_param_expansion_defaults() {
    let env = setup_posix_env();

    // ${var:-word} Use Default Values
    let (_, out) = run_posix_capture("echo ${UNSET_VAR:-default_val}", &env).await;
    assert_eq!(out.trim(), "default_val");

    // ${var:=word} Assign Default Values
    let (_, out) =
        run_posix_capture("echo ${ASSIGN_VAR:=assigned_val}; echo $ASSIGN_VAR", &env).await;
    assert_eq!(out.trim(), "assigned_val\nassigned_val");

    // ${var:+word} Use Alternative Value
    let (_, out) = run_posix_capture(
        "SET_VAR=hello; echo ${SET_VAR:+alternative}; echo ${UNSET_VAR2:+nope}",
        &env,
    )
    .await;
    assert_eq!(out.trim(), "alternative");
}

#[tokio::test]
async fn test_posix_param_expansion_substring_and_slicing() {
    let env = setup_posix_env();

    // Slicing ${var:offset:length}
    let (_, out) = run_posix_capture(r#"STR="abcdefghij"; echo ${STR:2:4}"#, &env).await;
    assert_eq!(out.trim(), "cdef");

    // Slicing from offset to end ${var:offset}
    let (_, out) = run_posix_capture(r#"STR="abcdefghij"; echo ${STR:5}"#, &env).await;
    assert_eq!(out.trim(), "fghij");

    // Slicing with UTF-8 multi-byte characters
    let (_, out) = run_posix_capture(r#"UTF="🦀🚀🌟🎉"; echo ${UTF:1:2}"#, &env).await;
    assert_eq!(out.trim(), "🚀🌟");
}

#[tokio::test]
async fn test_posix_param_expansion_pattern_removal() {
    let env = setup_posix_env();

    // Shortest prefix removal ${var#pattern}
    let (_, out) = run_posix_capture(r#"FILE="foo.bar.baz"; echo ${FILE#*.}"#, &env).await;
    assert_eq!(out.trim(), "bar.baz");

    // Longest prefix removal ${var##pattern}
    let (_, out) = run_posix_capture(r#"FILE="foo.bar.baz"; echo ${FILE##*.}"#, &env).await;
    assert_eq!(out.trim(), "baz");

    // Shortest suffix removal ${var%pattern}
    let (_, out) = run_posix_capture(r#"PATH_VAR="/a/b/c.txt"; echo ${PATH_VAR%.*}"#, &env).await;
    assert_eq!(out.trim(), "/a/b/c");

    // Longest suffix removal ${var%%pattern}
    let (_, out) = run_posix_capture(r#"FILE="foo.bar.baz"; echo ${FILE%%.*}"#, &env).await;
    assert_eq!(out.trim(), "foo");
}

#[tokio::test]
async fn test_posix_param_expansion_pattern_replacement() {
    let env = setup_posix_env();

    // First match replacement ${var/pattern/replacement}
    let (_, out) = run_posix_capture(
        r#"STR="apple orange apple"; echo ${STR/apple/banana}"#,
        &env,
    )
    .await;
    assert_eq!(out.trim(), "banana orange apple");

    // Global replacement ${var//pattern/replacement}
    let (_, out) = run_posix_capture(
        r#"STR="apple orange apple"; echo ${STR//apple/banana}"#,
        &env,
    )
    .await;
    assert_eq!(out.trim(), "banana orange banana");
}

#[tokio::test]
async fn test_posix_param_length() {
    let env = setup_posix_env();
    let (_, out) = run_posix_capture(r#"TEXT="hello world"; echo ${#TEXT}"#, &env).await;
    assert_eq!(out.trim(), "11");

    let (_, out) = run_posix_capture(r#"UNICODE="🦀🦀"; echo ${#UNICODE}"#, &env).await;
    assert_eq!(out.trim(), "2");
}

// ---------------------------------------------------------------------------
// 2. POSIX Arithmetic $((...)) Edge Cases & Extremes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_posix_arithmetic_precedence_and_operators() {
    let env = setup_posix_env();

    let (_, out) = run_posix_capture("echo $(( 2 + 3 * 4 ))", &env).await;
    assert_eq!(out.trim(), "14");

    let (_, out) = run_posix_capture("echo $(( (2 + 3) * 4 ))", &env).await;
    assert_eq!(out.trim(), "20");

    let (_, out) = run_posix_capture("echo $(( 2 ** 10 ))", &env).await;
    assert_eq!(out.trim(), "1024");

    let (_, out) = run_posix_capture("echo $(( 100 / 3 ))", &env).await;
    assert_eq!(out.trim(), "33");

    let (_, out) = run_posix_capture("echo $(( 100 % 3 ))", &env).await;
    assert_eq!(out.trim(), "1");
}

#[tokio::test]
async fn test_posix_arithmetic_bitwise_and_shifts() {
    let env = setup_posix_env();

    let (_, out) = run_posix_capture("echo $(( 1 << 8 ))", &env).await;
    assert_eq!(out.trim(), "256");

    let (_, out) = run_posix_capture("echo $(( 256 >> 4 ))", &env).await;
    assert_eq!(out.trim(), "16");

    let (_, out) = run_posix_capture("echo $(( 0xFF & 0x0F ))", &env).await;
    assert_eq!(out.trim(), "15");

    let (_, out) = run_posix_capture("echo $(( 0xF0 | 0x0F ))", &env).await;
    assert_eq!(out.trim(), "255");

    let (_, out) = run_posix_capture("echo $(( 0xFF ^ 0x0F ))", &env).await;
    assert_eq!(out.trim(), "240");
}

#[tokio::test]
async fn test_posix_arithmetic_ternary_and_assignment() {
    let env = setup_posix_env();

    let (_, out) = run_posix_capture("echo $(( 1 ? 42 : 99 ))", &env).await;
    assert_eq!(out.trim(), "42");

    let (_, out) = run_posix_capture("echo $(( 0 ? 42 : 99 ))", &env).await;
    assert_eq!(out.trim(), "99");

    // Variable assignment inside arithmetic
    let (_, out) = run_posix_capture("x=10; echo $(( x += 5 )); echo $x", &env).await;
    assert_eq!(out.trim(), "15\n15");
}

#[tokio::test]
async fn test_posix_arithmetic_bases() {
    let env = setup_posix_env();

    let (_, out) = run_posix_capture("echo $(( 0x2A ))", &env).await; // hex 42
    assert_eq!(out.trim(), "42");

    let (_, out) = run_posix_capture("echo $(( 052 ))", &env).await; // octal 42
    assert_eq!(out.trim(), "42");

    let (_, out) = run_posix_capture("echo $(( 2#101010 ))", &env).await; // binary 42
    assert_eq!(out.trim(), "42");
}

#[tokio::test]
async fn test_posix_arithmetic_division_by_zero_does_not_panic() {
    let env = setup_posix_env();
    let res = fshell_posix::arithmetic::eval_arithmetic_expr("10 / 0", &env);
    assert!(res.is_err(), "Division by zero should return error Result");

    let res = fshell_posix::arithmetic::eval_arithmetic_expr("10 % 0", &env);
    assert!(res.is_err(), "Modulo by zero should return error Result");
}

// ---------------------------------------------------------------------------
// 3. POSIX IFS Field Splitting
// ---------------------------------------------------------------------------

#[test]
fn test_posix_ifs_whitespace_collapsing() {
    let res = split_ifs("   apple   banana   cherry   ", " \t\n");
    assert_eq!(res, vec!["apple", "banana", "cherry"]);
}

#[test]
fn test_posix_ifs_custom_delimiter() {
    let res = split_ifs("a:b::c", ":");
    assert_eq!(res, vec!["a", "b", "", "c"]);
}

#[test]
fn test_posix_ifs_empty_delimiter_no_splitting() {
    let res = split_ifs("a b c", "");
    assert_eq!(res, vec!["a b c"]);
}

#[test]
fn test_posix_ifs_mixed_delimiters() {
    let res = split_ifs("a, b , c", ", ");
    assert_eq!(res, vec!["a", "b", "c"]);
}

// ---------------------------------------------------------------------------
// 4. POSIX Subshells & Variable Scoping
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_posix_subshell_variable_isolation() {
    let env = setup_posix_env();
    let script = "X=original; (X=modified; echo sub: $X); echo parent: $X";
    let (_, out) = run_posix_capture(script, &env).await;
    assert_eq!(out.trim(), "sub: modified\nparent: original");
}

#[tokio::test]
async fn test_posix_brace_group_variable_persistence() {
    let env = setup_posix_env();
    let script = "X=original; { X=modified; echo brace: $X; }; echo parent: $X";
    let (_, out) = run_posix_capture(script, &env).await;
    assert_eq!(out.trim(), "brace: modified\nparent: modified");
}

// ---------------------------------------------------------------------------
// 5. POSIX Command Substitution & Nesting
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_posix_nested_command_substitution() {
    let env = setup_posix_env();
    let script = "echo $(echo $(echo deeply $(echo nested)))";
    let (_, out) = run_posix_capture(script, &env).await;
    assert_eq!(out.trim(), "deeply nested");
}

#[tokio::test]
async fn test_posix_command_substitution_backticks() {
    let env = setup_posix_env();
    let script = "echo `echo hello `";
    let (_, out) = run_posix_capture(script, &env).await;
    assert_eq!(out.trim(), "hello");
}

// ---------------------------------------------------------------------------
// 6. POSIX Heredoc & Herestring
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_posix_heredoc_with_variable_expansion() {
    let env = setup_posix_env();
    let script = "NAME=fshell; cat <<EOF\nHello $NAME\nEOF";
    let (_, out) = run_posix_capture(script, &env).await;
    assert_eq!(out.trim(), "Hello fshell");
}

#[tokio::test]
async fn test_posix_heredoc_quoted_delimiter_literal() {
    let env = setup_posix_env();
    let script = "NAME=fshell; cat <<'EOF'\nHello $NAME\nEOF";
    let (_, out) = run_posix_capture(script, &env).await;
    assert_eq!(out.trim(), "Hello $NAME");
}

#[tokio::test]
async fn test_posix_herestring() {
    let env = setup_posix_env();
    let script = "cat <<< 'hello from herestring'";
    let (_, out) = run_posix_capture(script, &env).await;
    assert_eq!(out.trim(), "hello from herestring");
}

// ---------------------------------------------------------------------------
// 7. POSIX Builtins: test / [, printf, getopts, shift, type
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_posix_test_brackets() {
    let env = setup_posix_env();
    assert_eq!(run_posix("[ -n 'nonempty' ]", &env).await, 0);
    assert_ne!(run_posix("[ -z 'nonempty' ]", &env).await, 0);
    assert_eq!(run_posix("[ -z '' ]", &env).await, 0);
    assert_eq!(run_posix("[ 10 -gt 5 ]", &env).await, 0);
    assert_eq!(run_posix("[ 5 -lt 10 ]", &env).await, 0);
    assert_eq!(run_posix("[ 'abc' = 'abc' ]", &env).await, 0);
    assert_ne!(run_posix("[ 'abc' != 'abc' ]", &env).await, 0);
}

#[tokio::test]
async fn test_posix_printf_formatting() {
    let env = setup_posix_env();
    let (_, out) = run_posix_capture(r#"printf "Name: %s, Age: %d\n" Alice 30"#, &env).await;
    assert_eq!(out.trim(), "Name: Alice, Age: 30");

    let (_, out) = run_posix_capture(r#"printf "%05d\n" 42"#, &env).await;
    assert_eq!(out.trim(), "00042");

    let (_, out) = run_posix_capture(r#"printf "hex: %x\n" 255"#, &env).await;
    assert_eq!(out.trim(), "hex: ff");
}

#[tokio::test]
async fn test_posix_shift_positional() {
    let env = setup_posix_env();
    let script = "set -- a b c d; shift 2; echo $1 $2";
    let (_, out) = run_posix_capture(script, &env).await;
    assert_eq!(out.trim(), "c d");
}

#[tokio::test]
async fn test_posix_eval_builtin() {
    let env = setup_posix_env();
    let script = "CMD='EVALUATED_VAR=yes'; eval $CMD";
    let code = run_posix(script, &env).await;
    assert_eq!(code, 0);
    assert_eq!(
        env.vars.read().get("EVALUATED_VAR"),
        Some(&Val::String("yes".to_string()))
    );
}

// ---------------------------------------------------------------------------
// 8. POSIX Control Flow & Exit Status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_posix_conditional_logic_exit_codes() {
    let env = setup_posix_env();

    // true && echo ok -> 0
    let (code, out) = run_posix_capture("true && echo ok", &env).await;
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "ok");

    // false || echo fallback -> 0
    let (code, out) = run_posix_capture("false || echo fallback", &env).await;
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "fallback");

    // false && echo no -> 1 (exit code of false)
    let (code, out) = run_posix_capture("false && echo no", &env).await;
    assert_ne!(code, 0);
    assert_eq!(out.trim(), "");
}

#[tokio::test]
async fn test_posix_function_definition_and_recursion() {
    let env = setup_posix_env();
    let script = r#"
countdown() {
    if [ $1 -gt 0 ]; then
        echo $1
        countdown $(( $1 - 1 ))
    fi
}
countdown 3
"#;
    let (_, out) = run_posix_capture(script, &env).await;
    assert_eq!(out.trim(), "3\n2\n1");
}
