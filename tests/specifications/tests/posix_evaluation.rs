//! POSIX builtin-only execution and scoped argument state. No external jobs,
//! background processes or real subshell processes are launched by these cases.

use fshell_core::Val;
use fshell_engine::{EngineError, Env};
use fshell_posix::{EvalConfig, eval_source, eval_source_stream, parse_posix_script};
use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;

fn environment() -> Env {
    let env = Env::for_command();
    env.options.write().errexit = false;
    env.options.write().nounset = false;
    env.vars.write().remove("env");
    env
}

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("POSIX evaluation deadline")
}

async fn capture(
    source: &str,
    env: &Env,
    cfg: &EvalConfig,
) -> Result<(i32, Option<Vec<u8>>), EngineError> {
    let parsed = parse_posix_script(source).expect("valid POSIX fixture");
    bounded(eval_source_stream(&parsed, env, cfg, true)).await
}

fn assert_capture(actual: (i32, Option<Vec<u8>>), code: i32, bytes: &[u8]) {
    assert_eq!(actual, (code, Some(bytes.to_vec())));
}

fn seed_arguments(env: &Env) {
    let mut vars = env.vars.write();
    vars.insert(
        "@".into(),
        Val::List(vec![Val::Int(7), Val::String("parent argument".into())]),
    );
    vars.insert("#".into(), Val::Int(2));
    vars.insert("1".into(), Val::Int(7));
    vars.insert("2".into(), Val::String("parent argument".into()));
    vars.insert("0".into(), Val::String("caller script".into()));
}

