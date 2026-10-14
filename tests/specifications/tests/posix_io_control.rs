//! Builtin-only POSIX I/O and control-transfer contracts. Whole-script exit
//! ownership is distinct from streamed evaluation inside a caller.
use fshell_core::Val;
use fshell_engine::{EngineError, Env};
use fshell_posix::{EvalConfig, eval_source, eval_source_stream, parse_posix_script};
use fshell_test_support::fixture::ShellFixture;
use std::{future::Future, io, time::Duration};

fn environment() -> Env {
    let env = Env::for_command();
    env.vars.write().remove("env");
    env.options.write().errexit = false;
    env.options.write().nounset = false;
    env
}
async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("POSIX I/O deadline")
}
async fn capture(source: &str, env: &Env) -> Result<(i32, Option<Vec<u8>>), EngineError> {
    let parsed = parse_posix_script(source).expect("valid builtin-only POSIX script");
    bounded(eval_source_stream(
        &parsed,
        env,
        &EvalConfig::default(),
        true,
    ))
    .await
}
async fn whole(source: &str, env: &Env) -> Result<i32, EngineError> {
    let parsed = parse_posix_script(source).expect("valid whole-script fixture");
    bounded(eval_source(&parsed, env, &EvalConfig::default())).await
}
fn assert_output(actual: (i32, Option<Vec<u8>>), code: i32, bytes: &[u8]) {
    assert_eq!(actual, (code, Some(bytes.to_vec())));
}
fn fixture_environment(fixture: &ShellFixture) -> Env {
    let env = environment();
    env.set_cwd(fixture.workspace());
    env
}

#[tokio::test]
async fn exit_retains_all_preceding_output_and_stops_later_effects() {
    let env = environment();
    assert_output(
        capture(
            "printf before; { printf nested; exit 23; }; after=yes",
            &env,
        )
        .await
        .expect("exit"),
        23,
        b"beforenested",
    );
    assert_eq!(env.exit_code(), 23);
    assert!(!env.vars.read().contains_key("after"));
}
#[tokio::test]
async fn function_return_keeps_output_and_exposes_status_to_its_caller() {
    let env = environment();
    assert_output(
        capture(
            "f() { printf body; return 19; wrong=yes; }; printf before; f; seen=$?; printf after",
            &env,
        )
        .await
        .expect("return"),
        0,
        b"beforebodyafter",
    );
    assert_eq!(env.vars.read().get("seen"), Some(&Val::String("19".into())));
    assert!(!env.vars.read().contains_key("wrong"));
}
#[tokio::test]
async fn loop_break_and_continue_keep_output_emitted_before_the_transfer() {
    let env = environment();
    assert_output(capture("for i in a b c; do printf '%s' \"$i\"; if [ \"$i\" = a ]; then continue; fi; break; done; printf end", &env).await.expect("loop transfers"), 0, b"abend");
    assert_eq!(env.vars.read().get("i"), Some(&Val::String("b".into())));
}
#[tokio::test]
async fn errexit_retains_output_and_prevents_the_next_command() {
    let env = environment();
    assert_output(
        capture("set -e; printf before; false; after=yes", &env)
            .await
            .expect("errexit"),
        1,
        b"before",
    );
    assert_eq!(env.exit_code(), 1);
    assert!(!env.vars.read().contains_key("after"));
}
#[tokio::test]
async fn if_and_elif_conditions_share_the_callers_output_route() {
    let env = environment();
    assert_output(capture("if printf first; false; then wrong=yes; elif printf second; true; then printf body; fi; printf end", &env).await.expect("condition output"), 0, b"firstsecondbodyend");
    assert!(!env.vars.read().contains_key("wrong"));
}
#[tokio::test]
async fn while_conditions_emit_output_including_the_final_false_condition() {
    let env = environment();
    assert_output(capture("i=0; while printf 'c%s' \"$i\"; [ \"$i\" -lt 2 ]; do printf 'b%s' \"$i\"; i=$((i+1)); done", &env).await.expect("while output"), 0, b"c0b0c1b1c2");
}
#[tokio::test]
async fn stdout_capture_drains_beyond_channel_capacity_without_losing_or_repeating_bytes() {
    let env = environment();
    assert_output(
        capture(
            "i=0; while [ \"$i\" -lt 100 ]; do printf x; i=$((i+1)); done; exit 7",
            &env,
        )
        .await
        .expect("bounded capture transport"),
        7,
        &[b'x'; 100],
    );
}

