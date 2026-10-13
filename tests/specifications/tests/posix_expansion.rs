//! POSIX single-word expansion contracts (IEEE 1003.1, sections 2.6.2–2.6.7).
//! Expectations are explicit fields/bytes/state, not another shell at test time.

use fshell_core::{RwLock, Val};
use fshell_engine::{EngineError, Env};
use fshell_posix::expand::{ExpansionConfig, expand_word, split_ifs};
use fshell_test_support::fixture::ShellFixture;
use std::{io, sync::Arc};

fn environment() -> Env {
    let env = Env::for_command();
    // Missing test variables must not fall back to the caller's environment.
    env.vars.write().remove("env");
    env
}

fn fields(word: &str, env: &Env) -> Vec<String> {
    expand_word(
        word,
        env,
        &ExpansionConfig {
            do_glob: false,
            ..Default::default()
        },
        &[],
    )
    .expect("word expansion")
}

#[test]
fn whitespace_ifs_collapses_only_the_selected_ascii_delimiters() {
    assert_eq!(split_ifs(" \talpha\n beta \t", " \t\n"), ["alpha", "beta"]);
    assert_eq!(split_ifs("\talpha\t beta\n", " "), ["\talpha\t", "beta\n"]);
    assert_eq!(
        split_ifs("alpha\u{a0}beta gamma", " \t\n"),
        ["alpha\u{a0}beta", "gamma"]
    );
    assert!(split_ifs(" \t\n", " \t\n").is_empty());
}

#[test]
fn nonwhitespace_ifs_keeps_leading_and_internal_null_fields_not_a_trailing_one() {
    for (input, expected) in [
        ("", vec![]),
        (":", vec![""]),
        ("::", vec!["", ""]),
        ("a:", vec!["a"]),
        (":a", vec!["", "a"]),
        ("a::b", vec!["a", "", "b"]),
        ("a::", vec!["a", ""]),
    ] {
        assert_eq!(split_ifs(input, ":"), expected, "input {input:?}");
    }
}

#[test]
fn mixed_ifs_attaches_whitespace_to_adjacent_nonwhitespace_delimiters() {
    assert_eq!(
        split_ifs("\t: alpha :: beta: \t", ": \t"),
        ["", "alpha", "", "beta"]
    );
}

#[test]
fn empty_ifs_disables_splitting_without_changing_nonempty_text() {
    assert_eq!(split_ifs(" a\tb\n ", ""), [" a\tb\n "]);
    let env = environment();
    env.vars
        .write()
        .insert("IFS".into(), Val::String(String::new()));
    env.vars
        .write()
        .insert("value".into(), Val::String(" a\tb\n ".into()));
    assert_eq!(fields("$value", &env), [" a\tb\n "]);
}

#[test]
fn single_quotes_are_literal_and_double_quotes_suppress_field_splitting() {
    let env = environment();
    env.vars
        .write()
        .insert("value".into(), Val::String(" alpha\t beta ".into()));
    assert_eq!(fields("'$value'", &env), ["$value"]);
    assert_eq!(fields("\"$value\"", &env), [" alpha\t beta "]);
    assert_eq!(fields("$value", &env), ["alpha", "beta"]);
}

#[test]
fn quoted_null_arguments_survive_but_unquoted_empty_expansions_disappear() {
    for ifs in [" \t\n", ""] {
        let env = environment();
        env.vars
            .write()
            .insert("IFS".into(), Val::String(ifs.into()));
        env.vars
            .write()
            .insert("value".into(), Val::String(String::new()));
        for word in ["''", "\"\"", "\"$value\"", "\"$missing\""] {
            assert_eq!(fields(word, &env), [""], "{word}");
        }
        for word in ["$value", "$missing"] {
            assert!(fields(word, &env).is_empty(), "{word}, IFS={ifs:?}");
        }
    }
}

