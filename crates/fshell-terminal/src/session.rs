// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Scoped terminal session management and viewport configuration.
//!
//! Provides RAII ownership of raw mode, alternate screen buffers, mouse capture,
//! and bracketed paste. Follows strict explicit-ownership semantics: on drop,
//! only the terminal modes that *this specific session* modified are restored.
//! Nested sessions will not disable outer session modes (e.g. REPL raw mode).

use std::io::{self, Write};

use ratatui::Terminal;

use crate::ansi;
use crate::backend::FshellBackend;
use crate::lifecycle::{PanicHookGuard, SignalGuard};
use crate::raw;

/// Physical terminal device to interact with.
#[derive(Debug)]
pub enum TerminalDevice {
    Stdio(io::Stdout),
    ControllingTty(std::fs::File),
}

impl Write for TerminalDevice {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Stdio(stdout) => stdout.write(buf),
            Self::ControllingTty(file) => file.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Stdio(stdout) => stdout.flush(),
            Self::ControllingTty(file) => file.flush(),
        }
    }
}

impl TerminalDevice {
    /// Use standard stdout.
    pub fn stdio() -> Self {
        Self::Stdio(io::stdout())
    }

    /// Open controlling terminal (`/dev/tty`).
    pub fn open_controlling_tty() -> io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")?;
        Ok(Self::ControllingTty(file))
    }

    /// Auto-detect: if both stdin and stdout are interactive TTYs, use Stdio;
    /// otherwise, open controlling terminal `/dev/tty`.
    pub fn auto() -> io::Result<Self> {
        #[cfg(unix)]
        {
            if unsafe {
                libc::isatty(libc::STDIN_FILENO) == 1 && libc::isatty(libc::STDOUT_FILENO) == 1
            } {
                Ok(Self::stdio())
            } else {
                Self::open_controlling_tty()
            }
        }
        #[cfg(not(unix))]
        {
            Ok(Self::stdio())
        }
    }
}

/// Viewport mode for a TUI session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalMode {
    /// Fullscreen alternate-screen mode.
    Fullscreen,
    /// Fixed-height inline mode within standard terminal scrollback.
    Inline { height: u16 },
}

/// Configuration options for entering a terminal session.
#[derive(Debug, Clone)]
pub struct TerminalSessionOptions {
    pub mode: TerminalMode,
    pub enable_mouse: bool,
    pub enable_bracketed_paste: bool,
    pub hide_cursor: bool,
    pub install_signal_guard: bool,
    pub install_panic_hook: bool,
}

impl Default for TerminalSessionOptions {
    fn default() -> Self {
        Self {
            mode: TerminalMode::Fullscreen,
            enable_mouse: false,
            enable_bracketed_paste: false,
            hide_cursor: true,
            install_signal_guard: true,
            install_panic_hook: true,
        }
    }
}

/// An active terminal session with configured viewport and explicit mode tracking.
pub struct TerminalSession {
    terminal: Terminal<FshellBackend<TerminalDevice>>,
    mode: TerminalMode,
    did_enable_raw: bool,
    #[cfg(unix)]
    orig_tty_termios: Option<(std::os::unix::io::RawFd, libc::termios)>,
    did_enter_alt_screen: bool,
    did_enable_mouse: bool,
    did_enable_paste: bool,
    did_hide_cursor: bool,
    inline_height: Option<u16>,
    _signal_guard: Option<SignalGuard>,
    _panic_guard: Option<PanicHookGuard>,
}

