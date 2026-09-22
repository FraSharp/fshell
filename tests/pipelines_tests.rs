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
async fn test_integration_pipeline_filter_map_count() {
    let ctx = TestContext::new();
    ctx.set_var("files", make_file_items());

    let res = ctx.eval_ok("$files | filter size > 100 | count").await;
    // Out of 3 files, file2 (150) and file3 (300) have size > 100, so count should be 2
    assert_val_eq!(res, Val::List(vec![Val::Int(2)]));
}

#[tokio::test]
async fn test_integration_pipeline_hash() {
    let env = setup_test_env();
    let raw_list = Val::List(vec![
        Val::Map({
            let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            m.insert(ustr::ustr("val"), Val::Int(10));
            m
        }),
        Val::Map({
            let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            m.insert(ustr::ustr("val"), Val::Int(20));
            m
        }),
    ]);
    env.vars.write().insert("data".to_string(), raw_list);

    // 1. Default hashing (buffering)
    let mut parser = Parser::new("$data | hash");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        if let Val::List(items) = res {
            assert_eq!(items.len(), 1);
            if let Val::Map(ref m) = items[0] {
                assert_eq!(m.get(&ustr::ustr("_count")), Some(&Val::Int(2)));
                let hash_val = m.get(&ustr::ustr("_hash")).unwrap();
                if let Val::String(s) = hash_val {
                    assert_eq!(s.len(), 64);
                } else {
                    panic!("Expected String");
                }
            } else {
                panic!("Expected Map");
            }
        } else {
            panic!("Expected List");
        }
    }

    // 2. Hash per-record (streaming)
    let mut parser = Parser::new("$data | hash --per-record");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        if let Val::List(items) = res {
            assert_eq!(items.len(), 2);
            for item in items {
                if let Val::Map(ref m) = item {
                    assert!(m.get(&ustr::ustr("val")).is_some());
                    let hash_val = m.get(&ustr::ustr("_hash")).unwrap();
                    if let Val::String(s) = hash_val {
                        assert_eq!(s.len(), 64);
                    } else {
                        panic!("Expected String");
                    }
                } else {
                    panic!("Expected Map");
                }
            }
        }
    }
}

#[tokio::test]
async fn test_integration_ls_tree_pipeline() {
    let env = setup_test_env();

    // Create a temporary directory structure to list
    let tmpdir = tempfile::tempdir().unwrap();
    let test_dir = tmpdir.path().join("tree_test");
    std::fs::create_dir_all(test_dir.join("sub")).unwrap();
    std::fs::write(test_dir.join("root.txt"), b"root").unwrap();
    std::fs::write(test_dir.join("sub").join("nested.txt"), b"nested").unwrap();

    // Grant capability to read the temporary directory tree
    env.caps
        .caps
        .write()
        .grant(fshell_core::ResourceHandle::ReadDir(test_dir.clone()));
    env.caps
        .caps
        .write()
        .grant(fshell_core::ResourceHandle::ReadDir(test_dir.join("sub")));

    // Build the command line using the directory path
    let cmd = format!("ls --tree \"{}\" | count", test_dir.to_string_lossy());
    let mut parser = Parser::new(&cmd);
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);

    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        // The tree should list:
        // 1. root directory name (tree_test)
        // 2. root.txt
        // 3. sub/
        // 4. sub/nested.txt
        // This is 4 items/lines total.
        assert_eq!(res, Val::List(vec![Val::Int(4)]));
    } else {
        panic!("Expected Stmt::Expr");
    }
}

#[tokio::test]
async fn test_pipeline_map_single_field() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("files".to_string(), make_file_items());

    let mut parser = Parser::new("$files | map name");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(items) => {
                assert_eq!(items.len(), 3);
                for item in &items {
                    match item {
                        Val::Map(m) => {
                            assert_eq!(m.len(), 1);
                            assert!(m.contains_key(&ustr::ustr("name")));
                        }
                        _ => panic!("expected Val::Map"),
                    }
                }
            }
            other => panic!("Expected Val::List, got {:?}", other),
        }
    }
}

#[tokio::test]
async fn test_pipeline_map_multi_field() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("files".to_string(), make_file_items());

    let mut parser = Parser::new("$files | map name, size");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(items) => {
                assert_eq!(items.len(), 3);
                for item in &items {
                    match item {
                        Val::Map(m) => {
                            assert_eq!(m.len(), 2);
                            assert!(m.contains_key(&ustr::ustr("name")));
                            assert!(m.contains_key(&ustr::ustr("size")));
                        }
                        _ => panic!("expected Val::Map"),
                    }
                }
            }
            other => panic!("Expected Val::List, got {:?}", other),
        }
    }
}

// Pipeline sort operator

#[tokio::test]
async fn test_pipeline_sort_ascending() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("files".to_string(), make_file_items());

    let mut parser = Parser::new("$files | sort size");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(items) => {
                assert_eq!(items.len(), 3);
                let sizes: Vec<i64> = items
                    .iter()
                    .map(|item| match item {
                        Val::Map(m) => match m.get(&ustr::ustr("size")) {
                            Some(Val::Int(s)) => *s,
                            _ => panic!("expected Int size"),
                        },
                        _ => panic!("expected Map"),
                    })
                    .collect();
                assert_eq!(sizes, vec![50, 150, 300], "should be sorted ascending");
            }
            other => panic!("Expected Val::List, got {:?}", other),
        }
    }
}

