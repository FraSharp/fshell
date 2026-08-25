// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::panic))]
#![allow(clippy::result_large_err)]
use fshell_capabilities::CapsRegistry;
use fshell_core::diagnostic::{ErrorCode, StringError};
use fshell_core::{Expr, FshDiag, FxIndexMap, PromptConfig, Stmt, StringPart, Val};
use fshell_hash::FxHashMap;
pub mod ast_cache;
pub mod error;
pub mod keybindings;
pub use error::EngineError;

// Optional POSIX handler — registered by the binary crate (fshell) to avoid
// a cyclic dependency fshell-engine -> fshell-posix -> fshell-engine.
use std::future::Future;
use std::pin::Pin;
type PosixHandler = Arc<
    dyn Fn(
            String,
            Vec<String>,
            Env,
            bool,
        )
            -> Pin<Box<dyn Future<Output = Result<(i32, Option<Vec<u8>>), EngineError>> + Send>>
        + Send
        + Sync,
>;
static POSIX_HANDLER: fshell_core::RwLock<Option<PosixHandler>> = fshell_core::RwLock::new(None);

pub fn register_posix_handler<F, Fut>(handler: F)
where
    F: Fn(String, Vec<String>, Env, bool) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(i32, Option<Vec<u8>>), EngineError>> + Send + 'static,
{
    let wrapped: PosixHandler = Arc::new(
        move |content: String, args: Vec<String>, env: Env, capture: bool| {
            let fut = handler(content, args, env, capture);
            Box::pin(fut)
                as Pin<Box<dyn Future<Output = Result<(i32, Option<Vec<u8>>), EngineError>> + Send>>
        },
    );
    *POSIX_HANDLER.write() = Some(wrapped);
}

pub fn posix_handler() -> Option<PosixHandler> {
    POSIX_HANDLER.read().clone()
}
pub mod caps;
pub mod completions;
pub mod config;
pub mod login;
pub use completions::{load_completions, save_completions};
pub mod exe;
pub mod hooks;
pub mod job_control;
pub mod multicall;
pub mod profiler;
pub mod prompt;
pub mod reactive;
pub mod scope;
pub mod special_vars;

impl From<EngineError> for StringError {
    fn from(e: EngineError) -> Self {
        let code = match &e {
            EngineError::CapabilityDenied { .. } => ErrorCode::CapabilityDenied,
            EngineError::DivisionByZero { .. } => ErrorCode::RuntimeError,
            EngineError::IoError { .. } => ErrorCode::IoError,
            EngineError::MatchNonExhaustive { .. } => ErrorCode::RuntimeError,
            EngineError::MutationNotAllowed { .. } => ErrorCode::CapabilityDenied,
            EngineError::Parse(_) => ErrorCode::ParseError,
            EngineError::ConditionFalse { .. } => ErrorCode::ConditionFalse,
            EngineError::PipelineError { .. } => ErrorCode::PipelineError,
            EngineError::TypeMismatch { .. } => ErrorCode::TypeError,
            EngineError::VariableNotFound { .. } => ErrorCode::RuntimeError,
            EngineError::CycleDetected { .. } => ErrorCode::RuntimeError,
            EngineError::Generic { .. } => ErrorCode::General,
            _ => ErrorCode::General,
        };
        StringError::new(code, e.to_string())
    }
}
use fshell_core::RwLock;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use tokio::sync::Notify;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::watch;
use ustr::ustr;

tokio::task_local! {
    pub static IS_TRUSTED_CONTEXT: bool;
}

use std::os::unix::io::RawFd;

/// Read one pipe fd until EOF, echoing everything to the original fd while
/// appending non-alt-screen content to the session log (1 MB cap). Alt-screen
/// mode switches are tracked so TUI apps don't pollute the scrollback log.
#[allow(clippy::too_many_arguments)]
fn spawn_fd_logger(
    read_fd: i32,
    orig_fd: i32,
    mut log_file: std::fs::File,
    log_path: std::path::PathBuf,
    is_writing: Arc<AtomicBool>,
    flush_requested: Arc<AtomicBool>,
    flush_notify: Arc<Condvar>,
    flush_mutex: Arc<Mutex<bool>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        use std::io::Write;
        let mut buf = [0; 4096];
        let mut in_alt_screen = false;
        let mut total_bytes_written = std::fs::metadata(log_path).map(|m| m.len()).unwrap_or(0);
        let max_bytes = 1024 * 1024;
        let mut truncated_notified = false;

        loop {
            is_writing.store(false, Ordering::SeqCst);
            let n =
                unsafe { libc::read(read_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n <= 0 {
                break;
            }
            is_writing.store(true, Ordering::SeqCst);
            let bytes = &buf[..n as usize];
            let _ =
                unsafe { libc::write(orig_fd, bytes.as_ptr() as *const libc::c_void, bytes.len()) };

            let mut last_idx = 0;
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                    let mut j = i + 2;
                    while j < bytes.len()
                        && (bytes[j] == b'?'
                            || bytes[j] == b';'
                            || (bytes[j] >= b'0' && bytes[j] <= b'9'))
                    {
                        j += 1;
                    }
                    if j < bytes.len() {
                        let seq = &bytes[i..=j];
                        if seq == b"\x1b[?1049h" || seq == b"\x1b[?47h" {
                            if !in_alt_screen && i > last_idx {
                                if total_bytes_written < max_bytes {
                                    let log_slice = &bytes[last_idx..i];
                                    let to_write = std::cmp::min(
                                        log_slice.len(),
                                        (max_bytes - total_bytes_written) as usize,
                                    );
                                    if to_write > 0 {
                                        let _ = log_file.write_all(&log_slice[..to_write]);
                                        total_bytes_written += to_write as u64;
                                    }
                                }
                                if total_bytes_written >= max_bytes && !truncated_notified {
                                    let _ = log_file.write_all(
                                        b"\n\n--- SESSION LOG TRUNCATED AT 1MB LIMIT ---\n",
                                    );
                                    let _ = log_file.flush();
                                    truncated_notified = true;
                                }
                            }
                            in_alt_screen = true;
                            last_idx = j + 1;
                        } else if seq == b"\x1b[?1049l" || seq == b"\x1b[?47l" {
                            in_alt_screen = false;
                            last_idx = j + 1;
                        }
                        i = j;
                    }
                }
                i += 1;
            }

            if !in_alt_screen && bytes.len() > last_idx {
                if total_bytes_written < max_bytes {
                    let log_slice = &bytes[last_idx..];
                    let to_write =
                        std::cmp::min(log_slice.len(), (max_bytes - total_bytes_written) as usize);
                    if to_write > 0 {
                        let _ = log_file.write_all(&log_slice[..to_write]);
                        total_bytes_written += to_write as u64;
                    }
                }
                if total_bytes_written >= max_bytes && !truncated_notified {
                    let _ = log_file.write_all(b"\n\n--- SESSION LOG TRUNCATED AT 1MB LIMIT ---\n");
                    let _ = log_file.flush();
                    truncated_notified = true;
                }
            }
            let _ = log_file.flush();

            if flush_requested.load(Ordering::Relaxed)
                && let Ok(_guard) = flush_mutex.lock()
            {
                flush_notify.notify_all();
            }
        }
        is_writing.store(false, Ordering::SeqCst);
        if flush_requested.load(Ordering::Relaxed)
            && let Ok(_guard) = flush_mutex.lock()
        {
            flush_notify.notify_all();
        }
    })
}

pub struct SessionLogger {
    orig_stdout: RawFd,
    orig_stderr: RawFd,
    pipe_stdout_r: RawFd,
    pipe_stderr_r: RawFd,
    pipe_stdout_w: RawFd,
    pipe_stderr_w: RawFd,
    is_redirected: AtomicBool,
    is_writing_stdout: Arc<AtomicBool>,
    is_writing_stderr: Arc<AtomicBool>,
    flush_requested: Arc<AtomicBool>,
    flush_notify: Arc<Condvar>,
    flush_mutex: Arc<Mutex<bool>>,
    stdout_thread: Option<std::thread::JoinHandle<()>>,
    stderr_thread: Option<std::thread::JoinHandle<()>>,
}

