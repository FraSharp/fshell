#![no_main]

use fshell_core::Parser;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if s.len() > 4096 {
            return;
        }
        let mut p = Parser::new(s);
        let _ = p.parse_statements();
    }
});