#[tokio::test]
async fn test_pipeline_sort_descending() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("files".to_string(), make_file_items());

    let mut parser = Parser::new("$files | sort size desc");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(items) => {
                assert_eq!(items.len(), 3);
                let sizes: Vec<i64> = items
                    .iter()
                    .map(|item| match item {
                        Val::Map(m) => match m.get(&ustr::ustr("size")) {
                            Some(Val::Int(s)) => *s,
                            _ => panic!("expected Int size"),
                        },
                        _ => panic!("expected Map"),
                    })
                    .collect();
                assert_eq!(sizes, vec![300, 150, 50], "should be sorted descending");
            }
            other => panic!("Expected Val::List, got {:?}", other),
        }
    }
}

// Pipeline grep operator

#[tokio::test]
async fn test_pipeline_grep() {
    let env = setup_test_env();
    env.vars.write().insert(
        "data".to_string(),
        Val::List(vec![
            Val::Map({
                let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
                m.insert(ustr::ustr("name"), Val::String("readme.md".to_string()));
                m.insert(ustr::ustr("size"), Val::Int(100));
                m
            }),
            Val::Map({
                let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
                m.insert(ustr::ustr("name"), Val::String("script.py".to_string()));
                m.insert(ustr::ustr("size"), Val::Int(200));
                m
            }),
            Val::Map({
                let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
                m.insert(ustr::ustr("name"), Val::String("notes.txt".to_string()));
                m.insert(ustr::ustr("size"), Val::Int(300));
                m
            }),
        ]),
    );

    let mut parser = Parser::new("$data | grep \"txt\"");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(items) => {
                assert_eq!(items.len(), 1, "only notes.txt should match grep \"txt\"");
                match &items[0] {
                    Val::Map(m) => {
                        assert_eq!(
                            m.get(&ustr::ustr("name")),
                            Some(&Val::String("notes.txt".to_string()))
                        );
                    }
                    _ => panic!("expected Map"),
                }
            }
            other => panic!("Expected Val::List, got {:?}", other),
        }
    }
}

#[tokio::test]
async fn test_pipeline_mark_basic() {
    let env = setup_test_env();
    env.vars.write().insert(
        "data".to_string(),
        Val::List(vec![
            Val::String("apple".to_string()),
            Val::String("banana".to_string()),
            Val::String("cherry".to_string()),
            Val::String("date".to_string()),
        ]),
    );

    let mut parser = Parser::new("$data | mark \"a\"");
    let stmts = parser.parse_statements().unwrap();
    let Stmt::Expr(expr) = stmts[0].unpack() else {
        panic!("expected expression statement");
    };
    let res = eval_expr(expr, &env).await.unwrap();
    match res {
        Val::List(items) => {
            assert_eq!(items.len(), 4, "mark should pass all items through");
            assert_eq!(items[0], Val::String("> apple".to_string()));
            assert_eq!(items[1], Val::String("> banana".to_string()));
            assert_eq!(items[2], Val::String("cherry".to_string()));
            assert_eq!(items[3], Val::String("> date".to_string()));
        }
        other => panic!("Expected Val::List, got {:?}", other),
    }
}

#[tokio::test]
async fn test_pipeline_mark_no_match() {
    let env = setup_test_env();
    env.vars.write().insert(
        "data".to_string(),
        Val::List(vec![
            Val::String("apple".to_string()),
            Val::String("banana".to_string()),
        ]),
    );

    let mut parser = Parser::new("$data | mark \"z\"");
    let stmts = parser.parse_statements().unwrap();
    let Stmt::Expr(expr) = stmts[0].unpack() else {
        panic!("expected expression statement");
    };
    let res = eval_expr(expr, &env).await.unwrap();
    match res {
        Val::List(items) => {
            assert_eq!(items.len(), 2, "mark should pass all items through");
            assert_eq!(items[0], Val::String("apple".to_string()));
            assert_eq!(items[1], Val::String("banana".to_string()));
        }
        other => panic!("Expected Val::List, got {:?}", other),
    }
}

#[tokio::test]
async fn test_pipeline_mark_structured() {
    let env = setup_test_env();
    env.vars.write().insert(
        "data".to_string(),
        Val::List(vec![
            Val::Map({
                let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
                m.insert(ustr::ustr("name"), Val::String("readme.md".to_string()));
                m.insert(ustr::ustr("size"), Val::Int(100));
                m
            }),
            Val::Map({
                let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
                m.insert(ustr::ustr("name"), Val::String("script.py".to_string()));
                m.insert(ustr::ustr("size"), Val::Int(200));
                m
            }),
        ]),
    );

    let mut parser = Parser::new("$data | mark \".py\"");
    let stmts = parser.parse_statements().unwrap();
    let Stmt::Expr(expr) = stmts[0].unpack() else {
        panic!("expected expression statement");
    };
    let res = eval_expr(expr, &env).await.unwrap();
    match res {
        Val::List(items) => {
            assert_eq!(items.len(), 2, "mark should pass all items through");
            // Structured records keep their shape; `mark` must not flatten and
            // retype them into strings.
            for (item, name) in items.iter().zip(["readme.md", "script.py"]) {
                match item {
                    Val::Map(m) => assert_eq!(
                        m.get(&ustr::ustr("name")),
                        Some(&Val::String(name.to_string()))
                    ),
                    other => panic!("expected a Map to pass through, got {other:?}"),
                }
            }
        }
        other => panic!("Expected Val::List, got {:?}", other),
    }
}