#[test]
fn quoted_at_preserves_argument_boundaries_spaces_and_empty_arguments() {
    let env = environment();
    let args = vec!["a b".into(), String::new(), " c ".into()];
    assert_eq!(
        expand_word("\"$@\"", &env, &ExpansionConfig::default(), &args).expect("quoted positional"),
        args
    );
    assert!(
        expand_word("\"$@\"", &env, &ExpansionConfig::default(), &[])
            .expect("no positional arguments")
            .is_empty()
    );
}

#[test]
fn unquoted_at_splits_each_argument_and_removes_implicit_null_arguments() {
    let env = environment();
    let args = vec!["a b".into(), String::new(), " c ".into()];
    let mut cfg = ExpansionConfig {
        do_glob: false,
        ..Default::default()
    };
    assert_eq!(
        expand_word("$@", &env, &cfg, &args).expect("unquoted positional"),
        ["a", "b", "c"]
    );
    cfg.ifs = Some(String::new());
    assert_eq!(
        expand_word("$@", &env, &cfg, &args).expect("unsplit positional"),
        ["a b", " c "]
    );
}

#[test]
fn default_and_alternative_operators_distinguish_unset_empty_and_present_values() {
    for (value, expected) in [
        (None, ["fallback", "fallback", "", ""]),
        (Some(""), ["", "fallback", "alternative", ""]),
        (
            Some("present"),
            ["present", "present", "alternative", "alternative"],
        ),
    ] {
        let env = environment();
        if let Some(value) = value {
            env.vars
                .write()
                .insert("value".into(), Val::String(value.into()));
        }
        for (word, expected) in [
            "\"${value-fallback}\"",
            "\"${value:-fallback}\"",
            "\"${value+alternative}\"",
            "\"${value:+alternative}\"",
        ]
        .into_iter()
        .zip(expected)
        {
            assert_eq!(
                fields(word, &env),
                [expected],
                "word {word}, value {value:?}"
            );
        }
        assert_eq!(
            env.vars.read().get("value").cloned(),
            value.map(|value| Val::String(value.into())),
            "nonassignment expansions must not mutate"
        );
    }
}

#[test]
fn default_assignment_writes_only_when_its_presence_condition_matches() {
    for (value, colon, expected) in [
        (None, false, "fallback"),
        (None, true, "fallback"),
        (Some(""), false, ""),
        (Some(""), true, "fallback"),
        (Some("present"), false, "present"),
        (Some("present"), true, "present"),
    ] {
        let env = environment();
        if let Some(value) = value {
            env.vars
                .write()
                .insert("value".into(), Val::String(value.into()));
        }
        let word = if colon {
            "\"${value:=fallback}\""
        } else {
            "\"${value=fallback}\""
        };
        assert_eq!(fields(word, &env), [expected]);
        assert_eq!(
            env.vars.read().get("value"),
            Some(&Val::String(expected.into()))
        );
    }
}

#[test]
fn required_parameters_report_typed_errors_without_assigning_a_value() {
    let env = environment();
    let cfg = ExpansionConfig {
        do_glob: false,
        ..Default::default()
    };
    for word in ["${value?required}", "${value:?required}"] {
        let error = expand_word(word, &env, &cfg, &[]).expect_err("missing parameter");
        assert!(
            matches!(error, EngineError::ParameterExpansion { ref parameter, ref message, span: None } if parameter == "${value}" && message == "required"),
            "{error:?}"
        );
        assert!(!env.vars.read().contains_key("value"));
    }
    env.vars
        .write()
        .insert("value".into(), Val::String(String::new()));
    assert_eq!(fields("\"${value?required}\"", &env), [""]);
    assert!(matches!(
        expand_word("${value:?required}", &env, &cfg, &[]),
        Err(EngineError::ParameterExpansion { .. })
    ));
}

