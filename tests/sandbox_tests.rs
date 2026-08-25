mod common;
use common::*;
use fshell_sandbox::{SandboxConfig, SandboxMode, SandboxProfile, run_sandboxed};

fn unique_test_path(prefix: &str, ext: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{prefix}_{}_{nanos}_{id}{ext}", std::process::id())
}

#[tokio::test]
async fn test_sandbox_allows_safe_command() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    let args = vec![
        Val::String("-c".into()),
        Val::String("echo hello world".into()),
    ];

    run_sandboxed(
        "/bin/sh",
        &args,
        None,
        &env,
        tx,
        &SandboxConfig::new(SandboxMode::ReadOnlySystem),
    )
    .unwrap();

    let timeout = tokio::time::sleep(std::time::Duration::from_secs(10));
    tokio::pin!(timeout);

    let mut output = Vec::new();
    loop {
        tokio::select! {
            payload = rx.recv() => {
                match payload {
                    Some(PipelinePayload::Data(d)) => {
                        if let Val::String(s) = &*d
                            && s.starts_with('\0')
                        {
                            break;
                        }
                        match &*d {
                            Val::String(s) => output.extend_from_slice(s.as_bytes()),
                            Val::Blob(b) => output.extend_from_slice(b),
                            _ => {}
                        }
                    }
                    Some(PipelinePayload::Bytes(b)) => {
                        output.extend_from_slice(&b);
                    }
                    Some(PipelinePayload::Structured(d)) => {
                        eprintln!("diagnostic: {:?}", d);
                    }
                    None => break,
                }
            }
            _ = &mut timeout => {
                panic!("test timed out — no exit code received");
            }
        }
    }

    let output_str = String::from_utf8_lossy(&output);
    assert!(
        output_str.contains("hello world"),
        "output was: {output_str:?}"
    );
}

#[tokio::test]
async fn test_sandbox_spawns_and_exits() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    let result = run_sandboxed(
        "/bin/sh",
        &[Val::String("-c".into()), Val::String("exit 42".into())],
        None,
        &env,
        tx,
        &SandboxConfig::new(SandboxMode::DenyAll),
    );
    assert!(result.is_ok(), "spawn failed: {:?}", result);

    let timeout = tokio::time::sleep(std::time::Duration::from_secs(10));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            payload = rx.recv() => {
                match payload {
                    Some(PipelinePayload::Data(d)) => {
                        if let Val::String(s) = &*d
                            && s.starts_with("\0exit:")
                        {
                            let code: i32 = s.strip_prefix("\0exit:").unwrap().parse().unwrap_or(-1);
                            assert_eq!(code, 42, "expected exit code 42, got {code}");
                            return;
                        }
                    }
                    Some(PipelinePayload::Bytes(b)) => {
                        if let Ok(s) = String::from_utf8(b.to_vec())
                            && s.starts_with("\0exit:")
                        {
                            let code: i32 = s.strip_prefix("\0exit:").unwrap().trim().parse().unwrap_or(-1);
                            assert_eq!(code, 42, "expected exit code 42, got {code}");
                            return;
                        }
                    }
                    Some(PipelinePayload::Structured(d)) => {
                        eprintln!("diagnostic: {:?}", d);
                    }
                    None => break,
                }
            }
            _ = &mut timeout => {
                panic!("test timed out — no exit code received");
            }
        }
    }
}

#[tokio::test]
async fn test_sandbox_blocks_system_write() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    let filename = unique_test_path("/etc/fshell_blocked_etc", ".tmp");
    let cmd = format!("touch {filename}");
    let args = vec![Val::String("-c".into()), Val::String(cmd)];

    run_sandboxed(
        "/bin/sh",
        &args,
        None,
        &env,
        tx,
        &SandboxConfig::new(SandboxMode::ReadOnlySystem),
    )
    .unwrap();

    let timeout = tokio::time::sleep(std::time::Duration::from_secs(10));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            payload = rx.recv() => {
                if let Some(PipelinePayload::Data(d)) = payload
                    && let Val::String(s) = &*d
                    && s.starts_with("\0exit:")
                {
                    let code: i32 = s.strip_prefix("\0exit:").unwrap().parse().unwrap_or(-1);
                    assert_ne!(code, 0, "expected touch /etc to fail with non-zero exit code under sandbox, got {code}");
                    return;
                }
            }
            _ = &mut timeout => { panic!("test timed out"); }
        }
    }
}

