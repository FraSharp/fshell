// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Process-level lifecycle management for interactive terminal sessions.
//!
//! Provides explicit RAII guards for:
//! - Unix signal handling (`SIGTSTP`, `SIGCONT`, `SIGHUP`)
//! - Panic hook installation with emergency terminal restoration
//! - Emergency terminal cleanup helpers

use std::io::Write;

#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(unix)]
static DID_SUSPEND: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
static GOT_SIGHUP: AtomicBool = AtomicBool::new(false);

/// Owns the process signal handlers used by an interactive terminal session.
///
/// Installing handlers is process-global, so it must have an explicit lifetime.
/// The guard restores previous dispositions when dropped.
#[cfg(unix)]
pub struct SignalGuard {
    previous: Vec<(libc::c_int, libc::sigaction)>,
}

#[cfg(not(unix))]
pub struct SignalGuard;

impl SignalGuard {
    #[cfg(unix)]
    pub fn install() -> std::io::Result<Self> {
        DID_SUSPEND.store(false, Ordering::Relaxed);
        GOT_SIGHUP.store(false, Ordering::Relaxed);
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
        DID_SUSPEND.store(false, Ordering::Relaxed);
        GOT_SIGHUP.store(false, Ordering::Relaxed);
        unsafe {
            for &(signal, ref previous) in self.previous.iter().rev() {
                let _ = libc::sigaction(signal, previous, std::ptr::null_mut());
            }
        }
    }
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
        // Safe reset bytes for emergency suspend: show cursor, disable mouse, disable bracketed paste
        let reset = b"\x1b[?25h\x1b[?1000l\x1b[?2004l";
        let _ = libc::write(libc::STDOUT_FILENO, reset.as_ptr().cast(), reset.len());
        DID_SUSPEND.store(true, Ordering::Relaxed);

        // Kernel stops process with default action; restore handler after raise returns
        let mut default_action: libc::sigaction = std::mem::zeroed();
        default_action.sa_sigaction = libc::SIG_DFL;
        libc::sigemptyset(&mut default_action.sa_mask);
        let mut previous: libc::sigaction = std::mem::zeroed();
        libc::sigaction(libc::SIGTSTP, &default_action, &mut previous);
        libc::raise(libc::SIGTSTP);
        libc::sigaction(libc::SIGTSTP, &previous, std::ptr::null_mut());
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
            emergency_restore_terminal();
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

/// Best-effort terminal cleanup used by panic hooks and emergency exits.
pub fn emergency_restore_terminal() {
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
