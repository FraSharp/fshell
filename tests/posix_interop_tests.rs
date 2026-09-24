//! Transparent fsh + POSIX interop, and agent-safe input, exercised through the
//! real `fsh` binary.

mod common;

use common::FshCmd;

#[test]
fn bare_assignment_declares_like_bash() {
    // Regression: `NAME=value` used to error with "Variable is not defined.
    // Use `let`" and (because that is a runtime error) never reached POSIX.
    let out = FshCmd::new()
        .cmd("G=/tmp/eboot.bin; echo \"G=$G\"")
        .run()
        .unwrap();
    out.assert_success();
    out.assert_stdout_contains("G=/tmp/eboot.bin");
    // ... and nudges toward `let` once.
    out.assert_stderr_contains("declared");
}

#[test]
fn update_on_undeclared_variable_matches_bash() {
    let out = FshCmd::new()
        .cmd("p=/usr; p+=\":/bin\"; echo $p")
        .run()
        .unwrap();
    out.assert_success();
    out.assert_stdout_contains("/usr:/bin");
}

#[test]
fn posix_constructs_run_without_any_flag() {
    // for/do/done used to work only through a narrow heuristic; now the POSIX
    // parser decides.
    let out = FshCmd::new()
        .cmd("for f in a b; do echo $f; done")
        .run()
        .unwrap();
    out.assert_success();
    out.assert_stdout_trimmed_eq("a\nb");
}

#[test]
fn dollar_bang_is_empty_without_a_background_job() {
    // Regression: `$!` was hardcoded to "0", so `kill -9 $!` nuked the whole
    // process group.
    let out = FshCmd::new().cmd("echo \"P=[$!]\"").run().unwrap();
    out.assert_success();
    out.assert_stdout_contains("P=[]");
}

#[test]
fn background_job_has_a_real_pid_and_kill_spares_the_shell() {
    let out = FshCmd::new()
        .cmd("sleep 5 & P=$!; kill -9 $P 2>/dev/null; wait $P 2>/dev/null; echo survived")
        .run()
        .unwrap();
    out.assert_success();
    out.assert_stdout_contains("survived");
}

#[test]
fn background_runs_asynchronously() {
    let dir = std::env::temp_dir().join(format!("fsh_bg_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let t0 = dir.join("t0");
    let t1 = dir.join("t1");
    let script = format!(
        "date +%s > {t0}; sleep 2 & echo started; date +%s > {t1}; wait",
        t0 = t0.display(),
        t1 = t1.display()
    );
    FshCmd::new().cmd(&script).run().unwrap().assert_success();
    let a = std::fs::read_to_string(&t0).unwrap();
    let b = std::fs::read_to_string(&t1).unwrap();
    assert_eq!(
        a.trim(),
        b.trim(),
        "`sleep 2 &` blocked; background is not asynchronous"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn posix_jobs_lists_a_background_job() {
    let out = FshCmd::new()
        .arg("--posix")
        .cmd("sleep 1 & jobs; wait")
        .run()
        .unwrap();
    out.assert_success();
    out.assert_stdout_contains("Running");
    out.assert_stdout_contains("sleep 1");
}

#[test]
fn posix_kill_signals_the_background_job() {
    let out = FshCmd::new()
        .cmd("for f in a; do sleep 5 & P=$!; kill -9 $P 2>/dev/null; echo \"rc=$?\"; done")
        .run()
        .unwrap();
    out.assert_success();
    out.assert_stdout_contains("rc=0");
}

#[test]
fn posix_disown_removes_the_job_from_jobs() {
    let out = FshCmd::new()
        .arg("--posix")
        .cmd("sleep 1 & disown; jobs; echo done")
        .run()
        .unwrap();
    out.assert_success();
    out.assert_stdout_contains("disowned");
    out.assert_stdout_contains("done");
}

#[test]
fn native_flag_disables_the_posix_fallback() {
    let out = FshCmd::new()
        .arg("--native")
        .cmd("for f in a b; do echo $f; done")
        .run()
        .unwrap();
    out.assert_failure();
}

#[test]
fn reported_agent_command_shape_matches_bash() {
    let dir = std::env::temp_dir().join(format!("fsh_agent_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let base = dir.join("base1.log");
    let ring = dir.join("ring1.log");
    let script = format!(
        "G=/tmp/eboot.bin; \
         (true > {base} 2>&1 & P=$!; kill -9 $P 2>/dev/null; wait $P 2>/dev/null); \
         (SHAD_STORE_RING=1 true > {ring} 2>&1 & P=$!; kill -9 $P 2>/dev/null; wait $P 2>/dev/null); \
         for f in {base} {ring}; do echo \"=== $f ===\"; \
           C() {{ sed 's/x/y/g' $f | grep -a -c z; }}; \
           echo \"lines $(wc -l < $f) | echoes $(C 'Native echo')\"; \
         done",
        base = base.display(),
        ring = ring.display()
    );
    let out = FshCmd::new().cmd(&script).run().unwrap();
    out.assert_success();
    out.assert_stdout_contains("=== ");
    out.assert_stdout_contains("lines");
    out.assert_stdout_contains("echoes 0");
    let _ = std::fs::remove_dir_all(&dir);
}
