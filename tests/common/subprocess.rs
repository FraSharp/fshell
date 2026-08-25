use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tempfile::TempDir;

static SUBPROCESS_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Output result from running an `fsh` subprocess with capture.
#[derive(Debug, Clone)]
pub struct FshOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
    pub duration: Duration,
}

impl FshOutput {
    /// Assert that the command completed with exit code 0.
    #[track_caller]
    pub fn assert_success(&self) -> &Self {
        if !self.status.success() {
            panic!(
                "expected command to succeed, but exited with {:?}\n--- STDOUT ---\n{}\n--- STDERR ---\n{}",
                self.status.code(),
                self.stdout,
                self.stderr
            );
        }
        self
    }

    /// Assert that the command failed with a non-zero exit code.
    #[track_caller]
    pub fn assert_failure(&self) -> &Self {
        if self.status.success() {
            panic!(
                "expected command to fail, but succeeded with 0\n--- STDOUT ---\n{}\n--- STDERR ---\n{}",
                self.stdout, self.stderr
            );
        }
        self
    }

    /// Assert that the command exited with a specific exit code.
    #[track_caller]
    pub fn assert_exit_code(&self, expected: i32) -> &Self {
        let code = self.status.code();
        if code != Some(expected) {
            panic!(
                "expected exit code {}, but got {:?}\n--- STDOUT ---\n{}\n--- STDERR ---\n{}",
                expected, code, self.stdout, self.stderr
            );
        }
        self
    }

    /// Assert that stdout contains the specified substring.
    #[track_caller]
    pub fn assert_stdout_contains(&self, substr: &str) -> &Self {
        if !self.stdout.contains(substr) {
            panic!(
                "expected stdout to contain {:?}, but was:\n--- STDOUT ---\n{}\n--- STDERR ---\n{}",
                substr, self.stdout, self.stderr
            );
        }
        self
    }

    /// Assert that stdout does not contain the specified substring.
    #[track_caller]
    pub fn assert_stdout_not_contains(&self, substr: &str) -> &Self {
        if self.stdout.contains(substr) {
            panic!(
                "expected stdout NOT to contain {:?}, but was:\n--- STDOUT ---\n{}\n--- STDERR ---\n{}",
                substr, self.stdout, self.stderr
            );
        }
        self
    }

    /// Assert that stdout trimmed equals the expected string.
    #[track_caller]
    pub fn assert_stdout_trimmed_eq(&self, expected: &str) -> &Self {
        let trimmed = self.stdout.trim();
        if trimmed != expected {
            panic!(
                "expected trimmed stdout to be {:?}, but got {:?}\n--- FULL STDOUT ---\n{}\n--- STDERR ---\n{}",
                expected, trimmed, self.stdout, self.stderr
            );
        }
        self
    }

    /// Assert that stderr contains the specified substring.
    #[track_caller]
    pub fn assert_stderr_contains(&self, substr: &str) -> &Self {
        if !self.stderr.contains(substr) {
            panic!(
                "expected stderr to contain {:?}, but was:\n--- STDERR ---\n{}\n--- STDOUT ---\n{}",
                substr, self.stderr, self.stdout
            );
        }
        self
    }

    /// Assert that stderr is completely empty.
    #[track_caller]
    pub fn assert_stderr_empty(&self) -> &Self {
        if !self.stderr.trim().is_empty() {
            panic!(
                "expected stderr to be empty, but got:\n--- STDERR ---\n{}\n--- STDOUT ---\n{}",
                self.stderr, self.stdout
            );
        }
        self
    }

    /// Parse stdout as a `serde_json::Value`.
    pub fn stdout_json_val(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::from_str(self.stdout.trim())
    }

    /// Parse stdout as JSON into type `T`.
    pub fn stdout_json<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_str(self.stdout.trim())
    }
}

/// Hermetic subprocess builder for executing the `fsh` binary.
pub struct FshCmd {
    bin_path: PathBuf,
    args: Vec<String>,
    envs: HashMap<String, Option<String>>,
    stdin_data: Option<Vec<u8>>,
    current_dir: Option<PathBuf>,
    temp_dir: TempDir,
    timeout: Duration,
}

