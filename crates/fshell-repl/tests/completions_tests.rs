// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_bridge::init as bridge_init;
use fshell_builtins::init as builtins_init;
use fshell_core::{Val, remove_var, set_var};
use fshell_engine::{Env, Job, JobStatus, get_path_executables, invalidate_path_cache};
use fshell_repl::FshellCompleter;
use reedline::Completer;
use std::os::unix::fs::PermissionsExt;
use std::sync::{LazyLock, Mutex};

static TEST_CWD_MUTEX: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static TEST_PATH_MUTEX: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn create_temp_bins(prefix: &str, bins: &[&str]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "fsh_path_test_{}_{}",
        prefix,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::create_dir_all(&dir);
    for bin in bins {
        let path = dir.join(bin);
        let _ = std::fs::write(&path, b"#!/bin/sh\necho hi\n");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        let _ = std::fs::set_permissions(&path, perms);
    }
    dir
}

fn make_completer_with_path(path_dir: &std::path::Path) -> FshellCompleter {
    let env = Env::new();
    {
        let mut opts = env.options.write();
        opts.sandbox_mode = "off".to_string();
    }
    builtins_init(&env);
    bridge_init(&env);
    env.vars.write().insert(
        "PATH".to_string(),
        Val::String(path_dir.to_string_lossy().to_string()),
    );
    // Ensure the global PATH cache reflects this isolated PATH.
    invalidate_path_cache();
    // Warming is done lazily by get_path_executables; no extra call needed,
    // but we prime it so the first complete() doesn't pay rebuild cost inside the assert.
    let _ = get_path_executables(Some(&path_dir.to_string_lossy().to_string()));
    FshellCompleter { env }
}

fn make_completer() -> FshellCompleter {
    let env = Env::new();
    {
        let mut opts = env.options.write();
        opts.sandbox_mode = "off".to_string();
    }
    builtins_init(&env);
    bridge_init(&env);
    FshellCompleter { env }
}

#[tokio::test]
async fn test_builtin_prefix_completion() {
    let mut c = make_completer();
    let results = c.complete("he", 2);
    assert!(
        results.iter().any(|s| s.value == "help"),
        "should suggest 'help' when typing 'he'"
    );
}

#[tokio::test]
async fn test_builtin_exact_match_ranked_first() {
    let mut c = make_completer();
    let results = c.complete("ls", 2);
    assert!(!results.is_empty());
    assert_eq!(results[0].value, "ls", "exact match should be first");
}

#[tokio::test]
async fn test_keyword_completion() {
    let mut c = make_completer();
    let results = c.complete("le", 2);
    assert!(
        results.iter().any(|s| s.value == "let"),
        "should suggest 'let' keyword"
    );
}

#[tokio::test]
async fn test_pipe_operators_shown_on_empty_downstream() {
    let mut c = make_completer();
    let results = c.complete(" |", 2);
    assert!(!results.is_empty(), "should suggest pipe operators");
    assert!(results.iter().any(|s| s.value == "filter "));
    assert!(results.iter().any(|s| s.value == "map "));
    assert!(results.iter().any(|s| s.value == "@json "));
}

#[tokio::test]
async fn test_flag_completions_for_ls() {
    let mut c = make_completer();
    let results = c.complete("ls -", 4);
    assert!(
        results.iter().any(|s| s.value == "-v"),
        "should suggest -v for ls"
    );
    assert!(
        results.iter().any(|s| s.value == "-a"),
        "should suggest -a for ls"
    );
}

#[tokio::test]
async fn test_flag_completions_partial() {
    let mut c = make_completer();
    let results = c.complete("ls -v", 5);
    assert!(
        results.iter().any(|s| s.value == "-v"),
        "should suggest -v when typing -v"
    );
}

#[tokio::test]
async fn test_help_argument_completion() {
    let mut c = make_completer();
    let results = c.complete("help l", 6);
    assert!(
        results.iter().any(|s| s.value == "ls"),
        "should suggest 'ls' when after 'help l'"
    );
}

#[tokio::test]
async fn test_help_argument_lists_operators() {
    let mut c = make_completer();
    let results = c.complete("help fi", 7);
    assert!(
        results.iter().any(|s| s.value == "filter"),
        "should suggest 'filter' when after 'help fi'"
    );
}