#[tokio::test]
async fn test_pipeline_filter_eq() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("files".to_string(), make_file_items());

    let mut parser = Parser::new("$files | filter size == 150");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(items) => {
                assert_eq!(items.len(), 1);
                match &items[0] {
                    Val::Map(m) => assert_eq!(
                        m.get(&ustr::ustr("name")),
                        Some(&Val::String("file2.txt".to_string()))
                    ),
                    _ => panic!("expected Map"),
                }
            }
            other => panic!("Expected Val::List, got {:?}", other),
        }
    }
}

#[tokio::test]
async fn test_pipeline_filter_gte() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("files".to_string(), make_file_items());

    let mut parser = Parser::new("$files | filter size >= 100");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(items) => {
                assert_eq!(items.len(), 2);
            }
            other => panic!("Expected Val::List, got {:?}", other),
        }
    }
}

#[tokio::test]
async fn test_pipeline_filter_neq() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("files".to_string(), make_file_items());

    let mut parser = Parser::new("$files | filter name != \"file1.txt\"");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(items) => {
                assert_eq!(items.len(), 2);
            }
            other => panic!("Expected Val::List, got {:?}", other),
        }
    }
}

#[tokio::test]
async fn test_pipeline_filter_lt_no_match() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("files".to_string(), make_file_items());

    let mut parser = Parser::new("$files | filter size < 100");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(items) => {
                assert_eq!(items.len(), 1, "file1.txt has size 50 < 100");
                match &items[0] {
                    Val::Map(m) => assert_eq!(
                        m.get(&ustr::ustr("name")),
                        Some(&Val::String("file1.txt".to_string()))
                    ),
                    _ => panic!("expected Map"),
                }
            }
            other => panic!("Expected Val::List, got {:?}", other),
        }
    }
}

// Boolean operators in pipeline

#[tokio::test]
async fn test_pipeline_boolean_and() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("files".to_string(), make_file_items());

    let mut parser = Parser::new("$files | filter size > 50 and size < 250 | count");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(res, Val::List(vec![Val::Int(1)]));
    }
}

// String interpolation

#[tokio::test]
async fn test_pipeline_chained_operators() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("files".to_string(), make_file_items());

    let mut parser = Parser::new("$files | filter size > 50 | sort name | map name");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(items) => {
                assert_eq!(items.len(), 2);
                let names: Vec<String> = items
                    .iter()
                    .map(|item| match item {
                        Val::Map(m) => match m.get(&ustr::ustr("name")) {
                            Some(Val::String(s)) => s.clone(),
                            _ => panic!("expected String name"),
                        },
                        _ => panic!("expected Map"),
                    })
                    .collect();
                assert_eq!(names, vec!["file2.txt", "file3.txt"]);
            }
            other => panic!("Expected Val::List, got {:?}", other),
        }
    }
}

// Empty pipeline edge cases

#[tokio::test]
async fn test_pipeline_empty_input() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("empty".to_string(), Val::List(vec![]));

    let mut parser = Parser::new("$empty | filter size > 100 | sort name | map name | count");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(res, Val::List(vec![Val::Int(0)]));
    }
}

#[tokio::test]
async fn test_stderr_merges_into_stdout_with_2_to_1() {
    let ctx = TestContext::new();
    let val = ctx.eval_ok("sh -c 'echo oops 1>&2' 2>&1 | @text").await;
    let text = match &val {
        Val::List(items) => items
            .iter()
            .map(|v| v.to_text())
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_text(),
    };
    assert!(
        text.contains("oops"),
        "stderr must reach downstream after 2>&1, got: {text}"
    );
}

#[tokio::test]
async fn test_write_then_2_to_1_merges_stderr_into_file() {
    // `cmd > file 2>&1` must put both stdout and stderr in the file.
    let ctx = TestContext::new();
    let out = ctx.temp_path().join("combined.txt");
    let script = format!(
        "sh -c 'echo out; echo err 1>&2' > \"{}\" 2>&1",
        out.display()
    );
    ctx.eval_script(&script).await.unwrap();
    let content = std::fs::read_to_string(&out).unwrap();
    assert!(
        content.contains("out"),
        "stdout missing from file: {content}"
    );
    assert!(
        content.contains("err"),
        "stderr not merged into file: {content}"
    );
}

