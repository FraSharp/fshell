// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

pub mod agent;
pub mod ansi;
pub mod buffer;
pub mod capture;
pub mod clipboard;
pub mod completions;
pub mod cursor;
pub mod history;
pub mod margins;
pub mod mouse;
pub mod prompt;
pub mod raw;
pub mod statusbar;
pub mod widget_explorer;
pub mod widgets;

use chrono::TimeZone;
use crossterm::event::{
    self, Event, KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{
    Terminal, TerminalOptions, Viewport,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, BorderType, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Table,
    },
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::Notify;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

// CPU usage debug logging (FSH_DBG_CPU_USG=1)
/// Log a message to a file if FSH_DBG_CPU_USG is set.
/// File is `$TMPDIR/fsh_cpu_dbg.log` so it survives terminal closure.
pub(crate) fn cpu_dbg_log(msg: std::fmt::Arguments) {
    if std::env::var("FSH_DBG_CPU_USG").as_deref() != Ok("1") {
        return;
    }
    use std::io::Write;
    let path = format!(
        "{}/fsh_cpu_dbg.log",
        std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string())
    );
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(
            f,
            "[{}] [pid={}] {}",
            chrono::Local::now().format("%H:%M:%S%.3f"),
            std::process::id(),
            msg
        );
    }
}
#[macro_export]
macro_rules! cpu_dbg {
    ($($arg:tt)*) => {
        $crate::ftui::cpu_dbg_log(format_args!($($arg)*))
    };
}

/// Legacy per-command guard, still used as the fallback when the session-wide
/// `raw::Session` path is disabled (FSH_RAW_SESSION=0). New code should prefer
/// `raw::Session` + `SuspendGuard`.
struct TuiGuard;

impl TuiGuard {
    fn new() -> Self {
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::cursor::DisableBlinking,
            crossterm::event::EnableBracketedPaste,
            crossterm::event::EnableFocusChange,
            crossterm::event::EnableMouseCapture,
        );
        Self
    }
}

impl Drop for TuiGuard {
    fn drop(&mut self) {
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::Clear(crossterm::terminal::ClearType::FromCursorDown),
            crossterm::event::DisableBracketedPaste,
            crossterm::event::DisableFocusChange,
            crossterm::event::DisableMouseCapture,
            crossterm::cursor::Show,
            crossterm::cursor::EnableBlinking,
        );
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

use crate::alias_expansion::AliasExpansionState;
use crate::highlighter::FshellHighlighter;
use crate::theme_ext::ThemeColorRatatui;
use fshell_engine::Env;
use reedline::Hinter;

use agent::AgentModeState;
use buffer::TextBuffer;
use completions::{CompletionsManager, extract_partial_word};
use cursor::{Coord, CursorConfig, CursorState};
use history::HistoryManager;
use mouse::{MouseMode, MouseStateManager};
use prompt::PromptManager;
use statusbar::{StatusBar, StatusBarWidget};

// Signal handling for job control (Bug 5.2)
//
// SIGTSTP (Ctrl+Z) is a challenge in TUI apps because:
//   1. The default handler suspends the process immediately
//   2. Signal handlers CANNOT safely call most functions (tcsetattr, malloc, etc.)
//   3. Raw mode is left enabled, corrupting the terminal
//
// Our approach:
//   - Only use async-signal-safe write() in the actual signal handler
//   - Set a flag for the event loop to handle the heavy lifting
//   - The event loop checks flags and restores terminal state properly
//
// We use a static volatile flag (AtomicBool is fine for x86/x64).
// sigprocmask blocks SIGTSTP/SIGCONT during the critical section.
#[cfg(unix)]
static DID_SUSPEND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Flag set by the real SIGHUP signal handler. Checked by the REPL input loop
/// to exit cleanly when the controlling terminal is lost. We need a real signal
/// handler (not tokio::signal) because the single-threaded tokio runtime can't
/// poll its signal futures while crossterm::event::poll() blocks the thread.
#[cfg(unix)]
static GOT_SIGHUP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(unix)]
fn install_sigtstp_handler() {
    unsafe {
        install_sigaction(libc::SIGTSTP, sigtstp_action as *const () as usize);
        install_sigaction(libc::SIGCONT, sigcont_action as *const () as usize);
        install_sigaction(libc::SIGHUP, sighup_action as *const () as usize);
    }
}

#[cfg(unix)]
unsafe fn install_sigaction(sig: libc::c_int, handler: usize) {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handler as libc::sighandler_t;
        sa.sa_flags = 0;
        libc::sigemptyset(&mut sa.sa_mask);
        let _ = libc::sigaddset(&mut sa.sa_mask, sig);
        if sig == libc::SIGTSTP {
            let _ = libc::sigaddset(&mut sa.sa_mask, libc::SIGCONT);
        }
        libc::sigaction(sig, &sa, std::ptr::null_mut());
    }
}

#[cfg(unix)]
extern "C" fn sighup_action(_sig: i32) {
    // SAFETY: AtomicBool::store and libc::_exit are async-signal-safe on all POSIX platforms.
    GOT_SIGHUP.store(true, std::sync::atomic::Ordering::Relaxed);
    unsafe {
        libc::_exit(0);
    }
}

#[cfg(unix)]
extern "C" fn sigtstp_action(_sig: i32) {
    // Use only async-signal-safe ops: write(), sigaction/signal, raise().
    // Must not call malloc/tcsetattr. We temporarily install SIG_DFL,
    // raise, then restore via sigaction (which is async-signal-safe).
    unsafe {
        let fd = libc::STDOUT_FILENO;
        let seq = b"\x1b[?25h\x1b[?1000l\x1b[?2004l";
        libc::write(fd, seq.as_ptr() as *const _, seq.len());
        DID_SUSPEND.store(true, std::sync::atomic::Ordering::Relaxed);
        // Atomically swap to DFL and suspend. Use sigaction to avoid
        // the `signal` re-install race — save old action and restore it.
        let mut old_sa: libc::sigaction = std::mem::zeroed();
        let mut dfl: libc::sigaction = std::mem::zeroed();
        dfl.sa_sigaction = libc::SIG_DFL;
        libc::sigemptyset(&mut dfl.sa_mask);
        libc::sigaction(libc::SIGTSTP, &dfl, &mut old_sa);
        libc::raise(libc::SIGTSTP);
        libc::sigaction(libc::SIGTSTP, &old_sa, std::ptr::null_mut());
    }
}

#[cfg(unix)]
extern "C" fn sigcont_action(_sig: i32) {
    // Nothing to do here. The event loop checks DID_SUSPEND after
    // every poll to re-init the terminal. We cannot safely call
    // enable_raw_mode() from a signal handler.
    //
    // signal-safety note: Atomic store is implementation-defined safe
    // on x86/x64 but not on all architectures. We rely on the fact that
    // sigtstp_action already set DID_SUSPEND before raising SIGTSTP.
    // This handler mainly exists so SIGCONT doesn't kill us.
}

