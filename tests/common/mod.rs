//! Shared test utilities, RAII isolation guards, and assertions for fshell test suites.
//!
//! Import with `mod common;` and use `common::*`.

#![allow(clippy::await_holding_lock, unused_imports, dead_code)]

pub mod assertions;
pub mod context;
pub mod fixture_runner;
pub mod fixtures;
pub mod guard;
pub mod subprocess;

// Re-export everything test files need.
pub use assertions::*;
pub use context::TestContext;
pub use fixture_runner::{FixtureSpec, FixtureSuite};
pub use fixtures::*;
pub use guard::{CwdGuard, EnvVarGuard, ProcessLockGuard};
pub use subprocess::{FshCmd, FshOutput};

pub use fshell_core::{FxIndexMap, Parser, Stmt, Val, remove_var, set_var};
pub use fshell_engine::{
    EngineError, Env, PipelinePayload, eval_expr, eval_stmt, execute_pipeline, load_config_script,
};
pub use indexmap::IndexMap;
pub use ustr::ustr;

/// Create an `Env` configured for testing: sandbox off, frecency DB isolated,
/// FSH_TEST_ENV set, all builtins and bridge initialised.
pub fn setup_test_env() -> Env {
    let z_db_path = std::env::temp_dir().join(format!("fsh_z_test_{}.json", std::process::id()));
    set_var("FSH_Z_DB_PATH", &z_db_path.to_string_lossy());
    set_var("FSH_TEST_ENV", "1");

    let env = Env::new();
    {
        let mut opts = env.options.write();
        opts.sandbox_mode = "off".to_string();
    }
    fshell_builtins::init(&env);
    fshell_bridge::init(&env);
    fshell_engine::register_posix_handler(
        |content: String, args: Vec<String>, env: fshell_engine::Env, capture: bool| async move {
            let parsed = fshell_posix::parser::parse_posix_script(&content)?;
            let cfg = fshell_posix::eval::EvalConfig {
                positional: args,
                ..Default::default()
            };
            fshell_posix::eval::eval_source_stream(&parsed, &env, &cfg, capture).await
        },
    );
    env
}

/// Parse a single expression statement and evaluate, returning the result `Val`.
///
/// Panics if the script does not contain exactly one `Stmt::Expr`.
pub async fn eval_script(script: &str, env: &Env) -> Val {
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    assert_eq!(stmts.len(), 1, "expected exactly one statement");
    match stmts[0].unpack() {
        Stmt::Expr(expr) => eval_expr(expr, env).await.unwrap(),
        other => panic!("expected Expr statement, got {other:?}"),
    }
}

/// Parse and execute a multi-statement script, then return the value of `var`.
pub async fn get_var(script: &str, var: &str, env: &Env) -> Option<Val> {
    let mut parser = Parser::new(script);
    let stmts = parser.parse_statements().unwrap();
    for stmt in &stmts {
        eval_stmt(stmt, env, false).await.unwrap();
    }
    let vars = env.vars.read();
    vars.get(var).cloned()
}