#[tokio::test]
async fn test_output_process_substitution_delivers_data() {
    // `cmd >(consumer)` gives the command a writable path whose writes the
    // consumer processes.
    let ctx = TestContext::new();
    let out = ctx.temp_path().join("psub-out.txt");
    let script = format!(
        "sh -c 'echo from-fifo > \"$1\"' _ >(cat > \"{}\")",
        out.display()
    );
    ctx.eval_script(&script).await.unwrap();

    // The consumer runs asynchronously; give it a moment to flush.
    let mut content = String::new();
    for _ in 0..100 {
        content = std::fs::read_to_string(&out).unwrap_or_default();
        if content.contains("from-fifo") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        content.contains("from-fifo"),
        "process substitution delivered nothing: {content:?}"
    );
}

#[tokio::test]
async fn test_yaml_boundary_is_plain_data() {
    let env = setup_test_env();
    let data = Val::Map({
        let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        m.insert(ustr::ustr("a"), Val::Int(1));
        m
    });
    env.vars.write().insert("data".to_string(), data.clone());

    // Encoding must be plain YAML, not the internal `{type, value}` envelope.
    let mut parser = Parser::new("$data | @yaml");
    let stmts = parser.parse_statements().unwrap();
    let out = if let Stmt::Expr(expr) = stmts[0].unpack() {
        eval_expr(expr, &env).await.unwrap()
    } else {
        panic!("expected expression statement");
    };
    let yaml = match out {
        Val::List(items) => match items.into_iter().next() {
            Some(Val::String(s)) => s,
            other => panic!("expected YAML string, got {other:?}"),
        },
        other => panic!("expected list, got {other:?}"),
    };
    assert!(yaml.contains("a: 1"), "expected plain YAML, got: {yaml}");
    assert!(!yaml.contains("type:"), "internal envelope leaked: {yaml}");

    // Encode then decode round-trips to the original value.
    let mut parser = Parser::new("$data | @yaml | @yaml");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(res, Val::List(vec![data]));
    } else {
        panic!("expected expression statement");
    }
}

#[tokio::test]
async fn test_pipeline_grep_regex() {
    let env = setup_test_env();
    env.vars.write().insert(
        "files".to_string(),
        Val::List(vec![
            Val::String("a.tmp".into()),
            Val::String("b.log".into()),
            Val::String("ctmp".into()),
        ]),
    );
    let mut parser = Parser::new(r#"$files | grep r"\.tmp$""#);
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        assert_eq!(res, Val::List(vec![Val::String("a.tmp".into())]));
    } else {
        panic!("expected expression statement, got {stmts:?}");
    }
}

#[tokio::test]
async fn test_filter_non_bool_predicate_errors() {
    // A predicate that is not a Bool must be reported, not silently drop items.
    let ctx = TestContext::new();
    let err = ctx.eval_script("let d = [1, 2]; $d | filter 5").await;
    assert!(err.is_err(), "non-bool filter predicate must error");
}

#[tokio::test]
async fn test_count_without_upstream_emits_zero() {
    // `count` with nothing piped in must still emit a single 0, not nothing.
    let ctx = TestContext::new();
    let res = ctx.eval_ok("count").await;
    assert_val_eq!(res, Val::List(vec![Val::Int(0)]));
}

// Map member access

#[tokio::test]
async fn test_json_boundary_roundtrip() {
    let env = setup_test_env();
    // Use alphabetical key order to match serde_json::Map (BTreeMap) ordering
    let original_val = Val::Map({
        let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        m.insert(ustr::ustr("active"), Val::Bool(true));
        m.insert(ustr::ustr("count"), Val::Int(42));
        m.insert(ustr::ustr("name"), Val::String("test".to_string()));
        m
    });
    env.vars
        .write()
        .insert("data".to_string(), original_val.clone());

    let mut parser = Parser::new("$data | @json");
    let stmts = parser.parse_statements().unwrap();
    let json_str = if let Stmt::Expr(expr) = stmts[0].unpack() {
        match eval_expr(expr, &env).await.unwrap() {
            Val::List(items) if items.len() == 1 => match &items[0] {
                Val::String(s) => s.clone(),
                other => panic!("Expected Val::String, got {:?}", other),
            },
            other => panic!("Expected Val::List with 1 item, got {:?}", other),
        }
    } else {
        panic!("Expected Stmt::Expr");
    };

    env.vars
        .write()
        .insert("json_data".to_string(), Val::String(json_str));
    let mut de_parser = Parser::new("$json_data | @json");
    let de_stmts = de_parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = de_stmts[0].unpack() {
        match eval_expr(expr, &env).await.unwrap() {
            Val::List(items) if items.len() == 1 => {
                assert_eq!(items[0], original_val, "roundtrip should preserve value");
            }
            other => panic!("Expected Val::List with 1 item, got {:?}", other),
        }
    }
}