#[allow(clippy::collapsible_if)]
pub async fn run_ftui_repl(
    mut env: Env,
    session_id: String,
    init_done: Arc<Notify>,
    clear_screen: bool,
) {
    // Install panic hook to restore terminal state on panic.
    // This is a safety net: TuiGuard::Drop handles normal cleanup during
    // unwinding, but the default panic hook prints to stderr which may be
    // garbled in the alternate screen. This hook ensures a clean restore
    // and a readable panic message.
    {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Safe to write to stdout in the panic hook even if pipe is broken
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::event::DisableBracketedPaste,
                crossterm::event::DisableFocusChange,
                crossterm::event::DisableMouseCapture,
                crossterm::cursor::Show,
                crossterm::cursor::EnableBlinking,
            );
            let _ = crossterm::terminal::disable_raw_mode();
            let _ = std::io::Write::flush(&mut std::io::stdout());
            // Call the default hook to print the actual panic message
            default_hook(info);
        }));
    }

    // Install SIGTSTP handler (Bug 5.2): mark DID_SUSPEND on SIGTSTP/SIGCONT.
    // The event loop checks this flag to re-init terminal state after resume.
    #[cfg(unix)]
    install_sigtstp_handler();

    // Session-wide raw mode (A2 rewrite, now default — A/B via
    // `FSH_RAW_SESSION=0`): enter raw once for the whole interactive
    // session. Child commands that need a real terminal (vim, less, ssh, …)
    // borrow a SuspendGuard which drops raw for the duration of the command
    // and re-enters on Drop — even on panic/ctrl-c. Set
    // `FSH_RAW_SESSION=0` to fall back to legacy per-command toggle.
    let mut _raw_session: Option<Box<raw::Session>> = if std::env::var("FSH_RAW_SESSION").as_deref()
        == Ok("0")
    {
        None
    } else {
        match raw::Session::enter() {
            Ok(s) => Some(Box::new(s)),
            Err(e) => {
                eprintln!(
                    "\r\n\x1b[1;31merror:\x1b[0m FTUI raw session failed: {e} — falling back to per-command raw"
                );
                None
            }
        }
    };

    // Wait for deferred initialization (login shell env, PATH cache warmup) to finish
    init_done.notified().await;
    drop(init_done);

    if clear_screen {
        print!("\x1B[2J\x1B[1;1H");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }

    // Set up TUI state managers
    let alias_state = Arc::new(AliasExpansionState::new());
    let initial_aliases: std::collections::HashMap<String, String> =
        env.get_all_aliases().into_iter().collect();
    alias_state.update_registered(initial_aliases);

    // Initialize highlighter (include common external commands so they're recognized without a PATH lookup)
    let mut builtins = env.get_all_builtins();
    for &(name, _) in crate::COMMON_EXTERNAL_COMMANDS {
        if !builtins.contains(&name.to_string()) {
            builtins.push(name.to_string());
        }
    }
    let mut highlighter = FshellHighlighter::new(builtins)
        .with_env(env.clone())
        .with_alias_state(alias_state.clone());

    let mut prompt_mgr = PromptManager::new(env.clone());
    let mut text_buf = TextBuffer::new();
    let mut cursor_state = CursorState::new();
    let mut comp_mgr = CompletionsManager::new(env.clone());
    let mut mouse_mgr = MouseStateManager::new();
    let mut history_mgr = HistoryManager::new();
    let mut agent_state = AgentModeState::new();

    // Initialize FshellHinter for inline hints (history, path, and completion hints)
    let mut hinter = crate::hinter::FshellHinter::default().with_env(env.clone());
    let mut current_hint = String::new();

    let mut current_dir;

    let hostname = crate::history::get_hostname();
    let username = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
    let mut status_bar = StatusBar::new(hostname.clone(), username).with_env(&env);
    mouse_mgr.set_mode(MouseMode::Smart);

    // Event poll timeout (no longer drives redraws — only on input)
    let tick_rate = Duration::from_millis(50);

    let mut exit_repl = false;
    let mut in_continuation = false;

    let mut temp_input = String::new();
    let mut history_index: Option<usize> = None;
    let mut filtered_history: Vec<String> = Vec::new();
    let mut text_scroll_offset = 0;
    let mut help_visible = false;
    let mut widget_explorer = widget_explorer::WidgetExplorerManager::new();
    let mut output_lines: Vec<String> = Vec::new();
    // A6 long-term: output cap is unified — anchored mode sizes the output to
    // the actual viewport height available after prompt+popup. The 500-line
    // safety cap from the phase-1 patch is now retired; truncation reuses
    // `cap` so long outputs obey the same anchored pane boundary.
    const ANCHORED_OUTPUT_SAFETY_CAP: usize = 2000;
    let mut drag_anchor: Option<usize> = None;
    let anchor_output = std::env::var("FSH_REPL_ANCHOR_OUTPUT").as_deref() == Ok("1");

    'repl_loop: loop {
        status_bar.visible = StatusBar::is_enabled(&env);

        // Session-wide raw healing: a child (e.g. `cargo run` launching a
        // nested `fsh` with `FSH_RAW_SESSION=1`) shares the same PTY and may
        // have called `disable_raw_mode` on exit via `Session::drop`/`process::exit`.
        // The parent still thinks it is raw, so the next iteration would render
        // while the kernel is actually cooked → `\n` without `\r` smear seen
        // after `exit` (reported). Re-assert raw at the top of every iteration
        // when a session is active — idempotent.
        if let Some(s) = _raw_session.as_deref() {
            s.reenter_raw();
        }

        // Check if we resumed from SIGTSTP. The signal handler wrote minimal
        // reset sequences before suspending; the main loop re-enters raw and
        // auxiliary modes. With a session-wide `raw::Session` the per-command
        // re-enable below is skipped (raw never left).
        #[cfg(unix)]
        if DID_SUSPEND.swap(false, std::sync::atomic::Ordering::Relaxed) {
            if let Some(s) = _raw_session.as_deref() {
                s.reenter_raw();
            } else {
                let _ = crossterm::execute!(
                    std::io::stdout(),
                    crossterm::cursor::Show,
                    crossterm::style::Print("\r\n"),
                );
            }
        }

        // 1. Prepare terminal for this interactive step.
        //
        // With `FSH_RAW_SESSION=1` raw is already held by `raw::Session` for
        // the whole session — no per-command toggle, so no garble from
        // enable/disable interleaving with child output. Without the flag we
        // keep the legacy per-command behavior so the pane rewrite can be
        // A/B'd. A2's complete migration (raw always session-wide) will
        // remove this branch.
        current_dir = env.cwd().to_string_lossy().to_string();
        prompt_mgr.refresh_snapshot(&current_dir);

        let _guard: Option<TuiGuard> = if _raw_session.is_some() {
            None
        } else {
            if let Err(e) = crossterm::terminal::enable_raw_mode() {
                eprintln!("\r\n\x1b[1;31merror:\x1b[0m FTUI failed to enable raw mode: {e}");
                break 'repl_loop;
            }
            Some(TuiGuard::new())
        };

        if mouse_mgr.mode != MouseMode::Disabled {
            mouse_mgr.is_captured = false;
            // In session-wide raw mode auxiliary modes are already on from
            // `raw::Session::enter`; re-enabling is idempotent, so just call
            // through so Smart mode's `is_captured` stays coherent.
            mouse_mgr.enable_capture();
        }

        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::Clear(crossterm::terminal::ClearType::CurrentLine),
            crossterm::cursor::MoveToColumn(0),
        );

        let theme = env.active_theme();
        highlighter.update_theme(theme.clone());
        comp_mgr.update_theme(theme.clone());
        let mut terminal: Option<Terminal<CrosstermBackend<std::io::Stdout>>> = None;
        let mut status_terminal: Option<Terminal<CrosstermBackend<std::io::Stdout>>> = None;
        let mut current_viewport_height = 0u16;
        let mut prompt_origin_y: Option<u16> = None;
        let mut _last_relative_cursor_y = 0u16;
        let mut last_status_state: Option<StatusBarState> = None;
        let mut resized = false;
        let mut last_resize = std::time::Instant::now();

        let mut redraw = true;
        let mut command_to_execute = None;
        let mut aborted_command = None;
        let mut input_iter: u64 = 0;

        // Spin detector
        // If the input loop completes 500 iterations in under 50ms without blocking,
        // the process is stuck in an unblocked tight loop (e.g. crossterm spinning on
        // EOF after cmux pane / PTY disconnect). Break repl_loop to exit cleanly without 100% CPU.
        let mut spin_window_start = std::time::Instant::now();
        let mut spin_window_count: u64 = 0;
        const SPIN_WINDOW_THRESHOLD: u64 = 500;
        const SPIN_WINDOW_DURATION: Duration = Duration::from_millis(50);

        // ignoreeof handling: first Ctrl-D with empty buffer warns, second exits
        let mut eof_pending = false;

        // Capability prompt handler: drain the engine's cap_prompt channel and
        // interactively ask the user (suspending raw mode). Without this, TUI
        // strict-mode denials block 30s then deny.
        {
            let env_cap = env.clone();
            tokio::spawn(async move {
                let rx = {
                    let mut guard = match env_cap.caps.cap_prompt_rx.lock() {
                        Ok(g) => g,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    guard.take()
                };
                if let Some(mut rx) = rx {
                    while let Some(req) = rx.recv().await {
                        let _ = crossterm::terminal::disable_raw_mode();
                        let _ = crossterm::execute!(std::io::stdout(), crossterm::cursor::Show);
                        eprint!(
                            "\r\n[fshell] Allow '{}' to {:?}? [y/N/a] ",
                            req.cmd_name, req.action
                        );
                        let _ = std::io::Write::flush(&mut std::io::stderr());
                        let line = tokio::task::spawn_blocking(|| {
                            let mut s = String::new();
                            let _ = std::io::stdin().read_line(&mut s);
                            s
                        })
                        .await
                        .unwrap_or_default();
                        let resp = match line.trim().to_lowercase().as_str() {
                            "y" | "yes" => fshell_engine::CapPromptResponse::GrantOnce,
                            "a" | "always" => {
                                let handle_opt = match &req.action {
                                    fshell_engine::CapAction::ReadDir(p) => {
                                        Some(fshell_core::ResourceHandle::ReadDir(p.clone()))
                                    }
                                    fshell_engine::CapAction::WriteDir(p) => {
                                        Some(fshell_core::ResourceHandle::WriteDir(p.clone()))
                                    }
                                    fshell_engine::CapAction::ReadFile(p) => {
                                        Some(fshell_core::ResourceHandle::ReadFile(p.clone()))
                                    }
                                    fshell_engine::CapAction::WriteFile(p) => {
                                        Some(fshell_core::ResourceHandle::WriteFile(p.clone()))
                                    }
                                    fshell_engine::CapAction::Network(h) if h == "any" => {
                                        Some(fshell_core::ResourceHandle::NetworkAll)
                                    }
                                    fshell_engine::CapAction::Network(h) => {
                                        Some(fshell_core::ResourceHandle::NetworkSocket(h.clone()))
                                    }
                                    fshell_engine::CapAction::ReadEnv(v) => {
                                        Some(fshell_core::ResourceHandle::ReadEnv(v.clone()))
                                    }
                                    fshell_engine::CapAction::WriteEnv(v) => {
                                        Some(fshell_core::ResourceHandle::WriteEnv(v.clone()))
                                    }
                                    fshell_engine::CapAction::ProcessSpawn => {
                                        Some(fshell_core::ResourceHandle::ProcessSpawn)
                                    }
                                };
                                if let Some(h) = handle_opt {
                                    env_cap.caps.caps.write().grant(h);
                                }
                                fshell_engine::CapPromptResponse::GrantAlways
                            }
                            _ => fshell_engine::CapPromptResponse::Deny,
                        };
                        let _ = req.response_tx.send(resp);
                        let _ = crossterm::terminal::enable_raw_mode();
                        let _ = crossterm::execute!(std::io::stdout(), crossterm::cursor::Hide);
                    }
                }
            });
        }

        cpu_dbg!("--- entering input_loop ---");
        // 2. Interactive input entry loop
        'input_loop: loop {
            input_iter += 1;

            // Spin detection (PTY disconnect / EOF loop guard)
            spin_window_count += 1;
            if spin_window_count >= SPIN_WINDOW_THRESHOLD {
                let elapsed = spin_window_start.elapsed();
                if elapsed < SPIN_WINDOW_DURATION {
                    cpu_dbg!(
                        "SPIN DETECTED: {} iterations in {:?} — breaking repl_loop",
                        spin_window_count,
                        elapsed
                    );
                    break 'repl_loop;
                }
                // Window expired, reset
                spin_window_start = std::time::Instant::now();
                spin_window_count = 0;
            }

            // TTY health / signal checks
            #[cfg(unix)]
            if GOT_SIGHUP.load(Ordering::Relaxed) {
                cpu_dbg!("GOT_SIGHUP — breaking repl_loop");
                break 'repl_loop;
            }

            // Check the engine's cancellation flag (set by tokio signal
            // handler for SIGHUP/SIGTERM/SIGQUIT when the runtime does get
            // a chance to poll).
            if env.job_control.cancellation.load(Ordering::Relaxed) {
                cpu_dbg!("cancellation flag set — breaking repl_loop");
                break 'repl_loop;
            }
            // Poll for background AI agent results
            if let Ok(mut res_guard) = agent::AGENT_RESULT.lock()
                && let Some((qid, res)) = res_guard.take()
                && qid == agent_state.query_id
            {
                agent_state.is_loading = false;
                match res {
                    Ok(cmd) => {
                        agent_state.result_command = Some(cmd.clone());
                        text_buf.clear();
                        text_buf.insert_str(&cmd);
                    }
                    Err(err) => {
                        agent_state.error_msg = Some(err);
                    }
                }
                redraw = true;
            }

            // Update prompt timers / background widgets (triggers at most every 1s internally)
            if prompt_mgr.update() {
                redraw = true;
            }

            if redraw {
                let display_text = text_buf.text();

                let validation = if display_text.trim().is_empty() {
                    fshell_core::ValidationResult::Complete
                } else {
                    fshell_core::validate_input(&display_text)
                };

                // B2: dynamic viewport height — only uses the space actually needed
                let multi_line_count = if display_text.contains('\n') {
                    display_text.split('\n').count().max(1) as u16
                } else {
                    1
                };
                let prompt_h = if multi_line_count > 1 {
                    multi_line_count + 2 // 1 header + N code lines + 1 footer
                } else {
                    1
                };
                let (term_w, term_h) = crossterm::terminal::size().unwrap_or((80, 24));
                let cap = if status_bar.visible {
                    term_h.saturating_sub(2)
                } else {
                    term_h
                };
                let popup_needed_lines: u16 =
                    if history_mgr.active || agent_state.active || widget_explorer.active {
                        10
                    } else if comp_mgr.visible {
                        10.min(cap)
                    } else if help_visible {
                        6
                    } else {
                        0
                    };

                // A1+A6: viewport now grows to the terminal height when
                // there is anchored output (true fixed-height anchored pane).
                // Otherwise it stays compact to prompt+popup (no flicker on
                // popup open/close — the terminal keeps its height and popups
                // render in unused rows). This is the durable A1 fix;
                // B2's growth-only Inline semantics are replaced by a stable
                // fixed height once output is present.
                let prompt_and_popup_h = (prompt_h + popup_needed_lines).max(1).min(cap);
                let needed_height = if output_lines.is_empty() {
                    prompt_and_popup_h
                } else if popup_needed_lines > 0 {
                    // Popup visible while anchored output present: keep anchored
                    // output in the viewport but prioritize popup space.
                    let min_for_content = prompt_and_popup_h;
                    let min_for_output = 1u16; // at least one output line
                    let max_for_output = cap.saturating_sub(min_for_content);
                    let shown_output =
                        (output_lines.len() as u16).min(max_for_output.max(min_for_output));
                    (shown_output + min_for_content).min(cap)
                } else {
                    // Anchored pane: output gets the height that remains after
                    // reserving prompt, truncated to cap. This ties truncation
                    // to the viewport rather than an arbitrary constant (A6).
                    let reserved = prompt_h.max(1).min(cap);
                    let max_output_h = cap.saturating_sub(reserved).max(1);
                    let shown_output = (output_lines.len() as u16).min(max_output_h);
                    (shown_output + reserved).min(cap)
                };

                if prompt_origin_y.is_none() {
                    prompt_origin_y = safe_cursor_position().map(|(_, y)| y);
                }

                let limit_row = if status_bar.visible {
                    term_h.saturating_sub(2)
                } else {
                    term_h
                };

                let mut scroll_d = 0u16;
                let mut must_recreate = false;
                let mut origin_y = prompt_origin_y.unwrap_or(0);
                if origin_y + needed_height > limit_row {
                    let d = (origin_y + needed_height).saturating_sub(limit_row);
                    if d > 0 {
                        scroll_d = d;
                        must_recreate = true;
                        let mut stdout = std::io::stdout();
                        let _ = crossterm::execute!(
                            stdout,
                            crossterm::cursor::MoveTo(0, term_h.saturating_sub(1))
                        );
                        for _ in 0..d {
                            let _ = crossterm::execute!(
                                stdout,
                                crossterm::style::Print("\r\n"),
                                crossterm::cursor::MoveToColumn(0),
                            );
                        }
                        origin_y = origin_y.saturating_sub(d);
                        prompt_origin_y = Some(origin_y);
                        let _ = crossterm::execute!(stdout, crossterm::cursor::MoveTo(0, origin_y));
                        let _ = std::io::Write::flush(&mut stdout);
                    }
                }

                if terminal.is_none()
                    || current_viewport_height != needed_height
                    || resized
                    || must_recreate
                {
                    if resized {
                        status_terminal = None;
                        prompt_origin_y = None;
                    }
                    resized = false;
                    if let Some(t) = terminal.take() {
                        drop(t);
                    }
                    let old_height = current_viewport_height;
                    current_viewport_height = needed_height;

                    // Erase the rows that formed the previous viewport and will form the new viewport from origin_y down
                    let rows_to_erase = old_height
                        .max(needed_height)
                        .min(limit_row.saturating_sub(origin_y));
                    let mut stdout = std::io::stdout();
                    for row in 0..rows_to_erase {
                        let _ = crossterm::execute!(
                            stdout,
                            crossterm::cursor::MoveTo(0, origin_y + row),
                            crossterm::terminal::Clear(crossterm::terminal::ClearType::CurrentLine,),
                        );
                    }
                    // Place cursor at origin_y so ratatui's Inline(needed_height) binds to origin_y
                    let _ = crossterm::execute!(stdout, crossterm::cursor::MoveTo(0, origin_y));
                    let _ = std::io::Write::flush(&mut stdout);

                    let stdout = std::io::stdout();
                    let backend = CrosstermBackend::new(stdout);
                    if let Ok(t) = Terminal::with_options(
                        backend,
                        TerminalOptions {
                            viewport: Viewport::Inline(needed_height),
                        },
                    ) {
                        terminal = Some(t);
                    }
                }

                // Create or recreate status terminal if needed.
                if status_terminal.is_none() && status_bar.visible {
                    if term_h > 2 {
                        let stdout = std::io::stdout();
                        let backend = CrosstermBackend::new(stdout);
                        if let Ok(st) = Terminal::with_options(
                            backend,
                            TerminalOptions {
                                viewport: Viewport::Fixed(Rect::new(0, term_h - 2, term_w, 2)),
                            },
                        ) {
                            status_terminal = Some(st);
                        }
                    }
                }

                // Render status bar BEFORE the inline draw, so the inline
                // terminal's set_cursor_position is the last cursor command
                // flushed — no save/restore needed.
                if status_bar.visible {
                    // R7: timestamp changes every second and was included in
                    // StatusBarState diff, forcing a full status-bar redraw every
                    // wake even when idle. Exclude it from the diff — the bar's
                    // Widget reads Local::now() itself at render time.
                    let current_state = StatusBarState {
                        last_exit_code: status_bar.last_exit_code,
                        git_branch: status_bar.git_branch.clone(),
                        git_dirty: status_bar.git_dirty,
                        git_ahead: status_bar.git_ahead,
                        git_behind: status_bar.git_behind,
                        job_count: status_bar.job_count,
                        mode_indicator: status_bar.mode_indicator.clone(),
                        last_command_elapsed: status_bar.last_command_elapsed,
                        visible: status_bar.visible,
                        term_w,
                        term_h,
                    };

                    let need_draw = last_status_state.as_ref() != Some(&current_state)
                        || must_recreate
                        || scroll_d > 0;

                    if need_draw {
                        if let Some(st) = status_terminal.as_mut() {
                            if scroll_d > 0 || must_recreate {
                                let _ = st.clear();
                            }
                            let _ = st.draw(|f| {
                                f.render_widget(
                                    StatusBarWidget {
                                        status_bar: &status_bar,
                                        theme: &theme,
                                    },
                                    f.area(),
                                );
                            });
                            last_status_state = Some(current_state);
                        }
                    }
                }

                if let Some(term) = terminal.as_mut() {
                    let cursor_config = CursorConfig {
                        effect: cursor::CursorEffect::None,
                        ..CursorConfig::default()
                    };

                    // Calculate logical cursor layout coordinate
                    let prompt_left = prompt_mgr.render_prompt_left(false);
                    let prompt_len = prompt_left.width() as u16;

                    // Highlight natively in ratatui style — avoids the double
                    // conversion that used to happen here (highlight → nu_ansi_term
                    // → convert_ansi_style → ratatui) on every frame (A3). The
                    // conversion now lives in `ftui::ansi` via `highlight_ratatui`.
                    let mut text_line_spans = highlighter.highlight_ratatui(&display_text);

                    // Apply real-time syntax error underlining on invalid syntax
                    if let fshell_core::ValidationResult::Invalid { span, .. } = validation {
                        let total_chars = display_text.chars().count();
                        let mut start = span.offset();
                        let mut end = start + span.len();

                        if start >= total_chars {
                            if total_chars > 0 {
                                start = total_chars - 1;
                                end = total_chars;
                            } else {
                                start = 0;
                                end = 0;
                            }
                        } else if start == end {
                            end = (start + 1).min(total_chars);
                        }

                        if end > start {
                            let error_style = theme
                                .status
                                .error
                                .to_style()
                                .add_modifier(Modifier::UNDERLINED);
                            text_line_spans =
                                apply_style_override(text_line_spans, start, end, error_style);
                        }
                    }

                    // Inline gray hint (fish-style): hidden when the completion popup is visible
                    // so the user doesn't see duplicate ghost + popup for the same suffix.
                    if comp_mgr.visible {
                        current_hint.clear();
                    } else if !in_continuation
                        && text_buf.cursor() == text_buf.len()
                        && !text_buf.is_empty()
                    {
                        let current_text = text_buf.text();
                        let hist_adapter = crate::history::SqliteHistoryAdapter;
                        let hint = hinter.handle(
                            &current_text,
                            current_text.len(),
                            &hist_adapter,
                            false, // use_ansi_coloring = false for ratatui styling
                            &current_dir,
                        );
                        if !hint.is_empty() {
                            current_hint = hint.clone();
                            text_line_spans.push(Span::styled(hint, theme.status.muted.to_style()));
                        } else {
                            current_hint.clear();
                        }
                    } else {
                        current_hint.clear();
                    }

                    // Apply visual selection highlighting
                    if let Some((sel_lo, sel_hi)) = text_buf.selection_range() {
                        if sel_lo < sel_hi {
                            let sel_style = Style::default()
                                .bg(theme.prompt.selection_bg.to_ratatui_color())
                                .fg(theme.prompt.selection_fg.to_ratatui_color());
                            text_line_spans =
                                apply_style_override(text_line_spans, sel_lo, sel_hi, sel_style);
                        }
                    }

                    let size = term
                        .size()
                        .unwrap_or_else(|_| ratatui::layout::Size::new(80, 24));

                    let cursor_byte = display_text
                        .char_indices()
                        .nth(text_buf.cursor())
                        .map(|(i, _)| i)
                        .unwrap_or(display_text.len());
                    let cursor_visual_line = display_text[..cursor_byte].matches('\n').count();
                    let line_start_byte = display_text[..cursor_byte]
                        .rfind('\n')
                        .map(|pos| pos + 1)
                        .unwrap_or(0);
                    let line_start_char = display_text[..line_start_byte].chars().count();
                    let cursor_col = text_buf
                        .char_index_to_column_cached(text_buf.cursor(), Some(line_start_char));

                    let left_width = prompt_len as usize;
                    let (prefix_width, available_width) = if multi_line_count > 1 {
                        let gutter_num_width = format!("{}", multi_line_count).len().max(2);
                        let gutter_prefix_width = gutter_num_width + 4; // "▶ " or "  " (2) + num + "│ " (2)
                        let avail = (size.width as usize)
                            .saturating_sub(gutter_prefix_width)
                            .max(3);
                        (gutter_prefix_width, avail)
                    } else {
                        let avail = (size.width as usize).saturating_sub(left_width).max(3);
                        (left_width, avail)
                    };

                    let visible_width = available_width.saturating_sub(1);

                    // Horizontal scroll update — column-based for wide char support
                    if text_scroll_offset > cursor_col {
                        text_scroll_offset = cursor_col;
                    } else if visible_width > 0 && cursor_col >= text_scroll_offset + visible_width
                    {
                        text_scroll_offset = cursor_col + 1 - visible_width;
                    }

                    let input_line = Line::from(slice_spans_by_column(
                        &text_line_spans,
                        text_scroll_offset,
                        available_width,
                    ));

                    let render_x = (prefix_width as u16)
                        + (cursor_col as u16).saturating_sub(text_scroll_offset as u16);
                    let target_y = if multi_line_count > 1 {
                        1 + (cursor_visual_line as u16).min(multi_line_count.saturating_sub(1))
                    } else {
                        0
                    };

                    // Map buffer cursor position to visual column
                    cursor_state.update_logical_pos(Coord::new(render_x, target_y), &cursor_config);
                    let animated_cursor = cursor_state.get_render_pos(&cursor_config);

                    let mut relative_cursor_y = 0u16;

                    if term.draw(|f| {
                        let prompt_h = if multi_line_count > 1 {
                            multi_line_count + 2
                        } else {
                            1
                        };
                        let (constraints, prompt_area_idx, popup_area_idx) = if !output_lines.is_empty() {
                            let output_h = output_lines.len().min(
                                f.area().height.saturating_sub(prompt_h) as usize
                            );
                            (
                                vec![
                                    Constraint::Length(output_h as u16), // Output lines at top
                                    Constraint::Length(prompt_h),        // Prompt line(s)
                                    Constraint::Min(0),                  // Popup overlays
                                ],
                                1usize,
                                2usize,
                            )
                        } else {
                            (
                                vec![
                                    Constraint::Length(prompt_h), // Prompt line(s) at top
                                    Constraint::Min(0),           // Popup list / overlays below
                                ],
                                0usize,
                                1usize,
                            )
                        };

                        let chunks = Layout::default()
                            .direction(Direction::Vertical)
                            .constraints(constraints)
                            .split(f.area());

                        let prompt_line = chunks[prompt_area_idx];
                        let popup_area = chunks[popup_area_idx];

                        // Render captured output above the prompt
                        if !output_lines.is_empty() {
                            let output_h = output_lines.len().min(
                                f.area().height.saturating_sub(prompt_h) as usize
                            );
                            let output_area = chunks[0]; // Output area at top
                            let output_height = output_area.height.min(output_h as u16);
                            let skip = output_lines.len().saturating_sub(output_height as usize);
                            let display_lines: Vec<String> = output_lines
                                .iter()
                                .skip(skip)
                                .cloned()
                                .collect();
                            let output_text = display_lines.join("\n");
                            f.render_widget(
                                Paragraph::new(output_text)
                                    .style(theme.widgets.foreground.to_style()),
                                Rect::new(
                                    output_area.x,
                                    output_area.y,
                                    output_area.width,
                                    output_height,
                                ),
                            );
                        }

                        // Render Prompt and Text Line(s)
                        if multi_line_count > 1 {
                            let per_line_spans = split_spans_by_newline(
                                &text_line_spans,
                                cursor_visual_line,
                                text_scroll_offset,
                                available_width,
                            );
                            let mut text_lines: Vec<Line> = Vec::with_capacity((multi_line_count + 2) as usize);
                            let left_spans = prompt_left.spans.clone();
                            let left_width = prompt_len as usize;
                            let gutter_num_width = format!("{}", multi_line_count).len().max(2);

                            // Row 0: Elevated prompt header (with right prompt if space permits)
                            let mut header_spans = left_spans.clone();
                            let right_prompt = prompt_mgr.render_prompt_right();
                            let right_width = right_prompt.width();
                            if right_width > 0 && left_width + right_width + 1 < size.width as usize {
                                let pad = (size.width as usize).saturating_sub(left_width + right_width);
                                header_spans.push(Span::raw(" ".repeat(pad)));
                                header_spans.extend(right_prompt.spans.clone());
                            }
                            text_lines.push(Line::from(header_spans));

                            // Rows 1..=multi_line_count: Code lines flush at column 0
                            for (line_idx, line_spans) in per_line_spans.iter().enumerate() {
                                let is_cl = line_idx == cursor_visual_line;
                                let indicator = if is_cl {
                                    Span::styled("▶ ", theme.widgets.title.to_style_bold())
                                } else {
                                    Span::raw("  ")
                                };
                                let num_str = format!("{:>width$}", line_idx + 1, width = gutter_num_width);
                                let num_span = if is_cl {
                                    Span::styled(num_str, theme.widgets.title.to_style_bold())
                                } else {
                                    Span::styled(num_str, theme.status.muted.to_style())
                                };
                                let sep_span = Span::styled("│ ", theme.syntax.separator.to_style());

                                let mut spans: Vec<Span> = vec![indicator, num_span, sep_span];
                                spans.extend(line_spans.iter().cloned());
                                text_lines.push(Line::from(spans));
                            }

                            // Row multi_line_count + 1: Minimalist Transient Footer
                            let branch_pad = " ".repeat(gutter_num_width + 2);
                            let footer_branch = Span::styled(
                                format!("{}└── ", branch_pad),
                                theme.status.muted.to_style(),
                            );
                            let coord_span = Span::styled(
                                format!("[Ln {}, Col {}]", cursor_visual_line + 1, cursor_col + 1),
                                theme.status.info.to_style(),
                            );
                            let dot_span = Span::styled(" • ", theme.status.muted.to_style());
                            let hint_newline = Span::styled("⌥⏎ newline", theme.status.muted.to_style());
                            let hint_submit = Span::styled("⏎ submit", theme.status.muted.to_style());
                            let footer_line = Line::from(vec![
                                footer_branch,
                                coord_span,
                                dot_span.clone(),
                                hint_newline,
                                dot_span,
                                hint_submit,
                            ]);
                            text_lines.push(footer_line);

                            // Pad each line with spaces to fill the full terminal width.
                            // ratatui's Paragraph only writes cells that its spans cover,
                            // leaving untouched cells with old frame content.
                            let prompt_area_w = prompt_line.width as usize;
                            for line in text_lines.iter_mut() {
                                let line_w: usize = line
                                    .spans
                                    .iter()
                                    .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                                    .sum();
                                if line_w < prompt_area_w {
                                    line.spans
                                        .push(Span::raw(" ".repeat(prompt_area_w - line_w)));
                                }
                            }
                            f.render_widget(
                                Paragraph::new(Text::from(text_lines)),
                                prompt_line,
                            );
                            // Cursor positioned on the visual line
                            let render_x = animated_cursor.x.min(size.width.saturating_sub(1));
                            let cursor_y =
                                prompt_line.y + 1 + (cursor_visual_line as u16).min(multi_line_count.saturating_sub(1));
                            if let Some(cursor_style) =
                                cursor_state.get_style(true, &cursor_config, &theme)
                            {
                                let cursor_area = Rect::new(render_x, cursor_y, 1, 1);
                                f.render_widget(Block::default().style(cursor_style), cursor_area);
                            }
                            relative_cursor_y = cursor_y;
                            f.set_cursor_position(ratatui::layout::Position::new(
                                render_x,
                                cursor_y,
                            ));
                        } else {
                            let left_spans = if in_continuation {
                                vec![Span::styled(
                                    "> ",
                                    theme.status.muted.to_style(),
                                )]
                            } else {
                                prompt_left.spans.clone()
                            };
                            let mut combined_spans = left_spans;
                            combined_spans.extend(input_line.spans.clone());

                            // B3: Right prompt — pad with spaces to right-align
                            let right_prompt = prompt_mgr.render_prompt_right();
                            let right_width = right_prompt.width();
                            let left_plus_input = combined_spans
                                .iter()
                                .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                                .sum::<usize>();
                            if right_width > 0
                                && left_plus_input + right_width + 1 < size.width as usize
                            {
                                let pad =
                                    size.width as usize - left_plus_input - right_width;
                                combined_spans.push(Span::raw(" ".repeat(pad)));
                                combined_spans.extend(right_prompt.spans.clone());
                            }

                            // Pad with trailing spaces to fill the full terminal width.
                            // ratatui's Paragraph only writes cells that its spans cover,
                            // leaving untouched cells with old frame content. This causes
                            // ghost text from previous frames when content shrinks.
                            let total_w: usize = combined_spans
                                .iter()
                                .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                                .sum();
                            let prompt_area_w = prompt_line.width as usize;
                            if total_w < prompt_area_w {
                                combined_spans.push(Span::raw(" ".repeat(prompt_area_w - total_w)));
                            }
                            f.render_widget(
                                Paragraph::new(Line::from(combined_spans)),
                                prompt_line,
                            );

                            // Render visual animated cursor overlay
                            let render_x =
                                animated_cursor.x.min(size.width.saturating_sub(1));
                            if let Some(cursor_style) =
                                cursor_state.get_style(true, &cursor_config, &theme)
                            {
                                let cursor_area =
                                    Rect::new(render_x, prompt_line.y, 1, 1);
                                f.render_widget(
                                    Block::default().style(cursor_style),
                                    cursor_area,
                                );
                            }
                            // Move the system cursor to match
                            relative_cursor_y = prompt_line.y;
                            f.set_cursor_position(ratatui::layout::Position::new(
                                render_x,
                                prompt_line.y,
                            ));
                        }

                        // Render overlays / popups above the prompt line
                        if history_mgr.active {
                            let hist_area = popup_area;
                            if history_mgr.explorer_active {
                                let hist_block = Block::default()
                                    .borders(Borders::ALL)
                                    .border_type(BorderType::Rounded)
                                    .title(Span::styled(
                                        format!(
                                            " Interactive History Explorer: {} ({}) ",
                                            history_mgr.query,
                                            history_mgr.filter_mode.name()
                                        ),
                                        theme.status.info.to_style_bold(),
                                    ));

                                let hist_scroll_offset = history_mgr.scroll_offset;
                                let hist_selected_idx = history_mgr.selected_idx;
                                let max_h = hist_area.height.saturating_sub(3) as usize; // header is 1 line, borders 2

                                let header = Row::new(vec![
                                    Cell::from("Command").style(theme.widgets.title.to_style_bold()),
                                    Cell::from("Directory").style(theme.widgets.title.to_style_bold()),
                                    Cell::from("Exit").style(theme.widgets.title.to_style_bold()),
                                    Cell::from("Duration").style(theme.widgets.title.to_style_bold()),
                                    Cell::from("Time").style(theme.widgets.title.to_style_bold()),
                                ]);

                                let rows: Vec<Row> = history_mgr
                                    .results
                                    .iter()
                                    .skip(hist_scroll_offset)
                                    .take(max_h)
                                    .enumerate()
                                    .map(|(i, entry)| {
                                        let idx = hist_scroll_offset + i;
                                        let status_cell = match entry.exit_code {
                                            Some(0) => Cell::from("[ok] 0").style(theme.status.ok.to_style()),
                                            Some(code) => Cell::from(format!("[!] {}", code)).style(theme.status.error.to_style()),
                                            None => Cell::from("-"),
                                        };
                                        let local_time = chrono::Local.timestamp_millis_opt(entry.timestamp_ms)
                                            .single()
                                            .unwrap_or_else(chrono::Local::now);
                                        let time_str = local_time.format("%Y-%m-%d %H:%M:%S").to_string();

                                        let style = if idx == hist_selected_idx {
                                            Style::default()
                                                .fg(theme.widgets.item_selected_fg.to_ratatui_color())
                                                .bg(theme.widgets.item_selected_bg.to_ratatui_color())
                                                .add_modifier(Modifier::BOLD)
                                        } else {
                                            theme.widgets.foreground.to_style()
                                        };

                                        Row::new(vec![
                                            Cell::from(entry.command.clone()),
                                            Cell::from(entry.cwd.clone()),
                                            status_cell,
                                            Cell::from(format!("{}ms", entry.duration_ms)),
                                            Cell::from(time_str),
                                        ]).style(style)
                                    })
                                    .collect();

                                let table = Table::new(
                                    rows,
                                    [
                                        Constraint::Percentage(40),
                                        Constraint::Percentage(25),
                                        Constraint::Length(8),
                                        Constraint::Length(10),
                                        Constraint::Length(20),
                                    ],
                                )
                                .header(header)
                                .block(hist_block);

                                f.render_widget(table, hist_area);
                            } else {
                                let hist_block = Block::default()
                                    .borders(Borders::ALL)
                                    .border_type(BorderType::Rounded)
                                    .title(Span::styled(
                                        format!(
                                            " Fuzzy History: {} ({}) ",
                                            history_mgr.query,
                                            history_mgr.filter_mode.name()
                                        ),
                                        theme.status.info.to_style_bold(),
                                    ));

                                let hist_scroll_offset = history_mgr.scroll_offset; // B8: pre-computed
                                let hist_selected_idx = history_mgr.selected_idx;
                                let max_h = hist_area.height.saturating_sub(2) as usize;
                                let items: Vec<ListItem> = if history_mgr.aborted_active {
                                    let reversed_cmds: Vec<&String> =
                                        history_mgr.aborted_commands.iter().rev().collect();

                                    reversed_cmds
                                        .iter()
                                        .skip(hist_scroll_offset)
                                        .take(max_h)
                                        .enumerate()
                                        .map(|(i, &cmd)| {
                                            let idx = hist_scroll_offset + i;
                                            let style = if idx == hist_selected_idx {
                                                Style::default()
                                                    .fg(theme.widgets.item_selected_fg.to_ratatui_color())
                                                    .bg(theme.widgets.item_selected_bg.to_ratatui_color())
                                                    .add_modifier(Modifier::BOLD)
                                            } else {
                                                theme.widgets.foreground.to_style()
                                            };
                                            ListItem::new(Line::from(vec![
                                                Span::styled(" ~> ", theme.syntax.variable.to_style()),
                                                Span::styled(cmd.clone(), style),
                                            ]))
                                        })
                                        .collect()
                                } else {
                                    history_mgr
                                        .results
                                        .iter()
                                        .skip(hist_scroll_offset)
                                        .take(max_h)
                                        .enumerate()
                                        .map(|(i, entry)| {
                                            let idx = hist_scroll_offset + i;
                                            let status_icon = if entry.exit_code == Some(0) {
                                                Span::styled(" [ok] ", theme.status.ok.to_style())
                                            } else {
                                                Span::styled(" [!] ", theme.status.error.to_style())
                                            };
                                            let style = if idx == hist_selected_idx {
                                                Style::default()
                                                    .fg(theme.widgets.item_selected_fg.to_ratatui_color())
                                                    .bg(theme.widgets.item_selected_bg.to_ratatui_color())
                                                    .add_modifier(Modifier::BOLD)
                                            } else {
                                                theme.widgets.foreground.to_style()
                                            };
                                            ListItem::new(Line::from(vec![
                                                status_icon,
                                                Span::styled(entry.command.clone(), style),
                                                Span::styled(
                                                    format!(" ({})", entry.cwd),
                                                    theme.status.muted.to_style(),
                                                ),
                                            ]))
                                        })
                                        .collect()
                                };

                                let items = if items.is_empty() {
                                    let msg = if history_mgr.query.is_empty() {
                                        "  No history entries yet".to_string()
                                    } else {
                                        format!(
                                            "  No commands matching \"{}\"",
                                            history_mgr.query
                                        )
                                    };
                                    vec![ListItem::new(Line::from(Span::styled(
                                        msg,
                                        theme.status.muted.to_style(),
                                    )))]
                                } else {
                                    items
                                };

                                let list_widget = List::new(items).block(hist_block);
                                f.render_widget(list_widget, hist_area);
                            }
                        } else if agent_state.active {
                            let agent_area = popup_area;
                            let agent_block = Block::default()
                                .borders(Borders::ALL)
                                .border_type(BorderType::Rounded)
                                .title(Span::styled(
                                    " fsh-ai Agent Mode ",
                                    theme.syntax.keyword.to_style_bold(),
                                ));

                            let content = if agent_state.is_loading {
                                vec![
                                    Line::from("  Translating request into fsh command..."),
                                    Line::from("  Please wait while the AI provider responds."),
                                ]
                            } else if let Some(err) = &agent_state.error_msg {
                                vec![
                                    Line::from(vec![
                                        Span::styled("  Error: ", theme.status.error.to_style()),
                                        Span::raw(err),
                                    ]),
                                    Line::from("  Press Esc to return, Alt+Enter to try again."),
                                ]
                            } else {
                                vec![
                                    Line::from(format!("  Query: {}", agent_state.prompt)),
                                    Line::from(vec![
                                        Span::styled(
                                            "  Generated: ",
                                            theme.status.ok.to_style(),
                                        ),
                                        Span::styled(
                                            agent_state.result_command.as_deref().unwrap_or(""),
                                            theme.widgets.title.to_style_bold(),
                                        ),
                                    ]),
                                    Line::from("  Press Enter to edit/run, Esc to exit."),
                                ]
                            };

                            f.render_widget(Paragraph::new(content).block(agent_block), agent_area);
                        } else if comp_mgr.visible && !comp_mgr.suggestions.is_empty() {
                            let total_items = comp_mgr.suggestions.len();
                            let popup_budget_h = popup_area.height.max(3);
                            let max_w = size.width.saturating_sub(2);
                            let popup_w = if size.width >= 120 {
                                ((size.width as f64 * 0.5) as u16).clamp(50, 75).min(max_w)
                            } else {
                                (size.width / 2 + size.width / 4).clamp(40, 65).min(max_w)
                            };
                            let visual_cursor_col =
                                cursor_col.saturating_sub(text_scroll_offset);
                            let popup_x = (prompt_len + visual_cursor_col as u16)
                                .min(size.width.saturating_sub(popup_w).saturating_sub(1));

                            // R2: when the popup is at the bottom edge the inline
                            // viewport scroll already made `popup_area` tall enough
                            // to hold it below the prompt. Crushing only happens
                            // when the terminal itself is shorter than prompt +
                            // popup budget (tiny terminals). In that case render
                            // at least 3 rows and clamp to the terminal bottom.
                            let comp_h = popup_budget_h
                                .min(size.height.saturating_sub(popup_area.y).max(3));
                            let comp_area = Rect::new(
                                popup_x,
                                popup_area.y,
                                popup_w.min(size.width.saturating_sub(popup_x).saturating_sub(1)),
                                comp_h,
                            );

                            let list_area = comp_area;

                            // Find the flat index of the selected suggestion (accounting for group headers)
                            let selected_flat_idx = comp_mgr.flat_index_of(comp_mgr.selected_idx);

                            // Update scroll offset based on selected flat index
                            let visible_rows = comp_h.saturating_sub(2).max(1) as usize;
                            if selected_flat_idx >= comp_mgr.scroll_offset + visible_rows {
                                comp_mgr.scroll_offset = selected_flat_idx
                                    .saturating_sub(visible_rows)
                                    .saturating_add(1);
                            } else if selected_flat_idx < comp_mgr.scroll_offset {
                                comp_mgr.scroll_offset = selected_flat_idx;
                            }

                            // Determine if scrollbar is needed (accounts for headers)
                            let total_display = comp_mgr.grouped.as_ref().map_or(0, |g| g.display_lines());
                            let scrollbar_needed = total_display > visible_rows;
                            let render_width = list_area.width.saturating_sub(if scrollbar_needed { 1 } else { 0 });

                            // Build grouped list items (render_popup already windows
                            // by scroll_offset..scroll_offset+visible_rows — no second slice).
                            let (sections, _total_display) = comp_mgr.render_popup(
                                render_width,
                                visible_rows,
                            );

                            let mut list_items: Vec<ListItem> = Vec::new();
                            for (_is_header, items) in &sections {
                                for item in items {
                                    list_items.push(item.clone());
                                }
                            }

                            // Footer with count info
                            let footer_text = if total_items > visible_rows {
                                format!(
                                    " {} of {} — ↑↓ navigate, PgUp/PgDn page, Tab/→ accept, Enter run ",
                                    comp_mgr.selected_idx + 1,
                                    total_items,
                                )
                            } else {
                                format!(
                                    " {} item{} — ↑↓ navigate, Tab/→ accept, Enter run, Esc close ",
                                    total_items,
                                    if total_items == 1 { "" } else { "s" },
                                )
                            };



                            let comp_block = Block::default()
                                .borders(Borders::ALL)
                                .border_type(BorderType::Rounded)
                                .border_style(theme.status.muted.to_style())
                                .title(Span::styled(
                                    footer_text,
                                    theme.status.muted.to_style(),
                                ))
                                .title_alignment(ratatui::layout::Alignment::Left);

                            if _total_display > visible_rows {
                                let list_area = Rect::new(
                                    comp_area.x,
                                    comp_area.y,
                                    comp_area.width.saturating_sub(1),
                                    comp_area.height,
                                );
                                let scroll_area = Rect::new(
                                    comp_area.x + comp_area.width.saturating_sub(1),
                                    comp_area.y,
                                    1,
                                    comp_area.height,
                                );
                                let mut scrollbar_state =
                                    ScrollbarState::new(_total_display)
                                        .position(selected_flat_idx);
                                f.render_widget(
                                    List::new(list_items).block(comp_block),
                                    list_area,
                                );
                                f.render_stateful_widget(
                                    Scrollbar::default()
                                        .orientation(ScrollbarOrientation::VerticalRight)
                                        .begin_symbol(Some("▲"))
                                        .end_symbol(Some("▼"))
                                        .track_symbol(Some("│"))
                                        .thumb_symbol("█")
                                        .style(theme.status.muted.to_style())
                                        .thumb_style(theme.widgets.foreground.to_style()),
                                    scroll_area,
                                    &mut scrollbar_state,
                                );
                            } else {
                                f.render_widget(
                                    List::new(list_items).block(comp_block),
                                    list_area,
                                );
                            }


                        } else if comp_mgr.visible {
                            let empty_msg = Span::styled(
                                "  No completions match",
                                theme.status.muted.to_style(),
                            );
                            let empty_block = Block::default()
                                .borders(Borders::ALL)
                                .border_type(BorderType::Rounded)
                                .border_style(theme.status.muted.to_style());
                            f.render_widget(
                                Paragraph::new(Line::from(empty_msg)).block(empty_block),
                                popup_area,
                            );
                        } else if widget_explorer.active {
                            let exp_area = popup_area;
                            let title_str = if widget_explorer.query.is_empty() {
                                " [Keybindings & Widget Palette] ".to_string()
                            } else {
                                format!(" [Keybindings & Widget Palette: {}] ", widget_explorer.query)
                            };
                            let exp_block = Block::default()
                                .borders(Borders::ALL)
                                .border_type(BorderType::Rounded)
                                .border_style(theme.status.info.to_style())
                                .title(Span::styled(title_str, theme.status.info.to_style_bold()));

                            let max_h = exp_area.height.saturating_sub(3) as usize;
                            let header = Row::new(vec![
                                Cell::from("Category").style(theme.widgets.title.to_style_bold()),
                                Cell::from("Widget Name").style(theme.widgets.title.to_style_bold()),
                                Cell::from("Bound Key").style(theme.widgets.title.to_style_bold()),
                                Cell::from("Description").style(theme.widgets.title.to_style_bold()),
                            ]);

                            let scroll_offset = widget_explorer.scroll_offset;
                            let selected_idx = widget_explorer.selected_idx;

                            let rows: Vec<Row> = widget_explorer
                                .items
                                .iter()
                                .skip(scroll_offset)
                                .take(max_h)
                                .enumerate()
                                .map(|(i, item)| {
                                    let idx = scroll_offset + i;
                                    let style = if idx == selected_idx {
                                        Style::default()
                                            .fg(theme.widgets.item_selected_fg.to_ratatui_color())
                                            .bg(theme.widgets.item_selected_bg.to_ratatui_color())
                                            .add_modifier(Modifier::BOLD)
                                    } else {
                                        theme.widgets.foreground.to_style()
                                    };

                                    Row::new(vec![
                                        Cell::from(format!("[{}]", item.category)).style(theme.status.muted.to_style()),
                                        Cell::from(item.name).style(theme.status.info.to_style()),
                                        Cell::from(item.bound_chord.clone()).style(theme.status.ok.to_style()),
                                        Cell::from(item.description).style(theme.widgets.foreground.to_style()),
                                    ]).style(style)
                                })
                                .collect();

                            let table = Table::new(
                                rows,
                                [
                                    Constraint::Length(18),
                                    Constraint::Length(28),
                                    Constraint::Length(22),
                                    Constraint::Percentage(40),
                                ],
                            )
                            .header(header)
                            .block(exp_block);

                            f.render_widget(table, exp_area);
                        } else if help_visible {
                            // Render F1 Help Tooltip
                            let cmd_name = get_command_for_cursor(&display_text, text_buf.cursor());
                            if let Some(topic) = fshell_builtins::help::find_topic(&cmd_name) {
                                let help_area = popup_area;
                                let help_block = Block::default()
                                    .borders(Borders::ALL)
                                    .border_type(BorderType::Rounded)
                                    .border_style(theme.status.ok.to_style())
                                    .title(Span::styled(
                                        format!(" Help: {} ", topic.name),
                                        theme.status.ok.to_style_bold(),
                                    ));

                                let mut lines = Vec::new();
                                // Line 1: Summary
                                lines.push(Line::from(vec![
                                    Span::styled(topic.name, theme.status.info.to_style_bold()),
                                    Span::raw(" - "),
                                    Span::styled(topic.summary, theme.widgets.foreground.to_style()),
                                ]));
                                // Line 2: Syntax
                                lines.push(Line::from(vec![
                                    Span::styled("Syntax: ", theme.widgets.title.to_style()),
                                    Span::styled(topic.syntax, theme.widgets.foreground.to_style()),
                                ]));
                                // Line 3: Examples/Flags
                                if let Some(ex) = topic.examples.first() {
                                    lines.push(Line::from(vec![
                                        Span::styled("Example: ", theme.widgets.title.to_style()),
                                        Span::styled(ex.input, theme.status.info.to_style()),
                                        Span::raw(" ("),
                                        Span::styled(ex.explanation, theme.status.muted.to_style()),
                                        Span::raw(")"),
                                    ]));
                                } else if let Some(flag) = topic.flags.first() {
                                    lines.push(Line::from(vec![
                                        Span::styled("Flag: ", theme.widgets.title.to_style()),
                                        Span::styled(flag.flag, theme.status.info.to_style()),
                                        Span::raw(" - "),
                                        Span::styled(flag.desc, theme.widgets.foreground.to_style()),
                                    ]));
                                } else {
                                    lines.push(Line::from(vec![
                                        Span::styled(topic.description, theme.status.muted.to_style()),
                                    ]));
                                }

                                let list = List::new(lines.into_iter().map(ListItem::new).collect::<Vec<_>>())
                                    .block(help_block);
                                f.render_widget(list, help_area);
                            } else {
                                // Render a fallback box
                                let help_area = popup_area;
                                let help_block = Block::default()
                                    .borders(Borders::ALL)
                                    .border_type(BorderType::Rounded)
                                    .border_style(theme.status.muted.to_style())
                                    .title(Span::styled(" Help Tooltip ", theme.status.muted.to_style()));
                                let fallback_text = if cmd_name.is_empty() {
                                    "No command under cursor".to_string()
                                } else {
                                    format!("No help topic found for command: '{}'", cmd_name)
                                };
                                let paragraph = Paragraph::new(fallback_text)
                                    .style(theme.status.muted.to_style())
                                    .block(help_block);
                                f.render_widget(paragraph, help_area);
                            }
                        } else {
                            // Clear the reserved popup area so stale content from
                            // dismissed completions/help doesn't linger on screen.
                            // Needed because Viewport::Inline doesn't auto-clear cells.
                            f.render_widget(Clear, popup_area);
                        }

                    }).is_err() {
                        break 'repl_loop;
                    }
                    _last_relative_cursor_y = relative_cursor_y;
                }

                redraw = false;
            }

            // Bug 7.2: Check prompt_mgr.has_active_animations() which includes
            // both async widgets AND spinner/prompt animations.
            let has_active_animations = agent_state.is_loading
                || (comp_mgr.visible
                    && comp_mgr
                        .suggestions
                        .iter()
                        .any(|s| s.description.as_ref().is_some_and(|d| d.contains('\t'))))
                || prompt_mgr.has_active_animations();

            // Also try idle-based auto-commit for undo granularity (Bug 6.2)
            text_buf.try_idle_commit();

            // Determine poll timeout dynamically: fast (50ms) for active animations/widgets, slow (1000ms) for idle
            let poll_timeout = if has_active_animations {
                tick_rate
            } else {
                Duration::from_millis(1000)
            };

            if input_iter <= 5 || input_iter.is_multiple_of(200) {
                cpu_dbg!(
                    "poll: has_anim={} timeout={:?} iter={}",
                    has_active_animations,
                    poll_timeout,
                    input_iter
                );
            }

            // Read keystroke/mouse input
            // Handle EINTR gracefully (SIGTSTP, SIGCONT, etc.) — just retry

            // TTY health / signal checks

            let poll_start = std::time::Instant::now();
            let polled = loop {
                match event::poll(poll_timeout) {
                    Ok(has_event) => break has_event,
                    Err(e) => {
                        // EINTR on signal reception: retry or exit on SIGHUP/cancellation
                        use std::io::ErrorKind;
                        if e.kind() == ErrorKind::Interrupted {
                            #[cfg(unix)]
                            if GOT_SIGHUP.load(Ordering::Relaxed) {
                                cpu_dbg!(
                                    "GOT_SIGHUP inside EINTR event::poll — breaking repl_loop"
                                );
                                break 'repl_loop;
                            }
                            if env.job_control.cancellation.load(Ordering::Relaxed) {
                                cpu_dbg!(
                                    "cancellation inside EINTR event::poll — breaking repl_loop"
                                );
                                break 'repl_loop;
                            }
                            // Check if we need to re-init terminal (Bug 5.2)
                            #[cfg(unix)]
                            if DID_SUSPEND.swap(false, std::sync::atomic::Ordering::Relaxed) {
                                if let Some(s) = _raw_session.as_deref() {
                                    s.reenter_raw();
                                } else {
                                    let _ = crossterm::terminal::enable_raw_mode();
                                }
                                redraw = true;
                            }
                            continue;
                        }
                        cpu_dbg!("event::poll returned error (non-EINTR): {:?}", e);
                        break 'repl_loop;
                    }
                }
            };

            let poll_elapsed = poll_start.elapsed();
            if !polled {
                #[cfg(unix)]
                if GOT_SIGHUP.load(Ordering::Relaxed) {
                    cpu_dbg!("GOT_SIGHUP (in !polled path) — breaking repl_loop");
                    break 'repl_loop;
                }

                if poll_elapsed < poll_timeout / 2 {
                    cpu_dbg!(
                        "event::poll({:?}) returned Ok(false) in {:?} — busy-poll! (has_anim={})",
                        poll_timeout,
                        poll_elapsed,
                        has_active_animations
                    );
                    std::thread::sleep(Duration::from_millis(50));
                }
            }

            if polled {
                #[cfg(unix)]
                if GOT_SIGHUP.load(Ordering::Relaxed) {
                    cpu_dbg!("GOT_SIGHUP before event::read — breaking repl_loop");
                    break 'repl_loop;
                }

                if env.job_control.cancellation.load(Ordering::Relaxed) {
                    cpu_dbg!("cancellation flag set before event::read — breaking repl_loop");
                    break 'repl_loop;
                }

                let event = loop {
                    match event::read() {
                        Ok(e) => break e,
                        Err(e) => {
                            use std::io::ErrorKind;
                            if e.kind() == ErrorKind::Interrupted {
                                #[cfg(unix)]
                                if GOT_SIGHUP.load(Ordering::Relaxed) {
                                    cpu_dbg!(
                                        "GOT_SIGHUP in event::read Err(Interrupted) — breaking repl_loop"
                                    );
                                    break 'repl_loop;
                                }
                                if env.job_control.cancellation.load(Ordering::Relaxed) {
                                    cpu_dbg!(
                                        "cancellation in event::read Err(Interrupted) — breaking repl_loop"
                                    );
                                    break 'repl_loop;
                                }
                                #[cfg(unix)]
                                if DID_SUSPEND.swap(false, std::sync::atomic::Ordering::Relaxed) {
                                    if let Some(s) = _raw_session.as_deref() {
                                        s.reenter_raw();
                                    } else {
                                        let _ = crossterm::terminal::enable_raw_mode();
                                    }
                                }
                                continue;
                            }
                            cpu_dbg!("event::read returned error (non-EINTR): {:?}", e);
                            break 'repl_loop;
                        }
                    }
                };

                if let Event::Resize(_, _) = event {
                    let now = std::time::Instant::now();
                    if now.duration_since(last_resize) > Duration::from_millis(80) {
                        redraw = true;
                        resized = true;
                        // Resize invalidates Fixed viewport coords (status bar at term_h-2)
                        // and the inline viewport height derived from term_h. Recompute on next
                        // redraw and recreate both terminals. Also clear the inline viewport's
                        // "previous frame" diff so stale padding ghosts don't survive.
                        status_terminal = None;
                        current_viewport_height = 0;
                        last_resize = now;
                    }
                    continue;
                }

                if let Event::Mouse(MouseEvent {
                    kind,
                    column,
                    row,
                    modifiers: _,
                }) = event
                {
                    cpu_dbg!("Mouse event: {:?} col={} row={}", kind, column, row);
                    let size = terminal
                        .as_ref()
                        .and_then(|t| t.size().ok())
                        .unwrap_or_else(|| ratatui::layout::Size::new(80, 24));
                    let prompt_y = size.height.saturating_sub(1);
                    mouse_mgr.handle_click(row, prompt_y);

                    if mouse_mgr.is_captured {
                        match kind {
                            MouseEventKind::Drag(MouseButton::Left) => {
                                // History scroll via drag? No — only completions use drag-
                                // like scroll. When history is active, scroll wheel is
                                // handled below; drag stays for text selection only.
                                if history_mgr.active {
                                    // Let history handle scroll separately; ignore drag.
                                } else if !comp_mgr.visible {
                                    let prompt_left = prompt_mgr.render_prompt_left(false);
                                    let prompt_len = prompt_left.width() as u16;
                                    if column >= prompt_len {
                                        let click_x =
                                            column - prompt_len + text_scroll_offset as u16;
                                        let cursor =
                                            text_buf.column_to_char_index(click_x as usize);
                                        // If we don't have a drag anchor yet, start one at current cursor
                                        if drag_anchor.is_none() {
                                            drag_anchor = Some(text_buf.cursor());
                                        }
                                        if let Some(anchor) = drag_anchor {
                                            text_buf.set_selection(anchor, cursor);
                                            redraw = true;
                                        }
                                    }
                                }
                            }
                            MouseEventKind::ScrollDown => {
                                if comp_mgr.visible && !comp_mgr.suggestions.is_empty() {
                                    comp_mgr.select_next();
                                    if let Some(ref _grp) = comp_mgr.grouped {
                                        let visible_rows =
                                            (current_viewport_height.saturating_sub(2)).max(1)
                                                as usize;
                                        let flat_idx =
                                            comp_mgr.flat_index_of(comp_mgr.selected_idx);
                                        if flat_idx >= comp_mgr.scroll_offset + visible_rows {
                                            comp_mgr.scroll_offset = flat_idx + 1 - visible_rows;
                                        }
                                    }
                                    redraw = true;
                                } else if history_mgr.active {
                                    history_mgr.select_next();
                                    let max_h = current_viewport_height.saturating_sub(3) as usize;
                                    history_mgr.adjust_scroll(max_h);
                                    redraw = true;
                                } else {
                                    mouse_mgr.disable_capture();
                                    redraw = true;
                                }
                            }
                            MouseEventKind::ScrollUp => {
                                if comp_mgr.visible && !comp_mgr.suggestions.is_empty() {
                                    comp_mgr.select_prev();
                                    if let Some(ref _grp) = comp_mgr.grouped {
                                        let flat_idx =
                                            comp_mgr.flat_index_of(comp_mgr.selected_idx);
                                        if flat_idx < comp_mgr.scroll_offset {
                                            comp_mgr.scroll_offset = flat_idx;
                                        }
                                    }
                                    redraw = true;
                                } else if history_mgr.active {
                                    history_mgr.select_prev();
                                    let max_h = current_viewport_height.saturating_sub(3) as usize;
                                    history_mgr.adjust_scroll(max_h);
                                    redraw = true;
                                } else {
                                    mouse_mgr.disable_capture();
                                    redraw = true;
                                }
                            }
                            MouseEventKind::Down(MouseButton::Left) => {
                                if comp_mgr.visible && !comp_mgr.suggestions.is_empty() {
                                    // Map mouse click to a completion item index and accept it.
                                    // `comp_area` is recomputed on every draw; we recompute the same
                                    // mapping here from terminal size + cursor geometry so clicks are
                                    // handled without storing extra UI state between event and draw.
                                    let term_size = terminal
                                        .as_ref()
                                        .and_then(|t| t.size().ok())
                                        .unwrap_or(ratatui::layout::Size::new(80, 24));
                                    let prompt_left = prompt_mgr.render_prompt_left(false);
                                    let prompt_len = prompt_left.width() as u16;
                                    let cursor_col =
                                        text_buf.char_index_to_column(text_buf.cursor());
                                    let visual_cursor_col =
                                        cursor_col.saturating_sub(text_scroll_offset);
                                    let max_w = term_size.width.saturating_sub(2);
                                    let popup_w = if term_size.width >= 120 {
                                        ((term_size.width as f64 * 0.5) as u16)
                                            .clamp(50, 75)
                                            .min(max_w)
                                    } else {
                                        (term_size.width / 2 + term_size.width / 4)
                                            .clamp(40, 65)
                                            .min(max_w)
                                    };
                                    let popup_x = (prompt_len + visual_cursor_col as u16).min(
                                        term_size.width.saturating_sub(popup_w).saturating_sub(1),
                                    );
                                    let popup_area_y = {
                                        // Popup is rendered inside `popup_area` which is the second
                                        // chunk below `prompt_line`. In single-line mode prompt_line
                                        // is 1 row, so popup starts at cursor_y + 1. Recompute.
                                        let multi_line_count: u16 =
                                            if text_buf.text().contains('\n') {
                                                text_buf.text().split('\n').count().max(1) as u16
                                            } else {
                                                1
                                            };
                                        // Approximate prompt_y from terminal height and viewport;
                                        // fallback to safe_cursor_position if available.
                                        safe_cursor_position()
                                            .map(|(_, y)| y + multi_line_count)
                                            .unwrap_or(
                                                term_size
                                                    .height
                                                    .saturating_sub(popup_w)
                                                    .saturating_sub(2),
                                            )
                                    };
                                    // Click must be inside the popup rect to count.
                                    let inside_x = column >= popup_x && column < popup_x + popup_w;
                                    let inside_y = row >= popup_area_y;
                                    if inside_x && inside_y {
                                        // List is rendered with a 1-row border on top, so first
                                        // display line is at popup_area_y + 1. Visible rows exclude
                                        // top+bottom borders (2 rows).
                                        let clicked_display_row =
                                            row.saturating_sub(popup_area_y + 1) as usize;
                                        let flat_idx = comp_mgr.scroll_offset + clicked_display_row;
                                        // Convert flat display index (header lines included) to raw suggestion index.
                                        if let Some(raw_idx) =
                                            flat_to_raw_index(&comp_mgr, flat_idx)
                                        {
                                            if let Some(s) =
                                                comp_mgr.suggestions.get(raw_idx).cloned()
                                            {
                                                let line = text_buf.text();
                                                apply_completion(&mut text_buf, &line, &s);
                                                comp_mgr.clear();
                                                redraw = true;
                                            } else {
                                                redraw = true;
                                            }
                                        } else {
                                            // Clicked on a header line — select first item in that group.
                                            if let Some(raw_idx) =
                                                flat_header_next_item(&comp_mgr, flat_idx)
                                            {
                                                comp_mgr.selected_idx = raw_idx;
                                                // Keep scroll_offset stable; just update selection highlight.
                                                redraw = true;
                                            }
                                        }
                                    } else {
                                        // Click outside popup but completions visible: keep popup, don't steal cursor.
                                        redraw = true;
                                    }
                                } else {
                                    // Bug 2.3: Clear drag anchor on new click
                                    drag_anchor = None;
                                    // Bug 2.1: Position cursor at click
                                    let prompt_left = prompt_mgr.render_prompt_left(false);
                                    let prompt_len = prompt_left.width() as u16;
                                    if column >= prompt_len {
                                        let click_x =
                                            column - prompt_len + text_scroll_offset as u16;
                                        text_buf.set_cursor(
                                            text_buf.column_to_char_index(click_x as usize),
                                        );
                                        redraw = true;
                                    }
                                }
                            }
                            MouseEventKind::Up(MouseButton::Left) => {
                                // Bug 2.3: End of drag selection — clear anchor
                                drag_anchor = None;
                            }
                            _ => {}
                        }
                    }
                    continue;
                }

                if let Event::Paste(pasted_text) = event {
                    history_index = None;
                    // Bug 8.1: Delete active selection before inserting paste
                    if text_buf.has_selection() {
                        text_buf.delete_selection();
                    }
                    text_buf.insert_str(&pasted_text);
                    comp_mgr.update(&text_buf.text(), text_buf.cursor(), false);
                    redraw = true;
                    continue;
                }

                if let Event::Key(key) = event {
                    if key.kind == event::KeyEventKind::Release {
                        continue;
                    }
                    mouse_mgr.handle_keypress();

                    if history_mgr.active {
                        match key.code {
                            KeyCode::Esc => {
                                history_mgr.active = false;
                                history_mgr.aborted_active = false;
                                redraw = true;
                            }
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                // Bug 1.2: Ctrl+C in history overlays = exit
                                history_mgr.active = false;
                                history_mgr.aborted_active = false;
                                redraw = true;
                            }
                            KeyCode::F(1) => {
                                history_mgr.active = false;
                                history_mgr.aborted_active = false;
                                help_visible = !help_visible;
                                redraw = true;
                            }
                            KeyCode::Enter => {
                                if let Some(cmd) = history_mgr.get_selected() {
                                    text_buf.replace_content(&cmd);
                                }
                                history_mgr.active = false;
                                history_mgr.aborted_active = false;
                                redraw = true;
                            }
                            KeyCode::Up => {
                                history_mgr.select_prev();
                                let max_h = current_viewport_height.saturating_sub(3) as usize;
                                history_mgr.adjust_scroll(max_h);
                                redraw = true;
                            }
                            KeyCode::Down => {
                                history_mgr.select_next();
                                let max_h = current_viewport_height.saturating_sub(3) as usize;
                                history_mgr.adjust_scroll(max_h);
                                redraw = true;
                            }
                            KeyCode::PageUp => {
                                let max_h = current_viewport_height.saturating_sub(3) as usize;
                                for _ in 0..max_h {
                                    history_mgr.select_prev();
                                }
                                history_mgr.adjust_scroll(max_h);
                                redraw = true;
                            }
                            KeyCode::PageDown => {
                                let max_h = current_viewport_height.saturating_sub(3) as usize;
                                for _ in 0..max_h {
                                    history_mgr.select_next();
                                }
                                history_mgr.adjust_scroll(max_h);
                                redraw = true;
                            }
                            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                // Bug 1.5: Ctrl+R cycles to next result (pressing same key that
                                // opened the overlay should navigate, not change filter scope).
                                history_mgr.select_next();
                                let max_h = current_viewport_height.saturating_sub(3) as usize;
                                history_mgr.adjust_scroll(max_h);
                                redraw = true;
                            }
                            KeyCode::F(2) => {
                                // F2 toggles filter mode (was Ctrl+R, Bug 1.5)
                                history_mgr.filter_mode = history_mgr.filter_mode.next();
                                history_mgr.update_results(&current_dir, &hostname, &session_id);
                                redraw = true;
                            }
                            KeyCode::Backspace => {
                                history_mgr.query.pop();
                                history_mgr.selected_idx = 0;
                                history_mgr.update_results(&current_dir, &hostname, &session_id);
                                redraw = true;
                            }
                            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                history_mgr.delete_selected();
                                history_mgr.update_results(&current_dir, &hostname, &session_id);
                                redraw = true;
                            }
                            KeyCode::Char(c) => {
                                history_mgr.query.push(c);
                                history_mgr.selected_idx = 0;
                                history_mgr.update_results(&current_dir, &hostname, &session_id);
                                redraw = true;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    if agent_state.active {
                        match key.code {
                            KeyCode::Esc => {
                                agent_state.reset();
                                redraw = true;
                            }
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                // Bug 1.2: Ctrl+C in agent overlay = exit
                                agent_state.reset();
                                redraw = true;
                            }
                            KeyCode::Enter => {
                                if agent_state.is_loading {
                                    // Bug 8.11: Ignore Enter while loading
                                } else if agent_state.result_command.is_some() {
                                    agent_state.active = false;
                                } else {
                                    let prompt_str = agent_state.prompt.clone();
                                    agent_state.trigger_query(&prompt_str, &env);
                                }
                                redraw = true;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    if widget_explorer.active {
                        match key.code {
                            KeyCode::Esc | KeyCode::F(1) => {
                                widget_explorer.close();
                                redraw = true;
                            }
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                widget_explorer.close();
                                redraw = true;
                            }
                            KeyCode::Up => {
                                widget_explorer.select_prev();
                                if widget_explorer.selected_idx < widget_explorer.scroll_offset {
                                    widget_explorer.scroll_offset = widget_explorer.selected_idx;
                                }
                                redraw = true;
                            }
                            KeyCode::Down => {
                                widget_explorer.select_next();
                                let max_h =
                                    current_viewport_height.saturating_sub(5).max(1) as usize;
                                if widget_explorer.selected_idx
                                    >= widget_explorer.scroll_offset + max_h
                                {
                                    widget_explorer.scroll_offset =
                                        widget_explorer.selected_idx + 1 - max_h;
                                }
                                redraw = true;
                            }
                            KeyCode::PageUp => {
                                let page_size =
                                    current_viewport_height.saturating_sub(5).max(1) as usize;
                                widget_explorer.page_up(page_size);
                                widget_explorer.scroll_offset =
                                    widget_explorer.scroll_offset.saturating_sub(page_size);
                                redraw = true;
                            }
                            KeyCode::PageDown => {
                                let page_size =
                                    current_viewport_height.saturating_sub(5).max(1) as usize;
                                widget_explorer.page_down(page_size);
                                if widget_explorer.selected_idx
                                    >= widget_explorer.scroll_offset + page_size
                                {
                                    widget_explorer.scroll_offset =
                                        widget_explorer.selected_idx + 1 - page_size;
                                }
                                redraw = true;
                            }
                            KeyCode::Enter => {
                                if let Some(target_widget) = widget_explorer.get_selected_widget() {
                                    let target = target_widget.to_string();
                                    widget_explorer.close();
                                    let mut mode = env.keybindings.read().active_mode;
                                    let mut last_kill = None;
                                    let mut ctx = widgets::WidgetContext {
                                        text_buf: &mut text_buf,
                                        env: &env,
                                        keymap_mode: &mut mode,
                                        history_mgr: &mut history_mgr,
                                        comp_mgr: &mut comp_mgr,
                                        widget_explorer: &mut widget_explorer,
                                        current_dir: &current_dir,
                                        hostname: &hostname,
                                        session_id: &session_id,
                                        help_visible: &mut help_visible,
                                        last_kill: &mut last_kill,
                                        current_hint: &current_hint,
                                        history_index: &mut history_index,
                                        filtered_history: &mut filtered_history,
                                        temp_input: &mut temp_input,
                                    };
                                    if let widgets::WidgetAction::AcceptLine =
                                        widgets::execute_widget(&target, &mut ctx)
                                    {
                                        text_buf.commit_transaction();
                                        let command_line = text_buf.text();
                                        let trimmed = command_line.trim().to_string();
                                        if !trimmed.is_empty() {
                                            command_to_execute = Some(trimmed);
                                            break 'input_loop;
                                        }
                                    }
                                } else {
                                    widget_explorer.close();
                                }
                                redraw = true;
                            }
                            KeyCode::Backspace => {
                                widget_explorer.query.pop();
                                widget_explorer.update_filter(&env);
                                redraw = true;
                            }
                            KeyCode::Char(c)
                                if !key.modifiers.contains(KeyModifiers::CONTROL)
                                    && !key.modifiers.contains(KeyModifiers::ALT) =>
                            {
                                widget_explorer.query.push(c);
                                widget_explorer.update_filter(&env);
                                redraw = true;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    if comp_mgr.visible && comp_mgr.active_selection && key.code == KeyCode::Right {
                        if let Some(s) = comp_mgr.get_selected_suggestion().cloned() {
                            let line = text_buf.text().clone();
                            apply_completion(&mut text_buf, &line, &s);
                        }
                        comp_mgr.clear();
                        redraw = true;
                        continue;
                    }
                    // Typed characters fall through to the main input handler below,
                    // which inserts the character and re-filters completions live.

                    if comp_mgr.visible {
                        match key.code {
                            KeyCode::Esc => {
                                comp_mgr.clear();
                                help_visible = false;
                                history_index = None;
                                redraw = true;
                                continue;
                            }
                            KeyCode::F(1) => {
                                comp_mgr.clear();
                                help_visible = !help_visible;
                                redraw = true;
                                continue;
                            }
                            KeyCode::Tab => {
                                comp_mgr.active_selection = true;
                                comp_mgr.select_next();
                                redraw = true;
                                continue;
                            }
                            KeyCode::BackTab => {
                                comp_mgr.active_selection = true;
                                comp_mgr.select_prev();
                                redraw = true;
                                continue;
                            }
                            KeyCode::Down => {
                                comp_mgr.active_selection = true;
                                comp_mgr.select_next();
                                redraw = true;
                                continue;
                            }
                            KeyCode::Up => {
                                comp_mgr.active_selection = true;
                                comp_mgr.select_prev();
                                redraw = true;
                                continue;
                            }
                            KeyCode::PageDown => {
                                comp_mgr.active_selection = true;
                                let page_size =
                                    (current_viewport_height.saturating_sub(2)).max(1) as usize;
                                comp_mgr.page_down(page_size);
                                redraw = true;
                                continue;
                            }
                            KeyCode::PageUp => {
                                comp_mgr.active_selection = true;
                                let page_size =
                                    (current_viewport_height.saturating_sub(2)).max(1) as usize;
                                comp_mgr.page_up(page_size);
                                redraw = true;
                                continue;
                            }
                            KeyCode::Right if comp_mgr.active_selection => {
                                if let Some(s) = comp_mgr.get_selected_suggestion().cloned() {
                                    let line = text_buf.text().clone();
                                    apply_completion(&mut text_buf, &line, &s);
                                }
                                comp_mgr
                                    .refresh_after_completion(&text_buf.text(), text_buf.cursor());
                                redraw = true;
                                continue;
                            }
                            _ => {}
                        }
                    }

                    // --- First-Class Keybinding & Widget Architecture Dispatch ---
                    let chord = widgets::crossterm_key_to_chord(&key);
                    let bound_action = {
                        let reg = env.keybindings.read();
                        reg.get_action(reg.active_mode, &chord).cloned()
                    };

                    if let Some(action) = bound_action {
                        match action {
                            fshell_engine::keybindings::KeyAction::Widget(ref widget_name) => {
                                let mut mode = env.keybindings.read().active_mode;
                                let mut last_kill = None;
                                let mut ctx = widgets::WidgetContext {
                                    text_buf: &mut text_buf,
                                    env: &env,
                                    keymap_mode: &mut mode,
                                    history_mgr: &mut history_mgr,
                                    comp_mgr: &mut comp_mgr,
                                    widget_explorer: &mut widget_explorer,
                                    current_dir: &current_dir,
                                    hostname: &hostname,
                                    session_id: &session_id,
                                    help_visible: &mut help_visible,
                                    last_kill: &mut last_kill,
                                    current_hint: &current_hint,
                                    history_index: &mut history_index,
                                    filtered_history: &mut filtered_history,
                                    temp_input: &mut temp_input,
                                };
                                match widgets::execute_widget(widget_name, &mut ctx) {
                                    widgets::WidgetAction::AcceptLine => {
                                        help_visible = false;
                                        text_buf.commit_transaction();
                                        let command_line = text_buf.text();
                                        let trimmed = command_line.trim().to_string();
                                        if !trimmed.is_empty() {
                                            let validation =
                                                fshell_core::validate_input(&command_line);
                                            match validation {
                                                fshell_core::ValidationResult::Incomplete {
                                                    ..
                                                } => {
                                                    history_index = None;
                                                    let indent = fshell_core::compute_indent_depth(
                                                        &command_line,
                                                    );
                                                    text_buf.insert_char('\n');
                                                    for _ in 0..(indent * 4) {
                                                        text_buf.insert_char(' ');
                                                    }
                                                    in_continuation = true;
                                                    redraw = true;
                                                    continue;
                                                }
                                                _ => {
                                                    in_continuation = false;
                                                    command_to_execute = Some(trimmed);
                                                    break 'input_loop;
                                                }
                                            }
                                        } else if !in_continuation {
                                            prompt_mgr.refresh_snapshot(&current_dir);
                                            let final_ansi = prompt_mgr.render_prompt_final_ansi();
                                            let _ = crossterm::execute!(
                                                std::io::stdout(),
                                                crossterm::cursor::MoveToColumn(0),
                                                crossterm::terminal::Clear(
                                                    crossterm::terminal::ClearType::CurrentLine,
                                                ),
                                            );
                                            println!("\r\x1b[2K{}", final_ansi);
                                            let _ = std::io::Write::flush(&mut std::io::stdout());
                                            break 'input_loop;
                                        } else {
                                            in_continuation = false;
                                            text_buf.clear();
                                            redraw = true;
                                            continue;
                                        }
                                    }
                                    widgets::WidgetAction::EditInExternalEditor => {
                                        let editor = std::env::var("EDITOR")
                                            .unwrap_or_else(|_| "nano".to_string());
                                        let temp_path = std::env::temp_dir()
                                            .join(format!("fsh_edit_{}.fsh", std::process::id()));
                                        let _ = std::fs::write(&temp_path, text_buf.text());
                                        let _ = std::process::Command::new(editor)
                                            .arg(&temp_path)
                                            .status();
                                        if let Ok(new_content) = std::fs::read_to_string(&temp_path)
                                        {
                                            text_buf.replace_content(&new_content);
                                        }
                                        let _ = std::fs::remove_file(&temp_path);
                                        terminal = None;
                                        redraw = true;
                                        continue;
                                    }
                                    widgets::WidgetAction::Redraw => {
                                        redraw = true;
                                        continue;
                                    }
                                    widgets::WidgetAction::Abort => {
                                        history_index = None;
                                        if in_continuation {
                                            in_continuation = false;
                                            text_buf.clear();
                                            redraw = true;
                                            continue;
                                        }
                                        let cmd = text_buf.text();
                                        history_mgr.add_aborted(&cmd);
                                        aborted_command = Some(cmd);
                                        break 'input_loop;
                                    }
                                    widgets::WidgetAction::Exit => {
                                        exit_repl = true;
                                        break 'input_loop;
                                    }
                                    widgets::WidgetAction::InsertMacro(m) => {
                                        if m.ends_with('\n') {
                                            let text = m.trim_end_matches('\n');
                                            text_buf.insert_str(text);
                                            text_buf.commit_transaction();
                                            let command_line = text_buf.text();
                                            let trimmed = command_line.trim().to_string();
                                            if !trimmed.is_empty() {
                                                in_continuation = false;
                                                command_to_execute = Some(trimmed);
                                                break 'input_loop;
                                            }
                                        } else {
                                            text_buf.insert_str(&m);
                                            redraw = true;
                                            continue;
                                        }
                                    }
                                    widgets::WidgetAction::Continue => {}
                                }
                            }
                            fshell_engine::keybindings::KeyAction::Macro(m) => {
                                if m.ends_with('\n') {
                                    let text = m.trim_end_matches('\n');
                                    text_buf.insert_str(text);
                                    text_buf.commit_transaction();
                                    let command_line = text_buf.text();
                                    let trimmed = command_line.trim().to_string();
                                    if !trimmed.is_empty() {
                                        in_continuation = false;
                                        command_to_execute = Some(trimmed);
                                        break 'input_loop;
                                    }
                                } else {
                                    text_buf.insert_str(&m);
                                    redraw = true;
                                    continue;
                                }
                            }
                            fshell_engine::keybindings::KeyAction::Function(_fn_name) => {
                                redraw = true;
                                continue;
                            }
                        }
                    }

                    // ignoreeof: any non-EOF key clears the pending flag
                    {
                        let is_eof_key = key.code == KeyCode::Char('d')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                            && text_buf.is_empty()
                            && !in_continuation;
                        if !is_eof_key {
                            eof_pending = false;
                        }
                    }
                    match key.code {
                        KeyCode::Left => {
                            history_index = None;
                            if key.modifiers.contains(KeyModifiers::CONTROL)
                                || key.modifiers.contains(KeyModifiers::ALT)
                            {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    if !text_buf.has_selection() {
                                        text_buf.start_selection();
                                    }
                                    text_buf.move_word_left();
                                    text_buf.extend_selection();
                                } else {
                                    text_buf.clear_selection();
                                    text_buf.move_word_left();
                                }
                            } else if key.modifiers.contains(KeyModifiers::SHIFT) {
                                if !text_buf.has_selection() {
                                    text_buf.start_selection();
                                }
                                text_buf.move_left();
                                text_buf.extend_selection();
                            } else {
                                text_buf.clear_selection();
                                text_buf.move_left();
                            }
                            comp_mgr.clear();
                            redraw = true;
                        }
                        KeyCode::Right => {
                            history_index = None;
                            if key.modifiers.contains(KeyModifiers::CONTROL)
                                || key.modifiers.contains(KeyModifiers::ALT)
                            {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    if !text_buf.has_selection() {
                                        text_buf.start_selection();
                                    }
                                    text_buf.move_word_right();
                                    text_buf.extend_selection();
                                } else {
                                    text_buf.clear_selection();
                                    text_buf.move_word_right();
                                }
                            } else if text_buf.cursor() == text_buf.len()
                                && !current_hint.is_empty()
                            {
                                // Accept full hint at end of line (fish/VS Code behavior)
                                text_buf.clear_selection();
                                text_buf.insert_str(&current_hint);
                            } else if key.modifiers.contains(KeyModifiers::SHIFT) {
                                if !text_buf.has_selection() {
                                    text_buf.start_selection();
                                }
                                text_buf.move_right();
                                text_buf.extend_selection();
                            } else {
                                text_buf.clear_selection();
                                text_buf.move_right();
                            }
                            comp_mgr.clear();
                            redraw = true;
                        }
                        KeyCode::Up => {
                            text_buf.clear_selection();
                            if text_buf.move_up() {
                                redraw = true;
                                continue;
                            }
                            // In a multi-line buffer without active history search, stay on top line unless empty
                            if text_buf.text().contains('\n')
                                && history_index.is_none()
                                && !text_buf.is_empty()
                            {
                                redraw = true;
                                continue;
                            }
                            // On first Up press, query SQLite with the current prefix
                            if history_index.is_none() {
                                // Bug 8.8: Only commit if there are pending edits
                                if !text_buf.is_empty() {
                                    text_buf.commit_transaction();
                                }
                                temp_input = text_buf.text();
                                let prefix = temp_input.clone();
                                if prefix.is_empty() {
                                    filtered_history =
                                        crate::history::query_history_prefix("", 100)
                                            .unwrap_or_default();
                                } else {
                                    // Prefix search: bounded window. Cap how many
                                    // distinct matches we materialize so Up-recall
                                    // stays fast on large histories (users never
                                    // scroll past a few hundred commands).
                                    filtered_history =
                                        crate::history::query_history_prefix(&prefix, 256)
                                            .unwrap_or_default();
                                    if filtered_history.is_empty() {
                                        // No prefix matches; fall back to recent unfiltered history
                                        filtered_history =
                                            crate::history::query_history_prefix("", 100)
                                                .unwrap_or_default();
                                    }
                                }
                            }
                            if !filtered_history.is_empty() {
                                let next_idx = match history_index {
                                    None => Some(0),
                                    Some(idx) => {
                                        if idx + 1 < filtered_history.len() {
                                            Some(idx + 1)
                                        } else {
                                            Some(idx)
                                        }
                                    }
                                };
                                if let Some(idx) = next_idx {
                                    history_index = Some(idx);
                                    text_buf.replace_content(&filtered_history[idx]);
                                    redraw = true;
                                }
                            }
                        }
                        KeyCode::Down => {
                            text_buf.clear_selection();
                            if text_buf.move_down() {
                                redraw = true;
                                continue;
                            }
                            if text_buf.text().contains('\n') && history_index.is_none() {
                                redraw = true;
                                continue;
                            }
                            if !filtered_history.is_empty()
                                && let Some(idx) = history_index
                            {
                                if idx > 0 {
                                    history_index = Some(idx - 1);
                                    text_buf.replace_content(&filtered_history[idx - 1]);
                                } else {
                                    // Back to original input; reset prefix search
                                    history_index = None;
                                    text_buf.replace_content(&temp_input);
                                }
                                redraw = true;
                            }
                        }
                        KeyCode::Home => {
                            history_index = None;
                            if key.modifiers.contains(KeyModifiers::ALT) {
                                // Alt+Home = absolute buffer start (Bug 1.6)
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    if !text_buf.has_selection() {
                                        text_buf.start_selection();
                                    }
                                    text_buf.move_to_start();
                                    text_buf.extend_selection();
                                } else {
                                    text_buf.clear_selection();
                                    text_buf.move_to_start();
                                }
                            } else {
                                // Home = current line start (Bug 1.6 fix)
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    if !text_buf.has_selection() {
                                        text_buf.start_selection();
                                    }
                                    text_buf.move_to_line_start();
                                    text_buf.extend_selection();
                                } else {
                                    text_buf.clear_selection();
                                    text_buf.move_to_line_start();
                                }
                            }
                            comp_mgr.clear();
                            redraw = true;
                        }
                        KeyCode::End => {
                            history_index = None;
                            if key.modifiers.contains(KeyModifiers::ALT) {
                                // Alt+End = absolute buffer end (Bug 1.6)
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    if !text_buf.has_selection() {
                                        text_buf.start_selection();
                                    }
                                    text_buf.move_to_end();
                                    text_buf.extend_selection();
                                } else {
                                    text_buf.clear_selection();
                                    text_buf.move_to_end();
                                }
                            } else {
                                // End = current line end (Bug 1.6 fix)
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    if !text_buf.has_selection() {
                                        text_buf.start_selection();
                                    }
                                    text_buf.move_to_line_end();
                                    text_buf.extend_selection();
                                } else {
                                    text_buf.clear_selection();
                                    text_buf.move_to_line_end();
                                }
                            }
                            comp_mgr.clear();
                            redraw = true;
                        }
                        KeyCode::Backspace => {
                            history_index = None;
                            if key.modifiers.contains(KeyModifiers::CONTROL)
                                || key.modifiers.contains(KeyModifiers::ALT)
                            {
                                text_buf.delete_word_left();
                            } else {
                                let chars = text_buf.chars();
                                let cur = text_buf.cursor();
                                let line_start = chars[..cur]
                                    .iter()
                                    .rposition(|&c| c == '\n')
                                    .map(|p| p + 1)
                                    .unwrap_or(0);
                                let leading = &chars[line_start..cur];
                                if !leading.is_empty()
                                    && leading.iter().all(|&c| c == ' ')
                                    && leading.len() >= 2
                                {
                                    let dedent = if leading.len().is_multiple_of(2) {
                                        2
                                    } else {
                                        1
                                    };
                                    for _ in 0..dedent {
                                        text_buf.delete_left();
                                    }
                                } else {
                                    text_buf.delete_left();
                                }
                            }
                            if comp_mgr.visible {
                                let buf_text = text_buf.text();
                                let partial = extract_partial_word(&buf_text, text_buf.cursor());
                                comp_mgr.filter(partial);
                            } else {
                                comp_mgr.update(&text_buf.text(), text_buf.cursor(), false);
                            }
                            redraw = true;
                        }
                        KeyCode::Delete => {
                            history_index = None;
                            text_buf.delete_right();
                            if comp_mgr.visible {
                                let buf_text = text_buf.text();
                                let partial = extract_partial_word(&buf_text, text_buf.cursor());
                                comp_mgr.filter(partial);
                            } else {
                                comp_mgr.update(&text_buf.text(), text_buf.cursor(), false);
                            }
                            redraw = true;
                        }
                        KeyCode::Esc => {
                            history_index = None;
                            if mouse_mgr.mode == MouseMode::Simple {
                                if mouse_mgr.is_captured {
                                    mouse_mgr.disable_capture();
                                } else {
                                    mouse_mgr.enable_capture();
                                }
                            }
                            comp_mgr.clear();
                            help_visible = false;
                            redraw = true;
                        }
                        KeyCode::F(1) => {
                            help_visible = !help_visible;
                            redraw = true;
                        }
                        KeyCode::BackTab => {
                            // Bug 1.8: Shift+Tab outside of completion popup — add indentation or
                            // trigger reverse completion. For now: do nothing (explicit no-op).
                            // This prevents silent fallthrough to the wildcard arm.
                            redraw = true;
                        }
                        KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            history_index = None;
                            text_buf.undo();
                            comp_mgr.clear();
                            redraw = true;
                        }
                        KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            history_index = None;
                            text_buf.redo();
                            comp_mgr.clear();
                            redraw = true;
                        }
                        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            history_index = None;
                            text_buf.move_to_line_start();
                            comp_mgr.clear();
                            redraw = true;
                        }
                        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            history_index = None;
                            text_buf.move_to_line_end();
                            comp_mgr.clear();
                            redraw = true;
                        }
                        KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Bug 1.4: Ctrl+L clear screen
                            history_index = None;
                            let _ = crossterm::execute!(
                                std::io::stdout(),
                                crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
                                crossterm::cursor::MoveTo(0, 0),
                            );
                            // Drop the ratatui terminal so it is recreated on the next
                            // redraw.  After ClearType::All the physical screen is blank,
                            // but ratatui's internal "previous frame" buffer still describes
                            // the old content.  Without a drop, the next draw() diffs against
                            // that stale buffer and skips emitting cells that look unchanged —
                            // leaving a blank screen instead of repainting the prompt.
                            terminal = None;
                            comp_mgr.clear();
                            redraw = true;
                        }
                        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            history_index = None;
                            text_buf.delete_word_left();
                            if comp_mgr.visible {
                                let buf_text = text_buf.text();
                                let partial = extract_partial_word(&buf_text, text_buf.cursor());
                                comp_mgr.filter(partial);
                            } else {
                                comp_mgr.update(&text_buf.text(), text_buf.cursor(), false);
                            }
                            redraw = true;
                        }
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::ALT) => {
                            if text_buf.has_selection() {
                                let sel = text_buf.selected_text();
                                clipboard::copy_to_clipboard(&sel);
                            }
                            redraw = true;
                        }
                        KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::ALT) => {
                            if text_buf.has_selection() {
                                let sel = text_buf.selected_text();
                                clipboard::copy_to_clipboard(&sel);
                                text_buf.delete_selection();
                            }
                            redraw = true;
                        }
                        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::ALT) => {
                            if let Some(text) = clipboard::paste_from_clipboard() {
                                text_buf.insert_str(&text);
                            }
                            redraw = true;
                        }
                        KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::ALT) => {
                            history_index = None;
                            text_buf.clear_selection();
                            text_buf.move_word_left();
                            comp_mgr.clear();
                            redraw = true;
                        }
                        KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::ALT) => {
                            history_index = None;
                            text_buf.clear_selection();
                            text_buf.move_word_right();
                            comp_mgr.clear();
                            redraw = true;
                        }
                        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            history_index = None;
                            history_mgr.active = true;
                            history_mgr.query.clear();
                            history_mgr.selected_idx = 0;
                            history_mgr.update_results(&current_dir, &hostname, &session_id);
                            redraw = true;
                        }
                        KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            history_index = None;
                            history_mgr.active = true;
                            history_mgr.explorer_active = true;
                            history_mgr.query.clear();
                            history_mgr.selected_idx = 0;
                            history_mgr.update_results(&current_dir, &hostname, &session_id);
                            redraw = true;
                        }

                        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::ALT) => {
                            history_index = None;
                            history_mgr.active = true;
                            history_mgr.aborted_active = true;
                            history_mgr.selected_idx = 0;
                            redraw = true;
                        }
                        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::ALT) => {
                            history_index = None;
                            let prompt = text_buf.text();
                            agent_state.trigger_query(&prompt, &env);
                            redraw = true;
                        }
                        KeyCode::Enter
                            if key.modifiers.contains(KeyModifiers::ALT)
                                || key.modifiers.contains(KeyModifiers::SHIFT)
                                || key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            // Soft newline: insert newline directly into buffer with auto-indentation at cursor
                            history_index = None;
                            let indent = fshell_core::compute_indent_depth_at(
                                &text_buf.text(),
                                Some(text_buf.cursor()),
                            );
                            text_buf.insert_char('\n');
                            for _ in 0..(indent * 2) {
                                text_buf.insert_char(' ');
                            }
                            comp_mgr.clear();
                            redraw = true;
                        }
                        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl+J is standard LineFeed (soft newline) in terminal emulators
                            history_index = None;
                            let indent = fshell_core::compute_indent_depth_at(
                                &text_buf.text(),
                                Some(text_buf.cursor()),
                            );
                            text_buf.insert_char('\n');
                            for _ in 0..(indent * 2) {
                                text_buf.insert_char(' ');
                            }
                            comp_mgr.clear();
                            redraw = true;
                        }
                        KeyCode::Enter => {
                            help_visible = false;
                            text_buf.commit_transaction();
                            let command_line = text_buf.text();
                            let trimmed = command_line.trim().to_string();
                            if !trimmed.is_empty() {
                                let validation = fshell_core::validate_input(&command_line);
                                let is_last_line = {
                                    let chars = text_buf.chars();
                                    let cursor = text_buf.cursor();
                                    !chars.iter().skip(cursor).any(|&c| c == '\n')
                                };
                                match validation {
                                    fshell_core::ValidationResult::Incomplete { .. } => {
                                        // Incomplete multi-line input — insert newline with auto-indentation and remain in editor
                                        history_index = None;
                                        let indent = fshell_core::compute_indent_depth_at(
                                            &command_line,
                                            Some(text_buf.cursor()),
                                        );
                                        text_buf.insert_char('\n');
                                        for _ in 0..(indent * 2) {
                                            text_buf.insert_char(' ');
                                        }
                                        in_continuation = true;
                                        redraw = true;
                                        continue;
                                    }
                                    _ if !is_last_line && command_line.contains('\n') => {
                                        // Inside a multi-line buffer and not on the last line: split the line at cursor
                                        history_index = None;
                                        let indent = fshell_core::compute_indent_depth_at(
                                            &command_line,
                                            Some(text_buf.cursor()),
                                        );
                                        text_buf.insert_char('\n');
                                        for _ in 0..(indent * 2) {
                                            text_buf.insert_char(' ');
                                        }
                                        in_continuation = true;
                                        redraw = true;
                                        continue;
                                    }
                                    _ => {
                                        // Complete statement or definitive parse error — execute full buffer
                                        in_continuation = false;
                                        command_to_execute = Some(trimmed);
                                    }
                                }
                            } else if !in_continuation {
                                // Empty line (not continuation): print the prompt to
                                // scrollback like a real command, so the user sees a
                                // "new" prompt appear (like zsh does on bare Enter).
                                prompt_mgr.refresh_snapshot(&current_dir);
                                let final_ansi = prompt_mgr.render_prompt_final_ansi();
                                let _ = crossterm::execute!(
                                    std::io::stdout(),
                                    crossterm::cursor::MoveToColumn(0),
                                    crossterm::terminal::Clear(
                                        crossterm::terminal::ClearType::CurrentLine,
                                    ),
                                );
                                println!("\r\x1b[2K{}", final_ansi);
                                let _ = std::io::Write::flush(&mut std::io::stdout());
                                text_buf.clear();
                            } else {
                                // Empty line on continuation — present it
                                // This cancels continuation
                                in_continuation = false;
                                text_buf.clear();
                                redraw = true;
                                continue;
                            }
                            break 'input_loop;
                        }
                        KeyCode::Char('d')
                            if key.modifiers.contains(KeyModifiers::CONTROL)
                                && text_buf.is_empty() =>
                        {
                            if !in_continuation {
                                if env.options.read().ignoreeof {
                                    if eof_pending {
                                        exit_repl = true;
                                        break 'input_loop;
                                    } else {
                                        eof_pending = true;
                                        let _ = crossterm::execute!(
                                            std::io::stderr(),
                                            crossterm::style::Print(
                                                "\r\nUse 'exit' to leave shell (press Ctrl-D again)\r\n"
                                            )
                                        );
                                        redraw = true;
                                        continue;
                                    }
                                } else {
                                    exit_repl = true;
                                    break 'input_loop;
                                }
                            } else {
                                // Empty line in continuation: cancel
                                in_continuation = false;
                                text_buf.clear();
                                redraw = true;
                                continue;
                            }
                        }
                        KeyCode::Char('d')
                            if key.modifiers.contains(KeyModifiers::CONTROL)
                                && !text_buf.is_empty() =>
                        {
                            // Bug 1.3: Ctrl+D on non-empty buffer = delete_right (forward delete)
                            history_index = None;
                            text_buf.delete_right();
                            comp_mgr.update(&text_buf.text(), text_buf.cursor(), false);
                            redraw = true;
                        }
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            history_index = None;
                            if in_continuation {
                                // Cancel multi-line input on Ctrl+C
                                in_continuation = false;
                                text_buf.clear();
                                redraw = true;
                                continue;
                            }
                            let cmd = text_buf.text();
                            history_mgr.add_aborted(&cmd);
                            aborted_command = Some(cmd);
                            break 'input_loop;
                        }
                        KeyCode::Tab => {
                            history_index = None;
                            if !comp_mgr.visible {
                                // I2: First Tab — fetch suggestions
                                comp_mgr.update(&text_buf.text(), text_buf.cursor(), true);
                                if comp_mgr.visible && !comp_mgr.suggestions.is_empty() {
                                    // If exactly one suggestion, accept it
                                    if comp_mgr.suggestions.len() == 1 {
                                        if let Some(s) = comp_mgr.suggestions.first() {
                                            // Bug 1.1: Use Suggestion span
                                            let line = text_buf.text();
                                            apply_completion(&mut text_buf, &line, s);
                                        }
                                        comp_mgr.refresh_after_completion(
                                            &text_buf.text(),
                                            text_buf.cursor(),
                                        );
                                    } else if !comp_mgr.prefix_accepted {
                                        // Multiple suggestions — try longest common prefix
                                        if let Some(prefix) = comp_mgr.longest_common_prefix() {
                                            let cursor = text_buf.cursor();
                                            let chars = text_buf.chars();
                                            let last_word_start = chars[..cursor]
                                                .iter()
                                                .rposition(|&c| {
                                                    c.is_whitespace()
                                                        || c == '|'
                                                        || c == '>'
                                                        || c == '<'
                                                })
                                                .map(|idx| idx + 1)
                                                .unwrap_or(0);
                                            let current_word: String =
                                                chars[last_word_start..cursor].iter().collect();
                                            if prefix.len() > current_word.len() {
                                                // Fill in the common prefix
                                                let extra = &prefix[current_word.len()..];
                                                for c in extra.chars() {
                                                    text_buf.insert_char(c);
                                                }
                                                append_slash_if_dir(&mut text_buf, &prefix);
                                                comp_mgr.prefix_accepted = true;
                                                // Keep completions visible for next Tab
                                            } else {
                                                // No extension — just show popup
                                                comp_mgr.active_selection = true;
                                                comp_mgr.prefix_accepted = true;
                                            }
                                        } else {
                                            // No common prefix, just show popup
                                            comp_mgr.active_selection = true;
                                            comp_mgr.prefix_accepted = true;
                                        }
                                    } else {
                                        // Prefix already accepted, now cycle
                                        comp_mgr.active_selection = true;
                                        comp_mgr.select_next();
                                    }
                                }
                            } else {
                                // Already visible — cycle through
                                comp_mgr.active_selection = true;
                                comp_mgr.select_next();
                            }
                            redraw = true;
                        }
                        KeyCode::Char(c)
                            if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
                        {
                            history_index = None;
                            fshell_core::debug_log!("prefix_search cleared on char input");
                            text_buf.insert_char(c);
                            if comp_mgr.visible {
                                let buf_text = text_buf.text();
                                let partial = extract_partial_word(&buf_text, text_buf.cursor());
                                comp_mgr.filter(partial);
                            } else {
                                comp_mgr.update(&text_buf.text(), text_buf.cursor(), false);
                            }
                            redraw = true;
                        }
                        _ => {}
                    }
                }
            } else {
                // Event poll timed out (no user input)
                // Check if we need to force a redraw for animations/widgets
                let has_active_animations = agent_state.is_loading
                    || (comp_mgr.visible
                        && comp_mgr
                            .suggestions
                            .iter()
                            .any(|s| s.description.as_ref().is_some_and(|d| d.contains('\t'))))
                    || prompt_mgr
                        .widgets
                        .iter()
                        .any(|w| *w.is_running.lock().unwrap_or_else(|e| e.into_inner()));

                if has_active_animations {
                    redraw = true;
                }
            }
        }

        comp_mgr.clear();
        help_visible = false;
        history_mgr.reset();
        agent_state.active = false;

        // 3. Exit fullscreen TUI and restore terminal to normal mode,
        //    then print the final prompt + command for command execution.
        if let Some(t) = terminal.take() {
            // Explicitly clear the viewport area before dropping.
            {
                let _ = crossterm::execute!(
                    std::io::stdout(),
                    crossterm::cursor::SavePosition,
                    crossterm::cursor::MoveToColumn(0),
                    crossterm::terminal::Clear(crossterm::terminal::ClearType::FromCursorDown,),
                    crossterm::cursor::RestorePosition,
                );
                let _ = std::io::Write::flush(&mut std::io::stdout());
            }
            drop(t);
        }
        // With session-wide raw (`FSH_RAW_SESSION=1`) the inline viewport is
        // dropped but raw/auxiliary modes stay on — only the legacy
        // per-command path did `TuiGuard::drop` → `disable_raw_mode` here.
        if _guard.is_some() {
            drop(_guard);
        }

        if exit_repl {
            // Drop the inline viewport first so the exit doesn't leave the
            // alternate row range half-drawn, then restore the session.
            // With `FSH_RAW_SESSION=1` raw was session-wide, so we must
            // explicitly drop `Session` before breaking — otherwise the
            // parent shell (cmux/zsh) inherits raw (no echo, no ONLCR →
            // smear on next `ls`). With legacy per-command raw the guard
            // was already dropped above, so this is idempotent. Do NOT
            // break straight to `std::process::exit` elsewhere.
            if let Some(t) = terminal.take() {
                let _ = crossterm::execute!(
                    std::io::stdout(),
                    crossterm::cursor::SavePosition,
                    crossterm::cursor::MoveToColumn(0),
                    crossterm::terminal::Clear(crossterm::terminal::ClearType::FromCursorDown),
                    crossterm::cursor::RestorePosition,
                );
                let _ = std::io::Write::flush(&mut std::io::stdout());
                drop(t);
            }
            drop(_raw_session);
            break 'repl_loop;
        }

        if let Some(cmd) = aborted_command {
            let left_ansi = prompt_mgr.render_prompt_left_ansi();
            // In raw mode \n does not imply \r, so a multiline aborted command would
            // render as a staircase (each \n keeps the column of the previous line).
            // Rewrite \n -> \r\n and emit an explicit \r\n terminator so the next
            // prompt starts at column 0 even while _raw_session is still active.
            let safe_cmd = cmd.replace('\n', "\r\n");
            print!("\r\x1b[2K{}{}\x1b[31m ^C\x1b[0m\r\n", left_ansi, safe_cmd);
            let _ = std::io::Write::flush(&mut std::io::stdout());
            status_bar.end_command_timer();
            status_bar.set_exit_code(130);
            let snap = crate::refresh_prompt_snapshot(&env, &current_dir);
            status_bar.set_job_count(snap.job_count);
            if let Some(ref gs) = snap.git_status {
                status_bar.update_git(Some(gs.branch.clone()), !gs.clean, gs.ahead, gs.behind);
            }
            text_buf.clear();
            text_buf.clear_history();
            comp_mgr.clear();
            history_mgr.reset();
            history_index = None;
            text_scroll_offset = 0;
        } else if let Some(trimmed) = command_to_execute {
            if anchor_output {
                let anchor_debug_on = std::env::var("FSH_REPL_ANCHOR_DEBUG").as_deref() == Ok("1");
                if anchor_debug_on
                    && let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty")
                {
                    use std::io::Write;
                    let _ = writeln!(tty, "[anchor] executing anchored: {}", trimmed);
                }
                env.is_captured = true;
                let mut guard = capture::CaptureGuard::new();
                let capture_ok = guard.ok();
                let capture_err = guard.error().map(|s| s.to_string());
                status_bar.start_command_timer();
                env.is_command_running.store(true, Ordering::SeqCst);
                use futures::FutureExt;
                let handle_fut = std::panic::AssertUnwindSafe(crate::handle_line_generic(
                    &env,
                    &trimmed,
                    &current_dir,
                    &session_id,
                ));
                let handle_result = match handle_fut.catch_unwind().await {
                    Ok(r) => r,
                    Err(_) => {
                        eprintln!("\n\x1b[1;33mnotice:\x1b[0m FTUI recovered from panic.");
                        Ok(())
                    }
                };
                env.is_command_running.store(false, Ordering::SeqCst);
                // Flush stdout so any buffered output reaches the pipe before we close it
                use std::io::Write;
                let _ = std::io::stdout().flush();
                let mut captured = guard.finish();
                env.is_captured = false;
                if !capture_ok {
                    let msg = capture_err.unwrap_or_else(|| "capture failed".to_string());
                    // Surface capture failure instead of silently showing empty output (R6).
                    eprintln!("\r\x1b[33m[capture] {msg} — output not captured\x1b[0m");
                    captured.push(format!("[capture failed: {msg}]"));
                }
                // Split multiline commands into separate lines so viewport height accounting is correct (ghost fix).
                let cmd_lines: Vec<String> = trimmed.lines().map(|s| s.to_string()).collect();
                // Insert in order at front
                for line in cmd_lines.into_iter().rev() {
                    captured.insert(0, line);
                }
                // A6: keep only the viewport-height worth of anchored output
                // plus safety tail (unified cap, not unbounded 500). The pane
                // height is limited by `cap`, so storing far more lines wastes
                // memory and hides the anchoring invariant. Keep a bounded safety
                // tail for tiny terminals where the viewport hides output.
                // Correctness: never grow past ANCHORED_OUTPUT_SAFETY_CAP.
                let term_cap = {
                    let (_, h) = crossterm::terminal::size().unwrap_or((80, 24));
                    if status_bar.visible {
                        h.saturating_sub(2)
                    } else {
                        h
                    }
                };
                let keep = (term_cap as usize).clamp(20, ANCHORED_OUTPUT_SAFETY_CAP);
                if captured.len() > keep {
                    captured.drain(0..captured.len() - keep);
                }
                output_lines = captured;
                if handle_result.is_err() {
                    // `exit` / `ExitSignal`: out of session-wide raw *before*
                    // breaking, or the parent shell inherits raw (smear of
                    // next `ls` as in the reported trace).
                    if _raw_session.is_some() {
                        drop(_raw_session.take());
                    }
                    break 'repl_loop;
                }
                comp_mgr.clear();
                status_bar.end_command_timer();
                let snap = crate::refresh_prompt_snapshot(&env, &current_dir);
                status_bar.set_exit_code(snap.exit_code);
                status_bar.set_job_count(snap.job_count);
                if let Some(ref gs) = snap.git_status {
                    status_bar.update_git(Some(gs.branch.clone()), !gs.clean, gs.ahead, gs.behind);
                }
            } else {
                let ftui_debug = std::env::var("FSH_CNF_DEBUG").as_deref() == Ok("1");
                let ftui_start = std::time::Instant::now();
                // Fullscreen apps (vim, less, …) and legacy non-raw-session
                // still need a cooked PTY. In raw-session mode use the session's
                // SuspendGuard so raw is re-entered even on panic/ctrl-c.
                let _suspend: Option<raw::SuspendGuard<'_>> = if let Some(s) =
                    _raw_session.as_deref()
                {
                    match s.suspend() {
                        Ok(g) => Some(g),
                        Err(e) => {
                            eprintln!(
                                "\r\n\x1b[1;33mwarn:\x1b[0m raw suspend failed: {e} — continuing with legacy toggle"
                            );
                            let _ = crossterm::terminal::disable_raw_mode();
                            None
                        }
                    }
                } else {
                    let _ = crossterm::terminal::disable_raw_mode();
                    None
                };
                let t_disable = ftui_start.elapsed();
                let (term_w, term_h) = crossterm::terminal::size().unwrap_or((80, 24));
                prompt_mgr.refresh_snapshot(&current_dir);
                let t_refresh = ftui_start.elapsed();
                let final_ansi = prompt_mgr.render_prompt_final_ansi();
                if let Some(oy) = prompt_origin_y {
                    let mut stdout = std::io::stdout();
                    if trimmed.contains('\n') {
                        let lines: Vec<&str> = trimmed.split('\n').collect();
                        let total_lines = lines.len();
                        let total_printed_rows = 1 + total_lines;

                        // Row 0: Elevated prompt header
                        let _ = crossterm::execute!(
                            stdout,
                            crossterm::cursor::MoveTo(0, oy),
                            crossterm::terminal::Clear(crossterm::terminal::ClearType::CurrentLine),
                            crossterm::style::Print(format!("{}\r\n", final_ansi)),
                        );

                        // Rows 1..=total_lines: Clean code lines flush at column 0
                        for (i, line) in lines.iter().enumerate() {
                            let row_y = oy + 1 + i as u16;
                            let _ = crossterm::execute!(
                                stdout,
                                crossterm::cursor::MoveTo(0, row_y),
                                crossterm::terminal::Clear(
                                    crossterm::terminal::ClearType::CurrentLine
                                ),
                                crossterm::style::Print(format!("{}\r\n", line)),
                            );
                        }

                        // Clear any remaining rows from previous viewport/footer
                        for row in (total_printed_rows as u16)..current_viewport_height {
                            let _ = crossterm::execute!(
                                stdout,
                                crossterm::cursor::MoveTo(0, oy + row),
                                crossterm::terminal::Clear(
                                    crossterm::terminal::ClearType::CurrentLine
                                ),
                            );
                        }
                        let _ = crossterm::execute!(
                            stdout,
                            crossterm::cursor::MoveTo(
                                0,
                                (oy + total_printed_rows as u16).min(term_h.saturating_sub(1))
                            ),
                        );
                    } else {
                        let _ = crossterm::execute!(
                            stdout,
                            crossterm::cursor::MoveTo(0, oy),
                            crossterm::terminal::Clear(crossterm::terminal::ClearType::CurrentLine),
                            crossterm::style::Print(format!("{}{}\r\n", final_ansi, trimmed)),
                        );
                        for row in 1..current_viewport_height {
                            let _ = crossterm::execute!(
                                stdout,
                                crossterm::cursor::MoveTo(0, oy + row),
                                crossterm::terminal::Clear(
                                    crossterm::terminal::ClearType::CurrentLine
                                ),
                            );
                        }
                        let _ = crossterm::execute!(
                            stdout,
                            crossterm::cursor::MoveTo(0, (oy + 1).min(term_h.saturating_sub(1))),
                        );
                    }
                    let _ = std::io::Write::flush(&mut stdout);
                } else if trimmed.contains('\n') {
                    let lines: Vec<&str> = trimmed.split('\n').collect();
                    println!("\r\x1b[2K{}", final_ansi);
                    for line in lines {
                        println!("\r\x1b[2K{}", line);
                    }
                } else {
                    println!("\r\x1b[2K{}{}", final_ansi, trimmed);
                }

                status_bar.start_command_timer();
                let t_prompt = ftui_start.elapsed();

                let is_fullscreen = margins::is_fullscreen_app(&trimmed);
                let is_suspended_cmd = _suspend.is_some();

                let _margin_guard =
                    if !is_suspended_cmd && !is_fullscreen && status_bar.visible && term_h > 4 {
                        let guard = margins::MarginGuard::new(term_h);
                        margins::render_persistent_status_bar(
                            &mut status_terminal,
                            &status_bar,
                            &theme,
                            term_w,
                            term_h,
                        );
                        Some(guard)
                    } else {
                        None
                    };

                let is_ticker_running = Arc::new(AtomicBool::new(true));
                let is_ticker_running_clone = is_ticker_running.clone();

                let ticker_handle = if _margin_guard.is_some() && !is_suspended_cmd {
                    let status_bar_snap = status_bar.clone();
                    let theme_snap = theme.clone();
                    Some(tokio::spawn(async move {
                        let mut interval = tokio::time::interval(Duration::from_millis(200));
                        let start = std::time::Instant::now();
                        while is_ticker_running_clone.load(Ordering::Relaxed) {
                            interval.tick().await;
                            if !is_ticker_running_clone.load(Ordering::Relaxed) {
                                break;
                            }
                            margins::render_status_bar_live_tick(
                                &status_bar_snap,
                                &theme_snap,
                                start.elapsed(),
                                term_w,
                                term_h,
                            );
                        }
                    }))
                } else {
                    None
                };

                use futures::FutureExt;
                env.is_command_running.store(true, Ordering::SeqCst);
                let handle_fut = std::panic::AssertUnwindSafe(crate::handle_line_generic(
                    &env,
                    &trimmed,
                    &current_dir,
                    &session_id,
                ));
                let handle_result = match handle_fut.catch_unwind().await {
                    Ok(r) => r,
                    Err(_) => {
                        eprintln!("\n\x1b[1;33mnotice:\x1b[0m FTUI recovered from panic.");
                        Ok(())
                    }
                };
                env.is_command_running.store(false, Ordering::SeqCst);
                is_ticker_running.store(false, Ordering::Relaxed);
                if let Some(h) = ticker_handle {
                    let _ = h.await;
                }
                let t_exec = ftui_start.elapsed();
                let was_suspended = _suspend.is_some();
                let is_exit = handle_result.is_err();
                drop(_suspend);
                let t_reraw = if was_suspended {
                    ftui_start.elapsed()
                } else {
                    let _ = crossterm::terminal::enable_raw_mode();
                    ftui_start.elapsed()
                };

                if is_exit {
                    if _raw_session.is_some() {
                        drop(_raw_session.take());
                    }
                    break 'repl_loop;
                }
                status_bar.end_command_timer();
                let snap = crate::refresh_prompt_snapshot(&env, &current_dir);
                status_bar.set_exit_code(snap.exit_code);
                status_bar.set_job_count(snap.job_count);
                if let Some(ref gs) = snap.git_status {
                    status_bar.update_git(Some(gs.branch.clone()), !gs.clean, gs.ahead, gs.behind);
                }
                if ftui_debug {
                    eprintln!(
                        "[cnf_debug] {}:{}: FTUI exec: disable={:?} refresh={:?} prompt={:?} exec={:?} reraw={:?}",
                        file!(),
                        line!(),
                        t_disable,
                        t_refresh,
                        t_prompt,
                        t_exec,
                        t_reraw
                    );
                }
            }

            // Ensure the command output did not end in the middle of a line, which would corrupt the layout.
            // In session-wide raw (`FSH_RAW_SESSION=1`) the raw state is still on here and
            // `safe_cursor_position` would block asking the terminal; skip.
            if _raw_session.is_none() {
                let _ = crossterm::terminal::enable_raw_mode();
            }
            if let Some((cursor_x, _)) = safe_cursor_position() {
                if cursor_x > 0 {
                    let _ = crossterm::execute!(
                        std::io::stdout(),
                        crossterm::style::Print("\r\n"),
                        crossterm::cursor::MoveToColumn(0),
                    );
                }
            }

            // Check for DYM deferred 'e' edit suggestion (FTUI path with line_editor=None)
            if let Some(suggestion) = env.prompt.edit_suggestion.write().take() {
                text_buf.replace_content(&suggestion);
                text_buf.move_to_end();
            } else {
                text_buf.clear();
            }
            text_buf.clear_history();
            comp_mgr.clear();
            history_mgr.reset();
            history_index = None;
            text_scroll_offset = 0;
        } else if !exit_repl {
            history_mgr.reset();
            history_index = None;
            text_scroll_offset = 0;
        }
    }
}

