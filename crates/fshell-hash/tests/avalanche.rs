// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_hash::fhash256;

fn count_diff_bits(a: &[u8], b: &[u8]) -> u32 {
    a.iter()
        .zip(b.iter())
        .fold(0, |acc, (x, y)| acc + (x ^ y).count_ones())
}

#[test]
fn test_avalanche_effect() {
    let input = b"abcdefghijklmnopqrstuvwxyz012345";
    let original = fhash256(input);

    for bit in 0..256 {
        let mut modified = *input;
        modified[bit / 8] ^= 1 << (bit % 8);

        let hash = fhash256(&modified);
        let diff_bits = count_diff_bits(&original, &hash);
        let ratio = diff_bits as f64 / 256.0;

        assert!(
            ratio > 0.30 && ratio < 0.70,
            "bit flip at global bit {} gave {:.1}% diff (expected ~50%)",
            bit,
            ratio * 100.0
        );
    }
}

#[test]
fn test_avalanche_first_byte() {
    let a = fhash256(b"hello world");
    let mut modified = *b"hello world";
    modified[0] ^= 1;
    let b = fhash256(&modified);
    let diff = count_diff_bits(&a, &b);
    assert!(diff > 80 && diff < 176, "diff: {} bits", diff);
}

#[test]
fn test_avalanche_middle_byte() {
    let a = fhash256(b"hello world");
    let mut modified = *b"hello world";
    modified[4] ^= 0x80;
    let b = fhash256(&modified);
    let diff = count_diff_bits(&a, &b);
    assert!(diff > 80 && diff < 176, "diff: {} bits", diff);
}