impl TerminalSession {
    /// Enter a new terminal session on the given device with the requested options.
    pub fn enter(mut device: TerminalDevice, options: TerminalSessionOptions) -> io::Result<Self> {
        let mut did_enable_raw = false;
        #[cfg(unix)]
        let mut orig_tty_termios = None;

        // 1. Raw mode management
        match &device {
            TerminalDevice::Stdio(_) => {
                let raw_already = raw::is_raw_mode_enabled();
                if !raw_already {
                    raw::enable_raw_mode()?;
                    did_enable_raw = true;
                }
            }
            #[cfg(unix)]
            TerminalDevice::ControllingTty(file) => {
                use std::os::unix::io::AsRawFd;
                let fd = file.as_raw_fd();
                if unsafe { libc::isatty(fd) == 1 } {
                    let orig = raw::enable_raw_mode_fd(fd)?;
                    orig_tty_termios = Some((fd, orig));
                }
            }
            #[cfg(not(unix))]
            TerminalDevice::ControllingTty(_) => {
                let raw_already = raw::is_raw_mode_enabled();
                if !raw_already {
                    raw::enable_raw_mode()?;
                    did_enable_raw = true;
                }
            }
        }

        // 2. Viewport / alternate-screen setup
        let mut did_enter_alt_screen = false;
        let mut inline_height = None;

        match options.mode {
            TerminalMode::Fullscreen => {
                if let Err(e) = ansi::enter_alternate_screen(&mut device) {
                    Self::cleanup_raw(did_enable_raw, orig_tty_termios);
                    return Err(e);
                }
                did_enter_alt_screen = true;
            }
            TerminalMode::Inline { height } => {
                // Reserve scrollback space by printing newlines, then stepping back up
                let res = (|| -> io::Result<()> {
                    for _ in 0..height {
                        device.write_all(b"\n")?;
                    }
                    write!(device, "\x1b[{}A\r", height)?;
                    device.flush()
                })();
                if let Err(e) = res {
                    Self::cleanup_raw(did_enable_raw, orig_tty_termios);
                    return Err(e);
                }
                inline_height = Some(height);
            }
        }

        // 3. Auxiliary modes
        let mut did_enable_mouse = false;
        if options.enable_mouse {
            if let Err(e) = ansi::enable_mouse_capture(&mut device) {
                Self::cleanup_partial(
                    &mut device,
                    did_enter_alt_screen,
                    inline_height,
                    did_enable_raw,
                    orig_tty_termios,
                );
                return Err(e);
            }
            did_enable_mouse = true;
        }

        let mut did_enable_paste = false;
        if options.enable_bracketed_paste {
            if let Err(e) = ansi::enable_bracketed_paste(&mut device) {
                Self::cleanup_partial(
                    &mut device,
                    did_enter_alt_screen,
                    inline_height,
                    did_enable_raw,
                    orig_tty_termios,
                );
                return Err(e);
            }
            did_enable_paste = true;
        }

        let mut did_hide_cursor = false;
        if options.hide_cursor {
            if let Err(e) = ansi::hide_cursor(&mut device) {
                Self::cleanup_partial(
                    &mut device,
                    did_enter_alt_screen,
                    inline_height,
                    did_enable_raw,
                    orig_tty_termios,
                );
                return Err(e);
            }
            did_hide_cursor = true;
        }

        // 4. Guards
        let signal_guard = if options.install_signal_guard {
            match SignalGuard::install() {
                Ok(guard) => Some(guard),
                Err(e) => {
                    Self::cleanup_partial(
                        &mut device,
                        did_enter_alt_screen,
                        inline_height,
                        did_enable_raw,
                        orig_tty_termios,
                    );
                    return Err(e);
                }
            }
        } else {
            None
        };

        let panic_guard = if options.install_panic_hook {
            Some(PanicHookGuard::install())
        } else {
            None
        };

        // 5. Construct Ratatui Terminal
        let backend = FshellBackend::new(device);
        let terminal_options = match options.mode {
            TerminalMode::Fullscreen => ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Fullscreen,
            },
            TerminalMode::Inline { height } => ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Inline(height),
            },
        };

        let terminal = match Terminal::with_options(backend, terminal_options) {
            Ok(term) => term,
            Err(e) => {
                Self::cleanup_raw(did_enable_raw, orig_tty_termios);
                return Err(e);
            }
        };

        Ok(Self {
            terminal,
            mode: options.mode,
            did_enable_raw,
            #[cfg(unix)]
            orig_tty_termios,
            did_enter_alt_screen,
            did_enable_mouse,
            did_enable_paste,
            did_hide_cursor,
            inline_height,
            _signal_guard: signal_guard,
            _panic_guard: panic_guard,
        })
    }

    pub fn mode(&self) -> TerminalMode {
        self.mode
    }

    pub fn terminal_mut(&mut self) -> &mut Terminal<FshellBackend<TerminalDevice>> {
        &mut self.terminal
    }

    fn cleanup_raw(
        did_enable_raw: bool,
        #[cfg(unix)] orig_tty_termios: Option<(std::os::unix::io::RawFd, libc::termios)>,
        #[cfg(not(unix))] _orig_tty_termios: Option<()>,
    ) {
        if did_enable_raw {
            let _ = raw::disable_raw_mode();
        }
        #[cfg(unix)]
        if let Some((fd, orig)) = orig_tty_termios {
            let _ = raw::restore_raw_mode_fd(fd, &orig);
        }
    }

    fn cleanup_partial(
        device: &mut TerminalDevice,
        did_enter_alt_screen: bool,
        inline_height: Option<u16>,
        did_enable_raw: bool,
        #[cfg(unix)] orig_tty_termios: Option<(std::os::unix::io::RawFd, libc::termios)>,
        #[cfg(not(unix))] orig_tty_termios: Option<()>,
    ) {
        if did_enter_alt_screen {
            let _ = ansi::leave_alternate_screen(device);
        }
        if let Some(h) = inline_height {
            let _ = write!(device, "\x1b[{}B\r", h);
            let _ = device.flush();
        }
        Self::cleanup_raw(
            did_enable_raw,
            #[cfg(unix)]
            orig_tty_termios,
            #[cfg(not(unix))]
            orig_tty_termios,
        );
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let backend = self.terminal.backend_mut();

        // 1. Restore cursor visibility if this session hid it
        if self.did_hide_cursor {
            let _ = ansi::show_cursor(backend);
        }

        // 2. Disable mouse capture if this session enabled it
        if self.did_enable_mouse {
            let _ = ansi::disable_mouse_capture(backend);
        }

        // 3. Disable bracketed paste if this session enabled it
        if self.did_enable_paste {
            let _ = ansi::disable_bracketed_paste(backend);
        }

        // 4. Leave alternate screen if this session entered it
        if self.did_enter_alt_screen {
            let _ = ansi::leave_alternate_screen(backend);
        }

        // 5. Cleanly release inline viewport if this session was inline
        if let Some(height) = self.inline_height {
            let _ = write!(backend, "\x1b[{}B\r", height);
            let _ = backend.flush();
        }

        // 6. Disable raw mode only if this session was the one that enabled it
        if self.did_enable_raw {
            let _ = raw::disable_raw_mode();
        }

        // 7. Restore original termios for /dev/tty if modified
        #[cfg(unix)]
        if let Some((fd, orig)) = self.orig_tty_termios {
            let _ = raw::restore_raw_mode_fd(fd, &orig);
        }
    }
}