fn safe_cursor_position() -> Option<(u16, u16)> {
    crossterm::cursor::position().ok()
}

fn slice_spans_by_column(
    spans: &[Span<'static>],
    start_col: usize,
    len_cols: usize,
) -> Vec<Span<'static>> {
    if len_cols == 0 {
        return Vec::new();
    }
    let mut result = Vec::new();
    let end_col = start_col + len_cols;
    let mut current_col = 0;

    for span in spans {
        let span_width = UnicodeWidthStr::width(span.content.as_ref());
        let span_end_col = current_col + span_width;

        if span_end_col <= start_col || current_col >= end_col {
            current_col = span_end_col;
            continue;
        }

        let local_start = start_col.saturating_sub(current_col);
        let local_end = end_col.saturating_sub(current_col).min(span_width);

        if local_start < local_end {
            let sub = extract_columns_by_width(&span.content, local_start, local_end);
            if !sub.is_empty() {
                result.push(Span::styled(sub, span.style));
            }
        }

        current_col = span_end_col;
    }
    result
}

fn extract_columns_by_width(s: &str, start_col: usize, end_col: usize) -> String {
    let mut out = String::new();
    let mut col = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if w == 0 {
            continue;
        }
        let next_col = col + w;
        if next_col <= start_col {
            col = next_col;
            continue;
        }
        if col >= end_col {
            break;
        }
        if col < start_col && next_col > start_col {
            // Wide char straddles the left edge — replace with single-width
            // space so the column alignment stays correct (R10). Truncating
            // the char to half would corrupt UTF-8 width.
            if start_col < end_col {
                out.push(' ');
            }
            col = next_col;
            if col >= end_col {
                break;
            }
            continue;
        }
        if next_col > end_col && w > 1 {
            // Wide char would overflow right edge — omit rather than clip.
            break;
        }
        out.push(c);
        col = next_col;
        if col >= end_col {
            break;
        }
    }
    out
}

