#![allow(
    clippy::await_holding_lock,
    unused_must_use,
    unused_imports,
    clippy::needless_borrow
)]

mod common;
use common::*;
use fshell_core::{Expr, ResourceHandle, Stmt};
use fshell_engine::{EngineError, ReactiveEvent, execute_pipeline};
use std::path::PathBuf;

#[tokio::test]
async fn test_init_fsh_loading() {
    let _guard = ProcessLockGuard::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path().join(".config").join("fshell");
    std::fs::create_dir_all(&config_dir).unwrap();

    // Create init.fsh with set/setopt commands AND a reference to $options
    std::fs::write(
        config_dir.join("init.fsh"),
        r#"setopt pipefail
unsetopt autocd
set prompt "> "
set keybinding vi
let has_pipefail = if $options.pipefail { "yes" } else { "no" }
"#,
    )
    .unwrap();

    let original_config = std::env::var("FSH_CONFIG_DIR").ok();
    set_var("FSH_CONFIG_DIR", &config_dir.to_string_lossy());

    let env = setup_test_env();
    load_config_script(&env).await.unwrap();

    // Verify options from init.fsh set/setopt commands
    {
        let opts = env.options.read();
        assert!(!opts.autocd, "autocd should be false");
        assert!(opts.pipefail, "pipefail should be true");
    }

    // Verify prompt vars set by init.fsh
    {
        let vars = env.vars.read();
        assert_eq!(vars.get("FSH_PROMPT"), Some(&Val::String("> ".to_string())));
        assert_eq!(
            vars.get("FSH_KEYBINDING_MODE"),
            Some(&Val::String("vi".to_string()))
        );
    }

    // Verify $options vars map was populated before user code ran
    {
        let vars = env.vars.read();
        let options_val = vars.get("options").unwrap();
        if let Val::Map(options_map) = options_val {
            assert_eq!(
                options_map.get(&ustr::ustr("autocd")),
                Some(&Val::Bool(false))
            );
            assert_eq!(
                options_map.get(&ustr::ustr("pipefail")),
                Some(&Val::Bool(true))
            );
        } else {
            panic!("Expected options to be a Map");
        }
    }

    // Verify init.fsh was sourced — has_pipefail variable should exist
    {
        let vars = env.vars.read();
        assert_eq!(
            vars.get("has_pipefail"),
            Some(&Val::String("yes".to_string()))
        );
    }

    // Restore FSH_CONFIG_DIR
    if let Some(cfg) = original_config {
        set_var("FSH_CONFIG_DIR", &cfg);
    } else {
        remove_var("FSH_CONFIG_DIR");
    }
}

// ---------------------------------------------------------------------------
// Bridge structured output integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_e2e_set_creates_init_fsh() {
    let (tmp, orig, _guard) = setup_config_test();
    let config_dir = tmp.path().join(".config/fsh");

    let env = setup_test_env();

    // Run 'set prompt "> "' via parser
    let mut parser = Parser::new(r#"set prompt "> ""#);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    // Verify init.fsh was created with managed block
    let content = std::fs::read_to_string(config_dir.join("init.fsh")).unwrap();
    assert!(content.contains("# === fsh managed settings ==="));
    assert!(content.contains("# === end managed settings ==="));
    assert!(content.contains("set prompt"));

    teardown_config_test(&orig);
}

#[tokio::test]
async fn test_e2e_setopt_persists() {
    let (tmp, orig, _guard) = setup_config_test();
    let config_dir = tmp.path().join(".config/fsh");

    let env = setup_test_env();

    let mut parser = Parser::new("setopt pipefail");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    {
        let opts = env.options.read();
        assert!(opts.pipefail);
    }

    let content = std::fs::read_to_string(config_dir.join("init.fsh")).unwrap();
    assert!(content.contains("setopt pipefail"));

    teardown_config_test(&orig);
}

#[tokio::test]
async fn test_e2e_unsetopt_removes_from_init_fsh() {
    let (tmp, orig, _guard) = setup_config_test();
    let config_dir = tmp.path().join(".config/fsh");

    let env = setup_test_env();

    // First enable pipefail
    let mut parser = Parser::new("setopt pipefail");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    // Then disable it
    let mut parser = Parser::new("unsetopt pipefail");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    {
        let opts = env.options.read();
        assert!(!opts.pipefail);
    }

    // After unsetopt, all settings are at defaults so the managed block
    // is removed entirely (empty block + no user code = file deleted).
    if config_dir.join("init.fsh").exists() {
        let content = std::fs::read_to_string(config_dir.join("init.fsh")).unwrap();
        assert!(
            !content.contains("setopt pipefail"),
            "pipefail should be gone after unsetopt"
        );
    }

    teardown_config_test(&orig);
}