#[test]
fn prefix_and_suffix_patterns_select_shortest_or_longest_matches() {
    let env = environment();
    env.vars
        .write()
        .insert("value".into(), Val::String("α/β/終".into()));
    for (word, expected) in [
        ("\"${value#*/}\"", "β/終"),
        ("\"${value##*/}\"", "終"),
        ("\"${value%/*}\"", "α/β"),
        ("\"${value%%/*}\"", "α"),
    ] {
        assert_eq!(fields(word, &env), [expected], "{word}");
    }
}

#[test]
fn globs_use_the_logical_cwd_sort_results_and_respect_quote_provenance() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    for name in ["b.txt", "a.txt", ".hidden.txt", "nested/c.txt"] {
        fixture.create_file(name, b"fixture")?;
    }
    let before = std::env::current_dir()?;
    let env = environment();
    env.set_cwd(fixture.workspace());
    for (word, expected) in [
        ("*.txt", vec!["a.txt", "b.txt"]),
        ("'*.txt'", vec!["*.txt"]),
        ("\"*.txt\"", vec!["*.txt"]),
        ("\\*.txt", vec!["*.txt"]),
        ("nested/*.txt", vec!["nested/c.txt"]),
    ] {
        assert_eq!(
            expand_word(word, &env, &ExpansionConfig::default(), &[]).expect("pathname expansion"),
            expected,
            "{word}"
        );
    }
    assert_eq!(std::env::current_dir()?, before);
    fixture.close()
}

#[test]
fn unmatched_globs_stay_literal_and_hidden_files_require_an_explicit_dot() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    fixture.create_file(".hidden.txt", b"fixture")?;
    let env = environment();
    env.set_cwd(fixture.workspace());
    assert_eq!(
        expand_word("*.txt", &env, &ExpansionConfig::default(), &[]).expect("unmatched"),
        ["*.txt"]
    );
    assert_eq!(
        expand_word(".*.txt", &env, &ExpansionConfig::default(), &[]).expect("explicit dot"),
        [".hidden.txt"]
    );
    assert_eq!(
        expand_word("absent-*.txt", &env, &ExpansionConfig::default(), &[]).expect("unmatched"),
        ["absent-*.txt"]
    );
    fixture.close()
}

#[test]
fn command_substitution_removes_only_trailing_newlines_not_carriage_returns() {
    let env = environment();
    assert_eq!(fields("\"$(printf 'a\\r\\n\\n')\"", &env), ["a\r"]);
    assert_eq!(fields("\"$(printf 'a\\nb\\n')\"", &env), ["a\nb"]);
}

#[test]
fn command_substitution_publishes_status_without_leaking_variable_or_option_changes() {
    let env = environment();
    env.vars
        .write()
        .insert("value".into(), Val::String("parent".into()));
    env.options.write().nounset = false;
    assert_eq!(
        fields(
            "\"$(value=child; set -u; printf '%s' \"$value\"; false)\"",
            &env
        ),
        ["child"]
    );
    assert_eq!(env.exit_code(), 1);
    assert_eq!(
        env.vars.read().get("value"),
        Some(&Val::String("parent".into()))
    );
    assert!(!env.options.read().nounset);
}

#[test]
fn command_substitution_snapshots_enclosing_lexical_bindings_without_mutating_them() {
    let env = environment().push_scope(Arc::new(RwLock::new(Default::default())));
    env.local_vars
        .as_ref()
        .expect("lexical frame")
        .declare("value", Val::String("parent".into()));
    assert_eq!(
        fields("\"$(value=child; printf '%s' \"$value\")\"", &env),
        ["child"]
    );
    assert_eq!(
        env.local_vars.as_ref().expect("lexical frame").get("value"),
        Some(Val::String("parent".into()))
    );
}

#[test]
fn arithmetic_expansion_updates_shell_bindings_without_textual_coercion() {
    let env = environment();
    env.vars.write().insert("counter".into(), Val::Int(4));
    assert_eq!(fields("$((counter += 3))", &env), ["7"]);
    assert_eq!(env.vars.read().get("counter"), Some(&Val::Int(7)));
}