fn apply_style_override(
    spans: Vec<Span<'static>>,
    start_char_idx: usize,
    end_char_idx: usize,
    override_style: Style,
) -> Vec<Span<'static>> {
    let mut result = Vec::new();
    let mut current_char_idx = 0;

    for span in spans {
        let content = span.content.as_ref();
        let chars: Vec<char> = content.chars().collect();
        let span_len = chars.len();

        if current_char_idx + span_len <= start_char_idx || current_char_idx >= end_char_idx {
            // Span is entirely outside the override range
            result.push(span);
        } else {
            // Span intersects with the override range
            // 1. Before override
            let before_len = start_char_idx.saturating_sub(current_char_idx);
            if before_len > 0 {
                let part1: String = chars[0..before_len].iter().collect();
                result.push(Span::styled(part1, span.style));
            }

            // 2. Overlapping override
            let overlap_start = start_char_idx.saturating_sub(current_char_idx);
            let overlap_end = (end_char_idx.saturating_sub(current_char_idx)).min(span_len);
            if overlap_end > overlap_start {
                let part2: String = chars[overlap_start..overlap_end].iter().collect();
                let combined_style = span.style.patch(override_style);
                result.push(Span::styled(part2, combined_style));
            }

            // 3. After override
            let after_start = (end_char_idx.saturating_sub(current_char_idx)).min(span_len);
            if after_start < span_len {
                let part3: String = chars[after_start..].iter().collect();
                result.push(Span::styled(part3, span.style));
            }
        }
        current_char_idx += span_len;
    }
    result
}

