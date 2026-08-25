mod common;
use common::*;

#[tokio::test]
async fn test_param_expansion_default_unset() {
    let env = setup_test_env();
    {
        let mut opts = env.options.write();
        opts.nounset = false;
    }
    let mut parser = Parser::new(r#"let result = ${undefined_var:-"fallback"}"#);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(
        vars.get("result"),
        Some(&Val::String("fallback".to_string()))
    );
}

#[tokio::test]
async fn test_param_expansion_default_set() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("defined_var".to_string(), Val::String("hello".to_string()));
    let mut parser = Parser::new("let result = ${defined_var:-fallback}");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("result"), Some(&Val::String("hello".to_string())));
}

#[tokio::test]
async fn test_param_expansion_assign_default() {
    let env = setup_test_env();
    {
        let mut opts = env.options.write();
        opts.nounset = false;
    }
    env.vars.write().remove("new_var");
    let mut parser = Parser::new(r#"let result = ${new_var:="assigned"}"#);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(
        vars.get("result"),
        Some(&Val::String("assigned".to_string()))
    );
    assert_eq!(
        vars.get("new_var"),
        Some(&Val::String("assigned".to_string()))
    );
}

#[tokio::test]
async fn test_param_expansion_alternate() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("name".to_string(), Val::String("world".to_string()));
    let mut parser = Parser::new(r#"let result = ${name:+"hello"}"#);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("result"), Some(&Val::String("hello".to_string())));
}

#[tokio::test]
async fn test_param_expansion_alternate_unset() {
    let env = setup_test_env();
    {
        let mut opts = env.options.write();
        opts.nounset = false;
    }
    env.vars.write().remove("missing");
    let mut parser = Parser::new(r#"let result = ${missing:+"hello"}"#);
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("result"), Some(&Val::String("".to_string())));
}

#[tokio::test]
async fn test_param_expansion_substring() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("str".to_string(), Val::String("hello".to_string()));
    let mut parser = Parser::new("let result = ${str:1:3}");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("result"), Some(&Val::String("ell".to_string())));
}

#[tokio::test]
async fn test_param_expansion_substring_offset_only() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("str".to_string(), Val::String("hello".to_string()));
    let mut parser = Parser::new("let result = ${str:2}");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("result"), Some(&Val::String("llo".to_string())));
}

#[tokio::test]
async fn test_param_expansion_shortest_prefix() {
    let env = setup_test_env();
    env.vars.write().insert(
        "path".to_string(),
        Val::String("/usr/local/bin/fsh".to_string()),
    );
    let mut parser = Parser::new("let result = ${path#*/}");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(
        vars.get("result"),
        Some(&Val::String("usr/local/bin/fsh".to_string()))
    );
}

#[tokio::test]
async fn test_param_expansion_longest_prefix() {
    let env = setup_test_env();
    env.vars.write().insert(
        "path".to_string(),
        Val::String("/usr/local/bin/fsh".to_string()),
    );
    let mut parser = Parser::new("let result = ${path##*/}");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("result"), Some(&Val::String("fsh".to_string())));
}

#[tokio::test]
async fn test_param_expansion_shortest_suffix() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("file".to_string(), Val::String("main.rs".to_string()));
    let mut parser = Parser::new("let result = ${file%.*}");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("result"), Some(&Val::String("main".to_string())));
}

#[tokio::test]
async fn test_param_expansion_longest_suffix() {
    let env = setup_test_env();
    env.vars.write().insert(
        "file".to_string(),
        Val::String("archive.tar.gz".to_string()),
    );
    let mut parser = Parser::new("let result = ${file%%.*}");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(
        vars.get("result"),
        Some(&Val::String("archive".to_string()))
    );
}

#[tokio::test]
async fn test_param_expansion_replace_first() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("str".to_string(), Val::String("banana".to_string()));
    let mut parser = Parser::new("let result = ${str/a/b}");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("result"), Some(&Val::String("bbnana".to_string())));
}

#[tokio::test]
async fn test_param_expansion_replace_all() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("str".to_string(), Val::String("banana".to_string()));
    let mut parser = Parser::new("let result = ${str//a/o}");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("result"), Some(&Val::String("bonono".to_string())));
}

#[tokio::test]
async fn test_param_expansion_string_length() {
    let env = setup_test_env();
    env.vars
        .write()
        .insert("str".to_string(), Val::String("hello".to_string()));
    let mut parser = Parser::new("let result = ${#str}");
    let stmts = parser.parse_statements().unwrap();
    eval_stmt(&stmts[0], &env, false).await.unwrap();
    let vars = env.vars.read();
    assert_eq!(vars.get("result"), Some(&Val::Int(5)));
}
