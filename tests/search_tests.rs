mod common;
use common::*;

#[tokio::test]
async fn finder_predicates_are_not_redirections() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("empty.rs"), "").unwrap();
    std::fs::write(tmp.path().join("full.rs"), "data").unwrap();
    let ctx = TestContext::new();
    let result = ctx
        .eval_ok(&format!(
            "ff '{}' name = '[ef]*.rs' and size > 0 --count",
            tmp.path().display()
        ))
        .await;
    assert_eq!(result, Val::List(vec![Val::Int(1)]));
    let result = ctx
        .eval_ok(&format!("ff '{}' size < 0 --count", tmp.path().display()))
        .await;
    assert_eq!(result, Val::List(vec![Val::Int(0)]));
}

#[tokio::test]
async fn finder_is_complete_by_default_and_zero_limit_is_empty() {
    let tmp = tempfile::tempdir().unwrap();
    for i in 0..1100 {
        std::fs::write(tmp.path().join(format!("{i}.rs")), "").unwrap();
    }
    let ctx = TestContext::new();
    for (flags, expected) in [
        ("--count", 1100),
        ("-n 0 --count", 0),
        ("-n 17 --count", 17),
    ] {
        let result = ctx
            .eval_ok(&format!("ff '{}' {flags}", tmp.path().display()))
            .await;
        assert_eq!(result, Val::List(vec![Val::Int(expected)]));
    }
}

#[tokio::test]
async fn finder_prunes_hidden_and_ignored_subtrees() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join(".hidden")).unwrap();
    std::fs::write(tmp.path().join(".hidden/leak.rs"), "").unwrap();
    std::fs::write(tmp.path().join("ignored.rs"), "").unwrap();
    std::fs::write(tmp.path().join("visible.rs"), "").unwrap();
    std::fs::write(tmp.path().join(".gitignore"), "ignored.rs\n").unwrap();
    let ctx = TestContext::new();
    for (flags, expected) in [("", 1), ("hidden --no-ignore", 3)] {
        let result = ctx
            .eval_ok(&format!(
                "ff '{}' name = '*.rs' {flags} --count",
                tmp.path().display()
            ))
            .await;
        assert_eq!(result, Val::List(vec![Val::Int(expected)]));
    }
}

#[tokio::test]
async fn finder_paths_and_records_use_distinct_output_schemas() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("one.rs");
    std::fs::write(&path, "value").unwrap();
    let ctx = TestContext::new();
    assert_eq!(
        ctx.eval_ok(&format!("ff '{}' --paths", tmp.path().display()))
            .await,
        Val::List(vec![Val::String(path.to_string_lossy().into_owned())])
    );
    let result = ctx.eval_ok(&format!("ff '{}'", tmp.path().display())).await;
    let Val::List(records) = result else {
        panic!("expected records")
    };
    let Val::Map(record) = &records[0] else {
        panic!("expected map")
    };
    assert_eq!(record.get(&ustr::ustr("size")), Some(&Val::Int(5)));
    assert_eq!(
        record.get(&ustr::ustr("type")),
        Some(&Val::String("file".into()))
    );
}