fn argument_state(env: &Env) -> BTreeMap<String, Val> {
    env.vars
        .read()
        .iter()
        .filter(|(name, _)| {
            name.as_str() == "@"
                || name.as_str() == "#"
                || name.bytes().all(|byte| byte.is_ascii_digit())
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn arguments(count: usize) -> EvalConfig {
    EvalConfig {
        positional: (1..=count).map(|index| format!("inner-{index}")).collect(),
        ..Default::default()
    }
}

#[tokio::test]
async fn captured_printf_preserves_nuls_newlines_carriage_returns_and_utf8() {
    let env = environment();
    assert_capture(
        capture("printf 'a\\000b\\n\\ré'", &env, &EvalConfig::default())
            .await
            .expect("printf"),
        0,
        "a\0b\n\ré".as_bytes(),
    );
    assert_eq!(env.exit_code(), 0);
}

#[tokio::test]
async fn and_or_lists_skip_unselected_commands_and_preserve_output_order() {
    let env = environment();
    let script =
        "false && skipped=yes; true || skipped=yes; false || printf first; true && printf second";
    assert_capture(
        capture(script, &env, &EvalConfig::default())
            .await
            .expect("status chains"),
        0,
        b"firstsecond",
    );
    assert!(!env.vars.read().contains_key("skipped"));
}

#[tokio::test]
async fn condition_failures_do_not_trigger_errexit_or_execute_unselected_branches() {
    let env = environment();
    let script = "set -e; if false; then wrong=yes; elif true; then printf selected; else wrong=yes; fi; printf after";
    assert_capture(
        capture(script, &env, &EvalConfig::default())
            .await
            .expect("conditional"),
        0,
        b"selectedafter",
    );
    assert!(!env.vars.read().contains_key("wrong"));
    assert!(env.options.read().errexit);
}

#[tokio::test]
async fn for_continue_skips_the_tail_and_break_stops_remaining_iterations() {
    let env = environment();
    let script = "for item in 1 2 3 4; do if [ \"$item\" = 2 ]; then continue; fi; if [ \"$item\" = 3 ]; then break; fi; printf '%s' \"$item\"; done; printf after";
    assert_capture(
        capture(script, &env, &EvalConfig::default())
            .await
            .expect("loop control"),
        0,
        b"1after",
    );
    assert_eq!(env.vars.read().get("item"), Some(&Val::String("3".into())));
}

#[tokio::test]
async fn implicit_for_uses_live_arguments_set_by_the_script() {
    let env = environment();
    assert_capture(
        capture(
            "set -- 'a b' '' c; for item; do printf '<%s>' \"$item\"; done",
            &env,
            &EvalConfig::default(),
        )
        .await
        .expect("implicit positional loop"),
        0,
        b"<a b><><c>",
    );
}

#[tokio::test]
async fn implicit_for_snapshots_its_iteration_values_before_the_body_changes_arguments() {
    let env = environment();
    assert_capture(capture("set -- a b; for item; do set -- changed; printf '%s' \"$item\"; done; printf '<%s>' \"$1\"", &env, &EvalConfig::default()).await.expect("loop snapshot"), 0, b"ab<changed>");
}

#[tokio::test]
async fn function_called_without_arguments_does_not_inherit_caller_arguments() {
    let env = environment();
    seed_arguments(&env);
    let before = argument_state(&env);
    assert_capture(
        capture(
            "f() { printf '%s:<%s>' \"$#\" \"$1\"; }; f",
            &env,
            &EvalConfig::default(),
        )
        .await
        .expect("zero-argument function"),
        0,
        b"0:<>",
    );
    assert_eq!(argument_state(&env), before);
}

#[tokio::test]
async fn nested_functions_restore_each_callers_arguments_after_return() {
    let env = environment();
    seed_arguments(&env);
    let before = argument_state(&env);
    let script = "inner() { inner_seen=$1; return 17; wrong=yes; }; outer() { inner nested; result=$?; outer_seen=$1; }; outer outer-argument";
    assert_capture(
        capture(script, &env, &EvalConfig::default())
            .await
            .expect("nested functions"),
        0,
        b"",
    );
    assert_eq!(
        env.vars.read().get("inner_seen"),
        Some(&Val::String("nested".into()))
    );
    assert_eq!(
        env.vars.read().get("outer_seen"),
        Some(&Val::String("outer-argument".into()))
    );
    assert_eq!(
        env.vars.read().get("result"),
        Some(&Val::String("17".into()))
    );
    assert!(!env.vars.read().contains_key("wrong"));
    assert_eq!(argument_state(&env), before);
}

#[tokio::test]
async fn explicit_stream_arguments_restore_typed_bindings_and_remove_all_extra_indices() {
    let env = environment();
    seed_arguments(&env);
    let before = argument_state(&env);
    assert_capture(
        capture("printf '%s:%s' \"$#\" \"${80}\"", &env, &arguments(80))
            .await
            .expect("scoped args"),
        0,
        b"80:inner-80",
    );
    assert_eq!(argument_state(&env), before);
}

#[tokio::test]
async fn explicit_stream_arguments_restore_absence_when_the_caller_had_no_parameters() {
    let env = environment();
    let before = argument_state(&env);
    assert_capture(
        capture(":", &env, &arguments(80))
            .await
            .expect("scope completion"),
        0,
        b"",
    );
    assert_eq!(argument_state(&env), before);
}

#[tokio::test]
async fn hard_expansion_error_restores_arguments_and_stops_later_effects() {
    let env = environment();
    seed_arguments(&env);
    let before = argument_state(&env);
    let error = capture(
        "before=yes; : \"${required:?must be supplied}\"; after=yes",
        &env,
        &arguments(80),
    )
    .await
    .expect_err("required parameter");
    assert!(
        matches!(error, EngineError::ParameterExpansion { ref parameter, ref message, span: None } if parameter == "${required}" && message == "must be supplied"),
        "{error:?}"
    );
    assert_eq!(argument_state(&env), before);
    assert_eq!(
        env.vars.read().get("before"),
        Some(&Val::String("yes".into()))
    );
    assert!(!env.vars.read().contains_key("after"));
}

#[tokio::test]
async fn cancellation_before_execution_restores_arguments_without_running_commands() {
    let env = environment();
    seed_arguments(&env);
    let before = argument_state(&env);
    env.request_shell_cancellation();
    assert!(matches!(
        capture("ran=yes", &env, &arguments(80)).await,
        Err(EngineError::Interrupted { span: None })
    ));
    assert_eq!(env.exit_code(), 130);
    assert_eq!(argument_state(&env), before);
    assert!(!env.vars.read().contains_key("ran"));
}

#[tokio::test]
async fn unscoped_execution_keeps_argument_changes_made_by_the_script() {
    let env = environment();
    seed_arguments(&env);
    assert_capture(
        capture(
            "set -- changed; printf '%s:%s' \"$#\" \"$1\"",
            &env,
            &EvalConfig::default(),
        )
        .await
        .expect("persistent args"),
        0,
        b"1:changed",
    );
    assert_eq!(
        env.vars.read().get("@"),
        Some(&Val::List(vec![Val::String("changed".into())]))
    );
    assert_eq!(env.vars.read().get("#"), Some(&Val::Int(1)));
    assert!(!env.vars.read().contains_key("2"));
}

#[tokio::test]
async fn set_and_shift_remove_stale_numeric_bindings_beyond_sixty_four() {
    let env = environment();
    let script = format!(
        "set -- {}; shift 79; printf '%s:%s' \"$#\" \"$1\"",
        (1..=80)
            .map(|index| format!("arg-{index}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    assert_capture(
        capture(&script, &env, &EvalConfig::default())
            .await
            .expect("wide shift"),
        0,
        b"1:arg-80",
    );
    assert!(!env.vars.read().contains_key("80"));
    assert!(!env.vars.read().contains_key("2"));
    assert_capture(
        capture("set --", &env, &EvalConfig::default())
            .await
            .expect("clear args"),
        0,
        b"",
    );
    assert!(!env.vars.read().contains_key("1"));
}

#[tokio::test]
async fn whole_script_evaluation_restores_exact_argument_state_on_exit_and_error() {
    for script in ["exit 23", ": \"${required:?must be supplied}\""] {
        let env = environment();
        seed_arguments(&env);
        let before = argument_state(&env);
        let parsed = parse_posix_script(script).expect("script");
        let result = bounded(eval_source(&parsed, &env, &arguments(80))).await;
        if script == "exit 23" {
            assert_eq!(result.expect("script exit"), 23);
            assert_eq!(env.exit_code(), 23);
        } else {
            assert!(matches!(
                result,
                Err(EngineError::ParameterExpansion { .. })
            ));
        }
        assert_eq!(argument_state(&env), before);
    }
}

#[tokio::test]
async fn cancellation_of_a_running_loop_stops_its_tail_and_next_iteration() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let env = environment();
    seed_arguments(&env);
    let before = argument_state(&env);
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    // POSIX dispatches jobs through the production builtin registry. This
    // instrumented handler injects a real cancellation only after iteration 2
    // reaches that dispatch point; it does not implement the loop's stopping.
    #[expect(
        clippy::result_large_err,
        reason = "BuiltinHandler fixes the production ShellError return type"
    )]
    let handler: fshell_engine::BuiltinHandler = Arc::new(move |_, _, env, _, _| {
        if observed.fetch_add(1, Ordering::SeqCst) == 1 {
            env.request_shell_cancellation();
        }
        Ok(())
    });
    env.register_builtin("jobs", handler);
    let result = capture("iteration=0; while true; do iteration=$((iteration+1)); jobs; iteration_tail=$iteration; done", &env, &arguments(80)).await;
    assert!(
        matches!(result, Err(EngineError::Interrupted { span: None })),
        "{result:?}"
    );
    assert_eq!(env.exit_code(), 130);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        env.vars.read().get("iteration"),
        Some(&Val::String("2".into()))
    );
    assert_eq!(
        env.vars.read().get("iteration_tail"),
        Some(&Val::String("1".into()))
    );
    assert_eq!(argument_state(&env), before);
}
