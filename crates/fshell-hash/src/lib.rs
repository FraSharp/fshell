// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::panic))]
#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

use alloc::vec::Vec;

mod constants;
mod fast_hash;
mod permutation;
mod sponge;
mod state;

pub use sponge::Hasher;
use sponge::fhash;
pub use sponge::{fhash_kdf, fhash_kmac};

#[doc(hidden)]
pub use sponge::fhash_with_rounds;

#[doc(hidden)]
pub fn fhash_permute(state: &mut [u64; 16], rounds: usize) {
    permutation::permute(state, rounds);
}

/// Compute a 256-bit (32-byte) fhash digest.
pub fn fhash256(message: &[u8]) -> [u8; 32] {
    let result = fhash(message, 32, 0x00);
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

/// Compute a 512-bit (64-byte) fhash digest.
pub fn fhash512(message: &[u8]) -> [u8; 64] {
    let result = fhash(message, 64, 0x04);
    let mut out = [0u8; 64];
    out.copy_from_slice(&result);
    out
}

/// Compute a variable-length fhash XOF digest.
pub fn fhash_xof(message: &[u8], output_len: usize) -> Vec<u8> {
    fhash(message, output_len, 0x02)
}

impl Default for Hasher {
    fn default() -> Self {
        Self::new(0x00, 16)
    }
}

#[cfg(feature = "fast")]
pub fn fhash_fast64(message: &[u8]) -> u64 {
    let mut hasher = sponge::Hasher::new(0x06, 4);
    hasher.update(message);
    let res = hasher.finalize_const::<4>(8);
    let mut out = [0u8; 8];
    out.copy_from_slice(&res);
    u64::from_le_bytes(out)
}

#[cfg(feature = "fast")]
#[derive(Clone)]
pub struct FhashHasher {
    hasher: Hasher,
}

#[cfg(feature = "fast")]
impl FhashHasher {
    pub fn new() -> Self {
        Self {
            hasher: Hasher::new(0x06, 4),
        }
    }
}

#[cfg(feature = "fast")]
impl Default for FhashHasher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "fast")]
impl core::hash::Hasher for FhashHasher {
    fn write(&mut self, bytes: &[u8]) {
        self.hasher.update(bytes);
    }

    fn finish(&self) -> u64 {
        let res = self.hasher.clone().finalize_const::<4>(8);
        let mut out = [0u8; 8];
        out.copy_from_slice(&res);
        u64::from_le_bytes(out)
    }
}

#[cfg(feature = "fast")]
#[derive(Default, Clone, Copy)]
pub struct FhashBuildHasher;

#[cfg(feature = "fast")]
impl core::hash::BuildHasher for FhashBuildHasher {
    type Hasher = FhashHasher;

    fn build_hasher(&self) -> Self::Hasher {
        FhashHasher::new()
    }
}

pub use fast_hash::{MapBuildHasher, MapHasher};

// Convenience type aliases matching fxhash's API
#[cfg(feature = "std")]
pub type FxBuildHasher = MapBuildHasher;
#[cfg(feature = "std")]
pub type FxHashMap<K, V> = std::collections::HashMap<K, V, MapBuildHasher>;
#[cfg(feature = "std")]
pub type FxHashSet<V> = std::collections::HashSet<V, MapBuildHasher>;

#[cfg(feature = "digest")]
impl digest::OutputSizeUser for Hasher {
    type OutputSize = digest::consts::U32;
}

#[cfg(feature = "digest")]
impl digest::Update for Hasher {
    fn update(&mut self, data: &[u8]) {
        self.update(data);
    }
}

#[cfg(feature = "digest")]
impl digest::FixedOutput for Hasher {
    fn finalize_into(self, out: &mut digest::Output<Self>) {
        let res = self.finalize(32);
        out.copy_from_slice(&res);
    }
}

#[cfg(feature = "digest")]
impl digest::HashMarker for Hasher {}

#[cfg(feature = "digest")]
impl digest::Reset for Hasher {
    fn reset(&mut self) {
        *self = Hasher::new(self.domain, self.rounds);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fhash256_length() {
        let h = fhash256(b"hello");
        assert_eq!(h.len(), 32);
    }

    #[test]
    fn test_fhash512_length() {
        let h = fhash512(b"hello");
        assert_eq!(h.len(), 64);
    }

    #[test]
    fn test_xof_length() {
        let h = fhash_xof(b"hello", 100);
        assert_eq!(h.len(), 100);
    }

    #[test]
    fn test_xof_domain_separated_from_fixed() {
        // fhash256 (domain 0x00) and fhash_xof (domain 0x02) must differ
        let h256 = fhash256(b"test");
        let hxof = fhash_xof(b"test", 32);
        assert_ne!(&h256[..], &hxof[..]);
    }

    #[test]
    fn test_xof_has_prefix_property() {
        let h32 = fhash_xof(b"hello", 32);
        let h64 = fhash_xof(b"hello", 64);
        assert_eq!(&h64[..32], &h32[..]);
    }

    #[test]
    #[cfg(feature = "fast")]
    fn test_fast_hash() {
        use core::hash::Hasher;
        let h1 = fhash_fast64(b"hello");
        let h2 = fhash_fast64(b"hello");
        let h3 = fhash_fast64(b"world");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);

        let mut hasher = FhashHasher::new();
        hasher.write(b"hello");
        let hash_val = hasher.finish();
        assert_eq!(hash_val, h1);
    }

    #[test]
    #[cfg(feature = "digest")]
    fn test_digest_trait() {
        use digest::Digest;
        let mut hasher = Hasher::new(0x00, 16);
        Digest::update(&mut hasher, b"hello");
        let res = Digest::finalize(hasher);
        assert_eq!(res.len(), 32);

        let expected = fhash256(b"hello");
        assert_eq!(res.as_slice(), &expected[..]);
    }
}
