// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Session-owned raw mode.
//!
//! Long-term A2: raw mode is a session property, not a per-command toggle.
//! The terminal spends the whole interactive session in raw + mouse +
//! bracketed-paste. Only a [`SuspendGuard`] — held for the duration of a
//! command that actually needs a PTY — temporarily drops back to cooked.
//!
//! Everything else (ls, cat, builtins, piped output) runs while raw
//! stays on, with output flowing through `CaptureGuard` into the anchored
//! pane. That eliminates the `\n` → missing `\r\n` smear and the 5
//! scattered `enable/disable_raw_mode` callsites.

use std::io::Write;

#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(unix)]
static DID_SUSPEND: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
static GOT_SIGHUP: AtomicBool = AtomicBool::new(false);

/// Owns the process signal handlers used by an interactive terminal session.
///
/// Installing handlers is process-global, so it must have an equally explicit
/// lifetime. The guard restores every previous disposition when the session
/// ends; this is essential for embedding fshell and for tests that run more
/// than one shell in a process.
#[cfg(unix)]
pub struct SignalGuard {
    previous: Vec<(libc::c_int, libc::sigaction)>,
}

#[cfg(not(unix))]
pub struct SignalGuard;

impl SignalGuard {
    #[cfg(unix)]
    pub fn install() -> std::io::Result<Self> {
        let mut guard = Self {
            previous: Vec::with_capacity(3),
        };
        for signal in [libc::SIGTSTP, libc::SIGCONT, libc::SIGHUP] {
            if let Err(error) = guard.install_one(signal) {
                drop(guard);
                return Err(error);
            }
        }
        Ok(guard)
    }

    #[cfg(not(unix))]
    pub fn install() -> std::io::Result<Self> {
        Ok(Self)
    }

    #[cfg(unix)]
    fn install_one(&mut self, signal: libc::c_int) -> std::io::Result<()> {
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = match signal {
                libc::SIGTSTP => sigtstp_action as *const () as libc::sighandler_t,
                libc::SIGCONT => sigcont_action as *const () as libc::sighandler_t,
                libc::SIGHUP => sighup_action as *const () as libc::sighandler_t,
                _ => unreachable!("signal set is fixed above"),
            };
            action.sa_flags = 0;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaddset(&mut action.sa_mask, signal);
            if signal == libc::SIGTSTP {
                libc::sigaddset(&mut action.sa_mask, libc::SIGCONT);
            }

            let mut previous: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(signal, &action, &mut previous) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            self.previous.push((signal, previous));
        }
        Ok(())
    }

    pub fn suspended() -> bool {
        #[cfg(unix)]
        {
            DID_SUSPEND.swap(false, Ordering::Relaxed)
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    pub fn hup_received() -> bool {
        #[cfg(unix)]
        {
            GOT_SIGHUP.load(Ordering::Relaxed)
        }
        #[cfg(not(unix))]
        {
            false
        }
    }
}

#[cfg(unix)]
impl Drop for SignalGuard {
    fn drop(&mut self) {
        unsafe {
            for &(signal, ref previous) in self.previous.iter().rev() {
                let _ = libc::sigaction(signal, previous, std::ptr::null_mut());
            }
        }
    }
}

/// Restores the terminal while a panic hook is running.
type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>;
type PanicHookState = std::sync::Arc<std::sync::Mutex<Option<PanicHook>>>;

pub struct PanicHookGuard {
    previous: PanicHookState,
}

impl PanicHookGuard {
    pub fn install() -> Self {
        let previous = std::sync::Arc::new(std::sync::Mutex::new(Some(std::panic::take_hook())));
        let hook_previous = previous.clone();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            if let Ok(previous) = hook_previous.lock() {
                if let Some(previous) = previous.as_ref() {
                    previous(info);
                }
            } else {
                eprintln!("panic hook state is poisoned: {info}");
            }
        }));
        Self { previous }
    }
}

impl Drop for PanicHookGuard {
    fn drop(&mut self) {
        let previous = self
            .previous
            .lock()
            .ok()
            .and_then(|mut previous| previous.take());
        if let Some(previous) = previous {
            std::panic::set_hook(previous);
        }
    }
}

/// Best-effort terminal cleanup used by both normal Drop and panic handling.
pub fn restore_terminal() {
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x1b[=0u");
    let _ = crossterm::execute!(
        out,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::FromCursorDown),
        crossterm::event::DisableBracketedPaste,
        crossterm::event::DisableFocusChange,
        crossterm::event::DisableMouseCapture,
        crossterm::cursor::Show,
        crossterm::cursor::EnableBlinking,
    );
    let _ = out.flush();
    let _ = crossterm::terminal::disable_raw_mode();
}

