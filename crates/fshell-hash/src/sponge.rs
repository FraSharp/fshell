// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::permutation::permute;
use crate::state::State;
use alloc::vec::Vec;
use core::cmp::min;

const RATE_BYTES: usize = 64;
const DEFAULT_ROUNDS: usize = 16;

/// A streaming hasher for the fhash algorithm.
#[derive(Clone)]
pub struct Hasher {
    state: State,
    buffer: [u8; 64],
    buf_len: usize,
    pub(crate) domain: u8,
    pub(crate) rounds: usize,
    finished: bool,
}

impl Hasher {
    /// Create a new Hasher with specific domain separation and round count.
    pub fn new(domain: u8, rounds: usize) -> Self {
        Self {
            state: State::init_with_iv(),
            buffer: [0u8; 64],
            buf_len: 0,
            domain,
            rounds,
            finished: false,
        }
    }

    /// Update the hasher state with more input data.
    pub fn update(&mut self, mut data: &[u8]) {
        assert!(!self.finished, "Cannot update a finalized Hasher");

        while !data.is_empty() {
            let space = RATE_BYTES - self.buf_len;
            let to_write = min(space, data.len());
            self.buffer[self.buf_len..self.buf_len + to_write].copy_from_slice(&data[..to_write]);
            self.buf_len += to_write;
            data = &data[to_write..];

            if self.buf_len == RATE_BYTES {
                for i in 0..8 {
                    let word = u64::from_le_bytes([
                        self.buffer[i * 8],
                        self.buffer[i * 8 + 1],
                        self.buffer[i * 8 + 2],
                        self.buffer[i * 8 + 3],
                        self.buffer[i * 8 + 4],
                        self.buffer[i * 8 + 5],
                        self.buffer[i * 8 + 6],
                        self.buffer[i * 8 + 7],
                    ]);
                    self.state.0[i] ^= word;
                }
                permute(&mut self.state.0, self.rounds);
                self.buf_len = 0;
            }
        }
    }

    /// Finalize the hashing process and return the squeezed output.
    pub fn finalize(mut self, output_len: usize) -> Vec<u8> {
        self.finished = true;

        // Pad: append domain byte, then 0x01, then zeros, then 0x80
        let mut padding = Vec::new();
        padding.push(self.domain);
        padding.push(0x01);

        let current_total = self.buf_len + padding.len() + 1; // message + domain + 0x01 + 0x80
        let remainder = current_total % RATE_BYTES;
        let zeros = if remainder == 0 {
            0
        } else {
            RATE_BYTES - remainder
        };

        padding.resize(padding.len() + zeros, 0x00);
        padding.push(0x80);

        // Feed padding in (temporarily unsetting finished flag)
        self.finished = false;
        self.update(&padding);
        self.finished = true;

        // Squeeze output bytes
        let mut output = vec![0u8; output_len];
        let mut filled = 0;
        while filled < output_len {
            let all_bytes = self.state.to_bytes();
            let take = core::cmp::min(64, output_len - filled);
            output[filled..filled + take].copy_from_slice(&all_bytes[..take]);
            filled += take;
            if filled < output_len {
                permute(&mut self.state.0, self.rounds);
            }
        }
        output
    }

    /// Finalize from a clone of this hasher, leaving `self` unchanged.
    pub fn finalize_clone(&self, output_len: usize) -> Vec<u8> {
        self.clone().finalize(output_len)
    }

    /// Finalize with a compile-time-known round count for full unrolling.
    /// Same cryptographic operations as finalize(), but with const-generic permutation.
    pub fn finalize_const<const ROUNDS: usize>(mut self, output_len: usize) -> Vec<u8> {
        self.finished = true;

        let mut padding = Vec::new();
        padding.push(self.domain);
        padding.push(0x01);

        let current_total = self.buf_len + padding.len() + 1;
        let remainder = current_total % RATE_BYTES;
        let zeros = if remainder == 0 {
            0
        } else {
            RATE_BYTES - remainder
        };

        padding.resize(padding.len() + zeros, 0x00);
        padding.push(0x80);

        self.finished = false;
        self.update(&padding);
        self.finished = true;

        let mut output = vec![0u8; output_len];
        let mut filled = 0;
        while filled < output_len {
            let all_bytes = self.state.to_bytes();
            let take = core::cmp::min(64, output_len - filled);
            output[filled..filled + take].copy_from_slice(&all_bytes[..take]);
            filled += take;
            if filled < output_len {
                crate::permutation::permute_const::<ROUNDS>(&mut self.state.0);
            }
        }
        output
    }
}