impl SessionLogger {
    pub fn new(log_path: std::path::PathBuf) -> Result<Self, std::io::Error> {
        let orig_stdout = unsafe { libc::dup(1) };
        let orig_stderr = unsafe { libc::dup(2) };
        if orig_stdout < 0 || orig_stderr < 0 {
            return Err(std::io::Error::last_os_error());
        }

        let mut pipe_stdout = [0; 2];
        let mut pipe_stderr = [0; 2];
        unsafe {
            if libc::pipe(pipe_stdout.as_mut_ptr()) < 0 {
                libc::close(orig_stdout);
                libc::close(orig_stderr);
                return Err(std::io::Error::last_os_error());
            }
            if libc::pipe(pipe_stderr.as_mut_ptr()) < 0 {
                libc::close(orig_stdout);
                libc::close(orig_stderr);
                libc::close(pipe_stdout[0]);
                libc::close(pipe_stdout[1]);
                return Err(std::io::Error::last_os_error());
            }
            libc::fcntl(pipe_stdout[0], libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(pipe_stdout[1], libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(pipe_stderr[0], libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(pipe_stderr[1], libc::F_SETFD, libc::FD_CLOEXEC);
        }

        let is_writing_stdout = Arc::new(AtomicBool::new(false));
        let is_writing_stderr = Arc::new(AtomicBool::new(false));
        let flush_requested = Arc::new(AtomicBool::new(false));
        let flush_notify = Arc::new(Condvar::new());
        let flush_mutex = Arc::new(Mutex::new(false));

        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;

        let log_file_stderr = log_file.try_clone()?;

        // Reader threads: one per stream, sharing the alt-screen-aware logger body.
        let stdout_thread = spawn_fd_logger(
            pipe_stdout[0],
            orig_stdout,
            log_file,
            log_path.clone(),
            is_writing_stdout.clone(),
            flush_requested.clone(),
            flush_notify.clone(),
            flush_mutex.clone(),
        );

        let stderr_thread = spawn_fd_logger(
            pipe_stderr[0],
            orig_stderr,
            log_file_stderr,
            log_path.clone(),
            is_writing_stderr.clone(),
            flush_requested.clone(),
            flush_notify.clone(),
            flush_mutex.clone(),
        );

        Ok(Self {
            orig_stdout,
            orig_stderr,
            pipe_stdout_r: pipe_stdout[0],
            pipe_stderr_r: pipe_stderr[0],
            pipe_stdout_w: pipe_stdout[1],
            pipe_stderr_w: pipe_stderr[1],
            is_redirected: AtomicBool::new(false),
            is_writing_stdout,
            is_writing_stderr,
            flush_requested,
            flush_notify,
            flush_mutex,
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
        })
    }

    pub fn redirect(&self) {
        if !self.is_redirected.swap(true, Ordering::SeqCst) {
            unsafe {
                libc::dup2(self.pipe_stdout_w, 1);
                libc::dup2(self.pipe_stderr_w, 2);
            }
        }
    }

    pub fn restore(&self) {
        if self.is_redirected.swap(false, Ordering::SeqCst) {
            unsafe {
                libc::dup2(self.orig_stdout, 1);
                libc::dup2(self.orig_stderr, 2);
            }
        }
    }

    pub fn flush(&self) {
        self.flush_requested.store(true, Ordering::SeqCst);
        loop {
            let mut pending = 0;
            unsafe {
                libc::ioctl(self.pipe_stdout_r, libc::FIONREAD, &mut pending);
            }
            if pending == 0 && !self.is_writing_stdout.load(Ordering::SeqCst) {
                break;
            }
            if let Ok(guard) = self.flush_mutex.lock() {
                let _ = self
                    .flush_notify
                    .wait_timeout(guard, std::time::Duration::from_millis(1));
            }
        }
        loop {
            let mut pending = 0;
            unsafe {
                libc::ioctl(self.pipe_stderr_r, libc::FIONREAD, &mut pending);
            }
            if pending == 0 && !self.is_writing_stderr.load(Ordering::SeqCst) {
                break;
            }
            if let Ok(guard) = self.flush_mutex.lock() {
                let _ = self
                    .flush_notify
                    .wait_timeout(guard, std::time::Duration::from_millis(1));
            }
        }
        self.flush_requested.store(false, Ordering::SeqCst);
    }
}

impl Drop for SessionLogger {
    fn drop(&mut self) {
        self.restore();
        unsafe {
            libc::close(self.pipe_stdout_w);
            libc::close(self.pipe_stderr_w);
        }
        if let Some(h) = self.stdout_thread.take() {
            let _ = h.join();
        }
        if let Some(h) = self.stderr_thread.take() {
            let _ = h.join();
        }
        unsafe {
            libc::close(self.orig_stdout);
            libc::close(self.orig_stderr);
            libc::close(self.pipe_stdout_r);
            libc::close(self.pipe_stderr_r);
        }
    }
}

pub static SESSION_LOGGER: Mutex<Option<SessionLogger>> = Mutex::new(None);

pub fn shutdown_session_logging() {
    if let Ok(mut guard) = SESSION_LOGGER.lock() {
        *guard = None;
    }
}

extern "C" fn cleanup_on_exit() {
    shutdown_session_logging();
}

pub fn init_session_logger(log_path: PathBuf) {
    if std::env::var("FSH_TEST_ENV").is_err()
        && std::env::var("FSH_SESSION_LOG").as_deref() == Ok("1")
    {
        match SessionLogger::new(log_path) {
            Ok(logger) => {
                if let Ok(mut guard) = SESSION_LOGGER.lock() {
                    *guard = Some(logger);
                }
                unsafe {
                    libc::atexit(cleanup_on_exit);
                }
            }
            Err(e) => {
                eprintln!("Failed to initialize session logger: {e}");
            }
        }
    }
}

pub fn suspend_session_logging() {
    if let Ok(guard) = SESSION_LOGGER.lock()
        && let Some(logger) = guard.as_ref()
    {
        logger.flush();
        logger.restore();
    }
}

pub fn resume_session_logging() {
    if let Ok(guard) = SESSION_LOGGER.lock()
        && let Some(logger) = guard.as_ref()
    {
        logger.redirect();
    }
}

pub fn is_session_logging_active() -> bool {
    if let Ok(guard) = SESSION_LOGGER.lock() {
        if let Some(logger) = guard.as_ref() {
            logger.is_redirected.load(Ordering::SeqCst)
        } else {
            false
        }
    } else {
        false
    }
}

pub fn is_test_mode() -> bool {
    // Caching via OnceLock is unsafe here because FSH_TEST_ENV is set
    // lazily by setup_test_env() after the first call. Use direct checks.
    if std::env::var("FSH_TEST_ENV").is_ok()
        || std::env::var("CI").is_ok()
        || std::env::var("TERM").as_deref() == Ok("dumb")
        || cfg!(test)
    {
        return true;
    }
    // Integration test binaries live in target/debug/deps/ and are
    // invoked as `.../deps/<name>-<hash>`. The normal `fsh` binary lives
    // in `target/debug/fsh`. Detecting the `deps` path reliably marks
    // `cargo test` integration binaries as test mode even before
    // FSH_TEST_ENV is set (which happens inside setup_test_env()).
    if let Ok(exe) = std::env::current_exe() {
        let s = exe.to_string_lossy();
        if s.contains("/deps/") || s.contains("\\deps\\") {
            return true;
        }
        // Cargo test harness also passes --test-threads or lists test binary name
        // that ends with a hash. Fallback: check args for test harness flags.
        if std::env::args().any(|a| a == "--test" || a.starts_with("--test-threads")) {
            return true;
        }
    }
    false
}

pub fn is_stdout_a_tty() -> bool {
    if is_test_mode() {
        return false;
    }
    use std::io::IsTerminal;
    if let Ok(guard) = SESSION_LOGGER.lock()
        && let Some(logger) = guard.as_ref()
    {
        return unsafe { libc::isatty(logger.orig_stdout) != 0 };
    }
    std::io::stdout().is_terminal()
}

pub fn is_stderr_a_tty() -> bool {
    if is_test_mode() {
        return false;
    }
    use std::io::IsTerminal;
    if let Ok(guard) = SESSION_LOGGER.lock()
        && let Some(logger) = guard.as_ref()
    {
        return unsafe { libc::isatty(logger.orig_stderr) != 0 };
    }
    std::io::stderr().is_terminal()
}

pub fn is_interactive_terminal() -> bool {
    if is_test_mode() {
        return false;
    }
    (unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }) && is_stdout_a_tty()
}

/// Returns the original (pre-redirect) stdout file descriptor, if the session
/// logger is active. This is the real terminal fd, not the logging pipe.
pub fn orig_stdout_fd() -> Option<std::os::unix::io::RawFd> {
    if let Ok(guard) = SESSION_LOGGER.lock() {
        guard.as_ref().map(|l| l.orig_stdout)
    } else {
        None
    }
}

/// Returns the original (pre-redirect) stderr file descriptor, if the session
/// logger is active. This is the real terminal fd, not the logging pipe.
pub fn orig_stderr_fd() -> Option<std::os::unix::io::RawFd> {
    if let Ok(guard) = SESSION_LOGGER.lock() {
        guard.as_ref().map(|l| l.orig_stderr)
    } else {
        None
    }
}
// Lock ordering enforcement — see docs/LOCK-ORDERING.md
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[cfg(debug_assertions)]
#[allow(dead_code)]
pub(crate) enum LockLevel {
    Caps = 1,
    Vars = 2,
    Fns = 3,
    Jobs = 4,
    Reactive = 5,
    Tracked = 6,
    Options = 7,
}

#[cfg(debug_assertions)]
impl LockLevel {
    fn name(self) -> &'static str {
        match self {
            LockLevel::Caps => "caps",
            LockLevel::Vars => "vars",
            LockLevel::Fns => "fns",
            LockLevel::Jobs => "jobs",
            LockLevel::Reactive => "reactive",
            LockLevel::Tracked => "tracked",
            LockLevel::Options => "options",
        }
    }
}

#[cfg(debug_assertions)]
thread_local! {
    static LOCK_STACK: std::cell::RefCell<Vec<LockLevel>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(debug_assertions)]
#[track_caller]
pub(crate) fn check_lock_order(level: LockLevel) {
    let loc = std::panic::Location::caller();
    LOCK_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        if let Some(&last) = stack.last() {
            assert!(
                level >= last,
                "Lock ordering violation at {loc}:\n  \
                 trying to acquire '{name}' (level {level:?}) after '{last_name}' (level {last:?}).\n  \
                 Order must be: \
                 caps(1) < vars(2) < fns(3) < jobs(4) < reactive(5) < tracked(6) < options(7)",
                name = level.name(),
                last_name = last.name(),
            );
        }
        stack.push(level);
    });
}

pub struct LockGuard<T> {
    pub inner: T,
}

impl<T> std::ops::Deref for LockGuard<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T> std::ops::DerefMut for LockGuard<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

#[cfg(not(debug_assertions))]
impl<T> Drop for LockGuard<T> {
    fn drop(&mut self) {}
}

#[cfg(debug_assertions)]
impl<T> Drop for LockGuard<T> {
    fn drop(&mut self) {
        LOCK_STACK.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}

macro_rules! lock_ordered {
    ($expr:expr, $level:expr) => {{
        #[cfg(debug_assertions)]
        $crate::check_lock_order($level);
        $crate::LockGuard { inner: $expr }
    }};
}

macro_rules! lock_caps {
    ($expr:expr) => {
        lock_ordered!($expr, $crate::LockLevel::Caps)
    };
}

macro_rules! lock_vars {
    ($expr:expr) => {
        lock_ordered!($expr, $crate::LockLevel::Vars)
    };
}

macro_rules! lock_fns {
    ($expr:expr) => {
        lock_ordered!($expr, $crate::LockLevel::Fns)
    };
}

macro_rules! lock_jobs {
    ($expr:expr) => {
        lock_ordered!($expr, $crate::LockLevel::Jobs)
    };
}

macro_rules! lock_reactive {
    ($expr:expr) => {
        lock_ordered!($expr, $crate::LockLevel::Reactive)
    };
}

#[derive(Debug, Clone)]
pub enum PipelinePayload {
    /// Native structured value stream (fsh typed pipelines).
    Data(Arc<Val>),
    /// Raw unformatted byte stream (POSIX tools, binary streams).
    Bytes(bytes::Bytes),
    /// Diagnostic error marker flowing in-band.
    Structured(FshDiag),
}

impl PipelinePayload {
    /// Convert any payload to its textual representation for display.
    pub fn to_text(&self) -> String {
        match self {
            PipelinePayload::Data(v) => v.to_text(),
            PipelinePayload::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            PipelinePayload::Structured(d) => d.to_string(),
        }
    }
}

pub type PipeStream = Receiver<PipelinePayload>;
pub type PipeSender = Sender<PipelinePayload>;

#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum JobStatus {
    Running,
    Suspended,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Job {
    pub id: usize,
    pub pgid: i32,
    pub pids: Vec<i32>,
    pub cmd: String,
    pub status: JobStatus,
    #[serde(default)]
    pub disowned: bool,
    #[serde(skip)]
    pub started_at: Option<std::time::Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapPromptResponse {
    GrantOnce,
    GrantAlways,
    Deny,
}

pub struct CapPromptRequest {
    pub cmd_name: String,
    pub action: CapAction,
    pub response_tx: std::sync::mpsc::Sender<CapPromptResponse>,
}

#[derive(Debug, Clone)]
pub enum ReactiveEvent {
    TriggerCell(String),
    RegisterCell {
        name: String,
        pipeline: fshell_core::Pipeline,
        tx: watch::Sender<Arc<Vec<Val>>>,
    },
}

#[derive(Debug, Default, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SuggestionMode {
    Blocking,
    #[default]
    Deferred,
}

#[derive(Debug, Clone)]
pub struct PendingSuggestion {
    pub corrected: String,
    pub args: Vec<String>,
}

/// Named shell options that control fshell behavior.
/// Read from `config.toml` and toggled at runtime via `setopt`/`unsetopt`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ShellOptions {
    pub autocd: bool,
    pub pipefail: bool,
    pub notify: bool,
    pub status_bar: bool,
    pub notify_threshold: u64,
    pub json_auto_parse: bool,
    pub sandbox_mode: String,
    pub error_format: fshell_render::RenderFormat,
    pub error_color: bool,
    pub did_you_mean: bool,
    pub suggestion_mode: SuggestionMode,
    pub stderr_max_bytes: usize,
    pub sort_max_items: usize,
    pub pipeline_channel_size: usize,
    pub errexit: bool,
    pub nounset: bool,
    pub nullglob: bool,
    pub nocaseglob: bool,
    pub noclobber: bool,
    pub noexec: bool,
    pub xtrace: bool,
    pub verbose: bool,
    pub ignoreeof: bool,
    pub autopushd: bool,
    pub histignoredups: bool,
    pub cdable_vars: bool,
    pub quiet_aliases: bool,
    pub clear_on_reload: String,
    pub session_restore: String,
    pub theme: String,
    pub disabled_builtins: Vec<String>,
    pub command_binaries: std::collections::HashMap<String, String>,
    pub confirm_destructive: bool,
    pub sandbox_all: bool,
}

impl Default for ShellOptions {
    fn default() -> Self {
        Self {
            autocd: true,
            pipefail: false,
            notify: false,
            status_bar: matches!(
                std::env::var("FSH_STATUS_BAR").as_deref(),
                Ok("1") | Ok("true") | Ok("yes") | Ok("ON") | Ok("on")
            ),
            notify_threshold: 10,
            json_auto_parse: true,
            sandbox_mode: "prompt".into(),
            error_format: fshell_render::RenderFormat::Auto,
            error_color: true,
            did_you_mean: true,
            suggestion_mode: SuggestionMode::default(),
            stderr_max_bytes: 1_048_576,
            sort_max_items: 100_000,
            pipeline_channel_size: 100,
            errexit: false,
            nounset: true,
            nullglob: false,
            nocaseglob: false,
            noclobber: false,
            noexec: false,
            xtrace: false,
            verbose: false,
            ignoreeof: false,
            autopushd: false,
            histignoredups: false,
            cdable_vars: false,
            quiet_aliases: false,
            clear_on_reload: "ask".into(),
            session_restore: "none".into(),
            theme: "default".into(),
            disabled_builtins: Vec::new(),
            command_binaries: std::collections::HashMap::new(),
            confirm_destructive: true,
            sandbox_all: false,
        }
    }
}

macro_rules! shell_bool_options {
    ( $( $field:ident ),* $(,)? ) => {
        impl ShellOptions {
            pub fn for_each_bool<F>(&self, mut f: F)
            where
                F: FnMut(&'static str, &bool),
            {
                $( f(stringify!($field), &self.$field); )*
            }

            pub fn bool_keys() -> &'static [&'static str] {
                &[ $( stringify!($field) ),* ]
            }

            pub fn get_bool(&self, key: &str) -> Option<bool> {
                match key {
                    $( stringify!($field) => Some(self.$field), )*
                    _ => None,
                }
            }

            pub fn set_bool(&mut self, key: &str, value: bool) -> Result<(), String> {
                match key {
                    $( stringify!($field) => { self.$field = value; Ok(()) } )*
                    _ => Err(format!("unknown option: {key}")),
                }
            }
        }
    };
}

shell_bool_options! {
    autocd,
    pipefail,
    notify,
    status_bar,
    json_auto_parse,
    error_color,
    did_you_mean,
    errexit,
    nounset,
    nullglob,
    nocaseglob,
    noclobber,
    noexec,
    xtrace,
    verbose,
    ignoreeof,
    autopushd,
    histignoredups,
    cdable_vars,
    quiet_aliases,
    confirm_destructive,
    sandbox_all,
}

