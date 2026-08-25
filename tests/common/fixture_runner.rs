use super::context::TestContext;
use super::subprocess::FshCmd;
use std::path::{Path, PathBuf};

/// A parsed test fixture specification extracted from script comments.
#[derive(Debug, Clone, Default)]
pub struct FixtureSpec {
    pub path: PathBuf,
    pub name: String,
    pub content: String,
    pub expected_exit_code: Option<i32>,
    pub expected_stdout_exact: Option<String>,
    pub expected_stdout_contains: Vec<String>,
    pub expected_stderr_contains: Vec<String>,
    pub expect_stderr_empty: bool,
    pub skip_reason: Option<String>,
}

impl FixtureSpec {
    /// Parse a fixture file into a `FixtureSpec`.
    pub fn parse<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let content = std::fs::read_to_string(&path)?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unnamed_fixture".to_string());

        let mut expected_exit_code = None;
        let mut expected_stdout_exact = None;
        let mut expected_stdout_contains = Vec::new();
        let mut expected_stderr_contains = Vec::new();
        let mut expect_stderr_empty = false;
        let mut skip_reason = None;

        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(directive) = trimmed.strip_prefix("# EXPECT-EXIT:") {
                if let Ok(code) = directive.trim().parse::<i32>() {
                    expected_exit_code = Some(code);
                }
            } else if let Some(directive) = trimmed.strip_prefix("# EXPECT-STDOUT-EXACT:") {
                expected_stdout_exact = Some(directive.trim().to_string());
            } else if let Some(directive) = trimmed.strip_prefix("# EXPECT-STDOUT-CONTAINS:") {
                expected_stdout_contains.push(directive.trim().to_string());
            } else if let Some(directive) = trimmed.strip_prefix("# EXPECT-STDERR-CONTAINS:") {
                expected_stderr_contains.push(directive.trim().to_string());
            } else if trimmed == "# EXPECT-STDERR-EMPTY" {
                expect_stderr_empty = true;
            } else if let Some(directive) = trimmed.strip_prefix("# SKIP:") {
                skip_reason = Some(directive.trim().to_string());
            }
        }

        Ok(Self {
            path,
            name,
            content,
            expected_exit_code,
            expected_stdout_exact,
            expected_stdout_contains,
            expected_stderr_contains,
            expect_stderr_empty,
            skip_reason,
        })
    }

    /// Execute this fixture against the in-process POSIX engine.
    pub async fn run_in_process(&self) {
        if let Some(ref reason) = self.skip_reason {
            eprintln!("SKIPPING fixture {}: {}", self.name, reason);
            return;
        }

        let ctx = TestContext::new();
        let result = ctx.eval_posix(&self.content).await;

        let expected_exit = self.expected_exit_code.unwrap_or(0);
        match result {
            Ok(code) => {
                assert_eq!(
                    code, expected_exit,
                    "fixture `{}` failed: expected exit code {}, got {}",
                    self.name, expected_exit, code
                );
            }
            Err(e) => {
                if expected_exit == 0 {
                    panic!("fixture `{}` failed with engine error: {}", self.name, e);
                }
            }
        }
    }

    /// Execute this fixture as a hermetic child process.
    pub fn run_as_subprocess(&self) {
        if let Some(ref reason) = self.skip_reason {
            eprintln!("SKIPPING fixture {}: {}", self.name, reason);
            return;
        }

        let cmd = FshCmd::new();
        let script_file = cmd
            .create_file(format!("{}.sh", self.name), &self.content)
            .unwrap();

        let output = cmd
            .arg(&script_file)
            .run()
            .unwrap_or_else(|e| panic!("failed to run fixture `{}`: {}", self.name, e));

        let expected_exit = self.expected_exit_code.unwrap_or(0);
        output.assert_exit_code(expected_exit);

        if let Some(ref exact) = self.expected_stdout_exact {
            output.assert_stdout_trimmed_eq(exact);
        }

        for substr in &self.expected_stdout_contains {
            output.assert_stdout_contains(substr);
        }

        for substr in &self.expected_stderr_contains {
            output.assert_stderr_contains(substr);
        }

        if self.expect_stderr_empty {
            output.assert_stderr_empty();
        }
    }
}

/// Runner for discovering and running directory-driven fixture suites.
pub struct FixtureSuite {
    pub dir: PathBuf,
}

impl FixtureSuite {
    pub fn discover<P: AsRef<Path>>(dir: P) -> std::io::Result<Vec<FixtureSpec>> {
        let dir = dir.as_ref();
        let mut fixtures = Vec::new();
        if !dir.exists() {
            return Ok(fixtures);
        }

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("sh") {
                fixtures.push(FixtureSpec::parse(&path)?);
            }
        }

        fixtures.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(fixtures)
    }
}
