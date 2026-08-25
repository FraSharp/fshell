// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#[repr(align(64))]
#[derive(Clone, Copy)]
pub struct State(pub [u64; 16]);

impl State {
    #[allow(dead_code)]
    pub fn new() -> Self {
        State([0u64; 16])
    }

    pub fn init_with_iv() -> Self {
        State(crate::constants::IV)
    }

    #[allow(dead_code)]
    pub fn from_bytes(bytes: &[u8; 128]) -> Self {
        let mut words = [0u64; 16];
        for (i, word) in words.iter_mut().enumerate() {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(&bytes[i * 8..(i + 1) * 8]);
            *word = u64::from_le_bytes(arr);
        }
        State(words)
    }

    pub fn to_bytes(self) -> [u8; 128] {
        let mut bytes = [0u8; 128];
        for (i, word) in self.0.iter().enumerate() {
            bytes[i * 8..(i + 1) * 8].copy_from_slice(&word.to_le_bytes());
        }
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_new_is_all_zeros() {
        let s = State::new();
        assert_eq!(s.0, [0u64; 16]);
    }

    #[test]
    fn test_load_store_roundtrip() {
        let input = [0x0123456789ABCDEFu64; 16];
        let bytes = State(input).to_bytes();
        let loaded = State::from_bytes(&bytes);
        assert_eq!(loaded.0, input);
    }

    #[test]
    fn test_bytes_len() {
        let s = State::new();
        assert_eq!(s.to_bytes().len(), 128);
    }

    #[test]
    fn test_init_with_iv() {
        let s = State::init_with_iv();
        assert_eq!(s.0, crate::constants::IV);
    }
}
