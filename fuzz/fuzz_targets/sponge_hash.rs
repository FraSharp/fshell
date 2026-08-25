#![no_main]

use fshell_hash::{Hasher, fhash_kmac, fhash_xof, fhash256, fhash512};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 16384 {
        return;
    }

    // 1. Standard unkeyed sponge hashing
    let _ = fhash256(data);
    let _ = fhash512(data);
    let _ = fhash_xof(data, 128);

    // 2. Streaming sponge hasher
    let mut h = Hasher::new(0x00, 16);
    h.update(data);
    let _ = h.finalize(32);

    // 3. Keyed KMAC mode
    if data.len() >= 32 {
        let key = &data[0..32];
        let payload = &data[32..];
        let _ = fhash_kmac(key, payload, 32, &[]);
    }
});
