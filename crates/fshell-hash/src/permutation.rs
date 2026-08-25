// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

/// ARX layer with round-dependent pairings.
/// Statically unrolled for compiler-driven SIMD auto-vectorization.
pub fn arx_layer(state: &mut [u64; 16], round: usize) {
    #[inline(always)]
    fn mix(state: &mut [u64; 16], idx: usize, partner: usize, rot: u32) {
        let mut a = state[idx];
        let mut b = state[partner];
        a = a.wrapping_add(b);
        b ^= a;
        b = b.rotate_left(rot);
        state[idx] = b;
        state[partner] = a;
    }

    match round % 4 {
        0 => {
            mix(state, 0, 1, crate::constants::ROTATIONS[0]);
            mix(state, 2, 3, crate::constants::ROTATIONS[1]);
            mix(state, 4, 5, crate::constants::ROTATIONS[2]);
            mix(state, 6, 7, crate::constants::ROTATIONS[3]);
            mix(state, 8, 9, crate::constants::ROTATIONS[4]);
            mix(state, 10, 11, crate::constants::ROTATIONS[5]);
            mix(state, 12, 13, crate::constants::ROTATIONS[6]);
            mix(state, 14, 15, crate::constants::ROTATIONS[7]);
        }
        1 => {
            mix(state, 0, 2, crate::constants::ROTATIONS[0]);
            mix(state, 1, 3, crate::constants::ROTATIONS[1]);
            mix(state, 4, 6, crate::constants::ROTATIONS[2]);
            mix(state, 5, 7, crate::constants::ROTATIONS[3]);
            mix(state, 8, 10, crate::constants::ROTATIONS[4]);
            mix(state, 9, 11, crate::constants::ROTATIONS[5]);
            mix(state, 12, 14, crate::constants::ROTATIONS[6]);
            mix(state, 13, 15, crate::constants::ROTATIONS[7]);
        }
        2 => {
            mix(state, 0, 4, crate::constants::ROTATIONS[0]);
            mix(state, 1, 5, crate::constants::ROTATIONS[1]);
            mix(state, 2, 6, crate::constants::ROTATIONS[2]);
            mix(state, 3, 7, crate::constants::ROTATIONS[3]);
            mix(state, 8, 12, crate::constants::ROTATIONS[4]);
            mix(state, 9, 13, crate::constants::ROTATIONS[5]);
            mix(state, 10, 14, crate::constants::ROTATIONS[6]);
            mix(state, 11, 15, crate::constants::ROTATIONS[7]);
        }
        3 => {
            mix(state, 0, 8, crate::constants::ROTATIONS[0]);
            mix(state, 1, 9, crate::constants::ROTATIONS[1]);
            mix(state, 2, 10, crate::constants::ROTATIONS[2]);
            mix(state, 3, 11, crate::constants::ROTATIONS[3]);
            mix(state, 4, 12, crate::constants::ROTATIONS[4]);
            mix(state, 5, 13, crate::constants::ROTATIONS[5]);
            mix(state, 6, 14, crate::constants::ROTATIONS[6]);
            mix(state, 7, 15, crate::constants::ROTATIONS[7]);
        }
        _ => unreachable!(),
    }
}

/// Bit-sliced bijective Feistel-like χ transform across all 16 words.
/// Splitting the state into Left half (words 0..8) and Right half (words 8..16).
/// We update Left using a nonlinear combination of Right, then Right using Left.
/// This guarantees that the transformation is a bijection (no collisions).
/// Applied once for optimal performance and security balance.
pub fn chi_layer(state: &mut [u64; 16]) {
    // Step 1: Left half ^= !Right half & Right half shifted
    for i in 0..8 {
        state[i] ^= !state[i + 8] & state[((i + 1) % 8) + 8];
    }
    // Step 2: Right half ^= !Left half & Left half shifted
    for i in 8..16 {
        state[i] ^= !state[i - 8] & state[(i - 8 + 1) % 8];
    }
}

// Fixed permutation mapping for constant-time, data-independent mixing.
// Using a step size of 5 (coprime to 16) to ensure optimal diffusion of adjacent words.
const PERMUTATION: [usize; 16] = [3, 8, 13, 2, 7, 12, 1, 6, 11, 0, 5, 10, 15, 4, 9, 14];

/// Constant-time fixed word permutation.
pub fn shuffle_layer(state: &mut [u64; 16]) {
    let original = *state;
    for i in 0..16 {
        state[i] = original[PERMUTATION[i]];
    }
}