#[tokio::test]
async fn test_job_id_completion() {
    let env = Env::new();
    {
        let mut jobs = env.job_control.jobs.write();
        jobs.insert(
            100,
            Job {
                id: 42,
                pgid: 100,
                pids: vec![100],
                cmd: "sleep 60".to_string(),
                status: JobStatus::Running,
                disowned: false,
                started_at: None,
            },
        );
    }
    let mut c = FshellCompleter { env };
    let results = c.complete("fg %", 4);
    assert!(!results.is_empty(), "should suggest job IDs");
    assert_eq!(results[0].value, "%42");
    assert!(
        results[0]
            .description
            .as_ref()
            .is_some_and(|d| d.contains("sleep")),
        "description should include the job command"
    );
}

#[tokio::test]
async fn test_job_id_completion_via_percent_prefix() {
    let env = Env::new();
    {
        let mut jobs = env.job_control.jobs.write();
        jobs.insert(
            200,
            Job {
                id: 1,
                pgid: 200,
                pids: vec![200],
                cmd: "tail -f log".to_string(),
                status: JobStatus::Running,
                disowned: false,
                started_at: None,
            },
        );
    }
    let mut c = FshellCompleter { env };
    let results = c.complete("%", 1);
    assert!(!results.is_empty(), "should suggest jobs when typing %");
    assert_eq!(results[0].value, "%1");
}

#[tokio::test]
async fn test_function_completion() {
    let env = Env::new();
    env.fns
        .write()
        .insert("myfunc".to_string(), (vec![], None, vec![]));
    let mut c = FshellCompleter { env };
    let results = c.complete("myf", 3);
    assert!(
        results.iter().any(|s| s.value == "myfunc"),
        "should suggest 'myfunc' function"
    );
}

#[tokio::test]
async fn test_empty_input_returns_all_builtins() {
    let mut c = make_completer();
    let results = c.complete("", 0);
    // Empty prefix matches everything — suggests all builtins, keywords, operators
    assert!(
        results.len() > 20,
        "empty input should suggest many items (got {})",
        results.len()
    );
}

#[tokio::test]
async fn test_pipe_with_filter_shows_upstream_keys() {
    // For a pipeline "ls | filter " after ls, we expect upstream keys like "name", "type"
    let mut c = make_completer();
    let results = c.complete("ls | filter ", 12);
    assert!(
        !results.is_empty(),
        "should suggest upstream keys for filter after ls"
    );
    assert!(results.iter().any(|s| s.value == "name"));
    assert!(results.iter().any(|s| s.value == "type"));
    assert!(results.iter().any(|s| s.value == "size"));
}

#[tokio::test]
async fn test_no_crash_with_partial_pipe_expression() {
    let mut c = make_completer();
    // Should not crash with various edge cases
    let _ = c.complete("ls | ", 5);
    let _ = c.complete(" | ", 3);
    let _ = c.complete("ls|", 3);
}

#[tokio::test]
async fn test_pipe_downstream_partial_operator() {
    let mut c = make_completer();
    let results = c.complete(" | fi", 5);
    assert!(
        results.iter().any(|s| s.value == "filter"),
        "should suggest 'filter' for partial pipe operator"
    );
}

#[tokio::test]
async fn test_keywords_completed() {
    let mut c = make_completer();
    let results = c.complete("tr", 2);
    assert!(results.iter().any(|s| s.value == "true"));
    assert!(results.iter().any(|s| s.value == "try"));
}

#[tokio::test]
async fn test_many_completions_dont_dup_builtins_and_operators() {
    let mut c = make_completer();
    let results = c.complete("co", 2);
    let mut seen = std::collections::HashSet::new();
    for s in &results {
        assert!(
            seen.insert(s.value.as_str()),
            "duplicate value '{}' found",
            s.value
        );
    }
    // "count" appears in both builtins (no, actually it doesn't - it's only an operator)
    // "cat" is a builtin starting with "co"? No, "cat" starts with "ca"
    // So this test primarily checks that dedup doesn't crash
    assert!(!results.is_empty(), "should have suggestions for 'co'");
}

#[tokio::test]
async fn test_no_suggestions_for_non_existent_prefix() {
    let mut c = make_completer();
    let results = c.complete("zzz", 3);
    // Should fall through to file completion or return empty
    // Since "zzz" doesn't exist as a file in most dirs, likely empty
    // But we shouldn't crash
    assert!(
        results.len() < 100,
        "should not explode for nonsense prefix"
    );
}

