// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

/// Stateful byte framing. Complete lines borrow their input; only a line split
/// across payloads needs buffering. UTF-8 is interpreted after framing, never
/// independently for each chunk. Newline bytes remain available to the scanner.
#[derive(Default)]
pub struct LineFramer {
    pending: Vec<u8>,
}

impl LineFramer {
    /// Returns false when the consumer requests early termination.
    pub fn push(&mut self, mut bytes: &[u8], mut emit: impl FnMut(&[u8]) -> bool) -> bool {
        if !self.pending.is_empty() {
            if let Some(end) = memchr::memchr(b'\n', bytes) {
                self.pending.extend_from_slice(&bytes[..=end]);
                if !emit(&self.pending) {
                    return false;
                }
                self.pending.clear();
                bytes = &bytes[end + 1..];
            } else {
                self.pending.extend_from_slice(bytes);
                return true;
            }
        }
        while let Some(end) = memchr::memchr(b'\n', bytes) {
            if !emit(&bytes[..=end]) {
                return false;
            }
            bytes = &bytes[end + 1..];
        }
        self.pending.extend_from_slice(bytes);
        true
    }

    pub fn finish(&mut self, mut emit: impl FnMut(&[u8]) -> bool) -> bool {
        if self.pending.is_empty() {
            return true;
        }
        let result = emit(&self.pending);
        self.pending.clear();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chunk_size_preserves_lines_and_utf8() {
        let bytes = "αβ\n\nhello\n終".as_bytes();
        for size in 1..=bytes.len() {
            let mut framer = LineFramer::default();
            let mut lines = Vec::new();
            for chunk in bytes.chunks(size) {
                assert!(framer.push(chunk, |line| {
                    lines.push(line.to_vec());
                    true
                }));
            }
            framer.finish(|line| {
                lines.push(line.to_vec());
                true
            });
            assert_eq!(
                lines,
                ["αβ\n", "\n", "hello\n", "終"].map(|s| s.as_bytes().to_vec())
            );
        }
    }

    #[test]
    fn stop_does_not_consume_later_lines() {
        let mut framer = LineFramer::default();
        let mut calls = 0;
        assert!(!framer.push(b"first\nsecond\n", |_| {
            calls += 1;
            false
        }));
        assert_eq!(calls, 1);
    }
}
