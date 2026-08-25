// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

// crates/fshell-hash/src/fast_hash.rs

use core::hash::Hasher;

// fshell's own seed — must be odd for full period in the multiplier.
// Same algorithm as fxhash but with our own constant.
const SEED: u64 = 0x9E3779B97F4A7C15;
const ROTATE: u32 = 5;

// Additional seeds for interleaved chains on larger inputs
const SEED2: u64 = 0xBF58476D1CE4E5B9;
const SEED3: u64 = 0x94D049BB133111EB;
const SEED4: u64 = 0xC2B2E3922C78E5A3;

/// A fast, non-cryptographic hasher for hash map operations.
///
/// Uses the same `rotate_left(5) ^ word).wrapping_mul(SEED)` algorithm as fxhash
/// for maximum throughput on small inputs (the common case for map keys).
/// For larger inputs (≥64B), uses interleaved chains for ILP.
///
/// Note: hash values are platform-dependent (native-endian).
#[derive(Clone)]
pub struct MapHasher {
    hash: u64,
}

impl MapHasher {
    #[inline]
    pub fn new() -> Self {
        Self { hash: 0 }
    }
}

impl Default for MapHasher {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

/// The core mixing function — same structure as fxhash.
/// `(hash.rotate_left(5) ^ word).wrapping_mul(SEED)`
#[inline(always)]
fn hash_word(hash: u64, word: u64) -> u64 {
    (hash.rotate_left(ROTATE) ^ word).wrapping_mul(SEED)
}

/// Read a u64 from a native-endian byte slice (same as byteorder::NativeEndian::read_u64).
#[inline(always)]
fn read_u64(bytes: &[u8]) -> u64 {
    debug_assert!(bytes.len() >= 8);
    let mut buf = [0u8; 8];
    // SAFETY: caller guarantees bytes.len() >= 8
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf.as_mut_ptr(), 8);
    }
    u64::from_ne_bytes(buf)
}

/// Read a u32 from a native-endian byte slice.
#[inline(always)]
fn read_u32(bytes: &[u8]) -> u32 {
    debug_assert!(bytes.len() >= 4);
    let mut buf = [0u8; 4];
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf.as_mut_ptr(), 4);
    }
    u32::from_ne_bytes(buf)
}