fn get_command_for_cursor(text: &str, cursor_char_idx: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor_char_idx.min(chars.len());

    // Find the start of the current pipeline stage or statement by scanning backward
    let mut boundary = cursor;
    while boundary > 0 {
        let prev = chars[boundary - 1];
        if prev == '|' || prev == ';' || prev == '&' || prev == '(' || prev == '{' {
            break;
        }
        boundary -= 1;
    }

    // Find the first word after the boundary
    let mut pos = boundary;
    // Skip leading whitespace
    while pos < chars.len() && chars[pos].is_whitespace() {
        pos += 1;
    }

    // Read the word
    let mut word = String::new();
    while pos < chars.len() {
        let c = chars[pos];
        if c.is_whitespace()
            || c == '|'
            || c == ';'
            || c == '&'
            || c == '('
            || c == ')'
            || c == '{'
            || c == '}'
        {
            break;
        }
        word.push(c);
        pos += 1;
    }

    word
}

// Completion helpers

/// Accept a completion: replace the byte span given by `span` with `value`.
/// Uses the Suggestion span (byte offsets into the current line) rather than
/// scanning backward for whitespace — this is the correct behavior (Bug 1.1).
fn accept_completion_at_span(
    text_buf: &mut buffer::TextBuffer,
    line: &str,
    span: reedline::Span,
    value: &str,
    append_whitespace: bool,
) {
    let line_chars: Vec<char> = line.chars().collect();
    let total_chars = line_chars.len();

    // Convert byte span to char indices
    let start_char = byte_offset_to_char_index(line, span.start).min(total_chars);
    let end_char = byte_offset_to_char_index(line, span.end).min(total_chars);

    // Calculate how far ahead the cursor is from the span start
    let cursor = text_buf.cursor();
    if cursor > start_char && cursor <= total_chars {
        let len_to_delete = cursor - start_char;
        // Move cursor back to start of span
        text_buf.set_cursor(start_char);
        // Delete from start_char to old cursor
        for _ in 0..len_to_delete {
            text_buf.delete_right();
        }
    } else if cursor == start_char && end_char > start_char {
        // Delete the span forward
        let len_to_delete = end_char - start_char;
        for _ in 0..len_to_delete {
            text_buf.delete_right();
        }
    }

    // Now cursor is at start_char, insert the value
    text_buf.insert_str(value);
    append_completion_tail(text_buf, value, append_whitespace);
}