/// Evaluation environment tracking scopes, capabilities, and reactive cells.
///
/// ### Lock Acquisition Hierarchy (Deadlock Prevention)
/// When acquiring multiple locks concurrently, they MUST be acquired in this strict order:
/// 1. `caps` (CapsRegistry)
/// 2. `vars` (variables)
/// 3. `fns` (functions)
/// 4. `jobs`
/// 5. `reactive_cells` / `reactive_pipelines` / `reactive_deps`
/// 6. `tracked_reads` / `tracked_cells`
/// 7. `options` (ShellOptions — read-mostly, lowest priority)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Signal {
    Exit,
    Int,
    Term,
    Hup,
    Quit,
    Usr1,
    Usr2,
    Chld,
    Err,
}

impl Signal {
    pub fn from_name(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "EXIT" => Some(Signal::Exit),
            "INT" | "SIGINT" => Some(Signal::Int),
            "TERM" | "SIGTERM" => Some(Signal::Term),
            "HUP" | "SIGHUP" => Some(Signal::Hup),
            "QUIT" | "SIGQUIT" => Some(Signal::Quit),
            "USR1" | "SIGUSR1" => Some(Signal::Usr1),
            "USR2" | "SIGUSR2" => Some(Signal::Usr2),
            "CHLD" | "SIGCHLD" => Some(Signal::Chld),
            "ERR" => Some(Signal::Err),
            _ => None,
        }
    }

    pub fn to_str(&self) -> &'static str {
        match self {
            Signal::Exit => "EXIT",
            Signal::Int => "INT",
            Signal::Term => "TERM",
            Signal::Hup => "HUP",
            Signal::Quit => "QUIT",
            Signal::Usr1 => "USR1",
            Signal::Usr2 => "USR2",
            Signal::Chld => "CHLD",
            Signal::Err => "ERR",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChatConfig {
    pub provider_override: Option<String>,
    pub model_override: Option<String>,
}

#[derive(Clone)]
pub struct Env {
    pub scope: scope::Scope,
    pub job_control: job_control::JobControl,
    pub reactive: reactive::Reactivity,
    pub caps: caps::Caps,
    pub hooks: hooks::Hooks,
    pub prompt: prompt::Prompt,
    pub options: Arc<RwLock<ShellOptions>>,
    /// Count of pipeline backpressure events (channel full).
    pub backpressure_count: Arc<AtomicU64>,
    /// Gate: true while sourcing init.fsh at startup.
    /// Set builtin checks this to avoid re-persisting on boot.
    pub is_loading_init_script: Arc<AtomicBool>,
    /// Tracks whether the managed env variable has been modified from its
    /// host-initial state. When false, `run_external` can skip the env var
    /// iteration loop — the child inherits the correct environment via fork().
    pub is_env_modified: Arc<AtomicBool>,
    pub background_count: Arc<AtomicU64>,
    pub background_notify: Arc<Notify>,
    /// Set while the REPL is executing a command (to distinguish "command is
    /// running, no foreground job" i.e. a builtin from "REPL is idle").
    pub is_command_running: Arc<AtomicBool>,
    pub chat_mode: Arc<Mutex<Option<ChatConfig>>>,
    pub prompt_config: Arc<RwLock<PromptConfig>>,
    pub is_customizer_active: Arc<AtomicBool>,
    pub is_last_stage: bool,
    pub is_captured: bool,
    pub completions: Arc<RwLock<fshell_hash::FxHashMap<String, fshell_core::CommandCompletion>>>,
    pub ast_cache: Arc<RwLock<crate::ast_cache::AstCache>>,
    pub special_vars: Arc<special_vars::SpecialVars>,
    pub posix_traps: Arc<RwLock<FxHashMap<Signal, String>>>,
    pub theme: Arc<RwLock<Arc<fshell_core::theme::Theme>>>,
    pub preview_theme: Arc<RwLock<Option<Arc<fshell_core::theme::Theme>>>>,
    pub profiler: Arc<RwLock<crate::profiler::ProfilerState>>,
    pub temp_files: Arc<Mutex<Vec<tempfile::TempPath>>>,
    pub keybindings: Arc<RwLock<keybindings::KeybindingRegistry>>,
    pub exe_path: Arc<PathBuf>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum CapAction {
    ReadDir(PathBuf),
    WriteDir(PathBuf),
    ReadFile(PathBuf),
    WriteFile(PathBuf),
    Network(String),
    ReadEnv(String),
    WriteEnv(String),
    ProcessSpawn,
}

impl CapAction {
    pub fn to_resource_handle(&self) -> fshell_core::ResourceHandle {
        match self {
            CapAction::ReadDir(p) => fshell_core::ResourceHandle::ReadDir(p.clone()),
            CapAction::WriteDir(p) => fshell_core::ResourceHandle::WriteDir(p.clone()),
            CapAction::ReadFile(p) => fshell_core::ResourceHandle::ReadFile(p.clone()),
            CapAction::WriteFile(p) => fshell_core::ResourceHandle::WriteFile(p.clone()),
            CapAction::Network(host) => {
                if host == "any" {
                    fshell_core::ResourceHandle::NetworkAll
                } else {
                    fshell_core::ResourceHandle::NetworkSocket(host.clone())
                }
            }
            CapAction::ReadEnv(var) => fshell_core::ResourceHandle::ReadEnv(var.clone()),
            CapAction::WriteEnv(var) => fshell_core::ResourceHandle::WriteEnv(var.clone()),
            CapAction::ProcessSpawn => fshell_core::ResourceHandle::ProcessSpawn,
        }
    }

    pub fn format_result(&self, result: &str) -> String {
        match self {
            CapAction::ReadDir(p) => format!("ReadDir({:?}) -> {}", p, result),
            CapAction::WriteDir(p) => format!("WriteDir({:?}) -> {}", p, result),
            CapAction::ReadFile(p) => format!("ReadFile({:?}) -> {}", p, result),
            CapAction::WriteFile(p) => format!("WriteFile({:?}) -> {}", p, result),
            CapAction::Network(host) => format!("Network({}) -> {}", host, result),
            CapAction::ReadEnv(var) => format!("ReadEnv({}) -> {}", var, result),
            CapAction::WriteEnv(var) => format!("WriteEnv({}) -> {}", var, result),
            CapAction::ProcessSpawn => format!("ProcessSpawn -> {}", result),
        }
    }

    pub fn format_action(&self) -> String {
        match self {
            CapAction::ReadDir(p) => format!("ReadDir({:?})", p),
            CapAction::WriteDir(p) => format!("WriteDir({:?})", p),
            CapAction::ReadFile(p) => format!("ReadFile({:?})", p),
            CapAction::WriteFile(p) => format!("WriteFile({:?})", p),
            CapAction::Network(host) => format!("Network({})", host),
            CapAction::ReadEnv(var) => format!("ReadEnv({})", var),
            CapAction::WriteEnv(var) => format!("WriteEnv({})", var),
            CapAction::ProcessSpawn => "ProcessSpawn".to_string(),
        }
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Env {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Env")
            .field("scope", &self.scope)
            .field("job_control", &self.job_control)
            .field("reactive", &self.reactive)
            .field("caps", &self.caps)
            .field("hooks", &self.hooks)
            .field("prompt", &self.prompt)
            .field("options", &self.options)
            .field("backpressure_count", &self.backpressure_count)
            .field("chat_mode", &self.chat_mode)
            .field("is_customizer_active", &self.is_customizer_active)
            .field("completions", &self.completions)
            .finish()
    }
}

impl std::ops::Deref for Env {
    type Target = scope::Scope;
    fn deref(&self) -> &Self::Target {
        &self.scope
    }
}

fn clean_path(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut cleaned = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                cleaned.pop();
            }
            Component::CurDir => {}
            Component::Normal(c) => {
                cleaned.push(c);
            }
            Component::RootDir => {
                cleaned.push(std::path::Component::RootDir);
            }
            Component::Prefix(_p) => {
                cleaned.push(component.as_os_str());
            }
        }
    }
    cleaned
}

/// Check a capability action against a `CapsRegistry` (no lock acquisition).
fn is_action_allowed(
    action: &CapAction,
    cmd_name: &str,
    caps: &fshell_capabilities::CapsRegistry,
) -> bool {
    match action {
        CapAction::ReadDir(p) => caps.check_read_dir(p),
        CapAction::WriteDir(p) => caps.check_write_dir(p),
        CapAction::ReadFile(p) => caps.check_read_file(p),
        CapAction::WriteFile(p) => caps.check_write_file(p),
        CapAction::Network(host) => caps.check_network(host),
        CapAction::ReadEnv(var) => caps.check_env_read(var),
        CapAction::WriteEnv(var) => caps.check_env_write(var),
        CapAction::ProcessSpawn => caps.check_process_spawn(cmd_name),
    }
}

impl Env {
    /// Check if strict capability enforcement mode is active.
    pub fn is_strict_mode(&self) -> bool {
        self.caps.caps.read().strict_mode
            || self
                .caps
                .strict_mode_temp_count
                .load(std::sync::atomic::Ordering::SeqCst)
                > 0
    }

    /// Get the current active theme (preview takes precedence over persistent).
    pub fn active_theme(&self) -> Arc<fshell_core::theme::Theme> {
        self.preview_theme
            .read()
            .clone()
            .unwrap_or_else(|| self.theme.read().clone())
    }

    /// Set the persistent theme.
    pub fn set_theme(&self, theme: Arc<fshell_core::theme::Theme>) {
        *self.theme.write() = theme;
    }

    /// Apply a temporary preview theme (reverts on next command).
    pub fn preview_theme(&self, theme: Arc<fshell_core::theme::Theme>) {
        *self.preview_theme.write() = Some(theme);
    }

    /// Clear all registered temporary files, unlinking them from disk.
    pub fn clear_temp_files(&self) {
        if let Ok(mut tf) = self.temp_files.lock() {
            tf.clear();
        }
    }

    pub fn ensure_env_populated(&self) {
        let mut vars = lock_vars!(self.vars.write());
        if let Some(Val::Map(map)) = vars.get_mut("env") {
            for (k, v) in std::env::vars() {
                let ukey = ustr::ustr(&k);
                if !map.contains_key(&ukey) {
                    map.insert(ukey, fshell_core::Val::String(v));
                }
            }
        } else {
            let mut env_map =
                fshell_core::FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            for (k, v) in std::env::vars() {
                env_map.insert(ustr::ustr(&k), fshell_core::Val::String(v));
            }
            vars.insert("env".to_string(), fshell_core::Val::Map(env_map));
        }
    }

    pub fn load_universal_vars(&self) {
        if let Some(cfg_dir) = resolve_config_dir() {
            let path = cfg_dir.join("universal.json");
            if path.exists()
                && let Ok(content) = std::fs::read_to_string(&path)
                && let Ok(serde_json::Value::Object(map)) = serde_json::from_str(&content)
            {
                let mut vars = lock_vars!(self.vars.write());
                for (k, v) in map {
                    vars.insert(k, json_to_val(v));
                }
            }
        }
    }

    pub fn load_env_exports(&self) {
        self.ensure_env_populated();
        if let Some(cfg_dir) = resolve_config_dir() {
            let path = cfg_dir.join("env_exports.json");
            if path.exists()
                && let Ok(content) = std::fs::read_to_string(&path)
                && let Ok(serde_json::Value::Object(map)) = serde_json::from_str(&content)
            {
                let entries: Vec<(String, fshell_core::Val)> =
                    map.into_iter().map(|(k, v)| (k, json_to_val(v))).collect();
                let mut vars = lock_vars!(self.vars.write());
                for (k, val) in &entries {
                    if let Some(Val::Map(env_map)) = vars.get_mut("env") {
                        env_map.insert(ustr::ustr(k), val.clone());
                    }
                }
                for (k, val) in entries {
                    vars.insert(k, val);
                }
            }
        }
    }

