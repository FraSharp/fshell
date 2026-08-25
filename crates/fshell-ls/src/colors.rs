// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Terminal color escape sequences
//!
//! ANSI color codes for terminal output formatting.

use std::ops::Deref;
use std::sync::Mutex;

static DIR_COLOR_STR: Mutex<&'static str> = Mutex::new("\x1b[34m");
static LINK_COLOR_STR: Mutex<&'static str> = Mutex::new("\x1b[36m");
static EXEC_COLOR_STR: Mutex<&'static str> = Mutex::new("\x1b[32m");

pub struct ColorCode {
    cell: &'static Mutex<&'static str>,
}

impl Deref for ColorCode {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        if let Ok(guard) = self.cell.lock() {
            *guard
        } else {
            ""
        }
    }
}

impl ColorCode {
    pub fn as_bytes(&self) -> &[u8] {
        self.deref().as_bytes()
    }
}

impl AsRef<str> for ColorCode {
    fn as_ref(&self) -> &str {
        self.deref()
    }
}

impl std::fmt::Display for ColorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.deref())
    }
}

pub static BLUE: ColorCode = ColorCode {
    cell: &DIR_COLOR_STR,
};
pub static CYAN: ColorCode = ColorCode {
    cell: &LINK_COLOR_STR,
};
pub static GREEN: ColorCode = ColorCode {
    cell: &EXEC_COLOR_STR,
};
pub const RESET: &str = "\x1b[0m";
pub const REVERSE: &str = "\x1b[7m";
pub const BOLD: &str = "\x1b[1m";

/// Update color configurations globally for directory listing.
pub fn set_colors(dir: &str, link: &str, exec: &str) {
    if let Ok(mut d) = DIR_COLOR_STR.lock() {
        *d = Box::leak(dir.to_string().into_boxed_str());
    }
    if let Ok(mut l) = LINK_COLOR_STR.lock() {
        *l = Box::leak(link.to_string().into_boxed_str());
    }
    if let Ok(mut e) = EXEC_COLOR_STR.lock() {
        *e = Box::leak(exec.to_string().into_boxed_str());
    }
}
