// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::panic))]
#![allow(clippy::result_large_err)]
use fshell_core::ShellError;
use fshell_core::diagnostic::ErrorCode;
#[allow(unused_imports)]
use fshell_core::{ResourceHandle, Val};
use fshell_engine::Env;
#[allow(unused_imports)]
use fshell_engine::{PipeSender, PipeStream};
#[allow(unused_imports)]
use miette::SourceSpan;
use std::sync::Arc;

#[cfg(feature = "ai")]
pub mod ai;
#[cfg(feature = "ai")]
pub mod ai_provider;
pub mod cmd;
pub mod complete;
pub mod csv;
pub mod dev_env;
pub mod diff;
pub mod error;
#[cfg(feature = "ff")]
pub mod ff;
pub mod files;
pub mod help;
pub mod integrations;
pub mod json;
pub mod mux;
pub mod ps;
pub mod read;
#[cfg(feature = "replace")]
pub mod replace;
#[cfg(feature = "sandbox")]
pub mod sandbox;
pub mod select;
pub mod serve;
pub mod string_ops;
pub mod trap;
pub mod utils;

pub use cmd::bind::*;
pub use cmd::config::*;
pub use cmd::env::*;
pub use cmd::explain::*;
pub use cmd::frecency::*;
pub use cmd::fs::*;
pub use cmd::jobs::*;
pub use cmd::misc::*;
pub use cmd::profiler_builtin::*;
pub use cmd::security::*;
pub use cmd::sort::*;
pub use integrations::*;
pub use utils::*;

fn http_stub(
    _in_rx: Option<fshell_engine::PipeStream>,
    _args: Vec<Val>,
    _env: &Env,
    _tx: fshell_engine::PipeSender,
    _span: Option<miette::SourceSpan>,
) -> Result<(), ShellError> {
    Err("http: not yet implemented (enable with --features http)"
        .to_string()
        .into())
}

fn sql_stub(
    _in_rx: Option<fshell_engine::PipeStream>,
    _args: Vec<Val>,
    _env: &Env,
    _tx: fshell_engine::PipeSender,
    _span: Option<miette::SourceSpan>,
) -> Result<(), ShellError> {
    Err("sql: not yet implemented (enable with --features sql)"
        .to_string()
        .into())
}

fn chart_stub(
    _in_rx: Option<fshell_engine::PipeStream>,
    _args: Vec<Val>,
    _env: &Env,
    _tx: fshell_engine::PipeSender,
    _span: Option<miette::SourceSpan>,
) -> Result<(), ShellError> {
    Err(ShellError::new(
        ErrorCode::Unsupported,
        "chart: not yet implemented",
    ))
}

