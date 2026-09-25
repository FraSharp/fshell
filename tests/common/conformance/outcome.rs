//! Structured result of one shell invocation, and the machinery to capture it.
//!
//! Comparison operates on this structured outcome rather than on ad-hoc string
//! assertions, so a mismatch can be classified (`ExitStatusMismatch`,
//! `StdoutMismatch`, …) and reported with a reproduction command. Streams are
//! captured through reader threads with bounded joins, so a case that hangs or
//! leaks a pipe cannot stall the suite.

use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// How a process finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Termination {
    /// Exited normally with this status.
    Exited(i32),
    /// Killed by this signal.
    Signaled(i32),
    /// Killed by the harness after exceeding its deadline.
    TimedOut,
}

impl Termination {
    /// A compact, comparable rendering used in reports.
    pub fn label(self) -> String {
        match self {
            Termination::Exited(code) => format!("exit {code}"),
            Termination::Signaled(signal) => format!("signal {signal}"),
            Termination::TimedOut => "timeout".to_string(),
        }
    }
}

/// Everything observable about one invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub termination: Termination,
    pub stdout: String,
    pub stderr: String,
    pub duration: Duration,
    /// Contents of requested fixture-relative files, `"<missing>"` when absent.
    pub files: BTreeMap<String, String>,
}

impl Outcome {
    /// Construct an outcome with no streams, for literal (`Oracle::Expect`) expectations.
    pub fn expected(stdout: String, exit: i32) -> Self {
        Self {
            termination: Termination::Exited(exit),
            stdout,
            stderr: String::new(),
            duration: Duration::ZERO,
            files: BTreeMap::new(),
        }
    }

    /// Exit status, or `None` when signalled or timed out.
    pub fn exit_code(&self) -> Option<i32> {
        match self.termination {
            Termination::Exited(code) => Some(code),
            _ => None,
        }
    }

    /// Whether stderr carried anything that is not pure whitespace.
    pub fn stderr_is_empty(&self) -> bool {
        self.stderr.trim().is_empty()
    }

    /// One-line summary for reports.
    pub fn summary(&self) -> String {
        format!(
            "{} stdout={:?} stderr={:?}",
            self.termination.label(),
            self.stdout,
            self.stderr
        )
    }
}

/// A shell invocation to capture.
pub struct Invocation<'a> {
    pub program: &'a Path,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: &'a Path,
    pub stdin: Option<Vec<u8>>,
    pub timeout: Duration,
    /// Fixture-relative files whose contents should be captured afterwards.
    pub capture_files: Vec<String>,
}

impl<'a> Invocation<'a> {
    /// New invocation with a 10 second deadline.
    pub fn new(program: &'a Path, cwd: &'a Path) -> Self {
        Self {
            program,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd,
            stdin: None,
            timeout: Duration::from_secs(10),
            capture_files: Vec::new(),
        }
    }

    /// Append an argument.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Append several arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Set the controlled environment (`env_clear` happens first).
    pub fn env(mut self, env: BTreeMap<String, String>) -> Self {
        self.env = env;
        self
    }

    /// Files to capture after the run.
    pub fn capture_files<I, S>(mut self, files: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.capture_files.extend(files.into_iter().map(Into::into));
        self
    }
}

/// Run an invocation and capture its outcome.
pub fn capture(invocation: &Invocation<'_>) -> std::io::Result<Outcome> {
    let started = Instant::now();

    let mut command = Command::new(invocation.program);
    command.args(&invocation.args);
    command.env_clear();
    for (key, value) in &invocation.env {
        command.env(key, value);
    }
    command.current_dir(invocation.cwd);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.stdin(if invocation.stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });

    let mut child = command.spawn()?;

    if let Some(bytes) = invocation.stdin.as_ref() {
        use std::io::Write as _;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(bytes);
        }
    }

    // Drain both pipes on separate threads so a chatty child cannot deadlock
    // against a full pipe buffer while we wait for it to exit.
    let (stdout_tx, stdout_rx) = mpsc::channel();
    let (stderr_tx, stderr_rx) = mpsc::channel();
    spawn_reader(child.stdout.take(), stdout_tx);
    spawn_reader(child.stderr.take(), stderr_tx);

    let mut timed_out = false;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if started.elapsed() >= invocation.timeout => {
                timed_out = true;
                let _ = child.kill();
                break child.wait()?;
            }
            None => thread::sleep(Duration::from_millis(5)),
        }
    };

    let termination = if timed_out {
        Termination::TimedOut
    } else if let Some(code) = status.code() {
        Termination::Exited(code)
    } else if let Some(signal) = status.signal() {
        Termination::Signaled(signal)
    } else {
        Termination::Exited(-1)
    };

    let stdout = String::from_utf8_lossy(&receive(stdout_rx)).into_owned();
    let stderr = String::from_utf8_lossy(&receive(stderr_rx)).into_owned();

    let mut files = BTreeMap::new();
    for relative in &invocation.capture_files {
        let contents = std::fs::read_to_string(invocation.cwd.join(relative))
            .unwrap_or_else(|_| "<missing>".to_string());
        files.insert(relative.clone(), contents);
    }

    Ok(Outcome {
        termination,
        stdout,
        stderr,
        duration: started.elapsed(),
        files,
    })
}

fn spawn_reader(stream: Option<impl Read + Send + 'static>, sender: mpsc::Sender<Vec<u8>>) {
    match stream {
        Some(mut stream) => {
            thread::spawn(move || {
                let mut buffer = Vec::new();
                let _ = stream.read_to_end(&mut buffer);
                let _ = sender.send(buffer);
            });
        }
        None => {
            let _ = sender.send(Vec::new());
        }
    }
}

/// Collect a reader thread's payload without risking an unbounded wait: a
/// background grandchild can keep a pipe open after the shell we waited on has
/// gone, and a conformance run must never hang on that.
fn receive(receiver: mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
    receiver
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_default()
}