#[tokio::test]
async fn test_e2e_set_preserves_user_code() {
    let (tmp, orig, _guard) = setup_config_test();
    let config_dir = tmp.path().join(".config/fsh");

    // Pre-existing user code in init.fsh
    std::fs::write(
        config_dir.join("init.fsh"),
        "alias ll \"ls -la\"\nfn greet { echo hello }\n",
    )
    .unwrap();

    let env = setup_test_env();

    // Run set command
    let mut parser = Parser::new(r#"set prompt "fsh> ""#);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();

    let content = std::fs::read_to_string(config_dir.join("init.fsh")).unwrap();
    // Managed block present
    assert!(content.contains("# === fsh managed settings ==="));
    assert!(content.contains("set prompt"));
    // User code preserved
    assert!(content.contains("alias ll"), "user alias should survive");
    assert!(content.contains("fn greet"), "user function should survive");

    teardown_config_test(&orig);
}

#[tokio::test]
async fn test_e2e_startup_gate_prevents_write() {
    let (tmp, orig, _guard) = setup_config_test();
    let config_dir = tmp.path().join(".config/fsh");

    // Write init.fsh with setopt inside
    std::fs::write(
        config_dir.join("init.fsh"),
        "setopt pipefail\nset prompt \"fsh> \"\n",
    )
    .unwrap();

    let env = setup_test_env();

    // Record file mtime before load
    let meta_before = std::fs::metadata(config_dir.join("init.fsh")).unwrap();
    let mtime_before = meta_before.modified().unwrap();

    // Load config (sources init.fsh under gate)
    load_config_script(&env).await.unwrap();

    // File should NOT have been rewritten
    let meta_after = std::fs::metadata(config_dir.join("init.fsh")).unwrap();
    let mtime_after = meta_after.modified().unwrap();
    assert_eq!(
        mtime_before, mtime_after,
        "init.fsh should not change during startup"
    );

    // But options should be applied
    let opts = env.options.read();
    assert!(opts.pipefail);
    assert_eq!(
        env.vars.read().get("FSH_PROMPT"),
        Some(&Val::String("fsh> ".into()))
    );

    teardown_config_test(&orig);
}

#[tokio::test]
async fn test_e2e_multiple_settings_roundtrip() {
    let (tmp, orig, _guard) = setup_config_test();
    let config_dir = tmp.path().join(".config/fsh");

    let env = setup_test_env();

    // Apply multiple settings
    let script = "setopt pipefail\nsetopt notify\nset prompt \"fsh> \"\nset keybinding vi";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in &stmts {
        eval_stmt(stmt, &env, false).await.unwrap();
    }

    // Verify in-memory state
    {
        let opts = env.options.read();
        assert!(opts.pipefail);
        assert!(opts.notify);
    }
    {
        let vars = env.vars.read();
        assert_eq!(vars.get("FSH_PROMPT"), Some(&Val::String("fsh> ".into())));
        assert_eq!(
            vars.get("FSH_KEYBINDING_MODE"),
            Some(&Val::String("vi".into()))
        );
    }

    // Verify init.fsh
    let content = std::fs::read_to_string(config_dir.join("init.fsh")).unwrap();
    assert!(content.contains("setopt pipefail"));
    assert!(content.contains("setopt notify"));
    assert!(content.contains("set prompt"));
    assert!(content.contains("set keybinding vi"));

    teardown_config_test(&orig);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_e2e_config_reload_resets_and_resources() {
    let (tmp, orig, _guard) = setup_config_test();
    let config_dir = tmp.path().join(".config/fsh");

    // Write init.fsh with non-default settings
    std::fs::write(
        config_dir.join("init.fsh"),
        "setopt pipefail\nset prompt \"custom> \"\n",
    )
    .unwrap();

    let env = setup_test_env();
    let (tx, _rx) = tokio::sync::mpsc::channel(100);

    // Manually change something different in memory
    {
        let mut opts = env.options.write();
        opts.notify = true;
    }

    // Run config reload
    fshell_builtins::config_builtin(None, vec![Val::String("reload".into())], &env, tx, None)
        .unwrap();

    // Should have reset to defaults then re-sourced init.fsh
    let opts = env.options.read();
    assert!(opts.pipefail, "pipefail from init.fsh should be applied");
    assert!(!opts.notify, "notify should be back to default (false)");
    assert_eq!(
        env.vars.read().get("FSH_PROMPT"),
        Some(&Val::String("custom> ".into())),
    );

    teardown_config_test(&orig);
}

#[tokio::test]
async fn test_config_list_after_tui_crate_added() {
    let env = setup_test_env();
    let (tx, _rx) = tokio::sync::mpsc::channel(100);
    let result = fshell_builtins::config_builtin(None, vec![], &env, tx, None);
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_s035_setopt_options() {
    let _guard = ProcessLockGuard::acquire();
    let env = setup_test_env();

    // 1. Test nounset
    {
        // Default is true (VariableNotFound error)
        let mut parser = Parser::new("$nonexistent_var");
        let stmts = parser.parse_statements().unwrap();
        let res = eval_stmt(&stmts[0], &env, false).await;
        assert!(matches!(res, Err(EngineError::VariableNotFound { .. })));

        // Unsetopt nounset
        let mut parser = Parser::new("unsetopt nounset");
        let stmts = parser.parse_statements().unwrap();
        eval_stmt(&stmts[0], &env, false).await.unwrap();

        // Now returns null
        let mut parser = Parser::new("let check = $nonexistent_var");
        let stmts = parser.parse_statements().unwrap();
        eval_stmt(&stmts[0], &env, false).await.unwrap();
        let vars = env.vars.read();
        assert_eq!(vars.get("check"), Some(&Val::Null));
    }

    // 2. Test errexit
    {
        // Enable errexit
        let mut parser = Parser::new("setopt errexit");
        let stmts = parser.parse_statements().unwrap();
        eval_stmt(&stmts[0], &env, false).await.unwrap();

        // A statement that fails with non-zero exit status (e.g. 1 == 2 which returns false/exit code 1) should raise Exit
        let mut parser = Parser::new("1 == 2");
        let stmts = parser.parse_statements().unwrap();
        let res = eval_stmt(&stmts[0], &env, false).await;
        assert!(matches!(res, Ok(fshell_engine::Flow::Exit(1))));
    }

    // 3. Test noclobber
    {
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join(format!("noclobber_test_{}.txt", std::process::id()));
        std::fs::write(&temp_file, "hello").unwrap();

        // Grant capabilities for the temp file
        {
            let mut caps = env.caps.caps.write();
            caps.grant(ResourceHandle::WriteDir(temp_dir.clone()));
            caps.grant(ResourceHandle::WriteFile(temp_file.clone()));
            caps.grant(ResourceHandle::ReadFile(temp_file.clone()));
        }

        // Default: noclobber is false, we can overwrite it
        let script = format!(
            "let val1 = \"world\"; $val1 > \"{}\"",
            temp_file.to_string_lossy().escape_default()
        );
        let mut parser = Parser::new(&script);
        let stmts = parser.parse_statements().unwrap();
        eval_stmt(&stmts[0], &env, false).await.unwrap();
        eval_stmt(&stmts[1], &env, false).await.unwrap();
        assert_eq!(std::fs::read_to_string(&temp_file).unwrap(), "world\n");

        // Setopt noclobber
        let mut parser = Parser::new("setopt noclobber");
        let stmts = parser.parse_statements().unwrap();
        eval_stmt(&stmts[0], &env, false).await.unwrap();

        // Now redirect fails — with errexit still enabled this surfaces as Flow::Exit, otherwise as Err
        let script = format!(
            "let val2 = \"world2\"; $val2 > \"{}\"",
            temp_file.to_string_lossy().escape_default()
        );
        let mut parser = Parser::new(&script);
        let stmts = parser.parse_statements().unwrap();
        eval_stmt(&stmts[0], &env, false).await.unwrap();
        let res = eval_stmt(&stmts[1], &env, false).await;
        assert!(matches!(
            res,
            Err(_) | Ok(fshell_engine::Flow::Exit(_)) | Ok(fshell_engine::Flow::ConditionFalse)
        ));

        // Append redirection should still work
        let script = format!(
            "let val3 = \"world3\"; $val3 >> \"{}\"",
            temp_file.to_string_lossy().escape_default()
        );
        let mut parser = Parser::new(&script);
        let stmts = parser.parse_statements().unwrap();
        eval_stmt(&stmts[0], &env, false).await.unwrap();
        eval_stmt(&stmts[1], &env, false).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&temp_file).unwrap(),
            "world\nworld3\n"
        );

        std::fs::remove_file(temp_file).unwrap();
    }

    // 4. Test noexec
    {
        // Enable noexec
        let mut parser = Parser::new("setopt noexec");
        let stmts = parser.parse_statements().unwrap();
        eval_stmt(&stmts[0], &env, false).await.unwrap();

        // Check that variable assignment is NOT executed when run via run_script
        fshell_engine::run_script("let noexec_val = 100", &env)
            .await
            .unwrap();

        let vars = env.vars.read();
        assert!(vars.get("noexec_val").is_none());
    }

    // 5. Test autopushd & cdable_vars
    {
        // Setup options
        {
            let mut opts = env.options.write();
            opts.autopushd = true;
            opts.cdable_vars = true;
        }

        let orig_dir = std::env::current_dir().unwrap();

        // Set variable pointing to temp directory
        let temp_dir = std::env::temp_dir();
        let target_dir = temp_dir.join(format!("autopushd_test_{}", std::process::id()));
        std::fs::create_dir_all(&target_dir).unwrap();

        {
            let mut vars = env.vars.write();
            vars.insert(
                "MY_TARGET_DIR".to_string(),
                Val::String(target_dir.to_string_lossy().to_string()),
            );
        }

        // Run cd with a variable name directly
        let mut parser = Parser::new("cd MY_TARGET_DIR");
        let stmts = parser.parse_statements().unwrap();
        eval_stmt(&stmts[0], &env, false).await.unwrap();

        // DIRSTACK should contain the previous directory
        let vars = env.vars.read();
        if let Some(Val::List(dirstack)) = vars.get("DIRSTACK") {
            assert!(!dirstack.is_empty());
        } else {
            panic!("DIRSTACK not found or not a list");
        }

        let _ = std::env::set_current_dir(orig_dir);
        std::fs::remove_dir(target_dir).unwrap();
    }
}

// === Shell Parity Features Integration Tests ===

#[tokio::test]
async fn test_unified_config_get_set_apply() {
    let (tmp, orig, _guard) = setup_config_test();
    let config_dir = tmp.path().join(".config/fsh");
    let env = setup_test_env();

    // 1. Test setting various option types via config_set / apply_option
    fshell_builtins::cmd::config::config_set(&env, "autocd", &Val::Bool(false)).unwrap();
    fshell_builtins::cmd::config::config_set(&env, "pipeline_channel_size", &Val::Int(256))
        .unwrap();
    fshell_builtins::cmd::config::config_set(
        &env,
        "clear_on_reload",
        &Val::String("always".into()),
    )
    .unwrap();
    fshell_builtins::cmd::config::config_set(
        &env,
        "session_restore",
        &Val::String("picker".into()),
    )
    .unwrap();
    fshell_builtins::cmd::config::config_set(&env, "prompt", &Val::String("my_prompt> ".into()))
        .unwrap();
    fshell_builtins::cmd::config::config_set(&env, "keybinding", &Val::String("vi".into()))
        .unwrap();

    let mut binaries = fshell_core::FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    binaries.insert(ustr::ustr("ls"), Val::String("/bin/ls".into()));
    fshell_builtins::cmd::config::config_set(&env, "command_binaries", &Val::Map(binaries))
        .unwrap();

    // 2. Verify in-memory options and $options map
    {
        let opts = env.options.read();
        assert!(!opts.autocd);
        assert_eq!(opts.pipeline_channel_size, 256);
        assert_eq!(opts.clear_on_reload, "always");
        assert_eq!(opts.session_restore, "picker");
        assert_eq!(
            opts.command_binaries.get("ls"),
            Some(&"/bin/ls".to_string())
        );
    }
    {
        let vars = env.vars.read();
        assert_eq!(
            vars.get("FSH_PROMPT"),
            Some(&Val::String("my_prompt> ".into()))
        );
        assert_eq!(
            vars.get("FSH_KEYBINDING_MODE"),
            Some(&Val::String("vi".into()))
        );
        if let Some(Val::Map(opts_map)) = vars.get("options") {
            assert_eq!(opts_map.get(&ustr::ustr("autocd")), Some(&Val::Bool(false)));
            assert_eq!(
                opts_map.get(&ustr::ustr("pipeline_channel_size")),
                Some(&Val::Int(256))
            );
            assert_eq!(
                opts_map.get(&ustr::ustr("clear_on_reload")),
                Some(&Val::String("always".into()))
            );
            assert_eq!(
                opts_map.get(&ustr::ustr("session_restore")),
                Some(&Val::String("picker".into()))
            );
        } else {
            panic!("$options map missing in env.vars");
        }
    }

    // 3. Verify config_get
    let (tx, mut rx) = tokio::sync::mpsc::channel(10);
    fshell_builtins::cmd::config::config_builtin(
        None,
        vec![Val::String("get".into()), Val::String("autocd".into())],
        &env,
        tx,
        None,
    )
    .unwrap();
    if let Some(fshell_engine::PipelinePayload::Data(val)) = rx.recv().await {
        assert_eq!(*val, Val::Bool(false));
    } else {
        panic!("expected bool from config get autocd");
    }

    // 4. Verify init.fsh persistence
    let init_path = config_dir.join("init.fsh");
    assert!(init_path.exists());
    let content = std::fs::read_to_string(&init_path).unwrap();
    assert!(content.contains("unsetopt autocd"));
    assert!(content.contains("set pipeline_channel_size 256"));
    assert!(content.contains("set clear_on_reload always"));
    assert!(content.contains("set session_restore picker"));
    assert!(content.contains("set prompt \"my_prompt> \""));
    assert!(content.contains("set keybinding vi"));

    teardown_config_test(&orig);
}

#[tokio::test]
async fn test_unified_prompt_and_theme_paths() {
    let (tmp, orig, _guard) = setup_config_test();
    let config_dir = tmp.path().join(".config/fsh");

    // 1. Prompt config save & load
    let prompt_cfg = fshell_core::PromptConfig {
        left_separator: ">>>".to_string(),
        ..Default::default()
    };
    fshell_repl::prompt_config::save_config(&prompt_cfg).unwrap();

    assert!(config_dir.join("prompt.toml").exists());
    let loaded = fshell_repl::prompt_config::load_config();
    assert_eq!(loaded.left_separator, ">>>");

    // 2. Custom theme in config_dir/themes/
    let themes_dir = config_dir.join("themes");
    std::fs::create_dir_all(&themes_dir).unwrap();
    let theme_toml = r##"
name = "custom_test"

[syntax]
keyword = "#ff0000"
string = "#00ff00"
"##;
    std::fs::write(themes_dir.join("custom_test.toml"), theme_toml).unwrap();

    let env = setup_test_env();
    fshell_builtins::cmd::config::config_set(&env, "theme", &Val::String("custom_test".into()))
        .unwrap();

    assert_eq!(env.active_theme().name, "custom_test");

    teardown_config_test(&orig);
}

// === Multicall Binaries Integration Tests ===

#[test]
fn test_multicall_unknown_utility() {
    let (cmd, _symlink) = FshCmd::multicall("foo_unknown");
    let output = cmd.run().expect("failed to execute multicall binary");
    output
        .assert_failure()
        .assert_stderr_contains("not available as a standalone utility");
}

#[test]
fn test_multicall_ls_basic() {
    let (cmd, _symlink) = FshCmd::multicall("ls");
    let test_dir = cmd.temp_path().join("testdir");
    std::fs::create_dir(&test_dir).unwrap();
    std::fs::write(test_dir.join("a.txt"), b"hello").unwrap();
    std::fs::write(test_dir.join("b.txt"), b"world").unwrap();

    let output = cmd
        .arg(&test_dir)
        .run()
        .expect("multicall ls execution failed");

    output
        .assert_success()
        .assert_stdout_contains("a.txt")
        .assert_stdout_contains("b.txt");
}

#[test]
fn test_multicall_ls_error() {
    let (cmd, _symlink) = FshCmd::multicall("ls");
    let output = cmd
        .arg("nonexistent_path_xyz")
        .run()
        .expect("multicall ls execution failed");

    output
        .assert_failure()
        .assert_stderr_contains("nonexistent");
}

#[test]
fn test_multicall_ls_tree() {
    let (cmd, _symlink) = FshCmd::multicall("ls");
    let test_dir = cmd.temp_path().join("tree_test");
    std::fs::create_dir_all(test_dir.join("sub")).unwrap();
    std::fs::write(test_dir.join("root.txt"), b"root").unwrap();
    std::fs::write(test_dir.join("sub").join("nested.txt"), b"nested").unwrap();

    let output = cmd
        .arg("--tree")
        .arg(&test_dir)
        .run()
        .expect("multicall ls --tree execution failed");

    output
        .assert_success()
        .assert_stdout_contains("root.txt")
        .assert_stdout_contains("nested.txt");
}

#[test]
fn test_multicall_ls_pipe_strips_ansi() {
    let (cmd, _symlink) = FshCmd::multicall("ls");
    let test_dir = cmd.temp_path().join("pipe_test");
    std::fs::create_dir(&test_dir).unwrap();
    std::fs::write(test_dir.join("file.txt"), b"data").unwrap();

    let output = cmd
        .arg(&test_dir)
        .run()
        .expect("multicall ls execution failed");

    output.assert_success().assert_stdout_contains("file.txt");
    assert!(!output.stdout.contains("\x1b["));
}