/// Convert a byte offset to a character index.
fn byte_offset_to_char_index(s: &str, byte_offset: usize) -> usize {
    let byte_offset = byte_offset.min(s.len());
    s[..byte_offset].chars().count()
}

/// Dispatch function: use span-aware completion when the suggestion has a valid span,
/// otherwise fall back to the legacy word-boundary approach.
pub(crate) fn apply_completion(
    text_buf: &mut buffer::TextBuffer,
    line: &str,
    suggestion: &reedline::Suggestion,
) {
    let span = suggestion.span;
    // A meaningful span has start < end and covers a range that matches the buffer
    if span.start < span.end && span.end <= line.len() {
        accept_completion_at_span(
            text_buf,
            line,
            span,
            &suggestion.value,
            suggestion.append_whitespace,
        );
    } else {
        // Fallback: delete back to word boundary (legacy behavior for suggestions
        // from completers that don't return proper spans)
        let chars: Vec<char> = line.chars().collect();
        let cursor = text_buf.cursor();
        let last_word_start = chars[..cursor]
            .iter()
            .rposition(|&c| c.is_whitespace() || c == '|' || c == '>' || c == '<')
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let len_to_delete = cursor - last_word_start;
        for _ in 0..len_to_delete {
            text_buf.delete_left();
        }
        accept_completion_legacy(text_buf, &suggestion.value, suggestion.append_whitespace);
    }
}