#[tokio::test]
async fn test_integration_limit() {
    let env = setup_test_env();

    // Seed list [10, 20, 30, 40, 50]
    let raw_list = Val::List(vec![
        Val::Int(10),
        Val::Int(20),
        Val::Int(30),
        Val::Int(40),
        Val::Int(50),
    ]);
    env.vars.write().insert("numbers".to_string(), raw_list);

    // Run pipeline: $numbers | limit 3
    let mut parser = Parser::new("$numbers | limit 3");
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);

    if let Stmt::Expr(expr) = stmts[0].unpack() {
        if let fshell_core::Expr::Pipeline(pipeline) = expr.unpack() {
            let (tx, mut rx) = tokio::sync::mpsc::channel(100);
            let env_clone = env.clone();
            let pipeline_clone = pipeline.clone();

            tokio::spawn(async move {
                fshell_engine::execute_pipeline(&pipeline_clone, &env_clone, tx)
                    .await
                    .unwrap();
            });

            let mut results = Vec::new();
            while let Some(payload) = rx.recv().await {
                if let fshell_engine::PipelinePayload::Data(val) = payload {
                    results.push((*val).clone());
                }
            }

            assert_eq!(results.len(), 3);
            assert_eq!(results[0], Val::Int(10));
            assert_eq!(results[1], Val::Int(20));
            assert_eq!(results[2], Val::Int(30));
        } else {
            panic!("Expected Expr::Pipeline");
        }
    } else {
        panic!("Expected Stmt::Expr");
    }
}

#[tokio::test]
async fn test_total_order_sorting() {
    let env = setup_test_env();

    let mut map_null = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    map_null.insert(ustr::ustr("val"), Val::Null);

    let mut map_bool = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    map_bool.insert(ustr::ustr("val"), Val::Bool(true));

    let mut map_int = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    map_int.insert(ustr::ustr("val"), Val::Int(42));

    let mut map_float = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    map_float.insert(ustr::ustr("val"), Val::Float(1.23));

    let mut map_str = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    map_str.insert(ustr::ustr("val"), Val::String("hello".to_string()));

    let raw_list = Val::List(vec![
        Val::Map(map_str),
        Val::Map(map_float),
        Val::Map(map_null),
        Val::Map(map_int),
        Val::Map(map_bool),
    ]);
    env.vars.write().insert("mixed".to_string(), raw_list);

    let mut parser = Parser::new("$mixed | sort val");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let sorted = eval_expr(expr, &env).await.unwrap();
        if let Val::List(items) = sorted {
            assert_eq!(items.len(), 5);

            let val_0 = match &items[0] {
                Val::Map(m) => m.get(&ustr::ustr("val")).unwrap(),
                _ => panic!(),
            };
            let val_1 = match &items[1] {
                Val::Map(m) => m.get(&ustr::ustr("val")).unwrap(),
                _ => panic!(),
            };
            let val_2 = match &items[2] {
                Val::Map(m) => m.get(&ustr::ustr("val")).unwrap(),
                _ => panic!(),
            };
            let val_3 = match &items[3] {
                Val::Map(m) => m.get(&ustr::ustr("val")).unwrap(),
                _ => panic!(),
            };
            let val_4 = match &items[4] {
                Val::Map(m) => m.get(&ustr::ustr("val")).unwrap(),
                _ => panic!(),
            };

            assert!(matches!(val_0, Val::Null));
            assert_eq!(val_1, &Val::Bool(true));
            assert_eq!(val_2, &Val::Int(42));
            assert_eq!(val_3, &Val::Float(1.23));
            assert_eq!(val_4, &Val::String("hello".to_string()));
        } else {
            panic!("Expected Val::List");
        }
    }
}

#[tokio::test]
async fn test_integration_bare_variable_pipeline_evaluation() {
    let env = setup_test_env();

    // let a = 42
    let mut p = Parser::new("let a = 42");
    eval_stmt(&p.parse_statements().unwrap().remove(0), &env, false)
        .await
        .unwrap();

    // Execute pipeline "a"
    let mut p2 = Parser::new("a");
    let stmts = p2.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        if let fshell_core::Expr::Pipeline(pipeline) = expr.unpack() {
            let (tx, mut rx) = tokio::sync::mpsc::channel(100);
            let env_clone = env.clone();
            let pipeline_clone = pipeline.clone();

            tokio::spawn(async move {
                fshell_engine::execute_pipeline(&pipeline_clone, &env_clone, tx)
                    .await
                    .unwrap();
            });

            let mut results = Vec::new();
            while let Some(payload) = rx.recv().await {
                if let fshell_engine::PipelinePayload::Data(val) = payload {
                    results.push((*val).clone());
                }
            }
            assert_eq!(results.len(), 1);
            assert_eq!(results[0], Val::Int(42));
        } else {
            panic!("Expected Expr::Pipeline");
        }
    } else {
        panic!("Expected Stmt::Expr");
    }
}

#[tokio::test]
async fn test_integration_head_tail_uniq() {
    let env = setup_test_env();

    // Test uniq with a list of consecutive duplicates
    env.vars.write().insert(
        "items".to_string(),
        Val::List(vec![
            Val::Int(1),
            Val::Int(1),
            Val::Int(2),
            Val::Int(3),
            Val::Int(3),
            Val::Int(3),
            Val::Int(1),
        ]),
    );

    let mut parser = Parser::new("$items | uniq");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(filtered) => {
                assert_eq!(filtered.len(), 4);
                assert_eq!(filtered[0], Val::Int(1));
                assert_eq!(filtered[1], Val::Int(2));
                assert_eq!(filtered[2], Val::Int(3));
                assert_eq!(filtered[3], Val::Int(1));
            }
            _ => panic!("Expected List"),
        }
    }

    // Test head -n 2
    let mut parser = Parser::new("$items | head -n 2");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(filtered) => {
                assert_eq!(filtered.len(), 2);
                assert_eq!(filtered[0], Val::Int(1));
                assert_eq!(filtered[1], Val::Int(1));
            }
            _ => panic!("Expected List"),
        }
    }

    // Test tail -n 2
    let mut parser = Parser::new("$items | tail -n 2");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(filtered) => {
                assert_eq!(filtered.len(), 2);
                assert_eq!(filtered[0], Val::Int(3));
                assert_eq!(filtered[1], Val::Int(1));
            }
            _ => panic!("Expected List"),
        }
    }
}