#[tokio::test]
async fn test_sandbox_allows_tmp_write() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    let test_file = unique_test_path("/tmp/fshell_tmp_write", ".tmp");
    let cmd = format!("touch {test_file} && rm -f {test_file}");
    let args = vec![Val::String("-c".into()), Val::String(cmd)];

    run_sandboxed(
        "/bin/sh",
        &args,
        None,
        &env,
        tx,
        &SandboxConfig::new(SandboxMode::ReadOnlySystem),
    )
    .unwrap();

    let timeout = tokio::time::sleep(std::time::Duration::from_secs(10));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            payload = rx.recv() => {
                if let Some(PipelinePayload::Data(d)) = payload
                    && let Val::String(s) = &*d
                    && s.starts_with("\0exit:")
                {
                    let code: i32 = s.strip_prefix("\0exit:").unwrap().parse().unwrap_or(-1);
                    assert_eq!(code, 0, "expected write to /tmp to succeed under sandbox, got {code}");
                    return;
                }
            }
            _ = &mut timeout => { panic!("test timed out"); }
        }
    }
}

#[tokio::test]
async fn test_unsafe_fast_path() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    run_sandboxed(
        "/bin/sh",
        &[
            Val::String("-c".into()),
            Val::String("echo fast_path".into()),
        ],
        None,
        &env,
        tx,
        &SandboxConfig::new(SandboxMode::Off),
    )
    .unwrap();

    let timeout = tokio::time::sleep(std::time::Duration::from_secs(5));
    tokio::pin!(timeout);

    let mut output = Vec::new();
    loop {
        tokio::select! {
            payload = rx.recv() => {
                if let Some(PipelinePayload::Data(d)) = payload {
                    if let Val::String(s) = &*d
                        && s.starts_with('\0')
                    {
                        break;
                    }
                    match &*d {
                        Val::String(s) => output.extend_from_slice(s.as_bytes()),
                        Val::Blob(b) => output.extend_from_slice(b),
                        _ => {}
                    }
                }
            }
            _ = &mut timeout => { panic!("test timed out"); }
        }
    }

    let output_str = String::from_utf8_lossy(&output);
    assert!(
        output_str.contains("fast_path"),
        "output was: {output_str:?}"
    );
}

#[tokio::test]
async fn test_sandbox_custom_allowed_path() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    let temp_dir = tempfile::tempdir().unwrap();
    let temp_path = temp_dir.path().to_path_buf();
    let test_file = temp_path.join("sandbox_allowed.txt");
    let cmd = format!(
        "touch {} && rm -f {}",
        test_file.display(),
        test_file.display()
    );

    let profile = SandboxProfile::new(SandboxMode::ReadOnlySystem).allow_write(temp_path);
    let config = SandboxConfig::with_profile(profile);

    run_sandboxed(
        "/bin/sh",
        &[Val::String("-c".into()), Val::String(cmd)],
        None,
        &env,
        tx,
        &config,
    )
    .unwrap();

    let timeout = tokio::time::sleep(std::time::Duration::from_secs(5));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            payload = rx.recv() => {
                if let Some(PipelinePayload::Data(d)) = payload
                    && let Val::String(s) = &*d
                    && s.starts_with("\0exit:")
                {
                    let code: i32 = s.strip_prefix("\0exit:").unwrap().parse().unwrap_or(-1);
                    assert_eq!(code, 0, "expected write to custom allowed path to succeed, got {code}");
                    return;
                }
            }
            _ = &mut timeout => { panic!("test timed out"); }
        }
    }
}