#[tokio::test]
async fn normal_exit_trap_observes_original_status_and_does_not_replace_it() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let env = fixture_environment(&fixture);
    assert_eq!(
        whole("trap 'printf \"%s\" \"$?\" >> trace; false' EXIT; :", &env)
            .await
            .expect("normal exit"),
        0
    );
    assert_eq!(env.exit_code(), 0);
    assert_eq!(fixture.read_file("trace")?, b"0");
    assert!(env.posix_exit_trap.read().is_none());
    bounded(fshell_posix::eval::run_exit_trap(&env))
        .await
        .expect("already consumed trap");
    assert_eq!(fixture.read_file("trace")?, b"0");
    fixture.close()
}
#[tokio::test]
async fn explicit_exit_status_survives_a_successful_exit_handler() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let env = fixture_environment(&fixture);
    assert_eq!(
        whole(
            "trap 'printf \"%s\" \"$?\" > trace; true' EXIT; exit 23; wrong=yes",
            &env
        )
        .await
        .expect("explicit exit"),
        23
    );
    assert_eq!(env.exit_code(), 23);
    assert_eq!(fixture.read_file("trace")?, b"23");
    assert!(!env.vars.read().contains_key("wrong"));
    fixture.close()
}
#[tokio::test]
async fn exit_inside_the_exit_handler_overrides_status_without_reentering_the_handler()
-> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let env = fixture_environment(&fixture);
    assert_eq!(
        whole(
            "trap 'printf hit >> trace; exit 17; wrong=yes' EXIT; exit 23",
            &env
        )
        .await
        .expect("handler exit"),
        17
    );
    assert_eq!(env.exit_code(), 17);
    assert_eq!(fixture.read_file("trace")?, b"hit");
    assert!(env.posix_exit_trap.read().is_none());
    assert!(!env.vars.read().contains_key("wrong"));
    fixture.close()
}
#[tokio::test]
async fn hard_error_runs_the_exit_handler_once_with_failure_status_and_retains_its_type()
-> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let env = fixture_environment(&fixture);
    let error = whole(
        "trap 'printf \"%s\" \"$?\" >> trace' EXIT; : \"${required:?supply value}\"; wrong=yes",
        &env,
    )
    .await
    .expect_err("hard expansion error");
    assert!(
        matches!(error, EngineError::ParameterExpansion { ref parameter, ref message, span: None } if parameter == "${required}" && message == "supply value"),
        "{error:?}"
    );
    assert_eq!(fixture.read_file("trace")?, b"1");
    assert_eq!(env.exit_code(), 1);
    assert!(env.posix_exit_trap.read().is_none());
    assert!(!env.vars.read().contains_key("wrong"));
    fixture.close()
}
#[tokio::test]
async fn streamed_evaluation_does_not_claim_whole_script_exit_ownership() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let env = fixture_environment(&fixture);
    assert_output(
        capture(
            "trap 'printf hit >> trace' EXIT; printf body; exit 23",
            &env,
        )
        .await
        .expect("streamed exit"),
        23,
        b"body",
    );
    assert_eq!(
        env.posix_exit_trap.read().as_deref(),
        Some("printf hit >> trace")
    );
    assert!(!fixture.workspace().join("trace").exists());
    bounded(fshell_posix::eval::run_exit_trap(&env))
        .await
        .expect("explicit exit ownership");
    assert_eq!(fixture.read_file("trace")?, b"hit");
    assert_eq!(env.exit_code(), 23);
    fixture.close()
}