    pub fn save_exported_env_var(&self, name: &str, val: fshell_core::Val) -> Result<(), String> {
        if let Some(cfg_dir) = ensure_config_dir() {
            let path = cfg_dir.join("env_exports.json");
            let mut map = serde_json::Map::new();
            if path.exists()
                && let Ok(content) = std::fs::read_to_string(&path)
                && let Ok(serde_json::Value::Object(existing)) = serde_json::from_str(&content)
            {
                map = existing;
            }
            map.insert(name.to_string(), val_to_json(&val));
            let serialized = serde_json::to_string_pretty(&serde_json::Value::Object(map))
                .map_err(|e| e.to_string())?;
            std::fs::write(&path, serialized).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn save_universal_var(&self, name: &str, val: fshell_core::Val) -> Result<(), String> {
        // Update in memory
        {
            let mut vars = lock_vars!(self.vars.write());
            vars.insert(name.to_string(), val.clone());
        }

        // Persist to file
        if let Some(cfg_dir) = ensure_config_dir() {
            let path = cfg_dir.join("universal.json");
            let mut map = serde_json::Map::new();
            if path.exists()
                && let Ok(content) = std::fs::read_to_string(&path)
                && let Ok(serde_json::Value::Object(existing)) = serde_json::from_str(&content)
            {
                map = existing;
            }
            map.insert(name.to_string(), val_to_json(&val));
            let serialized = serde_json::to_string_pretty(&serde_json::Value::Object(map))
                .map_err(|e| e.to_string())?;
            std::fs::write(&path, serialized).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn remove_universal_var(&self, name: &str) -> Result<(), String> {
        // Remove from memory
        {
            let mut vars = lock_vars!(self.vars.write());
            vars.remove(name);
        }

        // Remove from file
        if let Some(cfg_dir) = ensure_config_dir() {
            let path = cfg_dir.join("universal.json");
            if path.exists() {
                let mut map = serde_json::Map::new();
                if let Ok(content) = std::fs::read_to_string(&path)
                    && let Ok(serde_json::Value::Object(existing)) = serde_json::from_str(&content)
                {
                    map = existing;
                }
                if map.remove(name).is_some() {
                    let serialized = serde_json::to_string_pretty(&serde_json::Value::Object(map))
                        .map_err(|e| e.to_string())?;
                    std::fs::write(&path, serialized).map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    }

    /// Single source of truth for exit-status bookkeeping.
    /// Sets BOTH `vars["?"]` and `prompt.last_exit_code` coherently.
    /// `is_last_stage` / `pipefail` routing is handled by the call-site
    /// finalizers (pipeline collectors); this helper just keeps the two
    /// locations in sync. Replaces the previous pattern where some paths
    /// set only one of the two, causing `false; echo $?` / `&&`/`||`
    /// to observe stale state.
    pub fn set_exit_code(&self, code: i64) {
        {
            let mut vars = self.vars.write();
            vars.insert("?".to_string(), Val::Int(code));
        }
        {
            let mut ec = self.prompt.last_exit_code.write();
            *ec = code;
        }
    }

    /// Record a diagnostic as the last active failure in the shell session.
    pub fn set_last_error(&self, diag: fshell_core::diagnostic::FshDiag) {
        let val = diag.to_val();
        {
            let mut err_slot = self.prompt.last_error.write();
            *err_slot = Some(diag);
        }
        {
            let mut vars = self.vars.write();
            vars.insert("last_error".to_string(), val.clone());
            vars.insert("err".to_string(), val);
        }
    }

    /// Retrieve the most recent diagnostic recorded in the session.
    pub fn get_last_error(&self) -> Option<fshell_core::diagnostic::FshDiag> {
        self.prompt.last_error.read().clone()
    }

    /// Clear the recorded last error.
    pub fn clear_last_error(&self) {
        {
            let mut err_slot = self.prompt.last_error.write();
            *err_slot = None;
        }
    }

    /// Back-compat shim: report a generic stage failure (exit 1).
    pub fn report_stage_error(&self) {
        self.set_exit_code(1);
    }

    pub fn report_stage_error_code(&self, code: i64) {
        self.set_exit_code(code);
    }

    pub fn enforce_capability(&self, cmd_name: &str, action: CapAction) -> Result<(), EngineError> {
        let mut action = action;
        let (allowed, strict_mode) = {
            let caps = lock_caps!(self.caps.caps.read());

            match &mut action {
                CapAction::ReadFile(p)
                | CapAction::WriteFile(p)
                | CapAction::ReadDir(p)
                | CapAction::WriteDir(p) => {
                    if let Ok(canonical) = p.canonicalize() {
                        *p = canonical;
                    } else {
                        let abs_path = if p.is_relative() {
                            std::env::current_dir().unwrap_or_default().join(&p)
                        } else {
                            p.clone()
                        };
                        *p = clean_path(&abs_path);
                    }
                }
                _ => {}
            }

            let allowed = is_action_allowed(&action, cmd_name, &caps);
            let strict_mode = caps.strict_mode
                || self
                    .caps
                    .strict_mode_temp_count
                    .load(std::sync::atomic::Ordering::SeqCst)
                    > 0;
            (allowed, strict_mode)
        };

        fshell_core::debug_log!(
            "enforce_capability: cmd_name={}, action={:?}, allowed={}",
            cmd_name,
            action,
            allowed
        );

        if allowed {
            self.log_audit(action.format_result("GRANTED"));
            return Ok(());
        }

        // Log denied attempt (before we decide what to do about it)
        self.log_audit(action.format_result("DENIED"));

        let audit_msg = action.format_action();

        // SAFETY: isatty only queries fd state, no side effects, signal-safe.
        let is_interactive = !is_test_mode() && unsafe { libc::isatty(0) == 1 };

        let is_trusted = IS_TRUSTED_CONTEXT.try_with(|t| *t).unwrap_or(false);

        if is_trusted || !strict_mode {
            // Non-strict mode or trusted context: auto-grant silently, audit trail only
            // But check deny list first — never auto-grant a denied capability
            {
                let caps = self.caps.caps.read();
                if caps.is_denied(&action.to_resource_handle()) {
                    return Err(EngineError::from(format!(
                        "Capability denied: {}",
                        audit_msg
                    )));
                }
            }
            {
                let mut caps = self.caps.caps.write(); // Re-check under write lock — another thread may have already granted
                if !is_action_allowed(&action, cmd_name, &caps) {
                    caps.grant(action.to_resource_handle());
                }
            }
            self.log_audit(format!("{} -> AUTO_GRANTED", audit_msg));
            return Ok(());
        }

        // Strict mode: prompt interactively, otherwise deny
        if is_interactive {
            let is_tui = std::env::var("FSH_TUI_MODE").unwrap_or_default() == "true";
            if is_tui {
                let (tx, rx) = std::sync::mpsc::channel();
                let req = CapPromptRequest {
                    cmd_name: cmd_name.to_string(),
                    action: action.clone(),
                    response_tx: tx,
                };
                let mut sent_and_recv = false;
                if self.caps.cap_prompt_tx.try_send(req).is_ok()
                    && let Ok(res) = rx.recv_timeout(std::time::Duration::from_secs(30))
                {
                    sent_and_recv = true;
                    match res {
                        CapPromptResponse::GrantOnce => {
                            {
                                let mut caps = self.caps.caps.write(); // Re-check under write lock — another thread may have already granted
                                if !is_action_allowed(&action, cmd_name, &caps) {
                                    caps.grant(action.to_resource_handle());
                                }
                                self.log_audit(format!("{} -> GRANTED_ONCE (TUI)", audit_msg));
                                return Ok(());
                            }
                        }
                        CapPromptResponse::GrantAlways => {
                            {
                                let mut caps = self.caps.caps.write(); // Re-check under write lock
                                if !is_action_allowed(&action, cmd_name, &caps) {
                                    caps.grant(action.to_resource_handle());
                                }
                                self.log_audit(format!("{} -> GRANTED_ALWAYS (TUI)", audit_msg));
                                self.persist_caps(&caps);
                                return Ok(());
                            }
                        }
                        CapPromptResponse::Deny => {
                            self.log_audit(format!("{} -> DENIED (TUI)", audit_msg));
                        }
                    }
                }
                if !sent_and_recv {
                    self.log_audit(format!("{} -> DENIED (TUI timeout/unavailable)", audit_msg));
                }
            } else {
                use std::io::Write;
                eprintln!(
                    "\x1b[1;33m[!] {} is requesting {}.\x1b[0m",
                    cmd_name, audit_msg
                );
                eprintln!("   Active PWD grants do not cover this path/resource.");
                eprintln!("   [g] Grant once  [a] Grant always  [d] Deny (Default)");
                eprint!("   > ");
                let _ = std::io::stderr().flush();
                let mut input = String::new();
                if std::io::stdin().read_line(&mut input).is_ok() {
                    let choice = input.trim().to_lowercase();
                    if choice == "g" {
                        let mut caps = self.caps.caps.write();
                        // Re-check under write lock — another thread may have already granted
                        if !is_action_allowed(&action, cmd_name, &caps) {
                            caps.grant(action.to_resource_handle());
                        }
                        eprintln!("\x1b[1;32mGranted once for this session.\x1b[0m");
                        return Ok(());
                    } else if choice == "a" {
                        let mut caps = self.caps.caps.write();
                        // Re-check under write lock
                        if !is_action_allowed(&action, cmd_name, &caps) {
                            caps.grant(action.to_resource_handle());
                        }
                        eprintln!("\x1b[1;32mGranted always and saved persistently.\x1b[0m");

                        self.persist_caps(&caps);
                        return Ok(());
                    }
                }
            }
        }

        let err_desc = match &action {
            CapAction::ProcessSpawn => {
                if cmd_name == "extract" {
                    "process-spawn capability is required for archive extraction".to_string()
                } else if cmd_name == "echo" {
                    "Spawn process capability is not active".to_string()
                } else {
                    "process-spawn capability is required".to_string()
                }
            }
            CapAction::ReadDir(p) => format!("No read permission granted for path {:?}", p),
            CapAction::WriteDir(p) => {
                if cmd_name == "extract" {
                    format!(
                        "No write permission granted for destination directory {:?}",
                        p
                    )
                } else if cmd_name == "cp" || cmd_name == "mv" {
                    format!("No write permission for {:?}", p)
                } else {
                    format!("No write permission granted for path {:?}", p)
                }
            }
            CapAction::ReadFile(p) => {
                if cmd_name == "extract" {
                    format!("No read permission granted for archive path {:?}", p)
                } else if cmd_name == "cp" {
                    format!("No read permission for {:?}", p)
                } else {
                    format!("No read permission granted for {:?}", p)
                }
            }
            CapAction::WriteFile(p) => format!("No write permission granted for path {:?}", p),
            _ => format!("No permission granted for {}", audit_msg),
        };

        Err(EngineError::CapabilityDenied {
            cmd_name: cmd_name.to_string(),
            action: err_desc,
            span: None,
        })
    }

    /// Persist capabilities atomically with restricted permissions.
    fn persist_caps(&self, caps: &fshell_capabilities::CapsRegistry) {
        let home = match std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
            Ok(h) => h,
            Err(_) => return,
        };
        let path = std::path::PathBuf::from(home).join(".config/fsh/caps.json");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let lock_path = path.with_extension("lock");
        use std::os::unix::io::AsRawFd;
        if let Ok(lock_file) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&lock_path)
        {
            let fd = lock_file.as_raw_fd();
            unsafe {
                libc::flock(fd, libc::LOCK_EX);
            }

            // Read existing caps from disk to merge
            let mut merged_held = caps.held.clone();
            if let Ok(content) = std::fs::read_to_string(&path)
                && let Ok(saved) = serde_json::from_str::<
                    std::collections::HashSet<fshell_core::ResourceHandle>,
                >(&content)
            {
                for h in saved {
                    merged_held.insert(h);
                }
            }

            if let Ok(content) = serde_json::to_string(&merged_held) {
                let tmp_path = path.with_extension("json.tmp");
                if std::fs::write(&tmp_path, &content).is_ok() {
                    let _ =
                        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600));
                    let _ = std::fs::rename(&tmp_path, &path);
                }
            }

            unsafe {
                libc::flock(fd, libc::LOCK_UN);
            }
        }
    }

    pub fn log_audit(&self, message: String) {
        if let Ok(mut log) = self.caps.audit_log.lock() {
            if log.len() >= 1000 {
                log.pop_front();
            }
            log.push_back(message);
        }
    }

    pub fn track_read(&self, path: PathBuf) {
        if !self.reactive.tracking_active.load(Ordering::Acquire) {
            return;
        }
        if let Some(set) = self.reactive.tracked_reads.write().as_mut() {
            if path.is_absolute() {
                set.insert(path);
            } else if let Ok(abs_path) = path.canonicalize() {
                set.insert(abs_path);
            } else {
                set.insert(path);
            }
        }
    }

    pub fn track_cell(&self, name: String) {
        if !self.reactive.tracking_active.load(Ordering::Acquire) {
            return;
        }
        if let Some(set) = self.reactive.tracked_cells.write().as_mut() {
            set.insert(name);
        }
    }

    pub fn new() -> Self {
        let (reactive_tx, reactive_rx) = tokio::sync::mpsc::channel(1000);
        let (cap_prompt_tx, cap_prompt_rx) = tokio::sync::mpsc::channel(PIPELINE_CHANNEL_SIZE);
        let exe_path = Arc::new(crate::exe::resolve_exe());
        let env = Env {
            scope: scope::Scope {
                vars: Arc::new(RwLock::new(FxHashMap::default())),
                fns: Arc::new(RwLock::new(FxHashMap::default())),
                builtins: Arc::new(RwLock::new(FxHashMap::default())),
                aliases: Arc::new(RwLock::new(indexmap::IndexMap::new())),
                fallback: Arc::new(RwLock::new(None)),
                builtins_cache: Arc::new(Mutex::new(None)),
                local_vars: None,
            },
            job_control: job_control::JobControl {
                jobs: Arc::new(RwLock::new(FxHashMap::default())),
                fg_mutex: Arc::new(Mutex::new(None)),
                fg_cvar: Arc::new(Condvar::new()),
                sigint_pending: Arc::new(AtomicBool::new(false)),
                cancellation: Arc::new(AtomicBool::new(false)),
            },
            reactive: reactive::Reactivity {
                cells: Arc::new(RwLock::new(FxHashMap::default())),
                tx: Arc::new(reactive_tx),
                pipelines: Arc::new(RwLock::new(FxHashMap::default())),
                deps: Arc::new(RwLock::new(FxHashMap::default())),
                tracked_reads: Arc::new(RwLock::new(None)),
                tracked_cells: Arc::new(RwLock::new(None)),
                tracking_active: Arc::new(AtomicBool::new(false)),
                has_cells: Arc::new(AtomicBool::new(false)),
            },
            caps: caps::Caps {
                caps: Arc::new(RwLock::new(CapsRegistry::new_with_defaults(
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
                ))),
                strict_mode_temp_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                audit_log: Arc::new(Mutex::new(std::collections::VecDeque::new())),
                cap_prompt_tx: Arc::new(cap_prompt_tx),
                cap_prompt_rx: Arc::new(Mutex::new(Some(cap_prompt_rx))),
            },
            hooks: hooks::Hooks {
                registry: Arc::new(RwLock::new(FxHashMap::default())),
            },
            prompt: prompt::Prompt {
                git_branch_cache: Arc::new(RwLock::new(None)),
                last_exit_code: Arc::new(RwLock::new(0i64)),
                last_duration: Arc::new(RwLock::new(std::time::Duration::ZERO)),
                last_error: Arc::new(RwLock::new(None)),
                alias_suppressed: Arc::new(AtomicBool::new(false)),
                pending_suggestion: Arc::new(RwLock::new(None)),
                suggestion_deferred: Arc::new(AtomicBool::new(false)),
                edit_suggestion: Arc::new(RwLock::new(None)),
            },
            options: Arc::new(RwLock::new(ShellOptions::default())),
            backpressure_count: Arc::new(AtomicU64::new(0)),
            is_loading_init_script: Arc::new(AtomicBool::new(false)),
            is_env_modified: Arc::new(AtomicBool::new(false)),
            background_count: Arc::new(AtomicU64::new(0)),
            background_notify: Arc::new(Notify::new()),
            is_command_running: Arc::new(AtomicBool::new(false)),
            chat_mode: Arc::new(Mutex::new(None)),
            prompt_config: Arc::new(RwLock::new(PromptConfig::default())),
            is_customizer_active: Arc::new(AtomicBool::new(false)),
            is_last_stage: false,
            is_captured: false,
            completions: Arc::new(RwLock::new(fshell_hash::FxHashMap::default())),
            ast_cache: Arc::new(RwLock::new(crate::ast_cache::AstCache::new(64))),
            special_vars: Arc::new(special_vars::SpecialVars::new()),
            posix_traps: Arc::new(RwLock::new(FxHashMap::default())),
            theme: Arc::new(RwLock::new(Arc::new(
                fshell_core::theme::Theme::default_theme(),
            ))),
            preview_theme: Arc::new(RwLock::new(None)),
            profiler: Arc::new(RwLock::new(crate::profiler::ProfilerState::new(
                std::env::var("FSH_PROFILE").is_ok(),
            ))),
            temp_files: Arc::new(Mutex::new(Vec::new())),
            keybindings: Arc::new(RwLock::new(keybindings::KeybindingRegistry::new())),
            exe_path,
        };

        // Pre-populate capabilities maps + self vars
        {
            let mut vars = lock_vars!(env.vars.write());
            vars.insert(
                "FSH_EXE".to_string(),
                Val::String(env.exe_path.to_string_lossy().to_string()),
            );
            vars.insert(
                "FSH_VERSION".to_string(),
                Val::String(crate::exe::version().to_string()),
            );
            vars.insert(
                "FSH_FULL_VERSION".to_string(),
                Val::String(crate::exe::full_version()),
            );
            if let Some(dt) = crate::exe::build_datetime() {
                vars.insert(
                    "FSH_BUILD_DATETIME".to_string(),
                    Val::String(dt.to_string()),
                );
            }
            if let Some(iso) = crate::exe::build_datetime_iso() {
                vars.insert(
                    "FSH_BUILD_DATETIME_ISO".to_string(),
                    Val::String(iso.to_string()),
                );
            }
            if let Some(commit) = crate::exe::git_commit() {
                vars.insert(
                    "FSH_GIT_COMMIT".to_string(),
                    Val::String(commit.to_string()),
                );
            }
            let mut process_map =
                fshell_core::FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            process_map.insert(
                ustr::ustr("spawn"),
                Val::Capability(fshell_core::ResourceHandle::ProcessSpawn),
            );
            vars.insert("process".to_string(), Val::Map(process_map));

            let mut net_map =
                fshell_core::FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
            net_map.insert(
                ustr::ustr("all"),
                Val::Capability(fshell_core::ResourceHandle::NetworkAll),
            );
            vars.insert("net".to_string(), Val::Map(net_map));
        }

        // Spawn central reactive scheduler only if tokio runtime is active
        let env_clone = env.clone();
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::spawn(async move {
                run_reactive_scheduler(env_clone, reactive_rx).await;
            });
        }
        // Load universal variables from universal.json
        env.load_universal_vars();
        // Load persisted env exports (export -U) from env_exports.json
        env.load_env_exports();

        env
    }

    /// Create a minimal environment for utility/multicall mode.
    /// Create a minimal environment for one-shot commands (`-c` mode).
    /// Skips: reactive scheduler, caps.json disk read, vars pre-population,
    /// job control, hooks, prompt state, background infrastructure.
    /// Uses an empty CapsRegistry — no disk I/O; grants come from the
    /// engine's non-strict bypass rather than held capabilities.
    pub fn for_command() -> Self {
        let env = Env {
            scope: scope::Scope {
                vars: Arc::new(RwLock::new(FxHashMap::default())),
                fns: Arc::new(RwLock::new(FxHashMap::default())),
                builtins: Arc::new(RwLock::new(FxHashMap::default())),
                aliases: Arc::new(RwLock::new(indexmap::IndexMap::new())),
                fallback: Arc::new(RwLock::new(None)),
                builtins_cache: Arc::new(Mutex::new(None)),
                local_vars: None,
            },
            job_control: job_control::JobControl {
                jobs: Arc::new(RwLock::new(FxHashMap::default())),
                fg_mutex: Arc::new(Mutex::new(None)),
                fg_cvar: Arc::new(Condvar::new()),
                sigint_pending: Arc::new(AtomicBool::new(false)),
                cancellation: Arc::new(AtomicBool::new(false)),
            },
            reactive: reactive::Reactivity {
                cells: Arc::new(RwLock::new(FxHashMap::default())),
                tx: Arc::new(tokio::sync::mpsc::channel(1).0),
                pipelines: Arc::new(RwLock::new(FxHashMap::default())),
                deps: Arc::new(RwLock::new(FxHashMap::default())),
                tracked_reads: Arc::new(RwLock::new(None)),
                tracked_cells: Arc::new(RwLock::new(None)),
                tracking_active: Arc::new(AtomicBool::new(false)),
                has_cells: Arc::new(AtomicBool::new(false)),
            },
            caps: {
                let mut caps = fshell_capabilities::CapsRegistry::new_permissive();
                caps.grant(fshell_core::ResourceHandle::ReadDir(
                    std::path::PathBuf::from("/"),
                ));
                caps::Caps {
                    caps: Arc::new(RwLock::new(caps)),
                    strict_mode_temp_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                    audit_log: Arc::new(Mutex::new(std::collections::VecDeque::new())),
                    cap_prompt_tx: Arc::new(tokio::sync::mpsc::channel(1).0),
                    cap_prompt_rx: Arc::new(Mutex::new(None)),
                }
            },
            hooks: hooks::Hooks {
                registry: Arc::new(RwLock::new(FxHashMap::default())),
            },
            prompt: prompt::Prompt {
                git_branch_cache: Arc::new(RwLock::new(None)),
                last_exit_code: Arc::new(RwLock::new(0i64)),
                last_duration: Arc::new(RwLock::new(std::time::Duration::ZERO)),
                last_error: Arc::new(RwLock::new(None)),
                alias_suppressed: Arc::new(AtomicBool::new(false)),
                pending_suggestion: Arc::new(RwLock::new(None)),
                suggestion_deferred: Arc::new(AtomicBool::new(false)),
                edit_suggestion: Arc::new(RwLock::new(None)),
            },
            options: Arc::new(RwLock::new(ShellOptions::default())),
            backpressure_count: Arc::new(AtomicU64::new(0)),
            is_loading_init_script: Arc::new(AtomicBool::new(false)),
            is_env_modified: Arc::new(AtomicBool::new(false)),
            background_count: Arc::new(AtomicU64::new(0)),
            background_notify: Arc::new(Notify::new()),
            is_command_running: Arc::new(AtomicBool::new(false)),
            chat_mode: Arc::new(Mutex::new(None)),
            prompt_config: Arc::new(RwLock::new(PromptConfig::default())),
            is_customizer_active: Arc::new(AtomicBool::new(false)),
            is_last_stage: false,
            is_captured: false,
            completions: Arc::new(RwLock::new(fshell_hash::FxHashMap::default())),
            ast_cache: Arc::new(RwLock::new(crate::ast_cache::AstCache::new(64))),
            special_vars: Arc::new(special_vars::SpecialVars::new()),
            posix_traps: Arc::new(RwLock::new(FxHashMap::default())),
            theme: Arc::new(RwLock::new(Arc::new(
                fshell_core::theme::Theme::default_theme(),
            ))),
            preview_theme: Arc::new(RwLock::new(None)),
            profiler: Arc::new(RwLock::new(crate::profiler::ProfilerState::new(true))),
            temp_files: Arc::new(Mutex::new(Vec::new())),
            keybindings: Arc::new(RwLock::new(keybindings::KeybindingRegistry::new())),
            exe_path: Arc::new(crate::exe::resolve_exe()),
        };

        {
            let mut vars = env.vars.write();
            vars.insert(
                "FSH_EXE".to_string(),
                Val::String(env.exe_path.to_string_lossy().to_string()),
            );
            vars.insert(
                "FSH_VERSION".to_string(),
                Val::String(crate::exe::version().to_string()),
            );
            vars.insert(
                "FSH_FULL_VERSION".to_string(),
                Val::String(crate::exe::full_version()),
            );
            if let Some(dt) = crate::exe::build_datetime() {
                vars.insert(
                    "FSH_BUILD_DATETIME".to_string(),
                    Val::String(dt.to_string()),
                );
            }
            if let Some(iso) = crate::exe::build_datetime_iso() {
                vars.insert(
                    "FSH_BUILD_DATETIME_ISO".to_string(),
                    Val::String(iso.to_string()),
                );
            }
            if let Some(commit) = crate::exe::git_commit() {
                vars.insert(
                    "FSH_GIT_COMMIT".to_string(),
                    Val::String(commit.to_string()),
                );
            }
        }

        env
    }

    /// Create a lightweight scope with local vars without cloning the full Env.
    /// Shares all Arcs with the parent; only local_vars is replaced.
    pub fn push_scope(&self, local_vars: Arc<RwLock<FxHashMap<String, Val>>>) -> Self {
        Env {
            scope: scope::Scope {
                local_vars: Some(local_vars),
                vars: self.scope.vars.clone(),
                fns: self.scope.fns.clone(),
                builtins: self.scope.builtins.clone(),
                aliases: self.scope.aliases.clone(),
                fallback: self.scope.fallback.clone(),
                builtins_cache: self.scope.builtins_cache.clone(),
            },
            job_control: self.job_control.clone(),
            reactive: self.reactive.clone(),
            caps: self.caps.clone(),
            hooks: self.hooks.clone(),
            prompt: self.prompt.clone(),
            options: self.options.clone(),
            backpressure_count: self.backpressure_count.clone(),
            is_loading_init_script: self.is_loading_init_script.clone(),
            is_env_modified: self.is_env_modified.clone(),
            background_count: self.background_count.clone(),
            background_notify: self.background_notify.clone(),
            is_command_running: self.is_command_running.clone(),
            chat_mode: self.chat_mode.clone(),
            prompt_config: self.prompt_config.clone(),
            is_customizer_active: self.is_customizer_active.clone(),
            is_last_stage: self.is_last_stage,
            is_captured: self.is_captured,
            completions: self.completions.clone(),
            ast_cache: self.ast_cache.clone(),
            special_vars: self.special_vars.clone(),
            posix_traps: self.posix_traps.clone(),
            theme: self.theme.clone(),
            preview_theme: self.preview_theme.clone(),
            profiler: self.profiler.clone(),
            temp_files: self.temp_files.clone(),
            keybindings: self.keybindings.clone(),
            exe_path: self.exe_path.clone(),
        }
    }

    /// Block until the given job_id is no longer the foreground job.
    /// Uses a Condvar so the thread is parked by the OS (no CPU waste).
    pub fn wait_foreground(&self, job_id: usize) -> Result<(), String> {
        let mut guard = self
            .job_control
            .fg_mutex
            .lock()
            .map_err(|_| "Lock poisoned: fg_mutex".to_string())?;
        while *guard == Some(job_id) {
            let (new_guard, result) = self
                .job_control
                .fg_cvar
                .wait_timeout(guard, std::time::Duration::from_secs(86400))
                .map_err(|_| "Lock poisoned: fg_cvar".to_string())?;
            guard = new_guard;
            if result.timed_out() {
                return Err("Foreground job timed out after 24 hours".to_string());
            }
        }
        Ok(())
    }

    /// Clear the foreground job and wake any thread waiting on it.
    pub fn clear_foreground(&self, job_id: usize) -> Result<(), String> {
        let mut guard = self
            .job_control
            .fg_mutex
            .lock()
            .map_err(|_| "Lock poisoned: fg_mutex".to_string())?;
        if *guard == Some(job_id) {
            *guard = None;
        }
        self.job_control.fg_cvar.notify_all();
        Ok(())
    }

    /// Read the current foreground job ID (if any).
    pub fn foreground_job(&self) -> Option<usize> {
        *self
            .job_control
            .fg_mutex
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Set the foreground job ID (used during job setup).
    pub fn set_foreground_job(&self, job_id: Option<usize>) -> Result<(), String> {
        let mut guard = self
            .job_control
            .fg_mutex
            .lock()
            .map_err(|_| "Lock poisoned: fg_mutex".to_string())?;
        *guard = job_id;
        Ok(())
    }
    // Registry methods — per-instance builtins, aliases, and fallback handler.
    /// Register a builtin handler for the given name.
    pub fn register_builtin(&self, name: &str, handler: BuiltinHandler) {
        let mut reg = self.builtins.write();
        reg.insert(name.to_string(), handler);
        // Invalidate the builtins cache
        let mut cache = self
            .builtins_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *cache = None;
    }

    /// Register many builtins in a single write-lock acquisition. Dramatically
    /// faster than calling `register_builtin` 70+ times in a row.
    pub fn register_builtins(&self, builtins: Vec<(String, BuiltinHandler)>) {
        let mut reg = self.builtins.write();
        for (name, handler) in builtins {
            reg.insert(name, handler);
        }
        // Invalidate the builtins cache
        let mut cache = self
            .builtins_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *cache = None;
    }

    pub fn invalidate_builtins_cache(&self) {
        let mut cache = self
            .builtins_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *cache = None;
    }

    /// Look up a builtin handler by name.
    pub fn get_builtin(&self, name: &str) -> Option<BuiltinHandler> {
        {
            let opts = self.options.read();
            if opts.disabled_builtins.iter().any(|d| d == name) {
                return None;
            }
        }
        let reg = self.builtins.read();
        reg.get(name).cloned()
    }

    /// Return all builtin names (sorted, cached).
    pub fn get_all_builtins(&self) -> Vec<String> {
        let mut cache = self
            .builtins_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(ref names) = *cache {
            let opts = self.options.read();
            return names
                .iter()
                .filter(|n| !opts.disabled_builtins.contains(n))
                .cloned()
                .collect();
        }
        let reg = self.builtins.read();
        let mut names: Vec<String> = reg.keys().cloned().collect();
        names.sort();
        *cache = Some(names.clone());

        let opts = self.options.read();
        names
            .into_iter()
            .filter(|n| !opts.disabled_builtins.contains(n))
            .collect()
    }

    /// Insert or replace an alias. Returns the previous expansion if one existed.
    pub fn register_alias(&self, name: &str, expansion: &str) -> Option<String> {
        let mut reg = self.aliases.write();
        reg.insert(name.to_string(), expansion.to_string())
    }

    /// Retrieve the expansion for an alias name, or None if not defined.
    pub fn get_alias(&self, name: &str) -> Option<String> {
        let reg = self.aliases.read();
        reg.get(name).cloned()
    }

    /// Remove an alias. Returns the expansion that was removed, or None.
    pub fn remove_alias(&self, name: &str) -> Option<String> {
        let mut reg = self.aliases.write();
        reg.shift_remove(name)
    }

    /// Return a snapshot of all current (name, expansion) pairs.
    pub fn get_all_aliases(&self) -> Vec<(String, String)> {
        let reg = self.aliases.read();
        reg.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// Set the fallback handler for commands not found as builtins or user functions.
    pub fn set_fallback_handler(&self, handler: FallbackHandler) {
        let mut reg = self.fallback.write();
        *reg = Some(handler);
    }

    /// Get the fallback handler, if set.
    pub fn get_fallback_handler(&self) -> Option<FallbackHandler> {
        let reg = self.fallback.read();
        reg.clone()
    }
}

pub fn resolve_config_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("FSH_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join("fsh"));
    }
    let home = std::env::var("FSH_HOME")
        .ok()
        .or_else(|| std::env::var("HOME").ok())?;
    Some(PathBuf::from(home).join(".config").join("fsh"))
}

pub fn ensure_config_dir() -> Option<PathBuf> {
    let dir = resolve_config_dir()?;
    let _ = std::fs::create_dir_all(&dir);
    Some(dir)
}

pub fn config_dir() -> Option<PathBuf> {
    ensure_config_dir()
}

pub fn cache_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("FSH_CACHE_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var("FSH_HOME")
        .ok()
        .or_else(|| std::env::var("HOME").ok())?;
    let dir = PathBuf::from(home).join(".cache").join("fsh");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir)
}

static CACHED_INIT_FSH: std::sync::Mutex<Option<(String, std::time::SystemTime)>> =
    std::sync::Mutex::new(None);

pub async fn load_config_script(env: &Env) -> Result<(), String> {
    let _ = load_completions(env);

    let Some(cfg_dir) = resolve_config_dir() else {
        fshell_core::debug_log!("load_config_script: no config_dir found");
        return Ok(());
    };

    let init_path = cfg_dir.join("init.fsh");

    // Auto-recovery: if init.fsh is empty or fails to read, restore from backup chain.
    if init_path.exists() {
        let should_recover = match std::fs::read_to_string(&init_path) {
            Ok(c) if c.trim().is_empty() => true,
            Err(_) => true,
            _ => false,
        };
        if should_recover {
            let recovered = crate::config::read_with_backup_chain(&init_path);
            if !recovered.is_empty() {
                fshell_core::debug_log!(
                    "load_config_script: init.fsh is empty/corrupt — restoring from backup"
                );
                let _ = std::fs::write(&init_path, &recovered);
            }
        }
    }

    let current_mtime = init_path.metadata().ok().and_then(|m| m.modified().ok());

    fshell_core::debug_log!(
        "load_config_script: init_path={:?} exists={} mtime={:?}",
        init_path,
        init_path.exists(),
        current_mtime
    );

    // Fast path: skip re-parsing if same config dir and same mtime

    {
        let cache = CACHED_INIT_FSH
            .lock()
            .map_err(|e| format!("Lock poisoned: {e}"))?;
        if let Some((ref cached_dir, ref cached_mtime)) = *cache
            && cached_dir == &cfg_dir.to_string_lossy().to_string()
            && Some(*cached_mtime) == current_mtime
        {
            fshell_core::debug_log!("load_config_script: cache hit, skipping re-parse");
            sync_options_map(env)?;
            return Ok(());
        }
    }

    // Sync $options var from ShellOptions BEFORE sourcing init.fsh
    // so user scripts can reference $options.autocd etc.
    sync_options_map(env)?;

    // Source init.fsh under the loading gate (prevents re-persistence)
    if init_path.exists() {
        fshell_core::debug_log!(
            "load_config_script: sourcing init.fsh (is_loading_init_script=true)"
        );
        let path_str = init_path.to_string_lossy().to_string();
        let source_stmt = Stmt::Source {
            path: Expr::String(vec![StringPart::Lit(path_str)]),
            bash: false,
        };
        env.is_loading_init_script
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let result = {
            let _g = crate::profiler::ProfilerState::guard(
                &env.profiler,
                "source init.fsh",
                crate::profiler::ProfilerCategory::Source,
            );
            eval_stmt(&source_stmt, env, false).await
        };
        env.is_loading_init_script
            .store(false, std::sync::atomic::Ordering::SeqCst);
        fshell_core::debug_log!(
            "load_config_script: init.fsh sourced (is_loading_init_script=false)"
        );
        result.map_err(|e| format!("Failed to source init.fsh: {e}"))?;

        // Re-sync after sourcing (user code may have mutated ShellOptions)
        sync_options_map(env)?;
    }

    if let Some(mtime) = current_mtime {
        let mut cache = CACHED_INIT_FSH
            .lock()
            .map_err(|e| format!("Lock poisoned: {e}"))?;
        *cache = Some((cfg_dir.to_string_lossy().to_string(), mtime));
    }
    Ok(())
}

impl Env {
    /// Populate `$options` map in `env.vars` from `ShellOptions` so fsh scripts can read `$options.autocd` etc.
    pub fn sync_options_map(&self) -> Result<(), String> {
        let snapshot = {
            let opts = self.options.read();
            opts.clone()
        };
        let mut vars = self.vars.write();
        let mut options_map = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        snapshot.for_each_bool(|name, val| {
            options_map.insert(ustr(name), Val::Bool(*val));
        });
        options_map.insert(
            ustr("sandbox_mode"),
            Val::String(snapshot.sandbox_mode.clone()),
        );
        let mode_str = match snapshot.suggestion_mode {
            SuggestionMode::Blocking => "blocking",
            SuggestionMode::Deferred => "deferred",
        };
        options_map.insert(ustr("suggestion_mode"), Val::String(mode_str.into()));
        options_map.insert(
            ustr("pipeline_channel_size"),
            Val::Int(snapshot.pipeline_channel_size as i64),
        );
        options_map.insert(
            ustr("clear_on_reload"),
            Val::String(snapshot.clear_on_reload.clone()),
        );
        options_map.insert(
            ustr("session_restore"),
            Val::String(snapshot.session_restore.clone()),
        );
        options_map.insert(ustr("theme"), Val::String(snapshot.theme.clone()));
        let disabled_list: Vec<Val> = snapshot
            .disabled_builtins
            .iter()
            .map(|b| Val::String(b.clone()))
            .collect();
        options_map.insert(ustr("disabled_builtins"), Val::List(disabled_list));

        let mut binaries_map = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        for (k, v) in &snapshot.command_binaries {
            binaries_map.insert(ustr(k), Val::String(v.clone()));
        }
        options_map.insert(ustr("command_binaries"), Val::Map(binaries_map));

        vars.insert("options".into(), Val::Map(options_map));
        Ok(())
    }
}

/// Populate $options map from ShellOptions so fsh scripts can read $options.autocd etc.
fn sync_options_map(env: &Env) -> Result<(), String> {
    env.sync_options_map()
}

/// Core wait logic shared by sync and async waiters.
/// Returns the exit code (0-255 or 128+signal). Handles job table, vars["?"],
/// last_exit_code with pipefail/is_last_stage semantics, and terminal restore.
fn wait_for_job_inner(
    env: &Env,
    pid: i32,
    job_id: usize,
    cmd_str: &str,
    restore_terminal: bool,
) -> i32 {
    let w_if_stopped = |status: i32| (status & 0xff) == 0x7f;
    let w_if_signaled = |status: i32| {
        let termsig = status & 0x7f;
        termsig != 0 && termsig != 0x7f
    };
    let w_if_exited = |status: i32| (status & 0x7f) == 0;
    let debug_fg = std::env::var("FSH_DEBUG_FG").is_ok();

    let restore_term = || {
        if restore_terminal {
            if debug_fg {
                eprintln!(
                    "[FSH_DEBUG_FG] waiter: restoring terminal to shell pgid={}",
                    unsafe { libc::getpgrp() }
                );
            }
            unsafe {
                libc::signal(libc::SIGTTOU, libc::SIG_IGN);
                libc::tcsetpgrp(libc::STDIN_FILENO, libc::getpgrp());
                libc::signal(libc::SIGTTOU, libc::SIG_DFL);
            }
        }
    };

    loop {
        let mut status = 0;
        if debug_fg {
            eprintln!("[FSH_DEBUG_FG] waiter: calling waitpid for pid={}", pid);
        }
        let res = unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) };
        if debug_fg {
            eprintln!(
                "[FSH_DEBUG_FG] waiter: waitpid returned res={}, status={}",
                res, status
            );
        }
        if res <= 0 {
            if debug_fg {
                eprintln!("[FSH_DEBUG_FG] waiter: waitpid returned <= 0, cleaning up");
            }
            restore_term();
            let mut jobs = lock_jobs!(env.job_control.jobs.write());
            jobs.retain(|k, _| *k != pid);
            drop(jobs);
            let is_foreground = {
                if let Ok(guard) = env.job_control.fg_mutex.lock() {
                    *guard == Some(job_id)
                } else {
                    false
                }
            };
            let _ = env.clear_foreground(job_id);
            let exit_code = 1;
            {
                let mut vars = lock_vars!(env.vars.write());
                vars.insert("?".to_string(), Val::Int(exit_code as i64));
            }
            if is_foreground {
                *env.prompt.last_exit_code.write() = exit_code as i64;
            } else {
                let is_pipefail = env.options.read().pipefail;
                let should_set = env.is_last_stage || is_pipefail;
                if should_set {
                    let mut ec = env.prompt.last_exit_code.write();
                    if !is_pipefail || *ec == 0 {
                        *ec = exit_code as i64;
                    }
                    let mut vars = lock_vars!(env.vars.write());
                    vars.insert("?".to_string(), Val::Int(exit_code as i64));
                }
            }
            return exit_code;
        }
        if w_if_stopped(status) {
            let stopsig = (status >> 8) & 0xff;
            if debug_fg {
                eprintln!("[FSH_DEBUG_FG] waiter: child stopped by signal {}", stopsig);
            }
            restore_term();
            let mut jobs = lock_jobs!(env.job_control.jobs.write());
            let disowned = jobs.get(&pid).is_some_and(|j| j.disowned);
            if let Some(j) = jobs.get_mut(&pid) {
                j.status = JobStatus::Suspended;
            }
            if !disowned {
                println!("\n[{}] + Suspended {}", job_id, cmd_str);
            }
            if !disowned {
                let _ = env.clear_foreground(job_id);
            }
            return 0;
        } else if w_if_exited(status) || w_if_signaled(status) {
            let exit_code = {
                if w_if_exited(status) {
                    let code = (status >> 8) & 0xff;
                    if debug_fg {
                        eprintln!("[FSH_DEBUG_FG] waiter: child exited with code {}", code);
                    }
                    code
                } else {
                    let sig = status & 0x7f;
                    if debug_fg {
                        eprintln!("[FSH_DEBUG_FG] waiter: child terminated by signal {}", sig);
                    }
                    128 + sig
                }
            };
            restore_term();
            let (elapsed_secs, was_disowned) = {
                let jobs = lock_jobs!(env.job_control.jobs.read());
                jobs.get(&pid)
                    .map(|j| {
                        (
                            j.started_at.map(|t| t.elapsed().as_secs()).unwrap_or(0),
                            j.disowned,
                        )
                    })
                    .unwrap_or((0, false))
            };
            let mut jobs = lock_jobs!(env.job_control.jobs.write());
            jobs.retain(|k, _| *k != pid);
            drop(jobs);
            let is_foreground = {
                if let Ok(guard) = env.job_control.fg_mutex.lock() {
                    *guard == Some(job_id)
                } else {
                    false
                }
            };
            let _ = env.clear_foreground(job_id);
            {
                let mut vars = lock_vars!(env.vars.write());
                vars.insert("?".to_string(), Val::Int(exit_code as i64));
            }
            if !is_foreground && !was_disowned {
                let (do_notify, threshold) = {
                    let opts = env.options.read();
                    (opts.notify, opts.notify_threshold)
                };
                if do_notify && elapsed_secs >= threshold {
                    eprintln!("\n[{}] Done\t{} (exit {})", job_id, cmd_str, exit_code);
                    // Terminal bell for desktop notification fallback
                    eprint!("\x07");
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                }
            }
            let pipefail = env.options.read().pipefail;
            let should_update_last = is_foreground || env.is_last_stage || pipefail;
            if should_update_last {
                if env.is_last_stage {
                    let mut ec = env.prompt.last_exit_code.write();
                    if !pipefail || *ec == 0 || exit_code != 0 {
                        *ec = exit_code as i64;
                    }
                } else if pipefail && exit_code != 0 {
                    *env.prompt.last_exit_code.write() = exit_code as i64;
                }
            }
            return exit_code;
        }
    }
}