/// Enter the terminal state expected by a line-oriented prompt or a child
/// process. This is the only cooked-mode transition used by FTUI.
pub(crate) fn enter_cooked_mode() -> std::io::Result<()> {
    let mut out = std::io::stdout();
    out.flush()?;
    out.write_all(b"\x1b[=0u")?;
    crossterm::execute!(
        out,
        crossterm::event::DisableBracketedPaste,
        crossterm::event::DisableFocusChange,
        crossterm::event::DisableMouseCapture,
        crossterm::cursor::Show,
        crossterm::cursor::EnableBlinking,
    )?;
    out.flush()?;
    crossterm::terminal::disable_raw_mode()
}

/// Enter the complete raw terminal state owned by an interactive FTUI
/// session. Keeping this transition in one place prevents prompt, child, and
/// signal paths from drifting apart.
pub(crate) fn enter_raw_mode() -> std::io::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    let mut out = std::io::stdout();
    let result = (|| {
        out.write_all(b"\x1b[=0u")?;
        crossterm::execute!(
            out,
            crossterm::cursor::DisableBlinking,
            crossterm::event::EnableBracketedPaste,
            crossterm::event::EnableFocusChange,
            crossterm::event::EnableMouseCapture,
        )?;
        out.flush()
    })();
    if result.is_err() {
        restore_terminal();
    }
    result
}

#[cfg(unix)]
extern "C" fn sighup_action(_signal: libc::c_int) {
    GOT_SIGHUP.store(true, Ordering::Relaxed);
}

#[cfg(unix)]
extern "C" fn sigcont_action(_signal: libc::c_int) {
    DID_SUSPEND.store(true, Ordering::Relaxed);
}

#[cfg(unix)]
extern "C" fn sigtstp_action(_signal: libc::c_int) {
    unsafe {
        let reset = b"\x1b[?25h\x1b[?1000l\x1b[?2004l";
        let _ = libc::write(libc::STDOUT_FILENO, reset.as_ptr().cast(), reset.len());
        DID_SUSPEND.store(true, Ordering::Relaxed);

        // Let the kernel perform the actual stop with the default action.
        // Restore our handler after raise returns so the next Ctrl-Z behaves
        // identically. All operations here are async-signal-safe.
        let mut default_action: libc::sigaction = std::mem::zeroed();
        default_action.sa_sigaction = libc::SIG_DFL;
        libc::sigemptyset(&mut default_action.sa_mask);
        let mut previous: libc::sigaction = std::mem::zeroed();
        libc::sigaction(libc::SIGTSTP, &default_action, &mut previous);
        libc::raise(libc::SIGTSTP);
        libc::sigaction(libc::SIGTSTP, &previous, std::ptr::null_mut());
    }
}

/// Owner of raw mode for the life of `run_ftui_repl`.
///
/// Exactly one exists per REPL session. Dropping it restores the terminal
/// even if the task panics. Nothing else in `ftui` should call
/// `enable_raw_mode`/`disable_raw_mode` directly.
pub struct Session {
    // Private so only this module can construct / drop.
    _private: (),
}

/// Borrowed guard that temporarily drops the session back to cooked.
///
/// Created by [`Session::suspend`]. While this guard is alive the child
/// process sees a normal cooked terminal (echo, icannon, onlcr). When the
/// guard drops — including on panic/unwind — raw mode and all auxiliary
/// modes are reinstalled in the correct order and flushed.
pub struct SuspendGuard<'a> {
    session: &'a Session,
    armed: bool,
}

impl Session {
    /// Enter raw + auxiliary modes once for the session.
    ///
    /// Returns an error instead of panicking so `run_ftui_repl` can exit
    /// gracefully if the terminal cannot enter raw mode (e.g. not a tty).
    pub fn enter() -> std::io::Result<Self> {
        if fshell_engine::is_test_mode() {
            return Err(std::io::Error::other("refusing raw mode in test mode"));
        }
        enter_raw_mode()?;
        Ok(Self { _private: () })
    }

    /// Suspend raw for the duration of a command that needs a real PTY
    /// (vim, less, ssh, fzf, …). The returned guard re-enables on drop.
    pub fn suspend(&self) -> std::io::Result<SuspendGuard<'_>> {
        enter_cooked_mode()?;
        Ok(SuspendGuard {
            session: self,
            armed: true,
        })
    }

    /// Explicit re-arm without a suspend — used after SIGTSTP resume where
    /// the kernel may have reset termios behind us. Idempotent.
    pub(crate) fn reenter_raw(&self) {
        let _ = enter_raw_mode();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        restore_terminal();
    }
}

impl<'a> Drop for SuspendGuard<'a> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Re-enter exactly the state `Session::enter` established.
        self.session.reenter_raw();
    }
}

impl<'a> SuspendGuard<'a> {
    /// Disarm — don't re-enter raw on drop. Used when the repl is exiting
    /// and `Session` itself will do the final restore.
    #[allow(dead_code)]
    pub fn disarm(mut self) {
        self.armed = false;
    }
}
