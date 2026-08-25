use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tempfile::TempDir;

use fshell_core::{Parser, Stmt, Val, set_var};
use fshell_engine::{EngineError, Env, eval_expr, eval_stmt};

use super::guard::{CwdGuard, EnvVarGuard};

static CONTEXT_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Encapsulates an isolated execution environment for tests.
pub struct TestContext {
    pub env: Env,
    pub temp_dir: TempDir,
    pub cwd_guard: Option<CwdGuard>,
    pub env_guard: Option<EnvVarGuard>,
}

impl TestContext {
    /// Create a standard in-memory isolated `TestContext` with its own isolated
    /// temporary frecency database and temporary scratch folder.
    pub fn new() -> Self {
        let id = CONTEXT_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp_dir = TempDir::new().expect("failed to create temp dir for TestContext");
        let z_db_path =
            temp_dir
                .path()
                .join(format!("fsh_z_test_{}_{}.json", std::process::id(), id));

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

        Self {
            env,
            temp_dir,
            cwd_guard: None,
            env_guard: None,
        }
    }

    /// Create a `TestContext` that also isolates the process working directory
    /// to this context's temporary directory.
    pub fn with_isolated_cwd() -> Self {
        let mut ctx = Self::new();
        let guard = CwdGuard::switch_to(ctx.temp_dir.path());
        ctx.cwd_guard = Some(guard);
        ctx
    }

    /// Enable tracked environment variable isolation for this context.
    pub fn with_env_guard(mut self) -> Self {
        self.env_guard = Some(EnvVarGuard::new());
        self
    }

    /// Path to the context's temporary directory.
    pub fn temp_path(&self) -> &Path {
        self.temp_dir.path()
    }

    /// Create a file with content in the context's temporary directory.
    pub fn create_file<P: AsRef<Path>>(
        &self,
        rel_path: P,
        content: &str,
    ) -> std::io::Result<PathBuf> {
        let path = self.temp_dir.path().join(rel_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
        Ok(path)
    }

    /// Create a directory in the context's temporary directory.
    pub fn create_dir<P: AsRef<Path>>(&self, rel_path: P) -> std::io::Result<PathBuf> {
        let path = self.temp_dir.path().join(rel_path);
        std::fs::create_dir_all(&path)?;
        Ok(path)
    }

    /// Parse a single expression script and evaluate it, returning `Result<Val, EngineError>`.
    pub async fn eval(&self, script: &str) -> Result<Val, EngineError> {
        let mut parser = Parser::new(script);
        let stmts = parser.parse_statements().map_err(EngineError::Parse)?;
        if stmts.is_empty() {
            return Ok(Val::Null);
        }
        match stmts[0].unpack() {
            Stmt::Expr(expr) => eval_expr(expr, &self.env).await,
            stmt => {
                eval_stmt(stmt, &self.env, false).await?;
                Ok(Val::Null)
            }
        }
    }

    /// Parse a single expression script and evaluate it, asserting success.
    pub async fn eval_ok(&self, script: &str) -> Val {
        self.eval(script).await.expect("expected eval to succeed")
    }

    /// Execute a multi-statement script.
    pub async fn eval_script(&self, script: &str) -> Result<(), EngineError> {
        let mut parser = Parser::new(script);
        let stmts = parser.parse_statements().map_err(EngineError::Parse)?;
        for stmt in &stmts {
            eval_stmt(stmt, &self.env, false).await?;
        }
        Ok(())
    }

    /// Execute a multi-statement script and fetch a variable value from the environment.
    pub async fn get_var_after_script(&self, script: &str, var_name: &str) -> Option<Val> {
        self.eval_script(script)
            .await
            .expect("script execution failed");
        self.get_var(var_name)
    }

    /// Get a variable value from the environment.
    pub fn get_var(&self, name: &str) -> Option<Val> {
        let vars = self.env.vars.read();
        vars.get(name).cloned()
    }

    /// Set a variable value in the environment.
    pub fn set_var(&self, name: &str, val: Val) {
        let mut vars = self.env.vars.write();
        vars.insert(name.to_string(), val);
    }

    /// Run a POSIX script using the POSIX engine and return the exit code.
    pub async fn eval_posix(&self, script: &str) -> Result<i32, String> {
        let parsed = fshell_posix::parser::parse_posix_script(script)?;
        let cfg = fshell_posix::eval::EvalConfig::default();
        fshell_posix::eval::eval_source(&parsed, &self.env, &cfg)
            .await
            .map_err(|e| e.to_string())
    }

    /// Collect all items from a pipeline evaluation into a `Vec<Val>`.
    pub async fn collect_pipeline(&self, script: &str) -> Result<Vec<Val>, EngineError> {
        let val = self.eval(script).await?;
        match val {
            Val::List(items) => Ok(items),
            other => Ok(vec![other]),
        }
    }

    /// Spawn a hermetic child process builder rooted in this test context's temporary directory.
    pub fn fsh_cmd(&self) -> super::subprocess::FshCmd {
        super::subprocess::FshCmd::new().current_dir(self.temp_dir.path())
    }
}

impl Default for TestContext {
    fn default() -> Self {
        Self::new()
    }
}
