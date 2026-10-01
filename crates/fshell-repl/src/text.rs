// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Small text helpers shared by the interactive front end.

/// Largest index at or below `pos` that lies on a UTF-8 character boundary.
///
/// `str::floor_char_boundary` is still unstable on the declared MSRV, so the
/// walk is spelled out here.
pub fn floor_char_boundary(line: &str, pos: usize) -> usize {
    let mut pos = pos.min(line.len());
    while !line.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}