impl Hasher for MapHasher {
    #[inline(always)]
    fn write(&mut self, mut bytes: &[u8]) {
        let len = bytes.len();

        if len < 64 {
            // Fast path for small inputs — same pattern as fxhash (slice advancement).
            let mut hash = self.hash;

            // Process 8-byte chunks
            while bytes.len() >= 8 {
                hash = hash_word(hash, read_u64(bytes));
                bytes = &bytes[8..];
            }
            // Process 4-byte chunk
            if bytes.len() >= 4 {
                hash = hash_word(hash, read_u32(bytes) as u64);
                bytes = &bytes[4..];
            }
            // Process remaining bytes (1-3)
            for &byte in bytes {
                hash = hash_word(hash, byte as u64);
            }
            self.hash = hash;
        } else {
            // 4 interleaved chains — maximum ILP for large inputs
            let mut h1 = self.hash;
            let mut h2 = self.hash ^ SEED2;
            let mut h3 = self.hash ^ SEED3;
            let mut h4 = self.hash ^ SEED4;

            let (chunks64, rem64) = bytes.as_chunks::<64>();
            for chunk in chunks64 {
                let a = read_u64(&chunk[0..8]);
                let b = read_u64(&chunk[8..16]);
                let c = read_u64(&chunk[16..24]);
                let d = read_u64(&chunk[24..32]);
                let e = read_u64(&chunk[32..40]);
                let f = read_u64(&chunk[40..48]);
                let g = read_u64(&chunk[48..56]);
                let h = read_u64(&chunk[56..64]);

                h1 = hash_word(h1, a);
                h2 = hash_word(h2, b);
                h3 = hash_word(h3, c);
                h4 = hash_word(h4, d);
                h1 = hash_word(h1, e);
                h2 = hash_word(h2, f);
                h3 = hash_word(h3, g);
                h4 = hash_word(h4, h);
            }

            // Process remaining 16-byte chunks
            let (chunks16, _) = rem64.as_chunks::<16>();
            for chunk in chunks16 {
                let a = read_u64(&chunk[0..8]);
                let b = read_u64(&chunk[8..16]);
                h1 = hash_word(h1, a);
                h2 = hash_word(h2, b);
            }

            // Handle final bytes (< 16)
            let final_offset = len & !15;
            let tail = &bytes[final_offset..];
            for (i, &byte) in tail.iter().enumerate() {
                match i % 4 {
                    0 => h1 = hash_word(h1, byte as u64),
                    1 => h2 = hash_word(h2, byte as u64),
                    2 => h3 = hash_word(h3, byte as u64),
                    _ => h4 = hash_word(h4, byte as u64),
                }
            }

            self.hash = h1 ^ h2 ^ h3 ^ h4;
        }
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

/// BuildHasher for `MapHasher`.
#[derive(Default, Clone, Copy)]
pub struct MapBuildHasher;

impl core::hash::BuildHasher for MapBuildHasher {
    type Hasher = MapHasher;

    #[inline]
    fn build_hasher(&self) -> MapHasher {
        MapHasher::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic() {
        let mut h1 = MapHasher::new();
        let mut h2 = MapHasher::new();
        h1.write(b"hello world");
        h2.write(b"hello world");
        assert_eq!(h1.finish(), h2.finish());
    }

    #[test]
    fn test_different_inputs_differ() {
        let mut h1 = MapHasher::new();
        let mut h2 = MapHasher::new();
        h1.write(b"hello");
        h2.write(b"world");
        assert_ne!(h1.finish(), h2.finish());
    }

    #[test]
    fn test_empty_input() {
        let mut h = MapHasher::new();
        h.write(b"");
        let _ = h.finish();
    }

    #[test]
    fn test_single_byte() {
        let mut h = MapHasher::new();
        h.write(b"a");
        let _ = h.finish();
    }

    #[test]
    fn test_exactly_8_bytes() {
        let mut h = MapHasher::new();
        h.write(b"12345678");
        let _ = h.finish();
    }

    #[test]
    fn test_exactly_16_bytes() {
        let mut h = MapHasher::new();
        h.write(b"1234567890123456");
        let _ = h.finish();
    }

    #[test]
    fn test_exactly_64_bytes() {
        let mut h = MapHasher::new();
        h.write(b"1234567890123456789012345678901234567890123456789012345678901234");
        let _ = h.finish();
    }

    #[test]
    fn test_large_input() {
        let data = vec![0xABu8; 1024];
        let mut h = MapHasher::new();
        h.write(&data);
        let _ = h.finish();
    }

    #[test]
    fn test_bit_flip_produces_different_hash() {
        let base = b"abcdefghijklmnop";
        let mut base_hasher = MapHasher::new();
        base_hasher.write(base);
        let base_hash = base_hasher.finish();

        let mut different_count = 0;
        for byte_idx in 0..16 {
            for bit_idx in 0..8 {
                let mut modified = *base;
                modified[byte_idx] ^= 1 << bit_idx;
                let mut h = MapHasher::new();
                h.write(&modified);
                let h_val = h.finish();
                if base_hash != h_val {
                    different_count += 1;
                }
            }
        }
        assert!(
            different_count > 100,
            "too many collisions: only {different_count}/128 bit flips changed hash"
        );
    }

    #[test]
    fn test_known_hash_value() {
        let mut h = MapHasher::new();
        h.write(b"hello");
        let hash_val = h.finish();
        assert_ne!(hash_val, 0, "hash should not be zero");
        assert_ne!(hash_val, u64::MAX, "hash should not be max");
    }

    #[test]
    #[allow(clippy::manual_hash_one)]
    fn test_build_hasher() {
        use core::hash::{BuildHasher, Hash};
        let map_build = MapBuildHasher;
        let mut h = map_build.build_hasher();
        42u64.hash(&mut h);
        let _ = h.finish();
    }
}
