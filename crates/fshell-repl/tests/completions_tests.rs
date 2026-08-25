// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_bridge::init as bridge_init;
use fshell_builtins::init as builtins_init;
use fshell_core::{remove_var, set_var};
use fshell_engine::{Env, Job, JobStatus};
use fshell_repl::FshellCompleter;
use reedline::Completer;
use std::sync::{LazyLock, Mutex};

static TEST_CWD_MUTEX: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

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
    let _guard = TEST_CWD_MUTEX.lock().unwrap();
    // Full-line test: user types "hx .co" / "hx .com" where hx is an alias.
    // Completer must resolve alias and fall through to file completion.
    let tmp = std::env::temp_dir().join("fsh_compl_alias");
    let _ = std::fs::create_dir_all(&tmp);
    let dotdir = tmp.join(".commandcode");
    std::fs::create_dir_all(&dotdir).ok();

    let cwd = std::env::current_dir().ok();
    std::env::set_current_dir(&tmp).ok();

    let env = Env::new();
    builtins_init(&env);
    bridge_init(&env);
    // Register hx as an alias (mimics the user's real config)
    env.register_alias("hx", "hx");
    let mut c = FshellCompleter { env };

    let results_dotco = c.complete("hx .co", 6);
    let results_dotcom = c.complete("hx .com", 7);

    if let Some(d) = cwd {
        std::env::set_current_dir(d).ok();
    }
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
    let _guard = TEST_CWD_MUTEX.lock().unwrap();
    // Create temp dir with a hidden .commandcode dir, then check it appears
    // for both ".co" and ".com" queries — regression: .com must still match.
    let tmp = std::env::temp_dir().join("fsh_compl_dotfile");
    let _ = std::fs::create_dir_all(&tmp);
    let dotdir = tmp.join(".commandcode");
    std::fs::create_dir_all(&dotdir).ok();

    let cwd = std::env::current_dir().ok();
    std::env::set_current_dir(&tmp).ok();

    let mut c = make_completer();

    // cd to the dir so bare ".prefix" resolves to local files
    let results_dotco = c.complete(".co", 3);
    let results_dotcom = c.complete(".com", 4);

    // Restore cwd
    if let Some(d) = cwd {
        std::env::set_current_dir(d).ok();
    }
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
    let _guard = TEST_CWD_MUTEX.lock().unwrap();
    // $PWD is always set and points to a real directory with known content
    let mut c = make_completer();
    // The current directory is the project root - $PWD/Cargo.toml should exist
    let word = "$PWD/";
    let results = c.complete(word, word.len());
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
    let _guard = TEST_CWD_MUTEX.lock().unwrap();
    let tmp = std::env::temp_dir().join("fsh_compl_tui_seq");
    let _ = std::fs::create_dir_all(&tmp);
    let dotdir = tmp.join(".commandcode");
    std::fs::create_dir_all(&dotdir).ok();

    let cwd = std::env::current_dir().ok();
    std::env::set_current_dir(&tmp).ok();

    let env = Env::new();
    {
        let mut opts = env.options.write();
        opts.sandbox_mode = "off".to_string();
    }
    builtins_init(&env);
    bridge_init(&env);
    env.register_alias("hx", "hx");

    let mut comp_mgr = fshell_repl::ftui::completions::CompletionsManager::new(env);

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
    if let Some(d) = cwd {
        std::env::set_current_dir(d).ok();
    }
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
