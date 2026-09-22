// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Terminal color escape sequences
//!
//! ANSI color codes for terminal output formatting.

use parking_lot::Mutex;
use std::io::{self, Write};
use std::sync::LazyLock;

static DIR_COLOR_STR: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new("\x1b[34m".to_owned()));
static LINK_COLOR_STR: LazyLock<Mutex<String>> =
    LazyLock::new(|| Mutex::new("\x1b[36m".to_owned()));
static EXEC_COLOR_STR: LazyLock<Mutex<String>> =
    LazyLock::new(|| Mutex::new("\x1b[32m".to_owned()));

pub struct ColorCode {
    cell: &'static LazyLock<Mutex<String>>,
}

impl ColorCode {
    pub fn write_to<W: Write + ?Sized>(&self, output: &mut W) -> io::Result<()> {
        let color = self.cell.lock();
        output.write_all(color.as_bytes())
    }
}

impl std::fmt::Display for ColorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.cell.lock())
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
    *DIR_COLOR_STR.lock() = dir.to_owned();
    *LINK_COLOR_STR.lock() = link.to_owned();
    *EXEC_COLOR_STR.lock() = exec.to_owned();
}
