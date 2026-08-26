// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::error::BuiltinError;
use fshell_core::ShellError;
use fshell_core::Val;
use fshell_engine::handoff::HandoffState;
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use std::os::unix::process::CommandExt;
use std::sync::Arc;

const CMD: &str = "session";
// Public entry point
pub fn session_builtin(
    _input: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    let subcmd = args.first().map(|v| v.to_text()).unwrap_or_default();

    match subcmd.as_str() {
        "" => cmd_resume(None, env, tx),
        "resume" => {
            let id = args.get(1).map(|v| v.to_text());
            cmd_resume(id.as_deref(), env, tx)
        }
        "list" => cmd_list(tx),
        "save" => {
            let name = args.get(1).map(|v| v.to_text());
            cmd_save(name.as_deref(), env)
        }
        "delete" => {
            let id = args.get(1).map(|v| v.to_text());
            cmd_delete(id.as_deref())
        }
        "rename" => {
            let id = args.get(1).map(|v| v.to_text());
            let name = args.get(2).map(|v| v.to_text());
            cmd_rename(id.as_deref(), name.as_deref())
        }
        other => Err(BuiltinError::InvalidArgument {
            cmd: CMD.into(),
            arg: other.into(),
            span: None,
        }
        .into()),
    }
}
// resume — spawn a nested fsh process
fn cmd_resume(id: Option<&str>, env: &Env, tx: PipeSender) -> Result<(), ShellError> {
    let exe = std::env::current_exe().map_err(|e| BuiltinError::InternalError {
        cmd: CMD.into(),
        message: format!("Cannot determine fsh binary path: {e}"),
        span: None,
    })?;

    let mut cmd = std::process::Command::new(&exe);

    if let Some(session_id) = id {
        cmd.arg("-r").arg(session_id);
    } else {
        cmd.arg("-r");
    }

    cmd.stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());

    // Inherit current working directory
    if let Ok(pwd) = std::env::current_dir() {
        cmd.current_dir(pwd);
    }

    // Reset signal mask in child before exec — same pattern as fshell-bridge
    unsafe {
        cmd.pre_exec(move || {
            let mut set = std::mem::zeroed::<libc::sigset_t>();
            libc::sigemptyset(&mut set);
            libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
            libc::signal(libc::SIGCHLD, libc::SIG_DFL);
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            Ok(())
        });
    }

    let env = env.clone();

    tokio::task::spawn_blocking(move || {
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.blocking_send(PipelinePayload::Data(Arc::new(Val::String(format!(
                    "session: failed to spawn fsh: {e}"
                )))));
                return;
            }
        };

        let pid = child.id() as i32;

        // Give child terminal ownership (interactive foreground)
        unsafe {
            libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            libc::tcsetpgrp(libc::STDIN_FILENO, pid);
            libc::signal(libc::SIGTTOU, libc::SIG_DFL);
        }

        let status = child.wait();

        // Reclaim terminal ownership
        unsafe {
            libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            libc::tcsetpgrp(libc::STDIN_FILENO, libc::getpid());
            libc::signal(libc::SIGTTOU, libc::SIG_DFL);
        }

        match status {
            Ok(exit) => {
                let code = exit.code().unwrap_or(1);
                env.vars
                    .write()
                    .insert("?".to_string(), Val::Int(code.into()));
            }
            Err(e) => {
                let _ = tx.blocking_send(PipelinePayload::Data(Arc::new(Val::String(format!(
                    "session: error waiting for fsh: {e}"
                )))));
            }
        }
    });

    Ok(())
}
// list — enumerate saved sessions
fn cmd_list(tx: PipeSender) -> Result<(), ShellError> {
    let sessions = load_sessions()?;

    if sessions.is_empty() {
        tokio::spawn(async move {
            let _ = tx
                .send(PipelinePayload::Data(Arc::new(Val::String(
                    "No saved sessions.\n".to_string(),
                ))))
                .await;
        });
        return Ok(());
    }

    let mut lines = Vec::new();
    lines.push(format!(
        "  {:<16} {:<20} {:<30} {}",
        "ID", "NAME", "CWD", "LAST ACTIVE"
    ));
    lines.push("-".repeat(80));

    for (_path, state, mtime) in &sessions {
        let age = std::time::SystemTime::now()
            .duration_since(*mtime)
            .unwrap_or_default();
        let age_str = format_age(age);

        let name = state
            .vars
            .get("FSH_SESSION_NAME")
            .and_then(|v| {
                if let Val::String(s) = v {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .unwrap_or("");

        let id_short = if state.session_id.len() > 14 {
            &state.session_id[..14]
        } else {
            &state.session_id
        };

        let cwd = if state.cwd.len() > 28 {
            format!("...{}", &state.cwd[state.cwd.len().saturating_sub(27)..])
        } else {
            state.cwd.clone()
        };

        lines.push(format!(
            "  {:<16} {:<20} {:<30} {}",
            id_short, name, cwd, age_str
        ));
    }

    let output = lines.join("\n");
    tokio::spawn(async move {
        let _ = tx
            .send(PipelinePayload::Data(Arc::new(Val::String(output))))
            .await;
    });
    Ok(())
}
// save — tag current session with a display name
fn cmd_save(name: Option<&str>, env: &Env) -> Result<(), ShellError> {
    let name = name.ok_or_else(|| BuiltinError::MissingArgument {
        cmd: CMD.into(),
        description: "session name".into(),
        span: None,
    })?;

    if name.is_empty() {
        return Err(BuiltinError::InvalidArgument {
            cmd: CMD.into(),
            arg: "name cannot be empty".into(),
            span: None,
        }
        .into());
    }

    let mut vars = env.vars.write();
    vars.insert("FSH_SESSION_NAME".to_string(), Val::String(name.into()));
    Ok(())
}
// delete — remove session files
fn cmd_delete(id: Option<&str>) -> Result<(), ShellError> {
    let id = id.ok_or_else(|| BuiltinError::MissingArgument {
        cmd: CMD.into(),
        description: "session id".into(),
        span: None,
    })?;

    let dir = sessions_dir().ok_or_else(|| BuiltinError::InternalError {
        cmd: CMD.into(),
        message: "Cannot determine config directory".into(),
        span: None,
    })?;

    let json_path = dir.join(format!("{id}.json"));
    let log_path = dir.join(format!("{id}.log"));

    let mut deleted_any = false;

    if json_path.exists() {
        std::fs::remove_file(&json_path).map_err(|e| BuiltinError::IoError {
            cmd: CMD.into(),
            message: format!("Cannot delete session file: {e}"),
            span: None,
        })?;
        deleted_any = true;
    }

    if log_path.exists() {
        let _ = std::fs::remove_file(&log_path);
    }

    if !deleted_any {
        return Err(BuiltinError::NotFound {
            cmd: CMD.into(),
            what: format!("session '{id}'"),
            span: None,
        }
        .into());
    }

    Ok(())
}
// rename — change display name in a saved session
fn cmd_rename(id: Option<&str>, name: Option<&str>) -> Result<(), ShellError> {
    let id = id.ok_or_else(|| BuiltinError::MissingArgument {
        cmd: CMD.into(),
        description: "session id".into(),
        span: None,
    })?;

    let name = name.ok_or_else(|| BuiltinError::MissingArgument {
        cmd: CMD.into(),
        description: "new session name".into(),
        span: None,
    })?;

    let dir = sessions_dir().ok_or_else(|| BuiltinError::InternalError {
        cmd: CMD.into(),
        message: "Cannot determine config directory".into(),
        span: None,
    })?;

    let json_path = dir.join(format!("{id}.json"));

    let content = std::fs::read_to_string(&json_path).map_err(|_| BuiltinError::NotFound {
        cmd: CMD.into(),
        what: format!("session '{id}'"),
        span: None,
    })?;

    let mut state: HandoffState =
        serde_json::from_str(&content).map_err(|e| BuiltinError::ParseFailed {
            cmd: CMD.into(),
            message: format!("Cannot parse session file: {e}"),
            span: None,
        })?;

    state
        .vars
        .insert("FSH_SESSION_NAME".to_string(), Val::String(name.into()));

    let updated =
        serde_json::to_string_pretty(&state).map_err(|e| BuiltinError::InternalError {
            cmd: CMD.into(),
            message: format!("Cannot serialize session state: {e}"),
            span: None,
        })?;

    std::fs::write(&json_path, updated).map_err(|e| BuiltinError::IoError {
        cmd: CMD.into(),
        message: format!("Cannot write session file: {e}"),
        span: None,
    })?;

    Ok(())
}
// Helpers
fn sessions_dir() -> Option<std::path::PathBuf> {
    fshell_engine::config_dir().map(|d| d.join("sessions"))
}

fn load_sessions()
-> Result<Vec<(std::path::PathBuf, HandoffState, std::time::SystemTime)>, ShellError> {
    let dir = sessions_dir().ok_or_else(|| BuiltinError::InternalError {
        cmd: CMD.into(),
        message: "Cannot determine config directory".into(),
        span: None,
    })?;

    if !dir.exists() {
        return Ok(Vec::new());
    }

    let entries = std::fs::read_dir(&dir).map_err(|e| BuiltinError::IoError {
        cmd: CMD.into(),
        message: format!("Cannot read sessions directory: {e}"),
        span: None,
    })?;

    let mut sessions = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json")
            && let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(state) = serde_json::from_str::<HandoffState>(&content)
            && let Ok(meta) = std::fs::metadata(&path)
            && let Ok(mtime) = meta.modified()
        {
            sessions.push((path, state, mtime));
        }
    }

    sessions.sort_by_key(|a| std::cmp::Reverse(a.2));
    Ok(sessions)
}

fn format_age(age: std::time::Duration) -> String {
    if age.as_secs() < 60 {
        "just now".to_string()
    } else if age.as_secs() < 3600 {
        format!("{}m ago", age.as_secs() / 60)
    } else if age.as_secs() < 86400 {
        format!("{}h ago", age.as_secs() / 3600)
    } else {
        format!("{}d ago", age.as_secs() / 86400)
    }
}
// Tests
#[cfg(test)]
mod tests {
    use super::*;
    use fshell_core::{remove_var, set_var};
    use fshell_engine::Env;
    use std::sync::Mutex;
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    /// Serialises tests that modify environment variables.
    struct SafeMutex(Mutex<()>);
    impl SafeMutex {
        fn lock(&self) -> Result<std::sync::MutexGuard<'_, ()>, ()> {
            Ok(self.0.lock().unwrap_or_else(|e| e.into_inner()))
        }
    }
    static SESSION_LOCK: SafeMutex = SafeMutex(Mutex::new(()));

    fn init_env() -> Env {
        Env::new()
    }

    /// Create a temp dir, set FSH_CONFIG_DIR to it, return guard.
    /// The temp dir is auto-cleaned when the guard drops.
    /// Requires holding SESSION_LOCK.
    struct SessionDir {
        _dir: TempDir,
        _prev: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    fn setup() -> SessionDir {
        let _lock = SESSION_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let prev = std::env::var("FSH_CONFIG_DIR").ok();
        set_var("FSH_CONFIG_DIR", &dir.path().to_string_lossy());
        SessionDir {
            _dir: dir,
            _prev: prev,
            _lock,
        }
    }

    impl Drop for SessionDir {
        fn drop(&mut self) {
            if let Some(ref prev) = self._prev {
                set_var("FSH_CONFIG_DIR", prev);
            } else {
                remove_var("FSH_CONFIG_DIR");
            }
        }
    }

    fn write_test_session(id: &str, name: Option<&str>, cwd: Option<&str>) {
        let cfg = fshell_engine::config_dir().unwrap();
        let dir = cfg.join("sessions");
        std::fs::create_dir_all(&dir).unwrap();

        let mut vars = fshell_hash::FxHashMap::default();
        if let Some(n) = name {
            vars.insert("FSH_SESSION_NAME".to_string(), Val::String(n.to_string()));
        }

        let state = HandoffState {
            vars,
            fns: fshell_hash::FxHashMap::default(),
            caps_held: std::collections::HashSet::new(),
            caps_strict_mode: false,
            reactive_pipelines: fshell_hash::FxHashMap::default(),
            session_id: id.to_string(),
            cwd: cwd.unwrap_or("/tmp").to_string(),
            options: fshell_engine::ShellOptions::default(),
            hooks: fshell_hash::FxHashMap::default(),
            last_exit_code: 0,
            last_duration_secs: 0.0,
        };

        let content = serde_json::to_string_pretty(&state).unwrap();
        let path = dir.join(format!("{id}.json"));
        std::fs::write(&path, content).unwrap();
    }

    #[tokio::test]
    async fn test_session_list_empty() {
        let _sd = setup();
        let (tx, mut rx) = mpsc::channel(100);

        cmd_list(tx).unwrap();
        let payload = rx.recv().await.unwrap();
        if let PipelinePayload::Data(val) = payload {
            let s = val.to_text();
            assert!(s.contains("No saved sessions"), "got: {s}");
        } else {
            panic!("expected Data payload");
        }
    }

    #[tokio::test]
    async fn test_session_list_with_sessions() {
        let _sd = setup();
        write_test_session("abc123", Some("work"), Some("/home/user/proj"));
        write_test_session("def456", None, Some("/tmp"));
        let (tx, mut rx) = mpsc::channel(100);

        cmd_list(tx).unwrap();
        let payload = rx.recv().await.unwrap();
        if let PipelinePayload::Data(val) = payload {
            let s = val.to_text();
            assert!(s.contains("abc123"), "should show session id, got: {s}");
            assert!(s.contains("work"), "should show name, got: {s}");
            assert!(s.contains("def456"), "should show second session, got: {s}");
        } else {
            panic!("expected Data payload");
        }
    }

    #[tokio::test]
    async fn test_session_save() {
        let env = init_env();
        cmd_save(Some("my-project"), &env).unwrap();
        let vars = env.vars.read();
        assert_eq!(
            vars.get("FSH_SESSION_NAME"),
            Some(&Val::String("my-project".into()))
        );
    }

    #[tokio::test]
    async fn test_session_save_empty_name() {
        let env = init_env();
        let err = cmd_save(Some(""), &env).unwrap_err();
        assert!(err.message.contains("cannot be empty"), "got: {err}");
    }

    #[tokio::test]
    async fn test_session_save_missing_name() {
        let env = init_env();
        let err = cmd_save(None, &env).unwrap_err();
        assert!(err.message.contains("missing argument"), "got: {err}");
    }

    #[tokio::test]
    async fn test_session_delete() {
        let _sd = setup();
        write_test_session("xyz789", None, None);
        let cfg = fshell_engine::config_dir().unwrap();
        let sessions_dir = cfg.join("sessions");
        assert!(sessions_dir.join("xyz789.json").exists());

        cmd_delete(Some("xyz789")).unwrap();
        assert!(!sessions_dir.join("xyz789.json").exists());
    }

    #[tokio::test]
    async fn test_session_delete_not_found() {
        let _sd = setup();
        let err = cmd_delete(Some("nonexistent")).unwrap_err();
        assert!(err.message.contains("not found"), "got: {err}");
    }

    #[tokio::test]
    async fn test_session_delete_missing_id() {
        let err = cmd_delete(None).unwrap_err();
        assert!(err.message.contains("missing argument"), "got: {err}");
    }

    #[tokio::test]
    async fn test_session_rename() {
        let _sd = setup();
        write_test_session("abc", None, None);

        cmd_rename(Some("abc"), Some("newname")).unwrap();

        let cfg = fshell_engine::config_dir().unwrap();
        let sessions_dir = cfg.join("sessions");
        let content = std::fs::read_to_string(sessions_dir.join("abc.json")).unwrap();
        let state: HandoffState = serde_json::from_str(&content).unwrap();
        assert_eq!(
            state.vars.get("FSH_SESSION_NAME"),
            Some(&Val::String("newname".into()))
        );
    }

    #[tokio::test]
    async fn test_session_rename_missing_id() {
        let err = cmd_rename(None, Some("x")).unwrap_err();
        assert!(err.message.contains("missing argument"), "got: {err}");
    }

    #[tokio::test]
    async fn test_session_rename_missing_name() {
        let err = cmd_rename(Some("x"), None).unwrap_err();
        assert!(err.message.contains("missing argument"), "got: {err}");
    }

    #[tokio::test]
    async fn test_session_rename_not_found() {
        let _sd = setup();
        let err = cmd_rename(Some("nonexistent"), Some("x")).unwrap_err();
        assert!(err.message.contains("not found"), "got: {err}");
    }

    #[tokio::test]
    async fn test_session_unknown_subcommand() {
        let env = init_env();
        let (tx, _rx) = mpsc::channel(100);
        let err = session_builtin(None, vec![Val::String("blargh".into())], &env, tx).unwrap_err();
        assert!(err.message.contains("invalid argument"), "got: {err}");
        assert!(err.message.contains("blargh"), "got: {err}");
    }

    #[test]
    fn test_format_age() {
        assert_eq!(format_age(std::time::Duration::from_secs(30)), "just now");
        assert_eq!(format_age(std::time::Duration::from_secs(120)), "2m ago");
        assert_eq!(format_age(std::time::Duration::from_secs(7200)), "2h ago");
        assert_eq!(format_age(std::time::Duration::from_secs(172800)), "2d ago");
    }
}