/// Spawn a blocking task to wait for a child process with full job-control semantics.
/// When `restore_terminal` is true, the task restores terminal ownership to the shell's
/// own process group when the child stops or exits (required for interactive/TUI jobs).
pub fn spawn_job_waiter(
    env: Env,
    cmd_str: String,
    pid: i32,
    job_id: usize,
    restore_terminal: bool,
) {
    tokio::task::spawn_blocking(move || {
        wait_for_job_inner(&env, pid, job_id, &cmd_str, restore_terminal);
    });
}

/// Synchronous (blocking) wait for a child. Called from within a `spawn_blocking`
/// context so it does not block the async executor. Returns exit code.
pub fn wait_for_job_sync(
    env: &Env,
    pid: i32,
    job_id: usize,
    cmd_str: &str,
    restore_terminal: bool,
) -> i32 {
    wait_for_job_inner(env, pid, job_id, cmd_str, restore_terminal)
}

fn forward_signal(env: &Env, signal: libc::c_int) {
    let fg_job = { env.foreground_job() };
    if let Some(job_id) = fg_job {
        let pgid = {
            let jobs = lock_jobs!(env.job_control.jobs.read());
            jobs.iter()
                .find(|(_, j)| j.id == job_id)
                .map(|(_, j)| j.pgid)
        };
        if let Some(pgid) = pgid {
            unsafe {
                libc::kill(-pgid, signal);
            }
        }
    }
}

