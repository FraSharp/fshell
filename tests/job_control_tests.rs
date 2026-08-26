mod common;
use common::*;
use fshell_core::Val;
use fshell_engine::{Job, JobStatus, run_script};

// ---------------------------------------------------------------------------
// 1. Job Control State & Builtins
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_job_control_registration_and_jobs_builtin() {
    let env = setup_test_env();

    // Register a simulated background job
    {
        let mut jobs = env.job_control.jobs.write();
        jobs.insert(
            1001,
            Job {
                id: 1,
                pgid: 1001,
                pids: vec![1001],
                cmd: "sleep 100".to_string(),
                status: JobStatus::Running,
                disowned: false,
                started_at: None,
            },
        );
        jobs.insert(
            1002,
            Job {
                id: 2,
                pgid: 1002,
                pids: vec![1002],
                cmd: "compute_task".to_string(),
                status: JobStatus::Suspended,
                disowned: false,
                started_at: None,
            },
        );
    }

    let script = r#"
let jobs_output = (jobs)
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    if let Some(Val::List(items)) = vars.get("jobs_output") {
        let texts: Vec<String> = items.iter().map(|v| v.to_text()).collect();
        assert!(
            texts
                .iter()
                .any(|t| t.contains("[  1] Running    sleep 100"))
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains("[  2] Suspended  compute_task"))
        );
    } else {
        panic!("Expected List of jobs output");
    }
}

#[tokio::test]
async fn test_disowned_jobs_excluded_from_listing() {
    let env = setup_test_env();

    {
        let mut jobs = env.job_control.jobs.write();
        jobs.insert(
            2001,
            Job {
                id: 1,
                pgid: 2001,
                pids: vec![2001],
                cmd: "daemon_proc".to_string(),
                status: JobStatus::Running,
                disowned: true, // Disowned!
                started_at: None,
            },
        );
    }

    let script = r#"
let jobs_output = (jobs)
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    if let Some(Val::List(items)) = vars.get("jobs_output") {
        assert!(
            items.is_empty(),
            "Disowned job should not appear in jobs listing"
        );
    }
}

#[tokio::test]
async fn test_foreground_job_state_tracking() {
    let env = setup_test_env();

    assert_eq!(*env.job_control.fg_mutex.lock().unwrap(), None);

    env.set_foreground_job(Some(42)).unwrap();
    assert_eq!(*env.job_control.fg_mutex.lock().unwrap(), Some(42));

    env.set_foreground_job(None).unwrap();
    assert_eq!(*env.job_control.fg_mutex.lock().unwrap(), None);
}

// ---------------------------------------------------------------------------
// 2. POSIX Background Job Spawning (`&`)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_posix_background_operator_spawns_asynchronously() {
    let env = setup_test_env();
    let script = r#"
sh {
    sleep 0.05 &
    BG_PID=$!
}
"#;
    run_script(script, &env).await.unwrap();

    let vars = env.vars.read();
    assert!(vars.get("BG_PID").is_some());
}