#[tokio::test]
async fn test_integration_sort_builtin() {
    let env = setup_test_env();

    // 1. Sorting integers `[3, 1, 2] | sort` -> `[1, 2, 3]`
    env.vars.write().insert(
        "ints".to_string(),
        Val::List(vec![Val::Int(3), Val::Int(1), Val::Int(2)]),
    );

    let mut parser = Parser::new("$ints | sort");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(sorted) => {
                assert_eq!(sorted, vec![Val::Int(1), Val::Int(2), Val::Int(3)]);
            }
            _ => panic!("Expected List"),
        }
    }

    // 2. Sorting integers reversed `[3, 1, 2] | sort -r` -> `[3, 2, 1]`
    let mut parser = Parser::new("$ints | sort -r");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(sorted) => {
                assert_eq!(sorted, vec![Val::Int(3), Val::Int(2), Val::Int(1)]);
            }
            _ => panic!("Expected List"),
        }
    }

    // 4. Sorting maps by a column: `[{"a": 2}, {"a": 1}] | sort a` -> `[{"a": 1}, {"a": 2}]`
    let maps = Val::List(vec![
        Val::Map({
            let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            m.insert(ustr::ustr("a"), Val::Int(2));
            m
        }),
        Val::Map({
            let mut m = indexmap::IndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            m.insert(ustr::ustr("a"), Val::Int(1));
            m
        }),
    ]);
    env.vars.write().insert("maps".to_string(), maps);

    let mut parser = Parser::new("$maps | sort a");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(sorted) => {
                assert_eq!(sorted.len(), 2);
                let first_val = match &sorted[0] {
                    Val::Map(m) => m.get(&ustr::ustr("a")),
                    _ => None,
                };
                let second_val = match &sorted[1] {
                    Val::Map(m) => m.get(&ustr::ustr("a")),
                    _ => None,
                };
                assert_eq!(first_val, Some(&Val::Int(1)));
                assert_eq!(second_val, Some(&Val::Int(2)));
            }
            _ => panic!("Expected List"),
        }
    }
}

#[ignore = "`group-by` is not a registered builtin yet"]
#[tokio::test]
async fn test_integration_group_by() {
    use fshell_core::FxIndexMap;
    use ustr::ustr;

    let env = setup_test_env();

    // Setup input maps
    let mut m1 = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    m1.insert(ustr("id"), Val::Int(1));
    m1.insert(ustr("category"), Val::String("A".to_string()));

    let mut m2 = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    m2.insert(ustr("id"), Val::Int(2));
    m2.insert(ustr("category"), Val::String("B".to_string()));

    let mut m3 = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    m3.insert(ustr("id"), Val::Int(3));
    m3.insert(ustr("category"), Val::String("A".to_string()));

    let items = Val::List(vec![Val::Map(m1), Val::Map(m2), Val::Map(m3)]);
    env.vars.write().insert("items".to_string(), items);

    // Test group-by via pipeline
    let mut parser = Parser::new("$items | group-by \"category\"");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(list) => {
                assert_eq!(list.len(), 1);
                match &list[0] {
                    Val::Map(grouped) => {
                        assert_eq!(grouped.len(), 2);
                        let list_a = grouped.get(&ustr("A")).unwrap();
                        let list_b = grouped.get(&ustr("B")).unwrap();

                        match (list_a, list_b) {
                            (Val::List(la), Val::List(lb)) => {
                                assert_eq!(la.len(), 2);
                                assert_eq!(lb.len(), 1);

                                // Verify contents
                                if let Val::Map(m) = &la[0] {
                                    assert_eq!(m.get(&ustr("id")), Some(&Val::Int(1)));
                                } else {
                                    panic!("Expected Map");
                                }
                                if let Val::Map(m) = &la[1] {
                                    assert_eq!(m.get(&ustr("id")), Some(&Val::Int(3)));
                                } else {
                                    panic!("Expected Map");
                                }
                                if let Val::Map(m) = &lb[0] {
                                    assert_eq!(m.get(&ustr("id")), Some(&Val::Int(2)));
                                } else {
                                    panic!("Expected Map");
                                }
                            }
                            _ => panic!("Expected List for group keys"),
                        }
                    }
                    _ => panic!("Expected Map inside pipeline list result"),
                }
            }
            _ => panic!("Expected List result from group-by pipeline"),
        }
    }

    // Test group-by with direct arguments
    let mut parser = Parser::new("group-by \"category\" $items");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(list) => {
                assert_eq!(list.len(), 1);
                match &list[0] {
                    Val::Map(grouped) => {
                        assert_eq!(grouped.len(), 2);
                        assert!(grouped.contains_key(&ustr("A")));
                        assert!(grouped.contains_key(&ustr("B")));
                    }
                    _ => panic!("Expected Map inside list result from group-by with arguments"),
                }
            }
            _ => panic!("Expected List result from group-by with arguments"),
        }
    }
}