pub fn setup_signal_handlers(env: Env) {
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigint = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to register SIGINT handler: {}", e);
                return;
            }
        };
        let mut sigtstp = match signal(SignalKind::from_raw(libc::SIGTSTP)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to register SIGTSTP handler: {}", e);
                return;
            }
        };
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to register SIGTERM handler: {}", e);
                return;
            }
        };
        let mut sigquit = match signal(SignalKind::quit()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to register SIGQUIT handler: {}", e);
                return;
            }
        };
        let mut sighup = match signal(SignalKind::hangup()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to register SIGHUP handler: {}", e);
                return;
            }
        };
        let mut sigchld = match signal(SignalKind::child()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to register SIGCHLD handler: {}", e);
                return;
            }
        };
        let mut sigwinch = match signal(SignalKind::window_change()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to register SIGWINCH handler: {}", e);
                return;
            }
        };
        'signal_loop: loop {
            if env.job_control.cancellation.load(Ordering::Acquire) {
                break;
            }
            // NOTE: tokio::select! polls branches in order of declaration.
            // This priority ordering is intentional: SIGINT is prioritized
            // over other signals like SIGTSTP or SIGTERM to ensure quick interactive responsiveness.
            tokio::select! {
                _ = sigint.recv() => {
                    let fg_job = { env.foreground_job() };
                    if fg_job.is_some() {
                        forward_signal(&env, libc::SIGINT);
                    } else {
                        println!();
                        let _ = std::io::Write::flush(&mut std::io::stdout());
                        env.job_control.sigint_pending.store(true, Ordering::SeqCst);
                        let env_clone = env.clone();
                        tokio::spawn(async move {
                            run_hooks("sigint", &env_clone).await;
                        });
                    }
                }
                _ = sigwinch.recv() => {
                    forward_signal(&env, libc::SIGWINCH);
                    let env_clone = env.clone();
                    tokio::spawn(async move {
                        run_hooks("sigwinch", &env_clone).await;
                    });
                }
                _ = sigtstp.recv() => {
                    let fg_job = { env.foreground_job() };
                    if fg_job.is_some() {
                        forward_signal(&env, libc::SIGTSTP);
                    } else if !env.is_command_running.load(Ordering::Relaxed)
                        && env.background_count.load(Ordering::Relaxed) == 0
                    {
                        unsafe { libc::raise(libc::SIGSTOP); }
                    } else if env.is_command_running.load(Ordering::Relaxed) {
                        // Builtin running in-process: abort it (can't suspend+resume)
                        env.job_control.sigint_pending.store(true, Ordering::SeqCst);
                    }
                    let env_clone = env.clone();
                    tokio::spawn(async move {
                        run_hooks("sigtstp", &env_clone).await;
                    });
                }
                _ = sigterm.recv() => {
                    forward_signal(&env, libc::SIGTERM);
                    let env_clone = env.clone();
                    tokio::spawn(async move {
                        run_hooks("sigterm", &env_clone).await;
                    });
                    env.job_control.cancellation.store(true, Ordering::SeqCst);
                    break 'signal_loop;
                }
                _ = sigquit.recv() => {
                    forward_signal(&env, libc::SIGQUIT);
                    let env_clone = env.clone();
                    tokio::spawn(async move {
                        run_hooks("sigquit", &env_clone).await;
                    });
                    env.job_control.cancellation.store(true, Ordering::SeqCst);
                    break 'signal_loop;
                }
                _ = sighup.recv() => {
                    forward_signal(&env, libc::SIGHUP);
                    let env_clone = env.clone();
                    tokio::spawn(async move {
                        run_hooks("sighup", &env_clone).await;
                    });
                    env.job_control.cancellation.store(true, Ordering::SeqCst);
                    break 'signal_loop;
                }
                _ = sigchld.recv() => {
                    let env_clone = env.clone();
                    tokio::spawn(async move {
                        run_hooks("sigchld", &env_clone).await;
                    });
                }
            }
        }
    });
}

