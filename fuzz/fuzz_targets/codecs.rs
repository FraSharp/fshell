#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 8192 {
        return;
    }

    // 1. Fuzz JSON deserializer
    if let Ok(s) = std::str::from_utf8(data) {
        let _: Result<serde_json::Value, _> = serde_json::from_str(s);
        let _: Result<fshell_core::Val, _> = serde_json::from_str(s);
    }

    // 2. Fuzz YAML deserializer
    if let Ok(s) = std::str::from_utf8(data) {
        let _: Result<serde_yaml::Value, _> = serde_yaml::from_str(s);
    }

    // 3. Fuzz MessagePack deserializer
    let _: Result<serde_json::Value, _> = rmp_serde::from_slice(data);
});