#[tokio::test]
async fn redirected_function_output_is_written_before_a_nested_return() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let env = fixture_environment(&fixture);
    assert_output(
        capture(
            "f() { { printf before; return 19; } > inner; }; f; seen=$?; printf after",
            &env,
        )
        .await
        .expect("redirected return"),
        0,
        b"after",
    );
    assert_eq!(fixture.read_file("inner")?, b"before");
    assert_eq!(env.vars.read().get("seen"), Some(&Val::String("19".into())));
    fixture.close()
}
#[tokio::test]
async fn function_call_redirection_is_inherited_and_nested_output_restores_the_outer_route()
-> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let env = fixture_environment(&fixture);
    assert_output(capture("f() { printf outer1; { printf inner; } > inner; printf outer2; }; f > outer; printf end", &env).await.expect("function routes"), 0, b"end");
    assert_eq!(fixture.read_file("outer")?, b"outer1outer2");
    assert_eq!(fixture.read_file("inner")?, b"inner");
    fixture.close()
}
#[tokio::test]
async fn compound_output_redirection_does_not_intercept_an_explicit_inner_override()
-> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let env = fixture_environment(&fixture);
    assert_output(
        capture(
            "{ printf first; printf inner > inner; printf last; } > outer; printf end",
            &env,
        )
        .await
        .expect("compound routes"),
        0,
        b"end",
    );
    assert_eq!(fixture.read_file("outer")?, b"firstlast");
    assert_eq!(fixture.read_file("inner")?, b"inner");
    fixture.close()
}
#[tokio::test]
async fn compound_descriptor_duplication_observes_source_order() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let env = fixture_environment(&fixture);
    assert_output(
        capture(
            "{ printf out; printf err >&2; } > combined 2>&1; printf end",
            &env,
        )
        .await
        .expect("combined descriptors"),
        0,
        b"end",
    );
    assert_eq!(fixture.read_file("combined")?, b"outerr");
    assert_output(
        capture(
            "{ printf out; printf err >&2; } 2>&1 > data; printf end",
            &env,
        )
        .await
        .expect("reversed descriptors"),
        0,
        b"errend",
    );
    assert_eq!(fixture.read_file("data")?, b"out");
    fixture.close()
}
#[tokio::test]
async fn grouped_pipeline_preserves_interstage_transport_and_redirects_only_final_output()
-> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let env = fixture_environment(&fixture);
    assert_output(capture("{ printf 'payload\\n' | { read -r line; printf '<%s>' \"$line\"; }; } > out; printf end", &env).await.expect("grouped pipeline"), 0, b"end");
    assert_eq!(fixture.read_file("out")?, b"<payload>");
    fixture.close()
}
#[tokio::test]
async fn unsupported_descriptor_duplication_is_a_command_failure_not_a_panic() {
    let env = environment();
    assert_output(
        capture("printf wrong 9>&1; seen=$?; printf after", &env)
            .await
            .expect("unsupported descriptor"),
        0,
        b"after",
    );
    assert_eq!(env.vars.read().get("seen"), Some(&Val::String("1".into())));
}
#[tokio::test]
async fn silent_redirection_truncates_and_append_retains_previous_bytes() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    fixture.create_file("out", b"stale")?;
    let env = fixture_environment(&fixture);
    assert_output(
        capture(
            ": > out; printf first >> out; printf second >> out; printf end",
            &env,
        )
        .await
        .expect("truncate and append"),
        0,
        b"end",
    );
    assert_eq!(fixture.read_file("out")?, b"firstsecond");
    fixture.close()
}
#[tokio::test]
async fn failed_redirection_keeps_earlier_file_effects_but_does_not_execute_the_command()
-> io::Result<()> {
    for command in [
        "printf wrong > early > absent/target",
        "{ ran=yes; printf wrong; } > early > absent/target",
    ] {
        let fixture = ShellFixture::new()?;
        fixture.create_file("early", b"stale")?;
        let env = fixture_environment(&fixture);
        let script = format!("{command}; failed=$?; printf resumed");
        assert_output(
            capture(&script, &env)
                .await
                .expect("recoverable redirect failure"),
            0,
            b"resumed",
        );
        assert_eq!(fixture.read_file("early")?, b"");
        assert_eq!(
            env.vars.read().get("failed"),
            Some(&Val::String("1".into()))
        );
        assert!(!env.vars.read().contains_key("ran"));
        fixture.close()?;
    }
    Ok(())
}
#[tokio::test]
async fn redirection_target_expansion_error_leaves_previous_file_effects_and_restores_the_next_route()
-> io::Result<()> {
    let fixture = ShellFixture::new()?;
    fixture.create_file("early", b"stale")?;
    let env = fixture_environment(&fixture);
    let error = capture(
        "printf wrong > early > \"${target:?target required}\"; ran=yes",
        &env,
    )
    .await
    .expect_err("target expansion failure");
    assert!(
        matches!(error, EngineError::ParameterExpansion { ref parameter, ref message, span: None } if parameter == "${target}" && message == "target required"),
        "{error:?}"
    );
    assert_eq!(fixture.read_file("early")?, b"");
    assert!(!env.vars.read().contains_key("ran"));
    assert_output(
        capture("printf resumed", &env)
            .await
            .expect("next evaluation"),
        0,
        b"resumed",
    );
    assert_eq!(fixture.read_file("early")?, b"");
    fixture.close()
}
#[tokio::test]
async fn function_input_redirection_reaches_builtin_read_and_preserves_raw_bytes() -> io::Result<()>
{
    let fixture = ShellFixture::new()?;
    fixture.create_file("input", b"a b\\c\n")?;
    let env = fixture_environment(&fixture);
    assert_output(
        capture(
            "f() { read -r line; printf '<%s>' \"$line\"; }; f < input; printf end",
            &env,
        )
        .await
        .expect("function input"),
        0,
        b"<a b\\c>end",
    );
    assert_eq!(fixture.read_file("input")?, b"a b\\c\n");
    fixture.close()
}