#[tokio::test]
async fn test_destructive_rm_root_blocked_in_non_interactive() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();
    let (tx, _rx) = tokio::sync::mpsc::channel(100);

    // rm -rf / should be caught by check_destructive_command and blocked in non-interactive mode
    let res = fshell_bridge::run_external(
        "rm",
        vec![Val::String("-rf".into()), Val::String("/".into())],
        None,
        &env,
        tx,
        false,
    );

    assert!(
        res.is_err(),
        "expected rm -rf / to be blocked by safety guard"
    );
    let err_msg = res.unwrap_err().to_string();
    assert!(
        err_msg.contains("Dangerous operation")
            && err_msg.contains("blocked by default safety guard"),
        "unexpected error message: {err_msg}"
    );
}

#[tokio::test]
async fn test_safe_rm_target_allowed() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();
    let (tx, _rx) = tokio::sync::mpsc::channel(100);

    let temp_dir = tempfile::tempdir().unwrap();
    let temp_file = temp_dir.path().join("safe_test_file.txt");
    std::fs::write(&temp_file, "hello").unwrap();

    // rm on a regular local file should NOT be blocked by safety guard
    let res = fshell_bridge::run_external(
        "rm",
        vec![Val::String(temp_file.to_string_lossy().to_string())],
        None,
        &env,
        tx,
        false,
    );

    assert!(
        res.is_ok(),
        "expected regular rm on local file to succeed without prompt"
    );
}

#[tokio::test]
async fn test_unsetopt_confirm_destructive_allows_rm() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();
    let (tx, _rx) = tokio::sync::mpsc::channel(100);

    // Disable confirm_destructive
    {
        let mut opts = env.options.write();
        opts.confirm_destructive = false;
    }

    // Now rm -rf / non-existent target should not fail in the safety guard (it will proceed to spawn rm)
    // Note: rm -rf /non_existent_path_xyz
    let res = fshell_bridge::run_external(
        "rm",
        vec![
            Val::String("-rf".into()),
            Val::String("/tmp/non_existent_path_xyz_123".into()),
        ],
        None,
        &env,
        tx,
        false,
    );

    assert!(
        res.is_ok(),
        "expected unsetopt confirm_destructive to bypass safety guard"
    );
}

#[tokio::test]
async fn test_fsh_script_catches_destructive_command() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();

    // Sourcing or running a .fsh script with a destructive command
    let script = "rm -rf /";
    let res = fshell_engine::run_script(script, &env).await;

    assert!(
        res.is_err(),
        "expected .fsh script running rm -rf / to be blocked"
    );
    let err_str = res.unwrap_err().to_string();
    assert!(
        err_str.contains("Dangerous operation")
            && err_str.contains("blocked by default safety guard"),
        "expected safety guard error, got: {err_str}"
    );
}

#[tokio::test]
async fn test_fsh_script_catches_dd_raw_disk() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();

    let script = "dd \"if=/dev/zero\" \"of=/dev/sda\" \"bs=1M\" \"count=1\"";
    let res = fshell_engine::run_script(script, &env).await;

    assert!(res.is_err(), "expected dd of=/dev/sda to be blocked");
    let err_str = res.unwrap_err().to_string();
    assert!(
        err_str.contains("Dangerous operation") && err_str.contains("raw block device write"),
        "expected dd block device error, got: {err_str}"
    );
}

#[tokio::test]
async fn test_fsh_script_catches_mkfs() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();

    let script = "mkfs \"/dev/nvme0n1\"";
    let res = fshell_engine::run_script(script, &env).await;

    assert!(res.is_err(), "expected mkfs to be blocked");
    let err_str = res.unwrap_err().to_string();
    assert!(
        err_str.contains("Dangerous operation") && err_str.contains("disk formatting"),
        "expected mkfs error, got: {err_str}"
    );
}

