#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if s.len() > 4096 {
            return;
        }
        let _ = fshell_posix::parser::parse_posix_script(s);
    }
});