#[allow(clippy::result_large_err)]
pub type BuiltinHandler = Arc<
    dyn Fn(Option<PipeStream>, Vec<Val>, &Env, PipeSender) -> Result<(), StringError> + Send + Sync,
>;

// Pipeline channel defaults

/// Default buffer size for inter-stage pipeline channels.
pub(crate) const PIPELINE_CHANNEL_SIZE: usize = 1024;

/// Read the configured pipeline channel size from env options, falling back to the constant.
pub fn pipeline_channel_size(env: &Env) -> usize {
    env.options.read().pipeline_channel_size
}

/// Pure pipeline finalizer: computes exit code and typed error from collected
/// error strings and the last stage's exit code. The `__condition_false__`
/// sentinel represents a logical `false` (filter false, test false) — exit 1
/// without hard-error rendering.
pub(crate) fn pipeline_finalize(
    errors: Vec<String>,
    last_ec: i64,
    pipefail: bool,
) -> (i64, Option<EngineError>) {
    let has_hard = errors.iter().any(|e| e != "__condition_false__");
    let has_any = !errors.is_empty();
    let exit_code = if has_any {
        if pipefail {
            if last_ec != 0 { last_ec } else { 1 }
        } else if has_hard {
            last_ec
        } else {
            1
        }
    } else {
        last_ec
    };
    let err = if errors.is_empty() {
        None
    } else if errors.iter().all(|e| e == "__condition_false__") {
        Some(EngineError::ConditionFalse { span: None })
    } else {
        let last_hard = errors
            .into_iter()
            .rfind(|e| e != "__condition_false__")
            .unwrap_or_else(|| "pipeline failed".to_string());
        Some(EngineError::PipelineError {
            message: last_hard,
            span: None,
        })
    };
    (exit_code, err)
}