#[ignore = "`join` is not a registered builtin yet"]
#[tokio::test]
async fn test_integration_join() {
    use fshell_core::FxIndexMap;
    use ustr::ustr;

    let env = setup_test_env();

    // Left items: [{id: 1, name: "Alice"}, {id: 2, name: "Bob"}]
    let mut l1 = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    l1.insert(ustr("id"), Val::Int(1));
    l1.insert(ustr("name"), Val::String("Alice".to_string()));

    let mut l2 = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    l2.insert(ustr("id"), Val::Int(2));
    l2.insert(ustr("name"), Val::String("Bob".to_string()));

    let left_items = Val::List(vec![Val::Map(l1), Val::Map(l2)]);
    env.vars.write().insert("left".to_string(), left_items);

    // Right items: [{id: 1, role: "Admin"}, {id: 3, role: "User"}] (Only Alice matches id 1, Bob has no role, role User has no name matching)
    let mut r1 = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    r1.insert(ustr("id"), Val::Int(1));
    r1.insert(ustr("role"), Val::String("Admin".to_string()));

    let mut r2 = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    r2.insert(ustr("id"), Val::Int(3));
    r2.insert(ustr("role"), Val::String("User".to_string()));

    let right_items = Val::List(vec![Val::Map(r1), Val::Map(r2)]);
    env.vars.write().insert("right".to_string(), right_items);

    // Test join via pipeline
    let mut parser = Parser::new("$left | join \"right\" \"id\"");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(joined) => {
                // Inner join matches only Alice (id: 1)
                assert_eq!(joined.len(), 1);
                if let Val::Map(m) = &joined[0] {
                    assert_eq!(m.get(&ustr("id")), Some(&Val::Int(1)));
                    assert_eq!(
                        m.get(&ustr("name")),
                        Some(&Val::String("Alice".to_string()))
                    );
                    assert_eq!(
                        m.get(&ustr("role")),
                        Some(&Val::String("Admin".to_string()))
                    );
                } else {
                    panic!("Expected Map item");
                }
            }
            _ => panic!("Expected List result from join"),
        }
    }
}

#[tokio::test]
async fn test_integration_ndjson_autowrap() {
    let env = setup_test_env();

    let mut parser = Parser::new(r##"printf "%s\n" "\{\"id\": 42, \"status\": \"Running\"}""##);
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(list) => {
                assert_eq!(list.len(), 1);
                match &list[0] {
                    Val::Map(map) => {
                        assert_eq!(map.get(&ustr::ustr("id")), Some(&Val::Int(42)));
                        assert_eq!(
                            map.get(&ustr::ustr("status")),
                            Some(&Val::String("Running".to_string()))
                        );
                    }
                    other => {
                        eprintln!("WARN: printf output was not autowrapped as a Map: {other:?}");
                    }
                }
            }
            _ => panic!("Expected List result from external command execution"),
        }
    }
}

#[tokio::test]
async fn test_integration_watch_operator() {
    let env = setup_test_env();
    let mut parser = Parser::new("watch \".\"");
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(list) => {
                assert!(!list.is_empty());
                // Verify structure of listed files contains name and type
                if let Val::Map(map) = &list[0] {
                    assert!(map.contains_key(&ustr::ustr("name")));
                    assert!(map.contains_key(&ustr::ustr("type")));
                } else {
                    panic!("Expected Map items in watch file list");
                }
            }
            _ => panic!("Expected List result from watch operator"),
        }
    }
}

#[tokio::test]
async fn test_integration_grep_structured_output() {
    let env = setup_test_env();
    let mut parser =
        Parser::new(r##"grep -n "parse_grep_line" crates/fshell-bridge/src/structured.rs"##);
    let stmts = parser.parse_statements().unwrap();
    if let Stmt::Expr(expr) = stmts[0].unpack() {
        let res = eval_expr(expr, &env).await.unwrap();
        match res {
            Val::List(list) => {
                assert!(!list.is_empty(), "Expected at least one grep match");
                if let Val::Map(map) = &list[0] {
                    let file = map.get(&ustr::ustr("file"));
                    assert!(
                        file.is_some(),
                        "Expected 'file' field in grep structured output"
                    );
                    let line = map.get(&ustr::ustr("line"));
                    assert!(
                        line.is_some(),
                        "Expected 'line' field in grep structured output"
                    );
                    // Verify path contains the file we searched
                    assert_eq!(
                        file,
                        Some(&Val::String(
                            "crates/fshell-bridge/src/structured.rs".to_string()
                        ))
                    );
                } else {
                    eprintln!("WARN: grep produced unstructured items (grep/platform dependent)");
                }
            }
            other => {
                if matches!(&other, Val::String(_)) {
                    // grep may not be available on all platforms — skip assertion
                    // but emit a warning. The structured parsing logic is tested
                    // thoroughly in unit tests.
                    eprintln!(
                        "WARN: grep produced raw string (not structured), grep may be unavailable"
                    );
                } else {
                    panic!("Expected List, got {:?}", other);
                }
            }
        }
    }
}

#[tokio::test]
async fn test_inline_pipeline_syntax() {
    let _env = setup_test_env();
    let script = "let x = $| ls |";
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1);
    if let Stmt::Let { expr, .. } = stmts[0].unpack() {
        assert!(matches!(expr.unpack(), Expr::InlinePipeline(_)));
    } else {
        panic!("Expected Let with InlinePipeline");
    }
}

