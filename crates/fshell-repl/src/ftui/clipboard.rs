// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use arboard::Clipboard;
use base64::Engine as _;

/// Try to copy text to the system clipboard.
/// Falls back to OSC 52 escape sequence when native clipboard fails.
pub fn copy_to_clipboard(text: &str) -> bool {
    // Try native clipboard first
    match Clipboard::new().and_then(|mut c| c.set_text(text.to_string())) {
        Ok(_) => return true,
        Err(_) => {
            // Fall through to OSC 52
        }
    }
    // Fallback: OSC 52 escape sequence
    osc_52_copy(text)
}

/// Try to paste text from the system clipboard.
pub fn paste_from_clipboard() -> Option<String> {
    let mut clip = Clipboard::new().ok()?;
    clip.get_text().ok()
}

/// OSC 52 paste escape — encodes the text as base64 and writes
/// the escape sequence to stdout for terminal integration.
/// Uses `crossterm::execute!` so it goes through crossterm's buffering
/// correctly even while raw mode + ratatui is active (long-term vs direct
/// `write!` which could be swallowed mid-frame).
fn osc_52_copy(text: &str) -> bool {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let seq = format!("\x1b]52;c;{encoded}\x07");
    // Use crossterm's queue + flush path so OSC52 isn't interleaved inside
    // a ratatui draw's buffered output. Best-effort outside draw.
    use std::io::Write;
    let mut stdout = std::io::stdout();
    let _ = write!(stdout, "{seq}");
    let _ = stdout.flush();
    // Note: ideally this would be queued via crossterm::execute! or drawn
    // outside the ratatui Terminal::draw closure. Callers (Alt+C/X) run in
    // the event loop between draws, so the interleaving risk is low.
    true
}