pub fn init(env: &Env) {
    #[allow(unused_mut)]
    let mut entries: Vec<(String, fshell_engine::BuiltinHandler)> = vec![
        ("direnv_init".to_string(), Arc::new(direnv_init_builtin)),
        ("zoxide_init".to_string(), Arc::new(zoxide_init_builtin)),
        ("starship_init".to_string(), Arc::new(starship_init_builtin)),
        ("fzf_init".to_string(), Arc::new(fzf_init_builtin)),
        ("bind".to_string(), Arc::new(builtin_bind)),
        ("bindkey".to_string(), Arc::new(builtin_bind)),
        ("which".to_string(), Arc::new(which_builtin)),
        ("explain".to_string(), Arc::new(explain_builtin)),
        ("mux".to_string(), Arc::new(mux::mux_builtin)),
        ("ls".to_string(), Arc::new(ls_builtin)),
        ("files".to_string(), Arc::new(files::files_builtin)),
        ("json".to_string(), Arc::new(json::json_builtin)),
        ("csv".to_string(), Arc::new(csv::csv_builtin)),
        ("diff".to_string(), Arc::new(diff::diff_builtin)),
        ("select".to_string(), Arc::new(select::select_builtin)),
        ("serve".to_string(), Arc::new(serve::serve_builtin)),
        ("mkdir".to_string(), Arc::new(mkdir_builtin)),
        ("touch".to_string(), Arc::new(touch_builtin)),
        ("cat".to_string(), Arc::new(cat_builtin)),
        ("string".to_string(), Arc::new(string_ops::string_builtin)),
        ("watch".to_string(), Arc::new(watch_builtin)),
        ("cd".to_string(), Arc::new(cd_builtin)),
        ("z".to_string(), Arc::new(z_builtin)),
        ("zi".to_string(), Arc::new(zi_builtin)),
        ("extract".to_string(), Arc::new(extract_builtin)),
        ("head".to_string(), Arc::new(head_builtin)),
        ("tail".to_string(), Arc::new(tail_builtin)),
        ("uniq".to_string(), Arc::new(uniq_builtin)),
        ("sort".to_string(), Arc::new(sort_builtin)),
        ("ps".to_string(), Arc::new(ps::ps_builtin)),
        ("jobs".to_string(), Arc::new(jobs_builtin)),
        ("fg".to_string(), Arc::new(fg_builtin)),
        ("bg".to_string(), Arc::new(bg_builtin)),
        ("kill".to_string(), Arc::new(kill_builtin)),
        ("export".to_string(), Arc::new(export_builtin)),
        ("env".to_string(), Arc::new(env_builtin)),
        ("funced".to_string(), Arc::new(funced_builtin)),
        ("funcsave".to_string(), Arc::new(funcsave_builtin)),
        ("help".to_string(), Arc::new(help::help_builtin)),
        ("pwd".to_string(), Arc::new(pwd_builtin)),
        ("read".to_string(), Arc::new(read::read_builtin)),
        (
            "eval_direnv".to_string(),
            Arc::new(dev_env::eval_direnv_builtin),
        ),
        (
            "load_env_file".to_string(),
            Arc::new(dev_env::load_env_file_builtin),
        ),
        ("pushd".to_string(), Arc::new(pushd_builtin)),
        ("popd".to_string(), Arc::new(popd_builtin)),
        ("dirs".to_string(), Arc::new(dirs_builtin)),
        ("echo".to_string(), Arc::new(echo_builtin)),
        ("clear".to_string(), Arc::new(clear_builtin)),
        ("wrap".to_string(), Arc::new(wrap_builtin)),
        ("type".to_string(), Arc::new(type_builtin)),
        ("reload".to_string(), Arc::new(reload_builtin)),
        ("alias".to_string(), Arc::new(alias_builtin)),
        ("hook".to_string(), Arc::new(hook_builtin)),
        ("setopt".to_string(), Arc::new(setopt_builtin)),
        ("unsetopt".to_string(), Arc::new(unsetopt_builtin)),
        ("unset".to_string(), Arc::new(unset_builtin)),
        ("config".to_string(), Arc::new(config_builtin)),
        ("set".to_string(), Arc::new(set_builtin)),
        ("wait".to_string(), Arc::new(wait_builtin)),
        ("disown".to_string(), Arc::new(disown_builtin)),
        ("true".to_string(), Arc::new(true_builtin)),
        ("false".to_string(), Arc::new(false_builtin)),
        ("sleep".to_string(), Arc::new(sleep_builtin)),
        ("test".to_string(), Arc::new(test_builtin)),
        ("[".to_string(), Arc::new(bracket_builtin)),
        ("printf".to_string(), Arc::new(printf_builtin)),
        ("exec".to_string(), Arc::new(exec_builtin)),
        ("prompt".to_string(), Arc::new(prompt_builtin)),
        ("complete".to_string(), Arc::new(complete::complete_builtin)),
        ("compgen".to_string(), Arc::new(complete::compgen_builtin)),
        ("hash".to_string(), Arc::new(cmd::hash::hash_builtin)),
        ("theme".to_string(), Arc::new(cmd::theme::theme_builtin)),
        (
            "session".to_string(),
            Arc::new(cmd::session::session_builtin),
        ),
        ("trap".to_string(), Arc::new(trap::trap_builtin)),
        ("profile".to_string(), Arc::new(profile_builtin)),
        ("self".to_string(), Arc::new(cmd::self_cmd::self_builtin)),
        ("http".to_string(), Arc::new(http_stub)),
        ("sql".to_string(), Arc::new(sql_stub)),
        ("chart".to_string(), Arc::new(chart_stub)),
    ];

    #[cfg(feature = "ff")]
    entries.push(("ff".to_string(), Arc::new(ff::ff_builtin)));

    #[cfg(feature = "replace")]
    entries.push(("replace".to_string(), Arc::new(replace::replace_builtin)));

    #[cfg(feature = "sandbox")]
    {
        entries.push(("caps-profile".to_string(), Arc::new(caps_profile_builtin)));
        entries.push((
            "fs-read".to_string(),
            Arc::new(make_path_cap_builtin("fs-read", ResourceHandle::ReadDir)),
        ));
        entries.push((
            "fs-write".to_string(),
            Arc::new(make_path_cap_builtin("fs-write", ResourceHandle::WriteDir)),
        ));
        entries.push(("fs-readwrite".to_string(), Arc::new(fs_readwrite_builtin)));
        entries.push((
            "net-connect".to_string(),
            Arc::new(make_str_cap_builtin(
                "net-connect",
                ResourceHandle::NetworkSocket,
            )),
        ));
        entries.push(("net-all".to_string(), Arc::new(net_all_builtin)));
        entries.push((
            "env-read".to_string(),
            Arc::new(make_str_cap_builtin("env-read", ResourceHandle::ReadEnv)),
        ));
        entries.push((
            "env-write".to_string(),
            Arc::new(make_str_cap_builtin("env-write", ResourceHandle::WriteEnv)),
        ));
        entries.push(("process-spawn".to_string(), Arc::new(process_spawn_builtin)));
        entries.push(("caps-audit".to_string(), Arc::new(caps_audit_builtin)));
        entries.push(("strict".to_string(), Arc::new(strict_builtin)));
        entries.push(("sandbox".to_string(), Arc::new(sandbox::sandbox_builtin)));
        entries.push(("unsafe".to_string(), Arc::new(sandbox::unsafe_builtin)));
    }

    #[cfg(feature = "ai")]
    {
        fn ai_builtin(
            input: Option<PipeStream>,
            args: Vec<Val>,
            env: &Env,
            tx: PipeSender,
            span: Option<SourceSpan>,
        ) -> Result<(), ShellError> {
            crate::ai::ai_main(input, args, env, tx, span)
        }
        entries.push(("ai".to_string(), Arc::new(ai_builtin)));
    }

    #[cfg(feature = "vault")]
    entries.push(("vault".to_string(), Arc::new(cmd::vault::vault_builtin)));

    env.register_builtins(entries);
    env.register_alias("cksum", "hash");
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::await_holding_lock,
        clippy::redundant_pattern_matching,
        clippy::collapsible_if
    )]
    use super::*;
    use fshell_core::{remove_var, set_var};
    use fshell_engine::PipelinePayload;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use tokio::sync::mpsc;

    struct SafeMutex(Mutex<()>);
    impl SafeMutex {
        fn lock(&self) -> Result<std::sync::MutexGuard<'_, ()>, ()> {
            Ok(self.0.lock().unwrap_or_else(|e| e.into_inner()))
        }
    }
    static CD_LOCK: SafeMutex = SafeMutex(Mutex::new(()));

    fn setup_with_lock() -> (impl std::ops::Deref<Target = ()>, PathBuf) {
        let lock = CD_LOCK.lock().unwrap();
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        (lock, home)
    }

    fn init_test_env() -> Env {
        let env = Env::new();
        init(&env);
        env
    }

    #[tokio::test]
    async fn test_builtins_registration() {
        let env = init_test_env();
        assert!(env.get_builtin("ls").is_some());
        assert!(env.get_builtin("cd").is_some());
    }
    // expand_tilde
    #[test]
    fn test_expand_tilde_home_only() {
        let (_lock, home) = setup_with_lock();
        assert_eq!(expand_tilde("~"), PathBuf::from(&home));
    }

    #[test]
    fn test_expand_tilde_home_slash() {
        let (_lock, home) = setup_with_lock();
        assert_eq!(expand_tilde("~/"), PathBuf::from(&home));
    }

    #[test]
    fn test_expand_tilde_home_with_subpath() {
        let (_lock, home) = setup_with_lock();
        assert_eq!(
            expand_tilde("~/Documents"),
            PathBuf::from(&home).join("Documents")
        );
    }

    #[test]
    fn test_expand_tilde_absolute_path_passthrough() {
        let _lock = CD_LOCK.lock().unwrap();
        assert_eq!(expand_tilde("/usr/local"), PathBuf::from("/usr/local"));
    }

    #[test]
    fn test_expand_tilde_relative_path_passthrough() {
        let _lock = CD_LOCK.lock().unwrap();
        assert_eq!(
            expand_tilde("relative/path"),
            PathBuf::from("relative/path")
        );
    }

    #[test]
    fn test_expand_tilde_user_form_not_expanded() {
        let _lock = CD_LOCK.lock().unwrap();
        // ~user is not a recognised pattern – returned verbatim
        assert_eq!(expand_tilde("~other"), PathBuf::from("~other"));
    }

    #[test]
    fn test_expand_tilde_empty_string() {
        let _lock = CD_LOCK.lock().unwrap();
        assert_eq!(expand_tilde(""), PathBuf::from(""));
    }

    #[test]
    fn test_expand_tilde_bare_tilde_inside_path_not_expanded() {
        let _lock = CD_LOCK.lock().unwrap();
        assert_eq!(expand_tilde("foo/~/bar"), PathBuf::from("foo/~/bar"));
    }
    // ls builtin
    #[tokio::test]
    async fn test_ls_valid_directory_returns_entries() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();
        let (tx, mut rx) = mpsc::channel(100);

        ls_builtin(
            None,
            vec![Val::String("-v".into()), Val::String(".".into())],
            &env,
            tx,
            None,
        )
        .unwrap();

        let payload = rx.recv().await;
        assert!(
            payload.is_some(),
            "expected at least one entry from ls -v ."
        );

        if let PipelinePayload::Data(val) = payload.unwrap() {
            match val.as_ref() {
                Val::Map(m) => {
                    assert!(m.contains_key(&ustr::ustr("name")));
                    assert!(m.contains_key(&ustr::ustr("type")));
                    assert!(m.contains_key(&ustr::ustr("size")));
                }
                other => panic!("expected Val::Map, got {other:?}"),
            }
        } else {
            panic!("expected PipelinePayload::Data");
        }
    }

    #[tokio::test]
    async fn test_ls_non_existent_path_returns_error() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();
        let (tx, _rx) = mpsc::channel(100);

        let err = ls_builtin(
            None,
            vec![Val::String("/fshell_test_nonexistent_dir_abc".into())],
            &env,
            tx,
            None,
        )
        .unwrap_err();
        assert!(err.message.contains("No such file"), "got: {err}");
    }

    #[tokio::test]
    async fn test_ls_non_string_argument_returns_error() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();
        let (tx, _rx) = mpsc::channel(100);

        let err = ls_builtin(None, vec![Val::Int(99)], &env, tx, None).unwrap_err();
        assert!(err.message.contains("string"), "got: {err}");
    }

    #[tokio::test]
    async fn test_ls_no_capability_returns_permission_denied() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();

        // Revoke caps for the current directory so the builtin cannot list it.
        let pwd = std::env::current_dir().unwrap();
        {
            let mut caps = env.caps.caps.write();
            caps.strict_mode = true;
            caps.revoke(&ResourceHandle::ReadDir(pwd.clone()));
            caps.revoke(&ResourceHandle::WriteDir(pwd.clone()));
            caps.revoke(&ResourceHandle::ReadFile(pwd.clone()));
            caps.revoke(&ResourceHandle::WriteFile(pwd));
        }

        let (tx, _rx) = mpsc::channel(100);
        let err = ls_builtin(None, vec![Val::String(".".into())], &env, tx, None).unwrap_err();
        assert!(
            err.message.contains("Permission denied") || err.message.contains("Capability denied"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn test_ls_no_args_uses_current_dir() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();
        let (tx, mut rx) = mpsc::channel(100);

        ls_builtin(None, vec![], &env, tx, None).unwrap();

        let payload = rx.recv().await;
        assert!(payload.is_some(), "expected entries from default cwd");
    }

    #[tokio::test]
    async fn test_ls_with_tilde_expands_to_home() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();
        let home = std::env::var("HOME").unwrap();
        // Grant caps for home because Env::new() only grants caps for cwd.
        env.caps
            .caps
            .write()
            .grant(ResourceHandle::ReadDir(PathBuf::from(&home)));

        let (tx, mut rx) = mpsc::channel(100);
        ls_builtin(None, vec![Val::String("~".into())], &env, tx, None).unwrap();

        let payload = rx.recv().await;
        assert!(payload.is_some(), "expected entries from ~");
    }

    #[tokio::test]
    async fn test_ls_with_tilde_slash() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();
        let home = std::env::var("HOME").unwrap();
        env.caps
            .caps
            .write()
            .grant(ResourceHandle::ReadDir(PathBuf::from(&home)));

        let (tx, mut rx) = mpsc::channel(100);
        ls_builtin(None, vec![Val::String("~/".into())], &env, tx, None).unwrap();

        let payload = rx.recv().await;
        assert!(payload.is_some(), "expected entries from ~/");
    }
    // ls flag tests
    #[tokio::test]
    async fn test_ls_v_flag_does_not_crash() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();
        let (tx, mut rx) = mpsc::channel(100);

        ls_builtin(None, vec![Val::String("-v".into())], &env, tx, None).unwrap();

        let payload = rx.recv().await;
        assert!(payload.is_some(), "expected entries with -v flag");
    }

    #[tokio::test]
    async fn test_ls_v_flag_adds_permissions_field() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();
        let (tx, mut rx) = mpsc::channel(100);

        ls_builtin(
            None,
            vec![Val::String("-v".into()), Val::String(".".into())],
            &env,
            tx,
            None,
        )
        .unwrap();

        let payload = rx.recv().await;
        assert!(payload.is_some(), "expected at least one entry");

        if let PipelinePayload::Data(val) = payload.unwrap() {
            match val.as_ref() {
                Val::Map(m) => {
                    assert!(m.contains_key(&ustr::ustr("name")));
                    assert!(
                        m.contains_key(&ustr::ustr("permissions")),
                        "expected permissions field in verbose mode"
                    );
                }
                other => panic!("expected Val::Map, got {other:?}"),
            }
        } else {
            panic!("expected PipelinePayload::Data");
        }
    }

    #[tokio::test]
    async fn test_ls_a_flag_does_not_crash() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();
        let (tx, mut rx) = mpsc::channel(100);

        ls_builtin(None, vec![Val::String("-a".into())], &env, tx, None).unwrap();

        let payload = rx.recv().await;
        assert!(payload.is_some(), "expected entries with -a flag");
    }

    #[tokio::test]
    async fn test_ls_combined_flags_does_not_crash() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();
        let (tx, mut rx) = mpsc::channel(100);

        ls_builtin(None, vec![Val::String("-va".into())], &env, tx, None).unwrap();

        let payload = rx.recv().await;
        assert!(payload.is_some(), "expected entries with -va flags");
    }

    #[tokio::test]
    async fn test_ls_double_dash_end_of_options() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();
        let (tx, mut rx) = mpsc::channel(100);

        // `ls -- .` should treat `.` as a path, not a flag
        ls_builtin(
            None,
            vec![Val::String("--".into()), Val::String(".".into())],
            &env,
            tx,
            None,
        )
        .unwrap();

        let payload = rx.recv().await;
        assert!(payload.is_some(), "expected entries with -- path");
    }

    #[tokio::test]
    async fn test_ls_v_flag_with_path() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();
        let (tx, mut rx) = mpsc::channel(100);

        // `ls -v .` should list the current directory with verbose info
        ls_builtin(
            None,
            vec![Val::String("-v".into()), Val::String(".".into())],
            &env,
            tx,
            None,
        )
        .unwrap();

        let payload = rx.recv().await;
        assert!(payload.is_some(), "expected entries from ls -v .");
    }
    // cd builtin
    #[tokio::test]
    async fn test_cd_valid_directory_succeeds() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();

        let tmp = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join("fshell_cd_test_valid");
        std::fs::create_dir_all(&tmp).unwrap();
        env.caps
            .caps
            .write()
            .grant(ResourceHandle::ReadDir(tmp.clone()));

        let old_dir = std::env::current_dir().unwrap();
        let (tx, _rx) = mpsc::channel(100);

        let result = cd_builtin(
            None,
            vec![Val::String(tmp.to_string_lossy().to_string())],
            &env,
            tx,
            None,
        );
        assert!(result.is_ok(), "cd to valid dir failed: {:?}", result.err());
        assert_eq!(std::env::current_dir().unwrap(), tmp);

        std::env::set_current_dir(old_dir).unwrap();
        std::fs::remove_dir(&tmp).unwrap();
    }

    #[tokio::test]
    async fn test_cd_non_existent_path_returns_error() {
        let env = init_test_env();
        let (tx, _rx) = mpsc::channel(100);

        let err = cd_builtin(
            None,
            vec![Val::String("/fshell_test_nonexistent_xyz".into())],
            &env,
            tx,
            None,
        )
        .unwrap_err();
        assert!(err.message.contains("invalid path"), "got: {err}");
    }

    #[tokio::test]
    async fn test_cd_non_string_argument_returns_error() {
        let env = init_test_env();
        let (tx, _rx) = mpsc::channel(100);

        let err = cd_builtin(None, vec![Val::Bool(false)], &env, tx, None).unwrap_err();
        assert!(err.message.contains("string"), "got: {err}");
    }

    #[tokio::test]
    async fn test_cd_no_args_goes_to_home() {
        let _lock = CD_LOCK.lock().unwrap();
        let _fsh_guard = save_fsh_home();
        let env = init_test_env();
        let home = std::env::var("HOME").unwrap();
        let home_canonical = std::fs::canonicalize(&home).unwrap();
        env.caps
            .caps
            .write()
            .grant(ResourceHandle::ReadDir(home_canonical.clone()));

        let old_dir = std::env::current_dir().unwrap();
        let (tx, _rx) = mpsc::channel(100);

        let result = cd_builtin(None, vec![], &env, tx, None);
        assert!(result.is_ok(), "cd with no args failed: {:?}", result.err());
        assert_eq!(
            std::env::current_dir().unwrap(),
            home_canonical,
            "expected to land in HOME"
        );

        std::env::set_current_dir(old_dir).unwrap();
    }

    #[tokio::test]
    async fn test_cd_no_capability_returns_permission_denied() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();
        let pwd = std::env::current_dir().unwrap();

        // Revoke all caps for the current directory.
        {
            let mut caps = env.caps.caps.write();
            caps.strict_mode = true;
            caps.revoke(&ResourceHandle::ReadDir(pwd.clone()));
            caps.revoke(&ResourceHandle::WriteDir(pwd.clone()));
            caps.revoke(&ResourceHandle::ReadFile(pwd.clone()));
            caps.revoke(&ResourceHandle::WriteFile(pwd));
        }

        let (tx, _rx) = mpsc::channel(100);

        let err = cd_builtin(None, vec![Val::String(".".into())], &env, tx, None).unwrap_err();
        assert!(
            err.message.contains("Permission denied") || err.message.contains("Capability denied"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn test_cd_with_tilde_expands_to_home() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();
        let home = std::env::var("HOME").unwrap();
        let home_canonical = std::fs::canonicalize(&home).unwrap();
        env.caps
            .caps
            .write()
            .grant(ResourceHandle::ReadDir(home_canonical.clone()));

        let old_dir = std::env::current_dir().unwrap();
        let (tx, _rx) = mpsc::channel(100);

        let result = cd_builtin(None, vec![Val::String("~".into())], &env, tx, None);
        assert!(result.is_ok(), "cd ~ failed: {:?}", result.err());
        assert_eq!(
            std::env::current_dir().unwrap(),
            home_canonical,
            "expected ~ to expand to HOME"
        );

        std::env::set_current_dir(old_dir).unwrap();
    }

    #[tokio::test]
    async fn test_cd_preserves_caps_after_movement() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();
        let old_dir = std::env::current_dir().unwrap();

        let tmp = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join("fshell_cd_caps_test");
        std::fs::create_dir_all(&tmp).unwrap();
        env.caps
            .caps
            .write()
            .grant(ResourceHandle::ReadDir(tmp.clone()));

        let (tx, _rx) = mpsc::channel(100);
        cd_builtin(
            None,
            vec![Val::String(tmp.to_string_lossy().to_string())],
            &env,
            tx,
            None,
        )
        .unwrap();

        {
            let caps = env.caps.caps.read();
            assert!(
                caps.check_read_dir(&tmp),
                "expected target dir to have ReadDir cap after cd"
            );
        }

        std::env::set_current_dir(old_dir).unwrap();
        std::fs::remove_dir(&tmp).unwrap();
    }
    // Builtin function handles (dispatch via registry)
    #[tokio::test]
    async fn test_ls_dispatch_via_registry() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();
        let ls = env.get_builtin("ls").unwrap();
        let (tx, mut rx) = mpsc::channel(100);

        ls(None, vec![Val::String(".".into())], &env, tx, None).unwrap();

        let mut count = 0;
        while let Some(_) = rx.recv().await {
            count += 1;
        }
        assert!(count > 0, "ls via registry should produce entries");
    }

    #[tokio::test]
    async fn test_cd_dispatch_via_registry() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();
        let cd = env.get_builtin("cd").unwrap();

        let old_dir = std::env::current_dir().unwrap();
        let tmp = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join("fshell_cd_registry_test");
        std::fs::create_dir_all(&tmp).unwrap();
        env.caps
            .caps
            .write()
            .grant(ResourceHandle::ReadDir(tmp.clone()));

        let (tx, _rx) = mpsc::channel(100);
        cd(
            None,
            vec![Val::String(tmp.to_string_lossy().to_string())],
            &env,
            tx,
            None,
        )
        .unwrap();

        assert_eq!(std::env::current_dir().unwrap(), tmp);

        std::env::set_current_dir(old_dir).unwrap();
        std::fs::remove_dir(&tmp).unwrap();
    }

    struct CwdGuard {
        old_dir: PathBuf,
    }
    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.old_dir);
        }
    }

    /// Restore `FSH_HOME` on drop — prevents leaks across tests even on panic.
    struct FshHomeGuard(Option<String>);
    impl Drop for FshHomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(h) => set_var("FSH_HOME", &h),
                None => remove_var("FSH_HOME"),
            }
        }
    }
    fn save_fsh_home() -> FshHomeGuard {
        let saved = std::env::var("FSH_HOME").ok();
        remove_var("FSH_HOME");
        FshHomeGuard(saved)
    }

    #[tokio::test]
    async fn test_cd_oldpwd_and_smart_fallback() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();
        {
            let mut caps = env.caps.caps.write();
            caps.strict_mode = true;
        }

        let old_dir = std::env::current_dir().unwrap();
        let _guard = CwdGuard {
            old_dir: old_dir.clone(),
        };

        let tmp_root = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let dummy_home = tmp_root.join("fshell_dummy_home_cd_test");
        let _ = std::fs::create_dir_all(&dummy_home);

        // Mock FSH_HOME
        let _fsh_guard = save_fsh_home();
        set_var("FSH_HOME", &dummy_home.to_string_lossy());

        let target_dir = dummy_home.join("target_dir");
        std::fs::create_dir_all(&target_dir).unwrap();

        // Grant capabilities
        env.caps
            .caps
            .write()
            .grant(ResourceHandle::ReadDir(target_dir.clone()));
        env.caps
            .caps
            .write()
            .grant(ResourceHandle::ReadDir(old_dir.clone()));

        // CD to target_dir
        let (tx, _rx) = mpsc::channel(100);
        cd_builtin(
            None,
            vec![Val::String(target_dir.to_string_lossy().to_string())],
            &env,
            tx,
            None,
        )
        .unwrap();

        // Check OLDPWD
        assert_eq!(
            std::env::var("OLDPWD").unwrap(),
            old_dir.to_string_lossy().to_string()
        );

        assert_eq!(
            env.vars.read().get("OLDPWD").unwrap().clone(),
            Val::String(old_dir.to_string_lossy().to_string())
        );

        // Re-grant before cd - back to old_dir because the previous CD revoked it!
        env.caps
            .caps
            .write()
            .grant(ResourceHandle::ReadDir(old_dir.clone()));

        // Check CD - back
        let (tx2, _rx2) = mpsc::channel(100);
        cd_builtin(None, vec![Val::String("-".to_string())], &env, tx2, None).unwrap();
        assert_eq!(std::env::current_dir().unwrap(), old_dir);

        // Test Smart CD Fallback
        // First log a visit to target_dir so it is in the database
        log_frecency_visit(&target_dir).unwrap();

        // Now change back to old_dir
        std::env::set_current_dir(&old_dir).unwrap();

        // Re-grant capability to target_dir since we just moved away
        env.caps
            .caps
            .write()
            .grant(ResourceHandle::ReadDir(target_dir.clone()));

        // Try CD with a fuzzy argument that doesn't exist on disk
        let (tx3, _rx3) = mpsc::channel(100);
        cd_builtin(
            None,
            vec![Val::String("target_dir".to_string())],
            &env,
            tx3,
            None,
        )
        .unwrap();

        assert_eq!(std::env::current_dir().unwrap(), target_dir);

        let _ = std::fs::remove_dir_all(&dummy_home);
    }

    #[tokio::test]
    async fn test_z_exact_fallback_and_slash() {
        let _lock = CD_LOCK.lock().unwrap();
        let env = init_test_env();
        {
            let mut caps = env.caps.caps.write();
            caps.strict_mode = true;
        }

        let old_dir = std::env::current_dir().unwrap();
        let _guard = CwdGuard {
            old_dir: old_dir.clone(),
        };

        let tmp_root = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let dummy_home = tmp_root.join("fshell_dummy_home_z_test");
        let _ = std::fs::create_dir_all(&dummy_home);

        // Mock FSH_HOME
        let _fsh_guard = save_fsh_home();
        set_var("FSH_HOME", &dummy_home.to_string_lossy());

        let target_dir = dummy_home.join("z_target");
        std::fs::create_dir_all(&target_dir).unwrap();

        // Grant capabilities
        env.caps
            .caps
            .write()
            .grant(ResourceHandle::ReadDir(target_dir.clone()));
        env.caps
            .caps
            .write()
            .grant(ResourceHandle::ReadDir(old_dir.clone()));

        // Test exact fallback (since target_dir exists, z target_dir will navigate directly)
        let (tx, _rx) = mpsc::channel(100);
        z_builtin(
            None,
            vec![Val::String(target_dir.to_string_lossy().to_string())],
            &env,
            tx,
            None,
        )
        .unwrap();
        assert_eq!(std::env::current_dir().unwrap(), target_dir);

        // Change back to old_dir
        std::env::set_current_dir(&old_dir).unwrap();

        // Log visits
        log_frecency_visit(&target_dir).unwrap();

        // Grant capability for target_dir since we're in old_dir
        env.caps
            .caps
            .write()
            .grant(ResourceHandle::ReadDir(target_dir.clone()));

        // Verify z_builtin fuzzy matching works
        let (tx2, _rx2) = mpsc::channel(100);
        z_builtin(
            None,
            vec![Val::String("z_target".to_string())],
            &env,
            tx2,
            None,
        )
        .unwrap();
        assert_eq!(std::env::current_dir().unwrap(), target_dir);

        // Change back to dummy_home to test subdirectory matching
        std::env::set_current_dir(&dummy_home).unwrap();

        // Log a subdirectory visit
        let subdir = target_dir.join("sub_foo");
        std::fs::create_dir_all(&subdir).unwrap();
        env.caps
            .caps
            .write()
            .grant(ResourceHandle::ReadDir(subdir.clone()));
        log_frecency_visit(&subdir).unwrap();

        // Log an unrelated visit
        let unrelated = dummy_home.join("unrelated_foo");
        std::fs::create_dir_all(&unrelated).unwrap();
        env.caps
            .caps
            .write()
            .grant(ResourceHandle::ReadDir(unrelated.clone()));
        log_frecency_visit(&unrelated).unwrap();

        // Current directory is dummy_home. Let's cd into target_dir first.
        std::env::set_current_dir(&target_dir).unwrap();

        // Grant capability to subdir
        env.caps
            .caps
            .write()
            .grant(ResourceHandle::ReadDir(subdir.clone()));

        // Verify "z foo /" matches subdirectory under target_dir (i.e. subdir), NOT unrelated_foo
        let (tx3, _rx3) = mpsc::channel(100);
        z_builtin(
            None,
            vec![Val::String("foo".to_string()), Val::String("/".to_string())],
            &env,
            tx3,
            None,
        )
        .unwrap();
        assert_eq!(std::env::current_dir().unwrap(), subdir);

        let _ = std::fs::remove_dir_all(&dummy_home);
    }

    #[tokio::test]
    async fn test_help_builtin_registration() {
        let env = init_test_env();
        assert!(env.get_builtin("help").is_some());
    }

    #[tokio::test]
    async fn test_help_builtin_no_args() {
        let mut env = Env::new();
        env.is_last_stage = true;
        let (tx, mut rx) = mpsc::channel(100);
        let res = help::help_builtin(None, vec![], &env, tx.clone(), None);
        assert!(res.is_ok());
        if let Some(PipelinePayload::Data(val_arc)) = rx.recv().await {
            match val_arc.as_ref() {
                Val::String(s) => assert!(s.contains("BUILTINS")),
                _ => panic!("Expected text output for interactive help"),
            }
        }
    }

    #[tokio::test]
    async fn test_help_builtin_topic() {
        let mut env = Env::new();
        env.is_last_stage = true;
        let (tx, mut rx) = mpsc::channel(100);
        let res = help::help_builtin(
            None,
            vec![Val::String("ls".to_string())],
            &env,
            tx.clone(),
            None,
        );
        assert!(res.is_ok());
        if let Some(PipelinePayload::Data(val_arc)) = rx.recv().await {
            match val_arc.as_ref() {
                Val::String(s) => assert!(s.contains("ls")),
                _ => panic!("Expected text output for interactive topic help"),
            }
        }
    }

    #[tokio::test]
    async fn test_help_builtin_pipeline_auto_structured() {
        let mut env = Env::new();
        env.is_last_stage = false; // Simulated pipeline stage

        // 1. Pipe help without args -> auto structured list
        {
            let (tx, mut rx) = mpsc::channel(100);
            let res = help::help_builtin(None, vec![], &env, tx.clone(), None);
            assert!(res.is_ok());
            let payload = rx.recv().await.expect("Should receive payload");
            match payload {
                PipelinePayload::Data(val_arc) => match val_arc.as_ref() {
                    Val::List(list) => {
                        assert!(!list.is_empty());
                        if let Val::Map(map) = &list[0] {
                            assert!(map.contains_key(&ustr::ustr("name")));
                        } else {
                            panic!("Expected map items in list");
                        }
                    }
                    _ => panic!("Expected Val::List"),
                },
                _ => panic!("Expected PipelinePayload::Data"),
            }
        }

        // 2. Pipe help ls -> auto structured map
        {
            let (tx, mut rx) = mpsc::channel(100);
            let res = help::help_builtin(
                None,
                vec![Val::String("ls".to_string())],
                &env,
                tx.clone(),
                None,
            );
            assert!(res.is_ok());
            let payload = rx.recv().await.expect("Should receive payload");
            match payload {
                PipelinePayload::Data(val_arc) => match val_arc.as_ref() {
                    Val::Map(map) => {
                        assert_eq!(
                            map.get(&ustr::ustr("name")),
                            Some(&Val::String("ls".to_string()))
                        );
                    }
                    _ => panic!("Expected Val::Map"),
                },
                _ => panic!("Expected PipelinePayload::Data"),
            }
        }
    }

    #[tokio::test]
    async fn test_help_builtin_structured() {
        let env = Env::new();

        // 1. All topics structured (using --structured)
        {
            let (tx, mut rx) = mpsc::channel(100);
            let res = help::help_builtin(
                None,
                vec![Val::String("--structured".to_string())],
                &env,
                tx.clone(),
                None,
            );
            assert!(res.is_ok());

            let payload = rx.recv().await.expect("Should receive help payload");
            match payload {
                PipelinePayload::Data(val_arc) => match val_arc.as_ref() {
                    Val::List(list) => {
                        assert!(!list.is_empty(), "Should not be empty");
                        for item in list {
                            match item {
                                Val::Map(map) => {
                                    assert!(map.contains_key(&ustr::ustr("name")));
                                    assert!(map.contains_key(&ustr::ustr("category")));
                                    assert!(map.contains_key(&ustr::ustr("summary")));
                                    assert!(map.contains_key(&ustr::ustr("description")));
                                    assert!(map.contains_key(&ustr::ustr("syntax")));
                                    assert!(map.contains_key(&ustr::ustr("examples")));
                                    assert!(map.contains_key(&ustr::ustr("flags")));
                                    assert!(map.contains_key(&ustr::ustr("related")));
                                }
                                _ => panic!("Expected a map inside the list"),
                            }
                        }
                    }
                    _ => panic!("Expected a Val::List"),
                },
                _ => panic!("Expected PipelinePayload::Data"),
            }
        }

        // 2. Specific topic structured (using --json)
        {
            let (tx, mut rx) = mpsc::channel(100);
            let res = help::help_builtin(
                None,
                vec![
                    Val::String("ls".to_string()),
                    Val::String("--json".to_string()),
                ],
                &env,
                tx.clone(),
                None,
            );
            assert!(res.is_ok());

            let payload = rx.recv().await.expect("Should receive help payload");
            match payload {
                PipelinePayload::Data(val_arc) => match val_arc.as_ref() {
                    Val::Map(map) => {
                        assert_eq!(
                            map.get(&ustr::ustr("name")),
                            Some(&Val::String("ls".to_string()))
                        );
                        assert_eq!(
                            map.get(&ustr::ustr("category")),
                            Some(&Val::String("builtin".to_string()))
                        );
                        assert!(map.contains_key(&ustr::ustr("summary")));
                    }
                    _ => panic!("Expected a Val::Map for single topic"),
                },
                _ => panic!("Expected PipelinePayload::Data"),
            }
        }

        // 3. Category structured
        {
            let (tx, mut rx) = mpsc::channel(100);
            let res = help::help_builtin(
                None,
                vec![
                    Val::String("security".to_string()),
                    Val::String("--structured".to_string()),
                ],
                &env,
                tx.clone(),
                None,
            );
            assert!(res.is_ok());

            let payload = rx.recv().await.expect("Should receive help payload");
            match payload {
                PipelinePayload::Data(val_arc) => match val_arc.as_ref() {
                    Val::List(list) => {
                        assert!(!list.is_empty());
                        for item in list {
                            match item {
                                Val::Map(map) => {
                                    assert_eq!(
                                        map.get(&ustr::ustr("category")),
                                        Some(&Val::String("security".to_string()))
                                    );
                                }
                                _ => panic!("Expected maps"),
                            }
                        }
                    }
                    _ => panic!("Expected a Val::List"),
                },
                _ => panic!("Expected PipelinePayload::Data"),
            }
        }

        // 4. Search structured
        {
            let (tx, mut rx) = mpsc::channel(100);
            let res = help::help_builtin(
                None,
                vec![
                    Val::String("--search".to_string()),
                    Val::String("chart".to_string()),
                    Val::String("--structured".to_string()),
                ],
                &env,
                tx.clone(),
                None,
            );
            assert!(res.is_ok());

            let payload = rx.recv().await.expect("Should receive help payload");
            match payload {
                PipelinePayload::Data(val_arc) => match val_arc.as_ref() {
                    Val::List(list) => {
                        assert!(!list.is_empty());
                        let mut found_chart = false;
                        for item in list {
                            if let Val::Map(map) = item {
                                if map.get(&ustr::ustr("name"))
                                    == Some(&Val::String("chart".to_string()))
                                {
                                    found_chart = true;
                                }
                            }
                        }
                        assert!(found_chart, "Should find 'chart' topic in search results");
                    }
                    _ => panic!("Expected a Val::List"),
                },
                _ => panic!("Expected PipelinePayload::Data"),
            }
        }
    }

    #[tokio::test]
    async fn test_help_render_quick_list() {
        let topics = help::help_topics();
        assert!(!topics.is_empty(), "Should have at least one topic");

        let ls = help::find_topic("ls");
        assert!(ls.is_some(), "Should find 'ls' topic");
        assert_eq!(ls.unwrap().name, "ls");

        let rendered = help::render_quick_list(true);
        assert!(rendered.contains("ls"));
        assert!(rendered.contains("filter"));
    }

    #[tokio::test]
    async fn test_help_render_full() {
        let ls = help::find_topic("ls").unwrap();
        let rendered = help::render_full(ls, true);
        assert!(rendered.contains("ls"), "Should contain topic name");
        assert!(
            rendered.contains("List directory"),
            "Should contain description"
        );
    }

    #[tokio::test]
    async fn test_help_find_topic_normalization() {
        assert!(
            help::find_topic("try-catch").is_some(),
            "Should find try-catch"
        );
        assert!(
            help::find_topic("trycatch").is_some(),
            "Should find trycatch as alias"
        );
        assert!(
            help::find_topic("with-caps").is_some(),
            "Should find with-caps"
        );
        assert!(
            help::find_topic("with caps").is_some(),
            "Should find 'with caps' as alias"
        );
        // Legacy short-name aliases must still work
        assert!(
            help::find_topic("try").is_some(),
            "Should find try as legacy alias"
        );
        assert!(
            help::find_topic("catch").is_some(),
            "Should find catch as legacy alias"
        );
        assert!(
            help::find_topic("caps").is_some(),
            "Should find caps as legacy alias"
        );
        assert!(
            help::find_topic("with").is_some(),
            "Should find with as legacy alias"
        );
        assert!(help::find_topic("nonexistent").is_none());
    }

    #[tokio::test]
    async fn test_clear_builtin() {
        let env = init_test_env();
        let (tx, mut rx) = mpsc::channel(100);

        let result = clear_builtin(None, vec![], &env, tx, None);
        assert!(result.is_ok(), "clear should succeed");

        let payload = rx.recv().await;
        assert!(payload.is_none(), "clear should not emit any payload");
    }

    #[tokio::test]
    async fn test_wrap_builtin() {
        let env = init_test_env();
        let (tx, mut rx) = mpsc::channel(100);

        let result = wrap_builtin(None, vec![], &env, tx, None);
        assert!(result.is_ok(), "wrap should succeed");

        let payload = rx.recv().await;
        assert!(payload.is_none(), "wrap should not emit any payload");
    }

    #[tokio::test]
    async fn test_type_builtin_no_args_errors() {
        let env = init_test_env();
        let (tx, _rx) = mpsc::channel(100);
        let result = type_builtin(None, vec![], &env, tx, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("missing operand"));
    }

    #[tokio::test]
    async fn test_type_builtin_builtin() {
        let env = init_test_env();
        let (tx, mut rx) = mpsc::channel(100);

        type_builtin(None, vec![Val::String("ls".to_string())], &env, tx, None).unwrap();

        let payload = rx.recv().await;
        assert!(payload.is_some());
        if let PipelinePayload::Data(val) = payload.unwrap() {
            match val.as_ref() {
                Val::Map(m) => {
                    assert_eq!(
                        m.get(&ustr::ustr("type")),
                        Some(&Val::String("builtin".to_string()))
                    );
                    assert_eq!(
                        m.get(&ustr::ustr("name")),
                        Some(&Val::String("ls".to_string()))
                    );
                }
                other => panic!("expected Val::Map, got {:?}", other),
            }
        } else {
            panic!("expected Data payload");
        }
    }

    #[tokio::test]
    async fn test_type_builtin_not_found() {
        let env = init_test_env();
        let (tx, mut rx) = mpsc::channel(100);

        type_builtin(
            None,
            vec![Val::String("nonexistent_command_xyzzy".to_string())],
            &env,
            tx,
            None,
        )
        .unwrap();

        let payload = rx.recv().await;
        assert!(payload.is_some());
        if let PipelinePayload::Data(val) = payload.unwrap() {
            match val.as_ref() {
                Val::Map(m) => {
                    assert_eq!(
                        m.get(&ustr::ustr("type")),
                        Some(&Val::String("not-found".to_string()))
                    );
                }
                other => panic!("expected Val::Map, got {:?}", other),
            }
        } else {
            panic!("expected Data payload");
        }
    }

    #[tokio::test]
    async fn test_type_builtin_external_command() {
        let env = init_test_env();
        let (tx, mut rx) = mpsc::channel(100);

        // "wc" is a standard system binary, not an fshell builtin
        type_builtin(None, vec![Val::String("wc".to_string())], &env, tx, None).unwrap();

        let payload = rx.recv().await;
        assert!(payload.is_some());
        if let PipelinePayload::Data(val) = payload.unwrap() {
            match val.as_ref() {
                Val::Map(m) => {
                    assert_eq!(
                        m.get(&ustr::ustr("type")),
                        Some(&Val::String("external".to_string()))
                    );
                    assert!(m.contains_key(&ustr::ustr("path")));
                }
                other => panic!("expected Val::Map, got {:?}", other),
            }
        } else {
            panic!("expected Data payload");
        }
    }
    // setopt / unsetopt
    #[tokio::test]
    async fn test_setopt_list() {
        let env = fshell_engine::Env::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel(100);

        setopt_builtin(None, vec![], &env, tx, None).unwrap();
        let payload = rx.try_recv().unwrap();
        let output_str = if let PipelinePayload::Data(val) = payload {
            match val.as_ref() {
                Val::String(s) => s.clone(),
                other => panic!("expected Val::String, got {other:?}"),
            }
        } else {
            panic!("expected PipelinePayload::Data");
        };
        assert!(output_str.contains("autocd"), "output should list autocd");
        assert!(
            output_str.contains("json_auto_parse"),
            "output should list json_auto_parse"
        );
        assert!(output_str.contains("on"), "default should be on");
    }

    #[tokio::test]
    async fn test_setopt_toggle() {
        let _lock = CD_LOCK.lock().unwrap();
        let original_config = std::env::var("FSH_CONFIG_DIR").ok();
        let tmp = tempfile::tempdir().unwrap();
        set_var("FSH_CONFIG_DIR", &tmp.path().to_string_lossy());

        let env = fshell_engine::Env::new();
        let (tx, _rx) = tokio::sync::mpsc::channel(100);

        // Disable autocd
        unsetopt_builtin(None, vec![Val::String("autocd".into())], &env, tx, None).unwrap();
        {
            let opts = env.options.read();
            assert!(!opts.autocd, "autocd should be off after unsetopt");
        }

        // Re-enable it
        let (tx2, _rx2) = tokio::sync::mpsc::channel(100);
        setopt_builtin(None, vec![Val::String("autocd".into())], &env, tx2, None).unwrap();
        {
            let opts = env.options.read();
            assert!(opts.autocd, "autocd should be on after setopt");
        }

        // Disable json_auto_parse
        let (tx3, _rx3) = tokio::sync::mpsc::channel(100);
        unsetopt_builtin(
            None,
            vec![Val::String("json_auto_parse".into())],
            &env,
            tx3,
            None,
        )
        .unwrap();
        {
            let opts = env.options.read();
            assert!(
                !opts.json_auto_parse,
                "json_auto_parse should be off after unsetopt"
            );
        }

        // Re-enable json_auto_parse
        let (tx4, _rx4) = tokio::sync::mpsc::channel(100);
        setopt_builtin(
            None,
            vec![Val::String("json_auto_parse".into())],
            &env,
            tx4,
            None,
        )
        .unwrap();
        {
            let opts = env.options.read();
            assert!(
                opts.json_auto_parse,
                "json_auto_parse should be on after setopt"
            );
        }

        if let Some(cfg) = original_config {
            set_var("FSH_CONFIG_DIR", &cfg);
        } else {
            remove_var("FSH_CONFIG_DIR");
        }
    }
    // config builtin
    #[tokio::test]
    async fn test_config_set_and_get() {
        let _lock = CD_LOCK.lock().unwrap();
        let original_config = std::env::var("FSH_CONFIG_DIR").ok();
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join(".config/fsh");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        set_var("FSH_CONFIG_DIR", &cfg_dir.to_string_lossy());

        let env = fshell_engine::Env::new();
        let (tx, _rx) = tokio::sync::mpsc::channel(100);

        config_builtin(
            None,
            vec![Val::String("get".into()), Val::String("autocd".into())],
            &env,
            tx,
            None,
        )
        .unwrap();

        config_set(&env, "autocd", &Val::Bool(false)).unwrap();
        {
            let opts = env.options.read();
            assert!(!opts.autocd, "autocd should be false after config set");
        }

        if let Some(cfg) = original_config {
            set_var("FSH_CONFIG_DIR", &cfg);
        } else {
            remove_var("FSH_CONFIG_DIR");
        }
    }

    #[tokio::test]
    async fn test_config_list() {
        let env = fshell_engine::Env::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel(100);

        config_builtin(None, vec![], &env, tx, None).unwrap();
        let payload = rx.try_recv().unwrap();
        if let PipelinePayload::Data(val) = payload {
            match val.as_ref() {
                Val::String(s) => {
                    assert!(s.contains("autocd"), "config list should contain autocd");
                }
                other => panic!("expected Val::String, got {other:?}"),
            }
        } else {
            panic!("expected PipelinePayload::Data");
        }
    }

    #[tokio::test]
    async fn test_config_set_persists_to_init_fsh() {
        let _lock = CD_LOCK.lock().unwrap();
        let original_config = std::env::var("FSH_CONFIG_DIR").ok();
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join(".config/fsh");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        set_var("FSH_CONFIG_DIR", &cfg_dir.to_string_lossy());

        let env = fshell_engine::Env::new();

        config_set(&env, "notify", &Val::Bool(true)).unwrap();

        let content = std::fs::read_to_string(cfg_dir.join("init.fsh")).unwrap();
        assert!(
            content.contains("setopt notify"),
            "init.fsh should contain setopt notify, got: {content}"
        );

        if let Some(cfg) = original_config {
            set_var("FSH_CONFIG_DIR", &cfg);
        } else {
            remove_var("FSH_CONFIG_DIR");
        }
    }

    #[tokio::test]
    async fn test_load_env_file() {
        let env = fshell_engine::Env::new();
        let (tx, _rx) = tokio::sync::mpsc::channel(100);

        let tmp = tempfile::tempdir().unwrap();
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        std::fs::write(
            tmp.path().join(".env"),
            b"# This is a comment\n\nDB_HOST=127.0.0.1\nexport API_KEY=\"secret_token\"\nPORT='8080'\n",
        )
        .unwrap();

        // Grant read permission for the .env path
        {
            let mut caps = env.caps.caps.write();
            caps.grant(ResourceHandle::ReadFile(tmp.path().join(".env")));
        }

        dev_env::load_env_file_builtin(None, vec![], &env, tx, None).unwrap();

        {
            let vars = env.vars.read();
            assert_eq!(vars.get("DB_HOST"), Some(&Val::String("127.0.0.1".into())));
            assert_eq!(
                vars.get("API_KEY"),
                Some(&Val::String("secret_token".into()))
            );
            assert_eq!(vars.get("PORT"), Some(&Val::String("8080".into())));
        }

        std::env::set_current_dir(original_dir).unwrap();
    }

    #[tokio::test]
    async fn test_setopt_persists_to_init_fsh() {
        let _lock = CD_LOCK.lock().unwrap();
        let original_config = std::env::var("FSH_CONFIG_DIR").ok();
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join(".config/fsh");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        set_var("FSH_CONFIG_DIR", &cfg_dir.to_string_lossy());

        let env = fshell_engine::Env::new();
        let (tx, _rx) = tokio::sync::mpsc::channel(100);

        // Gate is open (false) by default, so setopt should persist
        setopt_builtin(
            None,
            vec![Val::String("pipefail".into()), Val::String("notify".into())],
            &env,
            tx.clone(),
            None,
        )
        .unwrap();

        {
            let opts = env.options.read();
            assert!(opts.pipefail, "pipefail should be true");
            assert!(opts.notify, "notify should be true");
        }

        let init_content = std::fs::read_to_string(cfg_dir.join("init.fsh")).unwrap();
        assert!(
            init_content.contains("setopt pipefail"),
            "init.fsh should contain setopt pipefail, got: {init_content}"
        );
        assert!(
            init_content.contains("setopt notify"),
            "init.fsh should contain setopt notify, got: {init_content}"
        );

        // Now test unsetopt persistence
        unsetopt_builtin(None, vec![Val::String("pipefail".into())], &env, tx, None).unwrap();

        {
            let opts = env.options.read();
            assert!(!opts.pipefail, "pipefail should be false after unsetopt");
        }

        let init_content = std::fs::read_to_string(cfg_dir.join("init.fsh")).unwrap();
        assert!(
            !init_content.contains("setopt pipefail"),
            "init.fsh should not contain setopt pipefail after unsetopt, got: {init_content}"
        );
        assert!(
            init_content.contains("setopt notify"),
            "init.fsh should still contain setopt notify, got: {init_content}"
        );

        if let Some(cfg) = original_config {
            set_var("FSH_CONFIG_DIR", &cfg);
        } else {
            remove_var("FSH_CONFIG_DIR");
        }
    }

    #[tokio::test]
    async fn test_setopt_skips_persist_during_init() {
        let _lock = CD_LOCK.lock().unwrap();
        let original_config = std::env::var("FSH_CONFIG_DIR").ok();
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join(".config/fsh");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        set_var("FSH_CONFIG_DIR", &cfg_dir.to_string_lossy());

        let env = fshell_engine::Env::new();
        // Simulate startup — gate closed
        env.is_loading_init_script
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let (tx, _rx) = tokio::sync::mpsc::channel(100);
        setopt_builtin(None, vec![Val::String("pipefail".into())], &env, tx, None).unwrap();

        // Should NOT have created init.fsh
        assert!(
            !cfg_dir.join("init.fsh").exists(),
            "init.fsh should not be created during init"
        );

        if let Some(cfg) = original_config {
            set_var("FSH_CONFIG_DIR", &cfg);
        } else {
            remove_var("FSH_CONFIG_DIR");
        }
    }
}