#[tokio::test]
async fn test_fsh_script_catches_chmod_root() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();

    let script = "chmod -R 777 /";
    let res = fshell_engine::run_script(script, &env).await;

    assert!(res.is_err(), "expected chmod -R 777 / to be blocked");
    let err_str = res.unwrap_err().to_string();
    assert!(
        err_str.contains("Dangerous operation") && err_str.contains("recursive permission change"),
        "expected chmod error, got: {err_str}"
    );
}

#[tokio::test]
async fn test_fsh_script_allows_safe_commands_and_pipelines() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();

    let script = "let count = 42; echo \"the answer is ${count}\" | grep answer";
    let res = fshell_engine::run_script(script, &env).await;

    assert!(
        res.is_ok(),
        "expected safe fsh script with pipeline to succeed without prompt"
    );
}

#[tokio::test]
async fn test_fsh_script_sandbox_off_bypass() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();

    let temp_dir = tempfile::tempdir().unwrap();
    let target = temp_dir.path().join("sandbox_off_file.txt");
    std::fs::write(&target, "test").unwrap();

    let script = format!("sandbox --off rm {}", target.display());
    let res = fshell_engine::run_script(&script, &env).await;

    assert!(res.is_ok(), "expected sandbox --off to run command cleanly");
    assert!(!target.exists(), "expected target file to be deleted");
}

#[tokio::test]
async fn test_sandbox_blocks_usr_write() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    let filename = unique_test_path("/usr/fshell_blocked_usr", ".tmp");
    let cmd = format!("touch {filename}");

    run_sandboxed(
        "/bin/sh",
        &[Val::String("-c".into()), Val::String(cmd)],
        None,
        &env,
        tx,
        &SandboxConfig::new(SandboxMode::ReadOnlySystem),
    )
    .unwrap();

    let timeout = tokio::time::sleep(std::time::Duration::from_secs(10));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            payload = rx.recv() => {
                if let Some(PipelinePayload::Data(d)) = payload
                    && let Val::String(s) = &*d
                    && s.starts_with("\0exit:")
                {
                    let code: i32 = s.strip_prefix("\0exit:").unwrap().parse().unwrap_or(-1);
                    assert_ne!(code, 0, "expected touch /usr to fail under sandbox, got {code}");
                    return;
                }
            }
            _ = &mut timeout => { panic!("test timed out"); }
        }
    }
}

#[tokio::test]
async fn test_sandbox_blocks_bin_write() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    let filename = unique_test_path("/bin/fshell_blocked_bin", ".tmp");
    let cmd = format!("touch {filename}");

    run_sandboxed(
        "/bin/sh",
        &[Val::String("-c".into()), Val::String(cmd)],
        None,
        &env,
        tx,
        &SandboxConfig::new(SandboxMode::ReadOnlySystem),
    )
    .unwrap();

    let timeout = tokio::time::sleep(std::time::Duration::from_secs(10));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            payload = rx.recv() => {
                if let Some(PipelinePayload::Data(d)) = payload
                    && let Val::String(s) = &*d
                    && s.starts_with("\0exit:")
                {
                    let code: i32 = s.strip_prefix("\0exit:").unwrap().parse().unwrap_or(-1);
                    assert_ne!(code, 0, "expected touch /bin to fail under sandbox, got {code}");
                    return;
                }
            }
            _ = &mut timeout => { panic!("test timed out"); }
        }
    }
}

#[tokio::test]
async fn test_sandbox_allows_cwd_write() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    let test_file = unique_test_path("cwd_sandbox_test", ".tmp");
    let cmd = format!("touch {test_file} && rm -f {test_file}");

    run_sandboxed(
        "/bin/sh",
        &[Val::String("-c".into()), Val::String(cmd)],
        None,
        &env,
        tx,
        &SandboxConfig::new(SandboxMode::ReadOnlySystem),
    )
    .unwrap();

    let timeout = tokio::time::sleep(std::time::Duration::from_secs(10));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            payload = rx.recv() => {
                if let Some(PipelinePayload::Data(d)) = payload
                    && let Val::String(s) = &*d
                    && s.starts_with("\0exit:")
                {
                    let code: i32 = s.strip_prefix("\0exit:").unwrap().parse().unwrap_or(-1);
                    assert_eq!(code, 0, "expected write to $PWD to succeed under sandbox, got {code}");
                    return;
                }
            }
            _ = &mut timeout => { panic!("test timed out"); }
        }
    }
}