/// Hash `message` to `output_len` bytes using fhash sponge with variable rounds.
#[doc(hidden)]
pub fn fhash_with_rounds(message: &[u8], output_len: usize, domain: u8, rounds: usize) -> Vec<u8> {
    let mut hasher = Hasher::new(domain, rounds);
    hasher.update(message);
    hasher.finalize(output_len)
}

/// Hash `message` to `output_len` bytes using fhash sponge with domain separation.
pub fn fhash(message: &[u8], output_len: usize, domain: u8) -> Vec<u8> {
    fhash_with_rounds(message, output_len, domain, DEFAULT_ROUNDS)
}

/// Compute a Message Authentication Code (MAC) using the fhash sponge.
///
/// Under the sponge construction, the key and customization strings are absorbed
/// prefixed with their length and block-aligned, preventing prefix-collision and
/// domain-confusion attacks. Domain separation byte `0x08` is used.
pub fn fhash_kmac(key: &[u8], message: &[u8], output_len: usize, customization: &[u8]) -> Vec<u8> {
    let mut hasher = Hasher::new(0x08, DEFAULT_ROUNDS);

    // Key absorption with length-prefixing and rate-block alignment (64 bytes)
    let len_bytes = (key.len() as u64).to_le_bytes();
    hasher.update(&len_bytes);
    hasher.update(key);
    let remainder = (8 + key.len()) % 64;
    if remainder != 0 {
        let padding = [0u8; 64];
        hasher.update(&padding[..64 - remainder]);
    }

    // Customization string absorption with length-prefixing and rate-block alignment
    let cust_len_bytes = (customization.len() as u64).to_le_bytes();
    hasher.update(&cust_len_bytes);
    hasher.update(customization);
    let cust_remainder = (8 + customization.len()) % 64;
    if cust_remainder != 0 {
        let padding = [0u8; 64];
        hasher.update(&padding[..64 - cust_remainder]);
    }

    // Absorb the message
    hasher.update(message);
    hasher.finalize(output_len)
}

/// Derives keys from input keying material (IKM), salt, and info context using the fhash sponge.
///
/// Under the sponge construction, salt, IKM, and info are absorbed prefixed with
/// their length and block-aligned. Domain separation byte `0x10` is used.
pub fn fhash_kdf(ikm: &[u8], salt: &[u8], info: &[u8], output_len: usize) -> Vec<u8> {
    let mut hasher = Hasher::new(0x10, DEFAULT_ROUNDS);

    // Absorb salt prefixed with length and aligned
    let salt_len_bytes = (salt.len() as u64).to_le_bytes();
    hasher.update(&salt_len_bytes);
    hasher.update(salt);
    let salt_remainder = (8 + salt.len()) % 64;
    if salt_remainder != 0 {
        let padding = [0u8; 64];
        hasher.update(&padding[..64 - salt_remainder]);
    }

    // Absorb IKM prefixed with length and aligned
    let ikm_len_bytes = (ikm.len() as u64).to_le_bytes();
    hasher.update(&ikm_len_bytes);
    hasher.update(ikm);
    let ikm_remainder = (8 + ikm.len()) % 64;
    if ikm_remainder != 0 {
        let padding = [0u8; 64];
        hasher.update(&padding[..64 - ikm_remainder]);
    }

    // Absorb info prefixed with length and aligned
    let info_len_bytes = (info.len() as u64).to_le_bytes();
    hasher.update(&info_len_bytes);
    hasher.update(info);
    let info_remainder = (8 + info.len()) % 64;
    if info_remainder != 0 {
        let padding = [0u8; 64];
        hasher.update(&padding[..64 - info_remainder]);
    }

    hasher.finalize(output_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_absorb_squeeze_roundtrip() {
        let input = b"hello world";
        let output = fhash(input, 32, 0x00);
        assert_eq!(output.len(), 32);
    }

    #[test]
    fn test_deterministic() {
        let a = fhash(b"test", 32, 0x00);
        let b = fhash(b"test", 32, 0x00);
        assert_eq!(a, b);
    }

    #[test]
    fn test_different_inputs() {
        let a = fhash(b"abc", 32, 0x00);
        let b = fhash(b"xyz", 32, 0x00);
        assert_ne!(a, b);
    }

    #[test]
    fn test_xof_variable_length() {
        let h32 = fhash(b"hello", 32, 0x02);
        let h64 = fhash(b"hello", 64, 0x02);
        assert_eq!(h32.len(), 32);
        assert_eq!(h64.len(), 64);
        assert_eq!(&h64[..32], &h32[..]);
    }
}
