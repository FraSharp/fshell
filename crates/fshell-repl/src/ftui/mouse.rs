// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseMode {
    Disabled,
    Simple, // Toggle capture manually with Esc
    Smart,  // Automatically enable on keystroke/focus, disable on click above viewport
}

pub struct MouseStateManager {
    pub mode: MouseMode,
    pub is_captured: bool,
}

impl Default for MouseStateManager {
    fn default() -> Self {
        Self::new()
    }
}

impl MouseStateManager {
    pub fn new() -> Self {
        Self {
            mode: MouseMode::Smart,
            is_captured: false,
        }
    }

    pub fn set_mode(&mut self, mode: MouseMode) {
        self.mode = mode;
        match mode {
            MouseMode::Disabled => self.disable_capture(),
            _ => self.enable_capture(),
        }
    }

    pub fn enable_capture(&mut self) {
        if self.mode == MouseMode::Disabled {
            return;
        }
        if !self.is_captured {
            let mut stdout = std::io::stdout();
            let _ = execute!(stdout, EnableMouseCapture);
            let _ = stdout.flush();
            self.is_captured = true;
        }
    }

    pub fn disable_capture(&mut self) {
        if self.is_captured {
            let mut stdout = std::io::stdout();
            let _ = execute!(stdout, DisableMouseCapture);
            let _ = stdout.flush();
            self.is_captured = false;
        }
    }

    // Call when any key is pressed to automatically restore capture in smart mode
    pub fn handle_keypress(&mut self) {
        if self.mode == MouseMode::Smart && !self.is_captured {
            self.enable_capture();
        }
    }

    // Call on mouse click to check if user clicked above prompt viewport
    pub fn handle_click(&mut self, click_y: u16, prompt_y: u16) {
        if self.mode == MouseMode::Smart && click_y < prompt_y {
            // Clicked above the prompt; disable mouse capture to let terminal select text or scroll
            self.disable_capture();
        }
    }
}