impl FshCmd {
    /// Create a new `FshCmd` builder with automated hermetic environment isolation.
    pub fn new() -> Self {
        let id = SUBPROCESS_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp_dir = TempDir::new().expect("failed to create tempdir for FshCmd");
        let config_dir = temp_dir.path().join("config");
        std::fs::create_dir_all(&config_dir).expect("failed to create config dir");
        let z_db_path =
            temp_dir
                .path()
                .join(format!("fsh_z_sub_{}_{}.json", std::process::id(), id));

        let bin_path = PathBuf::from(env!("CARGO_BIN_EXE_fsh"));

        let mut envs = HashMap::new();
        envs.insert("FSH_TEST_ENV".to_string(), Some("1".to_string()));
        envs.insert(
            "FSH_CONFIG_DIR".to_string(),
            Some(config_dir.to_string_lossy().to_string()),
        );
        envs.insert(
            "FSH_Z_DB_PATH".to_string(),
            Some(z_db_path.to_string_lossy().to_string()),
        );
        envs.insert("NO_COLOR".to_string(), Some("1".to_string()));

        Self {
            bin_path,
            args: Vec::new(),
            envs,
            stdin_data: None,
            current_dir: None,
            temp_dir,
            timeout: Duration::from_secs(15),
        }
    }

    /// Create an `FshCmd` builder using a symlinked multicall binary name.
    pub fn multicall(utility_name: &str) -> (Self, PathBuf) {
        let mut cmd = Self::new();
        let symlink_path = cmd.temp_dir.path().join(utility_name);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&cmd.bin_path, &symlink_path)
            .expect("failed to create multicall symlink");

        cmd.bin_path = symlink_path.clone();
        (cmd, symlink_path)
    }

    /// Append a single argument.
    pub fn arg<S: AsRef<OsStr>>(mut self, arg: S) -> Self {
        self.args.push(arg.as_ref().to_string_lossy().to_string());
        self
    }

    /// Append multiple arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for arg in args {
            self.args.push(arg.as_ref().to_string_lossy().to_string());
        }
        self
    }

    /// Shorthand to run an inline command: `-c <script>`.
    pub fn cmd(self, script: &str) -> Self {
        self.arg("-c").arg(script)
    }

    /// Shorthand to run in strict mode: `-s`.
    pub fn strict(self) -> Self {
        self.arg("-s")
    }

    /// Set an environment variable for the child process.
    pub fn env<K: Into<String>, V: Into<String>>(mut self, key: K, val: V) -> Self {
        self.envs.insert(key.into(), Some(val.into()));
        self
    }

    /// Remove an environment variable for the child process.
    pub fn env_remove<K: Into<String>>(mut self, key: K) -> Self {
        self.envs.insert(key.into(), None);
        self
    }

    /// Supply stdin bytes for the child process.
    pub fn stdin<B: Into<Vec<u8>>>(mut self, bytes: B) -> Self {
        self.stdin_data = Some(bytes.into());
        self
    }

    /// Supply stdin string for the child process.
    pub fn stdin_str(self, text: &str) -> Self {
        self.stdin(text.as_bytes().to_vec())
    }

    /// Set the working directory for the child process.
    pub fn current_dir<P: AsRef<Path>>(mut self, dir: P) -> Self {
        self.current_dir = Some(dir.as_ref().to_path_buf());
        self
    }

    /// Set a hard timeout for execution (defaults to 15s).
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Path to this command's isolated temporary scratchpad.
    pub fn temp_path(&self) -> &Path {
        self.temp_dir.path()
    }

    /// Create a file in this command's temporary scratchpad before running.
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

    /// Execute the command synchronously with timeout enforcement and collect output.
    pub fn run(self) -> std::io::Result<FshOutput> {
        let mut command = Command::new(&self.bin_path);
        command.args(&self.args);

        for (k, v) in &self.envs {
            match v {
                Some(val) => command.env(k, val),
                None => command.env_remove(k),
            };
        }

        if let Some(ref dir) = self.current_dir {
            command.current_dir(dir);
        } else {
            command.current_dir(self.temp_dir.path());
        }

        if self.stdin_data.is_some() {
            command.stdin(Stdio::piped());
        } else {
            command.stdin(Stdio::null());
        }

        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        let start = Instant::now();
        let mut child = command.spawn()?;

        if let Some(input) = self.stdin_data {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(&input)?;
            }
        }

        // Wait with bounded polling timeout to prevent stuck child processes
        let poll_interval = Duration::from_millis(10);
        loop {
            match child.try_wait()? {
                Some(status) => {
                    let duration = start.elapsed();
                    let output = child.wait_with_output()?;
                    return Ok(FshOutput {
                        status,
                        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                        duration,
                    });
                }
                None => {
                    if start.elapsed() > self.timeout {
                        let _ = child.kill();
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!("FshCmd execution timed out after {:?}", self.timeout),
                        ));
                    }
                    std::thread::sleep(poll_interval);
                }
            }
        }
    }
}

impl Default for FshCmd {
    fn default() -> Self {
        Self::new()
    }
}
