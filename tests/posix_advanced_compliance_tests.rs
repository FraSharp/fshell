mod common;
use common::*;
use fshell_core::Val;
use fshell_engine::run_script;

// ---------------------------------------------------------------------------
// 1. POSIX Parameter Expansion Full Matrix
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_posix_param_expansion_prefix_and_suffix_removal() {
    let env = setup_test_env();
    let script = r#"
sh {
    FILE="src/core/parser/expr.rs"
    # Shortest prefix removal
    DIR_A="${FILE#*/}"
    # Longest prefix removal
    BASE="${FILE##*/}"
    # Shortest suffix removal
    NAME_ONLY="${BASE%.*}"
    # Longest suffix removal
    ARCHIVE="archive.tar.gz"
    TAR_NAME="${ARCHIVE%%.*}"
}
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(vars.get("DIR_A"), Some(&Val::String("core/parser/expr.rs".into())));
    assert_eq!(vars.get("BASE"), Some(&Val::String("expr.rs".into())));
    assert_eq!(vars.get("NAME_ONLY"), Some(&Val::String("expr".into())));
    assert_eq!(vars.get("TAR_NAME"), Some(&Val::String("archive".into())));
}

#[tokio::test]
async fn test_posix_param_expansion_defaults_and_alternates() {
    let env = setup_test_env();
    let script = r#"
sh {
    # Default when unset
    VAL1="${UNSET_KEY:-fallback_val}"
    # Assign default when unset
    VAL2="${NEW_KEY:=assigned_default}"
    # Alternate value when set
    SET_VAR="already_set"
    VAL3="${SET_VAR:+alternative_val}"
}
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(vars.get("VAL1"), Some(&Val::String("fallback_val".into())));
    assert_eq!(vars.get("VAL2"), Some(&Val::String("assigned_default".into())));
    assert_eq!(vars.get("NEW_KEY"), Some(&Val::String("assigned_default".into())));
    assert_eq!(vars.get("VAL3"), Some(&Val::String("alternative_val".into())));
}

#[tokio::test]
async fn test_posix_param_expansion_pattern_substitution() {
    let env = setup_test_env();
    let script = r#"
sh {
    TEXT="foo_bar_foo_baz"
    # Replace first occurrence
    FIRST="${TEXT/foo/qux}"
    # Replace all occurrences
    ALL="${TEXT//foo/qux}"
}
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(vars.get("FIRST"), Some(&Val::String("qux_bar_foo_baz".into())));
    assert_eq!(vars.get("ALL"), Some(&Val::String("qux_bar_qux_baz".into())));
}

// ---------------------------------------------------------------------------
// 2. POSIX Compound Arithmetic & Bitwise Operations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_posix_arithmetic_compound_and_bitwise() {
    let env = setup_test_env();
    let script = r#"
sh {
    RES_MATH=$(( (10 + 20) * 3 / 2 ))
    RES_BITWISE=$(( (0xFF & 0x0F) | (1 << 4) ))
    RES_TERNARY=$(( 100 > 50 ? 42 : 0 ))
}
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(vars.get("RES_MATH"), Some(&Val::String("45".into())));
    assert_eq!(vars.get("RES_BITWISE"), Some(&Val::String("31".into()))); // 15 | 16 = 31
    assert_eq!(vars.get("RES_TERNARY"), Some(&Val::String("42".into())));
}

// ---------------------------------------------------------------------------
// 3. POSIX Case Statements
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_posix_case_pattern_matching() {
    let env = setup_test_env();
    let script = r#"
sh {
    check_ext() {
        case "$1" in
            *.rs) echo "rust_file" ;;
            *.py) echo "python_file" ;;
            *) echo "unknown_file" ;;
        esac
    }
    OUT_RS=$(check_ext "main.rs")
    OUT_PY=$(check_ext "script.py")
    OUT_OTHER=$(check_ext "document.pdf")
}
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert_eq!(vars.get("OUT_RS"), Some(&Val::String("rust_file".into())));
    assert_eq!(vars.get("OUT_PY"), Some(&Val::String("python_file".into())));
    assert_eq!(vars.get("OUT_OTHER"), Some(&Val::String("unknown_file".into())));
}