#[tokio::test]
async fn test_sandbox_stdin_pipe_streaming() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);
    let (pipe_tx, pipe_rx) = tokio::sync::mpsc::channel(100);

    // Send input into stdin pipe stream
    pipe_tx
        .send(PipelinePayload::Data(std::sync::Arc::new(Val::String(
            "piped sandbox stream\n".into(),
        ))))
        .await
        .unwrap();
    drop(pipe_tx);

    run_sandboxed(
        "/usr/bin/tr",
        &[Val::String("a-z".into()), Val::String("A-Z".into())],
        Some(pipe_rx),
        &env,
        tx,
        &SandboxConfig::new(SandboxMode::ReadOnlySystem),
    )
    .unwrap();

    let timeout = tokio::time::sleep(std::time::Duration::from_secs(5));
    tokio::pin!(timeout);

    let mut output = Vec::new();
    loop {
        tokio::select! {
            payload = rx.recv() => {
                match payload {
                    Some(PipelinePayload::Data(d)) => {
                        if let Val::String(s) = &*d && s.starts_with('\0') {
                            break;
                        }
                        if let Val::String(s) = &*d {
                            output.extend_from_slice(s.as_bytes());
                        }
                    }
                    Some(PipelinePayload::Bytes(b)) => {
                        output.extend_from_slice(&b);
                    }
                    None => break,
                    _ => {}
                }
            }
            _ = &mut timeout => { panic!("test timed out"); }
        }
    }

    let output_str = String::from_utf8_lossy(&output);
    assert!(
        output_str.contains("PIPED SANDBOX STREAM"),
        "expected tr output in uppercase, got: {output_str:?}"
    );
}

#[tokio::test]
async fn test_sandbox_exit_codes_various() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();

    for expected_code in [0, 1, 7, 42, 127] {
        let (tx, mut rx) = tokio::sync::mpsc::channel(100);
        let cmd = format!("exit {expected_code}");

        run_sandboxed(
            "/bin/sh",
            &[Val::String("-c".into()), Val::String(cmd)],
            None,
            &env,
            tx,
            &SandboxConfig::new(SandboxMode::ReadOnlySystem),
        )
        .unwrap();

        let timeout = tokio::time::sleep(std::time::Duration::from_secs(5));
        tokio::pin!(timeout);

        loop {
            tokio::select! {
                payload = rx.recv() => {
                    if let Some(PipelinePayload::Data(d)) = payload
                        && let Val::String(s) = &*d
                        && s.starts_with("\0exit:")
                    {
                        let code: i32 = s.strip_prefix("\0exit:").unwrap().parse().unwrap_or(-1);
                        assert_eq!(code, expected_code, "expected exit code {expected_code}, got {code}");
                        break;
                    }
                }
                _ = &mut timeout => { panic!("test timed out for exit code {expected_code}"); }
            }
        }
    }
}

#[test]
fn test_sandbox_profile_builder_api() {
    let custom_path = std::path::PathBuf::from("/my/custom/path");
    let deny_path = std::path::PathBuf::from("/my/blocked/path");

    let profile = SandboxProfile::new(SandboxMode::Isolated)
        .allow_write(custom_path.clone())
        .deny_write(deny_path.clone());

    assert_eq!(profile.mode, SandboxMode::Isolated);
    assert!(!profile.allow_network);
    assert!(profile.allow_write_paths.contains(&custom_path));
    assert!(profile.deny_write_paths.contains(&deny_path));

    let config = SandboxConfig::with_profile(profile);
    assert_eq!(config.mode, SandboxMode::Isolated);
    assert!(!config.profile.allow_network);
}