/// XOR round constant into state[0].
pub fn iota_layer(state: &mut [u64; 16], round: usize) {
    let rc = crate::constants::ROUND_CONSTANTS[round % crate::constants::ROUND_CONSTANTS.len()];
    state[0] ^= rc;
}

/// Apply all 4 layers for `rounds` iterations.
pub fn permute(state: &mut [u64; 16], rounds: usize) {
    for round in 0..rounds {
        arx_layer(state, round);
        chi_layer(state);
        shuffle_layer(state);
        iota_layer(state, round);
    }
}

/// Const-generic permutation — allows full loop unrolling at compile time.
/// Uses the same ARX/chi/shuffle/iota layers as the non-const version.
#[inline(always)]
pub fn permute_const<const ROUNDS: usize>(state: &mut [u64; 16]) {
    for round in 0..ROUNDS {
        arx_layer(state, round);
        chi_layer(state);
        shuffle_layer(state);
        iota_layer(state, round);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arx_layer_basic() {
        let mut state = [1u64; 16];
        arx_layer(&mut state, 0);
        assert_ne!(state, [1u64; 16]);
    }

    #[test]
    fn test_arx_layer_deterministic() {
        let mut s1 = [0xDEADBEEF_CAFEBABEu64; 16];
        let mut s2 = s1;
        arx_layer(&mut s1, 0);
        arx_layer(&mut s2, 0);
        assert_eq!(s1, s2);
    }

    #[test]
    fn test_arx_layer_different_rounds() {
        let mut s1 = [0x0123456789ABCDEFu64; 16];
        let mut s2 = [0x0123456789ABCDEFu64; 16];
        arx_layer(&mut s1, 0);
        arx_layer(&mut s2, 1);
        assert_ne!(s1, s2, "different rounds should produce different mixing");
    }

    #[test]
    fn test_arx_layer_non_zero_output() {
        let mut state = [0x0123456789ABCDEFu64; 16];
        arx_layer(&mut state, 0);
        assert_ne!(state, [0x0123456789ABCDEFu64; 16]);
    }

    #[test]
    fn test_chi_layer_basic() {
        let mut state = core::array::from_fn(|i| i as u64);
        let original = state;
        chi_layer(&mut state);
        assert_ne!(state, original);
    }

    #[test]
    fn test_chi_layer_deterministic() {
        let mut s1 = [0xDEADBEEF_CAFEBABEu64; 16];
        let mut s2 = s1;
        chi_layer(&mut s1);
        chi_layer(&mut s2);
        assert_eq!(s1, s2);
    }

    #[test]
    fn test_permute_deterministic() {
        let mut s1 = [0x0123456789ABCDEFu64; 16];
        let mut s2 = s1;
        permute(&mut s1, 12);
        permute(&mut s2, 12);
        assert_eq!(s1, s2);
    }

    #[test]
    fn test_permute_changes_state() {
        let mut state = [0u64; 16];
        permute(&mut state, 1);
        assert_ne!(state, [0u64; 16]);
    }

    #[test]
    fn test_permute_zero_rounds() {
        let mut state = [0xDEADBEEF_CAFEBABEu64; 16];
        let original = state;
        permute(&mut state, 0);
        assert_eq!(state, original);
    }

    #[test]
    fn test_iota_layer_basic() {
        let mut state = [0u64; 16];
        iota_layer(&mut state, 0);
        assert_eq!(state[0], crate::constants::ROUND_CONSTANTS[0]);
        assert_eq!(state[1..], [0u64; 15]);
    }

    #[test]
    fn test_iota_different_rounds() {
        let mut s1 = [0u64; 16];
        let mut s2 = [0u64; 16];
        iota_layer(&mut s1, 0);
        iota_layer(&mut s2, 1);
        assert_ne!(s1, s2);
    }

    #[test]
    fn test_shuffle_basic() {
        let mut state = core::array::from_fn(|i| i as u64);
        let original = state;
        shuffle_layer(&mut state);
        let mut sorted = state;
        sorted.sort();
        let mut orig_sorted = original;
        orig_sorted.sort();
        assert_eq!(sorted, orig_sorted);
    }

    #[test]
    fn test_shuffle_deterministic() {
        let mut s1 = [0x0123456789ABCDEFu64; 16];
        let mut s2 = s1;
        shuffle_layer(&mut s1);
        shuffle_layer(&mut s2);
        assert_eq!(s1, s2);
    }

    #[test]
    fn test_chi_layer_all_ones() {
        let mut state = [!0u64; 16];
        chi_layer(&mut state);
        // χ: 1 ^= (~1) & 1 = 1 ^ (0) = 1, so all 1s stays all 1s
        assert_eq!(state, [!0u64; 16]);
    }
}