/// Legacy accept method — deletes back to the last word boundary.
/// Used only when a Suggestion has no meaningful span (e.g., default span of (0,0)).
fn accept_completion_legacy(
    text_buf: &mut buffer::TextBuffer,
    value: &str,
    append_whitespace: bool,
) {
    text_buf.insert_str(value);
    append_completion_tail(text_buf, value, append_whitespace);
}

pub(crate) fn append_slash_if_dir(text_buf: &mut buffer::TextBuffer, value: &str) {
    if text_buf.cursor() == text_buf.len() && !value.ends_with('/') && is_dir_expanded(value) {
        text_buf.insert_char('/');
    }
}

/// Complete the accepted suggestion at the cursor: directories get a trailing
/// '/', command/flag completions that request it get a trailing space, so
/// `git<Tab>` leaves `git ` and the user can keep typing.
fn append_completion_tail(text_buf: &mut buffer::TextBuffer, value: &str, append_whitespace: bool) {
    if text_buf.cursor() != text_buf.len() {
        return;
    }
    if !value.ends_with('/') && is_dir_expanded(value) {
        text_buf.insert_char('/');
    } else if append_whitespace && !value.ends_with(' ') {
        text_buf.insert_char(' ');
    }
}

/// Map a flat display index (including group header rows) to a raw suggestion
/// index. Returns `None` if `flat_idx` corresponds to a header row.
fn flat_to_raw_index(mgr: &completions::CompletionsManager, flat_idx: usize) -> Option<usize> {
    let grouped = mgr.grouped.as_ref()?;
    let sizes = grouped.group_sizes();
    let mut flat = 0usize;
    let mut seen = 0usize;
    for &count in &sizes {
        if flat == flat_idx {
            return None; // header
        }
        flat += 1; // header
        if flat_idx >= flat && flat_idx < flat + count {
            return Some(seen + (flat_idx - flat));
        }
        flat += count;
        seen += count;
    }
    None
}