#[tokio::test]
async fn test_sandbox_custom_denied_path() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    let temp_dir = tempfile::tempdir().unwrap();
    let denied_dir = temp_dir.path().join("strictly_denied");
    std::fs::create_dir_all(&denied_dir).unwrap();
    let test_file = denied_dir.join("blocked.txt");
    let cmd = format!("touch {}", test_file.display());

    let profile = SandboxProfile::new(SandboxMode::ReadOnlySystem).deny_write(denied_dir.clone());
    let config = SandboxConfig::with_profile(profile);

    run_sandboxed(
        "/bin/sh",
        &[Val::String("-c".into()), Val::String(cmd)],
        None,
        &env,
        tx,
        &config,
    )
    .unwrap();

    let timeout = tokio::time::sleep(std::time::Duration::from_secs(5));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            payload = rx.recv() => {
                if let Some(PipelinePayload::Data(d)) = payload
                    && let Val::String(s) = &*d
                    && s.starts_with("\0exit:")
                {
                    let code: i32 = s.strip_prefix("\0exit:").unwrap().parse().unwrap_or(-1);
                    assert_ne!(code, 0, "expected touch in custom denied path to fail under sandbox, got {code}");
                    return;
                }
            }
            _ = &mut timeout => { panic!("test timed out"); }
        }
    }
}

#[tokio::test]
async fn test_sandbox_blocks_home_ssh_write() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let test_file = format!("{home}/.ssh/fshell_sandbox_test_key_{}", std::process::id());
    let cmd = format!("touch {test_file}");

    run_sandboxed(
        "/bin/sh",
        &[Val::String("-c".into()), Val::String(cmd)],
        None,
        &env,
        tx,
        &SandboxConfig::new(SandboxMode::ReadOnlySystem),
    )
    .unwrap();

    let timeout = tokio::time::sleep(std::time::Duration::from_secs(5));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            payload = rx.recv() => {
                if let Some(PipelinePayload::Data(d)) = payload
                    && let Val::String(s) = &*d
                    && s.starts_with("\0exit:")
                {
                    let code: i32 = s.strip_prefix("\0exit:").unwrap().parse().unwrap_or(-1);
                    assert_ne!(code, 0, "expected touch ~/.ssh to fail under sandbox, got {code}");
                    return;
                }
            }
            _ = &mut timeout => { panic!("test timed out"); }
        }
    }
}

#[tokio::test]
async fn test_sandbox_builtin_command_cli_modes() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();

    // 1. sandbox --read-only-system
    let res = fshell_engine::run_script("sandbox --read-only-system echo ro_test", &env).await;
    assert!(
        res.is_ok(),
        "expected sandbox --read-only-system to succeed"
    );

    // 2. sandbox --isolated
    let res = fshell_engine::run_script("sandbox --isolated echo isolated_test", &env).await;
    assert!(res.is_ok(), "expected sandbox --isolated to succeed");

    // 3. sandbox --allow-all
    let res = fshell_engine::run_script("sandbox --allow-all echo off_test", &env).await;
    assert!(res.is_ok(), "expected sandbox --allow-all to succeed");

    // 4. sandbox missing command
    let res = fshell_engine::run_script("sandbox", &env).await;
    assert!(
        res.is_err(),
        "expected bare sandbox with no args to return error"
    );
}

#[tokio::test]
async fn test_fsh_script_setopt_sandbox_all_confines_external_command() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();

    // Enable sandbox_all
    {
        let mut opts = env.options.write();
        opts.sandbox_all = true;
    }

    // Now all external commands spawned inside scripts are sandboxed automatically
    let script = "touch /etc/fshell_sandbox_all_test";
    let res = fshell_engine::run_script(script, &env).await;

    assert!(
        res.is_err(),
        "expected touch /etc to fail when sandbox_all is active"
    );
}