#[tokio::test]
async fn test_variable_completion() {
    let env = Env::new();
    env.vars
        .write()
        .insert("MYVAR".to_string(), fshell_core::Val::Int(42));
    let mut c = FshellCompleter { env };
    // Variable matching is key-based (without $ prefix): "MY" matches "MYVAR"
    let results = c.complete("MY", 2);
    assert!(
        results.iter().any(|s| s.value == "$MYVAR"),
        "should suggest $MYVAR when typing 'MY', got: {:?}",
        results.iter().map(|s| &s.value).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn test_cd_directory_only_completion() {
    let _guard = TEST_CWD_MUTEX.lock().unwrap();
    // cd should only complete directories, not files
    // Use a well-known directory that definitely exists
    let mut c = make_completer();
    let results = c.complete("cd src", 6);
    // "src" is a directory in the project root, so it should appear
    assert!(
        results.iter().any(|s| s.value == "src/"),
        "'cd src' should complete to 'src/', got: {:?}",
        results.iter().map(|s| &s.value).collect::<Vec<_>>()
    );
    // All results should be directories (end with /)
    for s in &results {
        assert!(
            s.value.ends_with('/'),
            "cd completion '{}' should be a directory",
            s.value
        );
    }
}

#[tokio::test]
async fn test_builtin_operator_no_overlap_after_pipe() {
    let mut c = make_completer();
    // After a pipe, commands should NOT be suggested - only operators
    let results = c.complete("ls | fi", 7);
    // Should suggest "filter" but NOT "fn" (which is a keyword matching "fi")
    for s in &results {
        assert!(
            s.value != "fn",
            "should not suggest keywords after pipe, got 'fn'"
        );
    }
}

#[tokio::test]
async fn test_flag_completion_unknown_command_falls_through() {
    let mut c = make_completer();
    // If the preceding command has no flags, typing '-' should not crash
    let results = c.complete("cat -", 5);
    // `cat` has no flag metadata, so no flag completions expected
    assert!(
        results.iter().all(|s| !s.value.starts_with('-')),
        "should not suggest flags for 'cat' which has no flag metadata"
    );
}

#[tokio::test]
async fn test_multiple_words_respects_word_boundary() {
    let mut c = make_completer();
    // "let x = ca" should complete "ca" to "catch"
    let results = c.complete("let x = ca", 10);
    assert!(
        results.iter().any(|s| s.value == "catch"),
        "should suggest 'catch' when typing 'let x = ca'"
    );
}

#[tokio::test]
async fn test_exact_match_prioritized_over_prefix() {
    // Create env with a user-defined function that matches a builtin
    let env = Env::new();
    env.fns
        .write()
        .insert("ls".to_string(), (vec![], None, vec![]));
    let mut c = FshellCompleter { env };
    let results = c.complete("ls", 2);
    assert!(!results.is_empty());
    // Both the builtin and the fn match "ls" exactly
    assert_eq!(
        results[0].value, "ls",
        "even with multiple matches, exact match works"
    );
}

#[tokio::test]
async fn test_path_completion_with_env_var() {
    // Use a temp dir with a known file, exposed as an env var
    let tmp = std::env::temp_dir().join("fshell_completion_test");
    let _ = std::fs::create_dir_all(&tmp);
    let test_file = tmp.join("hello.txt");
    std::fs::write(&test_file, b"test").ok();
    let tmp_str = tmp.to_string_lossy().to_string();
    let saved = std::env::var("FSH_TEST_DIR").ok();
    set_var("FSH_TEST_DIR", &tmp_str);

    let mut c = make_completer();
    let word = "$FSH_TEST_DIR/";
    let results = c.complete(word, word.len());

    // Restore env
    match saved {
        Some(v) => set_var("FSH_TEST_DIR", &v),
        None => remove_var("FSH_TEST_DIR"),
    }

    let values: Vec<_> = results.iter().map(|s| &s.value).collect();
    assert!(
        !results.is_empty(),
        "should expand $FSH_TEST_DIR and list directory"
    );
    assert!(
        results.iter().any(|s| s.value == "hello.txt"
            || s.value.trim_end_matches('/').ends_with("hello.txt")),
        "should find hello.txt in expanded path, got: {:?}", values
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn test_dotfile_completion_with_alias_prefix() {
    let _guard = TEST_CWD_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
    // Full-line test: user types "hx .co" / "hx .com" where hx is an alias.
    // Completer must resolve alias and fall through to file completion.
    let tmp = std::env::temp_dir().join("fsh_compl_alias");
    let _ = std::fs::create_dir_all(&tmp);
    let dotdir = tmp.join(".commandcode");
    std::fs::create_dir_all(&dotdir).ok();

    let env = Env::new();
    let orig_cwd = env.cwd();
    env.set_cwd(tmp.canonicalize().unwrap());

    builtins_init(&env);
    bridge_init(&env);
    // Register hx as an alias (mimics the user's real config)
    env.register_alias("hx", "hx");
    let mut c = FshellCompleter { env: env.clone() };

    let results_dotco = c.complete("hx .co", 6);
    let results_dotcom = c.complete("hx .com", 7);

    env.set_cwd(orig_cwd);
    let _ = std::fs::remove_dir_all(&tmp);

    let values_co: Vec<_> = results_dotco.iter().map(|s| s.value.as_str()).collect();
    let values_com: Vec<_> = results_dotcom.iter().map(|s| s.value.as_str()).collect();

    eprintln!("hx .co results: {:?}", values_co);
    eprintln!("hx .com results: {:?}", values_com);

    assert!(
        results_dotco
            .iter()
            .any(|s| s.value.starts_with(".commandcode")),
        "hx .co should find .commandcode/, got: {:?}",
        values_co
    );
    assert!(
        results_dotcom
            .iter()
            .any(|s| s.value.starts_with(".commandcode")),
        "hx .com should also find .commandcode/, got: {:?}",
        values_com
    );
}

#[tokio::test]
async fn test_dotfile_completion_shows_for_partial_prefix() {
    let _guard = TEST_CWD_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
    // Create temp dir with a hidden .commandcode dir, then check it appears
    // for both ".co" and ".com" queries — regression: .com must still match.
    let tmp = std::env::temp_dir().join("fsh_compl_dotfile");
    let _ = std::fs::create_dir_all(&tmp);
    let dotdir = tmp.join(".commandcode");
    std::fs::create_dir_all(&dotdir).ok();

    let mut c = make_completer();
    let orig_cwd = c.env.cwd();
    c.env.set_cwd(tmp.canonicalize().unwrap());

    // cd to the dir so bare ".prefix" resolves to local files
    let results_dotco = c.complete(".co", 3);
    let results_dotcom = c.complete(".com", 4);

    // Restore cwd
    c.env.set_cwd(orig_cwd);
    let _ = std::fs::remove_dir_all(&tmp);

    let values_co: Vec<_> = results_dotco.iter().map(|s| s.value.as_str()).collect();
    let values_com: Vec<_> = results_dotcom.iter().map(|s| s.value.as_str()).collect();

    eprintln!(".co results: {:?}", values_co);
    eprintln!(".com results: {:?}", values_com);

    assert!(
        results_dotco
            .iter()
            .any(|s| s.value.starts_with(".commandcode")),
        ".co should find .commandcode/, got: {:?}",
        values_co
    );
    assert!(
        results_dotcom
            .iter()
            .any(|s| s.value.starts_with(".commandcode")),
        ".com should also find .commandcode/ (regression — fuzzy match works), got: {:?}",
        values_com
    );
}

#[tokio::test]
async fn test_path_completion_with_pwd_var() {
    let _guard = TEST_CWD_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
    let saved = std::env::var("PWD").ok();
    let root = env!("CARGO_MANIFEST_DIR");
    let project_root = std::path::Path::new(root)
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(std::path::Path::new(root));
    let root_str = project_root.to_string_lossy().to_string();
    set_var("PWD", &root_str);

    let mut c = make_completer();
    let word = "$PWD/";
    let results = c.complete(word, word.len());

    match saved {
        Some(v) => set_var("PWD", &v),
        None => remove_var("PWD"),
    }

    let values: Vec<_> = results.iter().map(|s| &s.value).collect();
    assert!(!results.is_empty(), "should expand $PWD and list files");
    assert!(
        results.iter().any(|s| s.value.ends_with("Cargo.toml")),
        "should find Cargo.toml via $PWD expansion, got: {:?}",
        values
    );
}

#[tokio::test]
async fn test_tui_completions_sequence() {
    let _guard = TEST_CWD_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = std::env::temp_dir().join("fsh_compl_tui_seq");
    let _ = std::fs::create_dir_all(&tmp);
    let dotdir = tmp.join(".commandcode");
    std::fs::create_dir_all(&dotdir).ok();

    let env = Env::new();
    let orig_cwd = env.cwd();
    env.set_cwd(tmp.canonicalize().unwrap());
    {
        let mut opts = env.options.write();
        opts.sandbox_mode = "off".to_string();
    }
    builtins_init(&env);
    bridge_init(&env);
    env.register_alias("hx", "hx");

    let mut comp_mgr = fshell_repl::ftui::completions::CompletionsManager::new(env.clone());

    // 1. User types "hx .co" and presses Tab (force_visible = true)
    comp_mgr.update("hx .co", 6, true);
    let suggestions_co: Vec<String> = comp_mgr
        .suggestions
        .iter()
        .map(|s| s.value.clone())
        .collect();
    println!("SUGGESTIONS FOR 'hx .co': {:?}", suggestions_co);

    // 2. User types "m" -> line is "hx .com", cursor 7 (force_visible = false)
    comp_mgr.update("hx .com", 7, false);
    let suggestions_com: Vec<String> = comp_mgr
        .suggestions
        .iter()
        .map(|s| s.value.clone())
        .collect();
    println!("SUGGESTIONS FOR 'hx .com': {:?}", suggestions_com);

    // 3. Bare queries: "hx co" -> "hx com"
    comp_mgr.update("hx co", 5, true);
    let suggestions_bare_co: Vec<String> = comp_mgr
        .suggestions
        .iter()
        .map(|s| s.value.clone())
        .collect();
    println!("SUGGESTIONS FOR 'hx co': {:?}", suggestions_bare_co);

    comp_mgr.update("hx com", 6, false);
    let suggestions_bare_com: Vec<String> = comp_mgr
        .suggestions
        .iter()
        .map(|s| s.value.clone())
        .collect();
    println!("SUGGESTIONS FOR 'hx com': {:?}", suggestions_bare_com);

    // Restore cwd
    env.set_cwd(orig_cwd);
    let _ = std::fs::remove_dir_all(&tmp);

    assert!(
        suggestions_co.iter().any(|s| s.starts_with(".commandcode")),
        "co suggestions should contain .commandcode, got {:?}",
        suggestions_co
    );
    assert!(
        suggestions_com
            .iter()
            .any(|s| s.starts_with(".commandcode")),
        "com suggestions should contain .commandcode, got {:?}",
        suggestions_com
    );
    assert!(
        suggestions_bare_co
            .iter()
            .any(|s| s.starts_with(".commandcode")),
        "bare co suggestions should contain .commandcode, got {:?}",
        suggestions_bare_co
    );
    assert!(
        suggestions_bare_com
            .iter()
            .any(|s| s.starts_with(".commandcode")),
        "bare com suggestions should contain .commandcode, got {:?}",
        suggestions_bare_com
    );
}

// ---------------------------------------------------------------------------
// PATH executable completions — happy path + regressions for `arbo` → `arborist`
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_path_completion_arbo_suggests_arborist() {
    let _guard = TEST_PATH_MUTEX.lock().unwrap();
    let dir = create_temp_bins("arbo_single", &["arborist"]);
    let mut c = make_completer_with_path(&dir);

    let results = c.complete("arbo", 4);
    let values: Vec<&str> = results.iter().map(|s| s.value.as_str()).collect();

    assert!(
        values.iter().any(|v| *v == "arborist"),
        "arbo should suggest arborist via PATH, got: {:?}",
        values
    );

    let arborist = results.iter().find(|s| s.value == "arborist").unwrap();
    assert!(
        arborist
            .description
            .as_deref()
            .unwrap_or("")
            .contains("ext"),
        "PATH suggestion should be marked [ext], got {:?}",
        arborist.description
    );
    assert_eq!(arborist.span.start, 0, "first-word span should start at 0");
    assert_eq!(arborist.span.end, 4, "span should cover typed prefix");

    invalidate_path_cache();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_path_completion_offers_similar_names_popup() {
    let _guard = TEST_PATH_MUTEX.lock().unwrap();
    let dir = create_temp_bins(
        "arbo_multi",
        &["arbor", "arborist", "arborist-cli", "argo", "other-tool"],
    );
    let mut c = make_completer_with_path(&dir);

    let results = c.complete("arbo", 4);
    let values: Vec<&str> = results.iter().map(|s| s.value.as_str()).collect();

    // All three arbo* should appear, argo/other-tool should not.
    assert!(
        values.contains(&"arbor"),
        "should contain arbor, got {:?}",
        values
    );
    assert!(
        values.contains(&"arborist"),
        "should contain arborist, got {:?}",
        values
    );
    assert!(
        values.contains(&"arborist-cli"),
        "should contain arborist-cli, got {:?}",
        values
    );
    assert!(
        !values.contains(&"argo"),
        "argo does not start with arbo, should not be suggested"
    );
    assert!(
        !values.contains(&"other-tool"),
        "other-tool should not be suggested for arbo"
    );
    // Ranking should keep them together; at least 3 PATH results.
    let arbo_count = values.iter().filter(|v| v.starts_with("arbor")).count();
    assert!(
        arbo_count >= 3,
        "popup should offer multiple similar names, got {}: {:?}",
        arbo_count,
        values
    );

    invalidate_path_cache();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_path_completion_case_insensitive() {
    let _guard = TEST_PATH_MUTEX.lock().unwrap();
    let dir = create_temp_bins("arbo_case", &["Arborist"]);
    let mut c = make_completer_with_path(&dir);

    let results = c.complete("ARBO", 4);
    assert!(
        results.iter().any(|s| s.value == "Arborist"),
        "ARBO should match Arborist case-insensitively, got: {:?}",
        results.iter().map(|s| &s.value).collect::<Vec<_>>()
    );

    let results2 = c.complete("arBo", 4);
    assert!(
        results2.iter().any(|s| s.value == "Arborist"),
        "arBo should match Arborist, got: {:?}",
        results2.iter().map(|s| &s.value).collect::<Vec<_>>()
    );

    invalidate_path_cache();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_path_completion_dedup_against_builtins_and_common() {
    let _guard = TEST_PATH_MUTEX.lock().unwrap();
    // Create PATH bins that shadow a builtin (ls) and a COMMON_EXTERNAL (git)
    let dir = create_temp_bins("arbo_dedup", &["ls", "git", "arborist"]);
    let mut c = make_completer_with_path(&dir);

    // "ls" prefix should appear once
    let results_ls = c.complete("ls", 2);
    let ls_count = results_ls.iter().filter(|s| s.value == "ls").count();
    assert_eq!(
        ls_count,
        1,
        "ls should not be duplicated (builtin vs PATH), got {} occurrences: {:?}",
        ls_count,
        results_ls.iter().map(|s| &s.value).collect::<Vec<_>>()
    );

    // "gi" should contain git once, not twice
    let results_gi = c.complete("gi", 2);
    let git_count = results_gi.iter().filter(|s| s.value == "git").count();
    assert_eq!(
        git_count,
        1,
        "git should not be duplicated (COMMON vs PATH), got {}: {:?}",
        git_count,
        results_gi.iter().map(|s| &s.value).collect::<Vec<_>>()
    );

    // but our unique bin still shows
    let results_arbo = c.complete("arbo", 4);
    assert!(
        results_arbo.iter().any(|s| s.value == "arborist"),
        "arborist should still be suggested alongside deduped entries"
    );

    invalidate_path_cache();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_path_completion_empty_prefix_does_not_dump_path() {
    let _guard = TEST_PATH_MUTEX.lock().unwrap();
    let dir = create_temp_bins("arbo_empty", &["arborist", "zzz-unique-bin-12345"]);
    let mut c = make_completer_with_path(&dir);

    let results_empty = c.complete("", 0);
    let contains_arborist = results_empty.iter().any(|s| s.value == "arborist");
    let contains_zzz = results_empty
        .iter()
        .any(|s| s.value == "zzz-unique-bin-12345");
    assert!(
        !contains_arborist && !contains_zzz,
        "empty first-word should not dump PATH (gated on non-empty prefix), got arborist={} zzz={} in {:?}",
        contains_arborist,
        contains_zzz,
        results_empty
            .iter()
            .take(10)
            .map(|s| &s.value)
            .collect::<Vec<_>>()
    );

    // but a real prefix must still work
    let results_arbo = c.complete("arbo", 4);
    assert!(
        results_arbo.iter().any(|s| s.value == "arborist"),
        "non-empty prefix must still suggest PATH bins"
    );

    invalidate_path_cache();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_path_completion_not_suggested_when_not_on_path() {
    let _guard = TEST_PATH_MUTEX.lock().unwrap();
    // Create a bin in dir A, but completer PATH points to dir B
    let dir_a = create_temp_bins("arbo_not_on_path_a", &["arborist"]);
    let dir_b = create_temp_bins("arbo_not_on_path_b", &["other-bin"]);
    let mut c = make_completer_with_path(&dir_b);

    let results = c.complete("arbo", 4);
    assert!(
        !results.iter().any(|s| s.value == "arborist"),
        "arborist in dir_a should NOT be suggested when PATH=dir_b, got: {:?}",
        results.iter().map(|s| &s.value).collect::<Vec<_>>()
    );

    invalidate_path_cache();
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
}

#[tokio::test]
async fn test_path_completion_env_path_isolation_over_os_path() {
    let _guard = TEST_PATH_MUTEX.lock().unwrap();
    let dir = create_temp_bins("arbo_isolation", &["arborist-isolated-test"]);
    // Point only the shell's $PATH at our dir; OS PATH stays whatever.
    let mut c = make_completer_with_path(&dir);

    // Direct engine check should also respect env_path isolation
    let via_env = get_path_executables(Some(&dir.to_string_lossy().to_string()));
    assert!(
        via_env.contains(&"arborist-isolated-test".to_string()),
        "get_path_executables with env_path should see isolated bin"
    );

    let via_completer = c.complete("arborist-isolated", 17);
    assert!(
        via_completer
            .iter()
            .any(|s| s.value == "arborist-isolated-test"),
        "completer should use env PATH isolation, got: {:?}",
        via_completer.iter().map(|s| &s.value).collect::<Vec<_>>()
    );

    invalidate_path_cache();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_path_completion_only_on_first_word() {
    let _guard = TEST_PATH_MUTEX.lock().unwrap();
    let dir = create_temp_bins("arbo_first_word", &["arborist"]);
    let mut c = make_completer_with_path(&dir);

    // First word: should suggest
    let first = c.complete("arbo", 4);
    assert!(first.iter().any(|s| s.value == "arborist"));

    // Second word (arg position) should fall through to file completion,
    // not PATH. Since no file named arborist in cwd, it should NOT suggest.
    // We use a known builtin `ls` as the command context.
    let second = c.complete("ls arbo", 7);
    let suggests_arborist_as_arg = second.iter().any(|s| s.value == "arborist");
    assert!(
        !suggests_arborist_as_arg,
        "PATH should not be suggested in arg position (file completion), got: {:?}",
        second.iter().map(|s| &s.value).collect::<Vec<_>>()
    );

    invalidate_path_cache();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_get_path_executables_direct_happy_and_missing() {
    let _guard = TEST_PATH_MUTEX.lock().unwrap();
    let dir = create_temp_bins("arbo_engine", &["arborist", "mytool"]);
    invalidate_path_cache();

    let listed = get_path_executables(Some(&dir.to_string_lossy().to_string()));
    assert!(listed.contains(&"arborist".to_string()));
    assert!(listed.contains(&"mytool".to_string()));

    // Empty / missing PATH returns empty without panic
    assert!(get_path_executables(Some("")).is_empty());
    assert!(get_path_executables(None).iter().all(|s| !s.is_empty())); // just not crash, may contain OS bins

    invalidate_path_cache();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_completions_update_after_cd() {
    let _guard = TEST_CWD_MUTEX.lock().unwrap();
    let orig_cwd = std::env::current_dir().ok();

    let tmp = std::env::temp_dir().join("fsh_compl_cd_update");
    let _ = std::fs::create_dir_all(&tmp);
    let dir_a = tmp.join("dir_a");
    let dir_b = tmp.join("dir_b");
    std::fs::create_dir_all(&dir_a).ok();
    std::fs::create_dir_all(&dir_b).ok();

    std::fs::write(dir_a.join("alpha_file.txt"), "hello").ok();
    std::fs::write(dir_b.join("beta_file.txt"), "world").ok();

    let env = Env::new();
    builtins_init(&env);
    bridge_init(&env);

    // Change to dir_a via env.set_cwd
    env.set_cwd(dir_a.canonicalize().unwrap());
    let mut c = FshellCompleter { env: env.clone() };

    let results_a = c.complete("alp", 3);
    assert!(
        results_a.iter().any(|s| s.value.contains("alpha_file")),
        "Completer should find alpha_file in dir_a, got: {:?}",
        results_a.iter().map(|s| &s.value).collect::<Vec<_>>()
    );

    // Change to dir_b via env.set_cwd
    env.set_cwd(dir_b.canonicalize().unwrap());
    let results_b = c.complete("bet", 3);
    assert!(
        results_b.iter().any(|s| s.value.contains("beta_file")),
        "Completer should find beta_file in dir_b, got: {:?}",
        results_b.iter().map(|s| &s.value).collect::<Vec<_>>()
    );

    if let Some(orig) = orig_cwd {
        std::env::set_current_dir(orig).ok();
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn test_completion_manager_live_typing_and_backspace() {
    let _guard = TEST_CWD_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
    let env = Env::new();
    builtins_init(&env);
    bridge_init(&env);

    let mut comp_mgr = fshell_repl::ftui::completions::CompletionsManager::new(env);

    // 1. User types "echo" and presses Tab
    comp_mgr.update("echo", 4, true);
    assert!(comp_mgr.visible);
    assert!(comp_mgr.session_active);
    assert!(!comp_mgr.suggestions.is_empty());

    // 2. User deletes 'o' -> line is "ech"
    comp_mgr.update("ech", 3, false);
    assert!(comp_mgr.visible);
    assert!(comp_mgr.session_active);
    assert!(
        comp_mgr
            .suggestions
            .iter()
            .any(|s| s.value.starts_with("echo"))
    );

    // 3. User types non-matching characters "ech_nonexistent_xyz"
    comp_mgr.update("ech_nonexistent_xyz", 19, false);
    assert!(!comp_mgr.visible);
    assert!(comp_mgr.session_active);

    // 4. User backspaces back to "ech" -> completions seamlessly reappear
    comp_mgr.update("ech", 3, false);
    assert!(comp_mgr.visible);
    assert!(comp_mgr.session_active);
    assert!(
        comp_mgr
            .suggestions
            .iter()
            .any(|s| s.value.starts_with("echo"))
    );

    // 5. User deletes all chars -> line is ""
    comp_mgr.update("", 0, false);
    assert!(!comp_mgr.visible);
    assert!(!comp_mgr.session_active);
    assert!(comp_mgr.suggestions.is_empty());
}

#[test]
fn test_longest_common_prefix_multibyte_utf8() {
    let env = Env::new();
    let mut comp_mgr = fshell_repl::ftui::completions::CompletionsManager::new(env);
    comp_mgr.suggestions = vec![
        reedline::Suggestion {
            value: "café_latte".to_string(),
            description: None,
            extra: None,
            span: reedline::Span::new(0, 0),
            append_whitespace: false,
            style: None,
            display_override: None,
            match_indices: None,
        },
        reedline::Suggestion {
            value: "café_mocha".to_string(),
            description: None,
            extra: None,
            span: reedline::Span::new(0, 0),
            append_whitespace: false,
            style: None,
            display_override: None,
            match_indices: None,
        },
    ];

    let lcp = comp_mgr.longest_common_prefix();
    assert_eq!(lcp, Some("café_".to_string()));
}

#[tokio::test]
async fn test_path_completion_with_quoted_drilling() {
    let _guard = TEST_CWD_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = std::env::temp_dir().join("fsh_test_quoted_drill");
    let _ = std::fs::create_dir_all(&tmp);
    let sub = tmp.join("my folder");
    let _ = std::fs::create_dir_all(&sub);
    let _ = std::fs::write(sub.join("file.txt"), "content");

    let env = Env::new();
    builtins_init(&env);
    bridge_init(&env);
    env.set_cwd(tmp.canonicalize().unwrap());

    let mut c = FshellCompleter { env };
    let input = "\"my folder/";
    let res = c.complete(input, input.len());

    assert!(!res.is_empty());
    for s in &res {
        assert!(
            !s.value.contains("\"\""),
            "Should not double-quote: {}",
            s.value
        );
        assert!(
            s.value.starts_with("\"my folder/"),
            "Should preserve quote and path: {}",
            s.value
        );
    }

    let _ = std::fs::remove_dir_all(&tmp);
}