/// When a header row is clicked, select the first item of that group.
fn flat_header_next_item(mgr: &completions::CompletionsManager, flat_idx: usize) -> Option<usize> {
    let grouped = mgr.grouped.as_ref()?;
    let sizes = grouped.group_sizes();
    let mut flat = 0usize;
    let mut seen = 0usize;
    for &count in &sizes {
        if flat == flat_idx && count > 0 {
            return Some(seen);
        }
        flat += 1 + count;
        seen += count;
    }
    None
}

fn is_dir_expanded(path: &str) -> bool {
    let expanded = if let Some(rest) = path.strip_prefix('~') {
        if let Some(home) = std::env::var("HOME")
            .ok()
            .or_else(|| dirs::home_dir().map(|p| p.to_string_lossy().to_string()))
        {
            format!("{home}{rest}")
        } else {
            path.to_string()
        }
    } else {
        path.to_string()
    };
    std::path::Path::new(&expanded).is_dir()
}

// Feature 8: multi-line gutter helpers

/// Split highlighted spans into per-line Vec<Vec<Span>> at '\n' boundaries.
/// Each Span's content may contain embedded '\n' characters. The `start_col`
/// and `len_cols` are passed through to `slice_spans_by_column` for horizontal
/// scroll clipping. Correctly handles leading and consecutive newlines (R3).
fn split_spans_by_newline(
    spans: &[Span<'static>],
    active_line: usize,
    start_col: usize,
    len_cols: usize,
) -> Vec<Vec<Span<'static>>> {
    // Flatten all span contents into a single char stream with per-char style,
    // then split at '\n'. This avoids the multi-span byte_offset bug where
    // consecutive spans each starting with '\n' were miscounted.
    let mut styled_chars: Vec<(char, Style)> = Vec::new();
    for span in spans {
        for ch in span.content.chars() {
            styled_chars.push((ch, span.style));
        }
    }
    if styled_chars.is_empty() {
        return vec![slice_spans_by_column(&[], start_col, len_cols)];
    }
    let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut cur_text = String::new();
    let mut cur_style: Option<Style> = None;
    let flush =
        |cur_text: &mut String, cur_style: &mut Option<Style>, current: &mut Vec<Span<'static>>| {
            if !cur_text.is_empty()
                && let Some(st) = cur_style.take()
            {
                current.push(Span::styled(std::mem::take(cur_text), st));
            }
        };
    for (ch, style) in styled_chars {
        if ch == '\n' {
            flush(&mut cur_text, &mut cur_style, &mut current);
            let line_idx = lines.len();
            let col = if line_idx == active_line {
                start_col
            } else {
                0
            };
            lines.push(slice_spans_by_column(&current, col, len_cols));
            current = Vec::new();
        } else {
            if cur_style != Some(style) {
                flush(&mut cur_text, &mut cur_style, &mut current);
                cur_style = Some(style);
            }
            cur_text.push(ch);
        }
    }
    flush(&mut cur_text, &mut cur_style, &mut current);
    let line_idx = lines.len();
    let col = if line_idx == active_line {
        start_col
    } else {
        0
    };
    lines.push(slice_spans_by_column(&current, col, len_cols));
    lines
}

#[derive(Clone, PartialEq)]
struct StatusBarState {
    last_exit_code: Option<i64>,
    git_branch: Option<String>,
    git_dirty: bool,
    git_ahead: usize,
    git_behind: usize,
    job_count: usize,
    mode_indicator: String,
    last_command_elapsed: Option<std::time::Duration>,
    visible: bool,
    term_w: u16,
    term_h: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_spans_by_newline_basic() {
        let spans = vec![Span::raw("fn foo() {\n    echo 1\n}")];
        let lines = split_spans_by_newline(&spans, 0, 0, 80);
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[0]
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            "fn foo() {"
        );
        assert_eq!(
            lines[1]
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            "    echo 1"
        );
        assert_eq!(
            lines[2]
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            "}"
        );
    }

    #[test]
    fn test_split_spans_by_newline_active_line_scrolling() {
        let spans = vec![Span::raw(
            "first line is long and scrolling\nsecond line\nthird line",
        )];
        // Line 0 is active and scrolled by 6 columns
        let lines = split_spans_by_newline(&spans, 0, 6, 80);
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[0]
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            "line is long and scrolling"
        );
        // Non-active lines are NOT scrolled
        assert_eq!(
            lines[1]
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            "second line"
        );
        assert_eq!(
            lines[2]
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            "third line"
        );
    }

    #[test]
    fn test_split_spans_by_newline_consecutive_and_trailing_newlines() {
        let spans = vec![Span::raw("a\n\nb\n")];
        let lines = split_spans_by_newline(&spans, 0, 0, 80);
        assert_eq!(lines.len(), 4);
        assert_eq!(
            lines[0]
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            "a"
        );
        assert_eq!(
            lines[1]
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            ""
        );
        assert_eq!(
            lines[2]
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            "b"
        );
        assert_eq!(
            lines[3]
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            ""
        );
    }
}