/// Send a pipeline payload with try_send first for backpressure visibility.
/// Logs a warning (rate-limited) and bumps the backpressure counter when the channel is full.
#[allow(clippy::result_unit_err)]
pub async fn send_with_backpressure(
    env: &Env,
    tx: &PipeSender,
    payload: PipelinePayload,
) -> Result<(), ()> {
    match tx.try_send(payload) {
        Ok(()) => Ok(()),
        Err(tokio::sync::mpsc::error::TrySendError::Full(returned)) => {
            let _ = env.backpressure_count.fetch_add(1, Ordering::Relaxed);
            // Only warn on the first backpressure event — this is normal flow
            // control when the consumer is slower than the producer.
            // Subsequent events are silently handled by the blocking send().
            // Use FSH_DEBUG=1 for detailed backpressure tracking.
            if std::env::var("FSH_DEBUG").as_deref() == Ok("1") {
                let count = env.backpressure_count.load(Ordering::Relaxed);
                eprintln!("[fshell] pipeline backpressure ({} events)", count);
            }
            tx.send(returned).await.map_err(|_| ())
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => Err(()),
    }
}

// Git completion cache

pub const GIT_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(5);

/// Invalidate the git branch completion cache (called on chpwd).
pub fn invalidate_git_cache(env: &Env) {
    let mut cache = env.prompt.git_branch_cache.write();
    *cache = None;
}

// Hook system

/// Register a function name as a hook for the given event.
pub fn register_hook(event: &str, fn_name: &str, env: &Env) -> Result<(), String> {
    let valid = [
        "precmd", "preexec", "chpwd", "exit", "sigint", "sigterm", "sigusr1", "sigusr2", "sighup",
        "sigquit", "sigchld",
    ];
    if !valid.contains(&event) {
        return Err(format!("Unknown hook '{}'. Valid: {:?}", event, valid));
    }
    let mut hooks = env.hooks.registry.write();
    let entry = hooks.entry(event.to_string()).or_default();
    if !entry.contains(&fn_name.to_string()) {
        entry.push(fn_name.to_string());
    }
    Ok(())
}

/// Remove a function from a hook event.
pub fn remove_hook(event: &str, fn_name: &str, env: &Env) -> Result<(), String> {
    let mut hooks = env.hooks.registry.write();
    if let Some(list) = hooks.get_mut(event) {
        list.retain(|n| n != fn_name);
    }
    Ok(())
}

/// Get all hooks for an event.
pub fn get_hooks(event: &str, env: &Env) -> Vec<String> {
    let hooks = env.hooks.registry.read();
    hooks.get(event).cloned().unwrap_or_default()
}

/// True iff the given `FshDiag` represents a logical `false` condition
/// (exit 1, no error line), regardless of whether it was wrapped as a
/// `StringError::ConditionFalse` or an `EngineError::ConditionFalse`.
/// Centralizes the only place where the false-vs-hard-error distinction is
/// made so collectors and `&&`/`||` don't string-match.
#[allow(clippy::result_large_err)]
pub fn is_condition_false_diag(diag: &fshell_core::FshDiag) -> bool {
    if diag.is_condition_false() {
        return true;
    }
    // Also recognize EngineError::ConditionFalse wrapped as a report.
    diag.report
        .downcast_ref::<EngineError>()
        .is_some_and(|e| e.is_condition_false())
}

thread_local! {
    static IS_RUNNING_HOOK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Execute all registered hooks for an event. AWAITS inline — not spawned.
/// Skips if already inside a hook (prevents infinite recursion).
pub async fn run_hooks(event: &str, env: &Env) {
    let already_running = IS_RUNNING_HOOK.with(|f| f.get());
    if already_running {
        return;
    }
    IS_RUNNING_HOOK.with(|f| f.set(true));

    let hook_names = get_hooks(event, env);
    for fn_name in hook_names {
        let fn_body = {
            let fns = lock_fns!(env.fns.read());
            fns.get(&fn_name).cloned()
        };
        if let Some((params, _ret_type, body)) = fn_body {
            if !params.is_empty() {
                eprintln!(
                    "hook '{}' ignored: function '{}' takes parameters",
                    event, fn_name
                );
                continue;
            }
            let hook_env = env.clone();
            for stmt in &body {
                if let Err(e) = eval_stmt(stmt, &hook_env, false).await {
                    eprintln!("hook '{}' ({}): {}", event, fn_name, e);
                }
            }
        } else if let Some(builtin) = env.get_builtin(&fn_name) {
            let (tx, _rx) = tokio::sync::mpsc::channel(100);
            if let Err(e) = builtin(None, vec![], env, tx) {
                eprintln!("hook '{}' ({}): {}", event, fn_name, e);
            }
        } else {
            eprintln!("hook '{}' ignored: function '{}' not found", event, fn_name);
        }
    }

    IS_RUNNING_HOOK.with(|f| f.set(false));
}

/// Evaluate an `on <signal> { <handler> }` statement.
/// Registers a hook for the given signal event.
pub async fn dispatch_on_signal(
    signal: &str,
    handler: &fshell_core::OnHandler,
    env: &Env,
) -> Result<(), EngineError> {
    match handler {
        fshell_core::OnHandler::FunctionName(fn_name) => {
            register_hook(signal, fn_name, env).map_err(|e| EngineError::Generic {
                message: e,
                span: None,
            })?;
        }
        fshell_core::OnHandler::Block(body) => {
            let fn_name = format!("_on_{signal}");
            // Register the inline block as a synthetic function
            {
                let mut fns = env.fns.write();
                fns.insert(fn_name.clone(), (vec![], None, body.clone()));
            }
            register_hook(signal, &fn_name, env).map_err(|e| EngineError::Generic {
                message: e,
                span: None,
            })?;
        }
    }
    Ok(())
}

pub type FallbackHandler = Arc<
    dyn Fn(&str, Vec<Val>, Option<PipeStream>, &Env, PipeSender, bool) -> Result<(), StringError>
        + Send
        + Sync,
>;

// Extracted modules
pub mod eval;
pub mod format;
pub mod glob;
pub mod handoff;
pub mod pipeline;
pub mod suggestions;
#[cfg(test)]
pub mod tests;

pub use eval::{
    eval_expr, eval_stmt, invalidate_path_cache, is_external_command, is_external_command_cached,
    resolve_cached_command_path, warmup_path_cache,
};
pub use format::{cmp_vals, format_expr, format_pipeline};
pub use glob::expand_globs;
pub use pipeline::{
    collect_pipeline, execute_pipeline, populate_env_from_host, run_script, spawn_pipeline_stream,
};
pub use suggestions::{get_suggested_command, is_script_trusted, parse_json_value};

pub(crate) use eval::{
    PATH_CACHE, decode_csv_input, expand_alias_with_args, render_bar_chart, render_table,
    run_boundary_operator,
};

impl Env {
    fn val_to_host_string(val: &Val) -> String {
        match val {
            Val::String(s) => s.clone(),
            Val::Int(i) => i.to_string(),
            Val::Float(f) => f.to_string(),
            Val::Bool(b) => b.to_string(),
            Val::Null => String::new(),
            other => other.to_text(),
        }
    }

    /// Set a shell variable without exporting to the environment.
    pub fn set_shell_var(&self, key: &str, val: Val) {
        if let Some(ref locals) = self.local_vars {
            locals.write().insert(key.to_string(), val.clone());
        }
        self.vars.write().insert(key.to_string(), val);
    }

    /// Set a variable and export it to the environment (vars + env map + host).
    pub fn set_exported_var(&self, key: &str, val: Val) {
        let host_str = Self::val_to_host_string(&val);
        if let Some(ref locals) = self.local_vars {
            locals.write().insert(key.to_string(), val.clone());
        }
        {
            let mut vars = self.vars.write();
            vars.insert(key.to_string(), val.clone());
            self.ensure_env_map_insert(&mut vars, key, val);
        }
        unsafe {
            std::env::set_var(key, host_str);
        }
        self.is_env_modified.store(true, Ordering::Release);
        if key == "PATH" {
            crate::eval::invalidate_path_cache();
        }
    }

    /// Promote an existing shell variable to the environment (export without value).
    /// Returns true if the variable existed and was exported.
    pub fn export_existing_var(&self, key: &str) -> bool {
        let val_opt = self.vars.read().get(key).cloned();
        if let Some(val) = val_opt {
            let host_str = Self::val_to_host_string(&val);
            {
                let mut vars = self.vars.write();
                self.ensure_env_map_insert(&mut vars, key, val);
            }
            unsafe {
                std::env::set_var(key, host_str);
            }
            self.is_env_modified.store(true, Ordering::Release);
            if key == "PATH" {
                crate::eval::invalidate_path_cache();
            }
            true
        } else {
            false
        }
    }

    /// Remove a variable from shell vars and environment.
    pub fn unset_var(&self, key: &str) {
        if let Some(ref locals) = self.local_vars {
            locals.write().remove(key);
        }
        {
            let mut vars = self.vars.write();
            vars.remove(key);
            if let Some(Val::Map(map)) = vars.get_mut("env") {
                map.swap_remove(&ustr::ustr(key));
            }
        }
        unsafe {
            std::env::remove_var(key);
        }
        self.is_env_modified.store(true, Ordering::Release);
        if key == "PATH" {
            crate::eval::invalidate_path_cache();
        }
    }

    fn ensure_env_map_insert(&self, vars: &mut FxHashMap<String, Val>, key: &str, val: Val) {
        match vars.get_mut("env") {
            Some(Val::Map(map)) => {
                map.insert(ustr::ustr(key), val);
            }
            _ => {
                let mut env_map =
                    fshell_core::FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
                for (k, v) in std::env::vars() {
                    env_map.insert(ustr::ustr(&k), Val::String(v));
                }
                env_map.insert(ustr::ustr(key), val);
                vars.insert("env".to_string(), Val::Map(env_map));
            }
        }
    }
}
pub(crate) use glob::{SUGGESTION_CACHE, SuggestionCache};
pub(crate) use pipeline::{collect_pipeline_silent, val_type_precedence};
fn topological_sort(
    nodes: &std::collections::HashSet<String>,
    deps: &FxHashMap<String, std::collections::HashSet<String>>,
) -> Result<Vec<String>, EngineError> {
    let mut visited = std::collections::HashSet::new();
    let mut temp = std::collections::HashSet::new();
    let mut order = Vec::new();

    fn visit(
        node: &str,
        deps: &FxHashMap<String, std::collections::HashSet<String>>,
        visited: &mut std::collections::HashSet<String>,
        temp: &mut std::collections::HashSet<String>,
        order: &mut Vec<String>,
    ) -> Result<(), EngineError> {
        if temp.contains(node) {
            return Err(EngineError::CycleDetected { span: None });
        }
        if !visited.contains(node) {
            temp.insert(node.to_string());
            if let Some(node_deps) = deps.get(node) {
                for dep in node_deps {
                    visit(dep, deps, visited, temp, order)?;
                }
            }
            temp.remove(node);
            visited.insert(node.to_string());
            order.push(node.to_string());
        }
        Ok(())
    }

    for node in nodes {
        visit(node, deps, &mut visited, &mut temp, &mut order)?;
    }

    Ok(order)
}

#[allow(clippy::type_complexity)]
async fn trigger_eval(
    name: &str,
    cells: &FxHashMap<String, (fshell_core::Pipeline, watch::Sender<Arc<Vec<Val>>>)>,
    deps: &mut FxHashMap<String, std::collections::HashSet<String>>,
    watched_paths: &mut FxHashMap<String, std::collections::HashSet<PathBuf>>,
    watchers: &mut FxHashMap<String, notify::RecommendedWatcher>,
    env: &Env,
) {
    use notify::Watcher;

    let (pipeline, tx) = match cells.get(name) {
        Some(c) => c,
        None => return,
    };

    let env_clone = env.clone();
    *env_clone.reactive.tracked_reads.write() = Some(std::collections::HashSet::new());
    *env_clone.reactive.tracked_cells.write() = Some(std::collections::HashSet::new());
    env_clone
        .reactive
        .tracking_active
        .store(true, Ordering::Release);

    let vals = collect_pipeline_silent(pipeline, &env_clone).await;
    let _ = tx.send(Arc::new(vals));

    env_clone
        .reactive
        .tracking_active
        .store(false, Ordering::Release);
    let tracked_paths = env_clone
        .reactive
        .tracked_reads
        .write()
        .take()
        .unwrap_or_default();
    let tracked_cells = env_clone
        .reactive
        .tracked_cells
        .write()
        .take()
        .unwrap_or_default();

    deps.insert(name.to_string(), tracked_cells);

    if !watchers.contains_key(name) {
        let trigger_tx = env.reactive.tx.clone();
        let cell_name = name.to_string();
        if let Ok(w) = notify::RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                if res.is_ok() {
                    let _ = trigger_tx.blocking_send(ReactiveEvent::TriggerCell(cell_name.clone()));
                }
            },
            notify::Config::default(),
        ) {
            watchers.insert(name.to_string(), w);
        }
    }

    if let Some(w) = watchers.get_mut(name) {
        let old_paths = watched_paths.entry(name.to_string()).or_default();
        for path in old_paths.iter() {
            if !tracked_paths.contains(path) {
                let _ = w.unwatch(path);
            }
        }
        for path in tracked_paths.iter() {
            if !old_paths.contains(path) {
                let _ = w.watch(path, notify::RecursiveMode::NonRecursive);
            }
        }
        *old_paths = tracked_paths;
    }
}

pub async fn run_reactive_scheduler(env: Env, mut rx: tokio::sync::mpsc::Receiver<ReactiveEvent>) {
    #[allow(clippy::type_complexity)]
    let mut cells: FxHashMap<String, (fshell_core::Pipeline, watch::Sender<Arc<Vec<Val>>>)> =
        FxHashMap::default();
    let mut deps: FxHashMap<String, std::collections::HashSet<String>> = FxHashMap::default();
    let mut watchers: FxHashMap<String, notify::RecommendedWatcher> = FxHashMap::default();
    let mut watched_paths: FxHashMap<String, std::collections::HashSet<PathBuf>> =
        FxHashMap::default();

    while let Some(event) = rx.recv().await {
        if env.job_control.cancellation.load(Ordering::Acquire) {
            break;
        }
        let mut dirty = std::collections::HashSet::new();

        match event {
            ReactiveEvent::RegisterCell { name, pipeline, tx } => {
                cells.insert(name.clone(), (pipeline, tx));
                dirty.insert(name.clone());
            }
            ReactiveEvent::TriggerCell(name) => {
                dirty.insert(name.clone());
            }
        }

        // Drain all pending events to avoid losing events under load
        while let Ok(next_event) = rx.try_recv() {
            match next_event {
                ReactiveEvent::RegisterCell { name, pipeline, tx } => {
                    cells.insert(name.clone(), (pipeline, tx));
                    dirty.insert(name.clone());
                }
                ReactiveEvent::TriggerCell(name) => {
                    dirty.insert(name.clone());
                }
            }
        }

        let all_names: std::collections::HashSet<String> = cells.keys().cloned().collect();
        let order = match topological_sort(&all_names, &deps) {
            Ok(o) => o,
            Err(_) => {
                eprintln!("\x1b[1;31mReactive cell cycle detected — skipping evaluation.\x1b[0m");
                continue;
            }
        };

        let mut affected = std::collections::HashSet::new();
        for d in &dirty {
            affected.insert(d.clone());
        }

        // Trace downstream dependencies
        for node in &order {
            if let Some(node_deps) = deps.get(node)
                && node_deps.iter().any(|d| affected.contains(d))
            {
                affected.insert(node.clone());
            }
        }

        // Re-evaluate affected cells in exact topological order
        for node in &order {
            if affected.contains(node) {
                trigger_eval(
                    node,
                    &cells,
                    &mut deps,
                    &mut watched_paths,
                    &mut watchers,
                    &env,
                )
                .await;
            }
        }
    }
}

pub(crate) fn json_to_val(v: serde_json::Value) -> fshell_core::Val {
    v.into()
}

pub(crate) fn val_to_json(v: &fshell_core::Val) -> serde_json::Value {
    v.into()
}
