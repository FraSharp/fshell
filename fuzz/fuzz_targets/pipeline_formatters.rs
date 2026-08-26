#![no_main]

use fshell_core::Val;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 8192 {
        return;
    }

    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(val) = serde_json::from_str::<Val>(s) {
            let _ = val.to_text();
            let _ = val.type_name();
            let _ = serde_json::to_string(&val);
            let _ = serde_yaml::to_string(&val);
            let _ = rmp_serde::to_vec(&val);
        }
    }

    if let Ok(val) = rmp_serde::from_slice::<Val>(data) {
        let _ = val.to_text();
        let _ = val.type_name();
        let _ = serde_json::to_string(&val);
    }
});