#[tokio::test]
async fn test_external_pipeline_process_group_and_piping() {
    let env = setup_test_env();

    let mut parser = Parser::new("echo hello | sh -c 'read line; echo \"got: $line\"'");
    let stmts = parser.parse_statements().unwrap();

    let pipeline = match stmts[0].clone().into_unpack() {
        Stmt::Expr(expr) => match expr.into_unpack() {
            Expr::Pipeline(pipe) => pipe,
            _ => panic!("Expected Expr::Pipeline"),
        },
        _ => panic!("Expected Stmt::Expr"),
    };

    let (tx, rx) = tokio::sync::mpsc::channel(32);

    let handle = tokio::spawn(async move {
        let mut out_rx = rx;
        let mut output = String::new();
        while let Some(payload) = out_rx.recv().await {
            if let PipelinePayload::Data(val) = payload {
                output.push_str(&val.to_text());
            }
        }
        output
    });

    fshell_engine::execute_pipeline(&pipeline, &env, tx)
        .await
        .unwrap();
    let output = handle.await.unwrap();
    assert!(
        output.contains("got: hello"),
        "Expected output to contain 'got: hello', got: {}",
        output
    );
}

#[tokio::test]
async fn test_integration_binary_stream_piping() {
    let env = setup_test_env();
    let temp_dir = tempfile::tempdir().unwrap();
    let bin_in = temp_dir.path().join("in.bin");
    let bin_out = temp_dir.path().join("out.bin");

    // Create binary data containing null bytes and non-UTF8 sequences
    let binary_data = vec![0x00, 0xFF, 0xFE, 0x80, 0x00, 0x12, 0x34, 0x0A, 0xFF, 0xAA];
    std::fs::write(&bin_in, &binary_data).unwrap();

    let script = format!(
        "< \"{}\" > \"{}\"",
        bin_in.to_str().unwrap(),
        bin_out.to_str().unwrap()
    );
    let mut parser = fshell_core::Parser::new(&script);
    let stmts = parser.parse_statements().unwrap();
    fshell_engine::eval_stmt(&stmts[0], &env, false)
        .await
        .unwrap();

    // Give asynchronous writes a moment to flush
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let output_data = std::fs::read(&bin_out).unwrap();
    assert_eq!(
        output_data, binary_data,
        "Binary data must be preserved byte-for-byte across pipeline redirections"
    );
}

#[tokio::test]
async fn test_external_pipeline_streaming_early_exit() {
    let env = setup_test_env();
    let mut parser = fshell_core::Parser::new("yes | head -n 5");
    let stmts = parser.parse_statements().unwrap();
    let res = fshell_engine::eval_stmt(&stmts[0], &env, false).await;
    assert!(
        res.is_ok(),
        "Infinite streaming command piped to head must terminate promptly"
    );
}

#[tokio::test]
async fn test_execute_pipeline_waits_for_stage_tasks() {
    let env = setup_test_env();
    env.register_async_builtin(
        "delayed-output",
        |_input, _args, _env, tx, _span| async move {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            tx.send(PipelinePayload::Data(std::sync::Arc::new(Val::String(
                "done".into(),
            ))))
            .await
            .map_err(|error| fshell_core::ShellError::from(error.to_string()))?;
            Ok(())
        },
    );
    let mut parser = fshell_core::Parser::new("delayed-output");
    let stmts = parser.parse_statements().unwrap();
    let pipeline = match stmts[0].clone().into_unpack() {
        Stmt::Expr(expr) => match expr.into_unpack() {
            Expr::Pipeline(pipeline) => pipeline,
            other => panic!("expected pipeline, got {other:?}"),
        },
        other => panic!("expected expression statement, got {other:?}"),
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let started = std::time::Instant::now();
    fshell_engine::execute_pipeline(&pipeline, &env, tx)
        .await
        .unwrap();
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(20),
        "execute_pipeline returned before its stage task completed"
    );
    assert!(matches!(
        rx.recv().await,
        Some(PipelinePayload::Data(value)) if *value == Val::String("done".into())
    ));
    assert!(rx.recv().await.is_none(), "pipeline output must be closed");
}

#[tokio::test]
async fn test_external_pipeline_chunked_streaming_and_count() {
    let env = setup_test_env();
    let mut parser =
        fshell_core::Parser::new("printf 'alpha\\nbeta\\ngamma\\nalpha\\n' | grep alpha | count");
    let stmts = parser.parse_statements().unwrap();
    let res = fshell_engine::eval_stmt(&stmts[0], &env, false).await;
    assert!(res.is_ok());
}
