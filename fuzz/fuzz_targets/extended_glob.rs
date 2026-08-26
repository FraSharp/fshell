#![no_main]

use fshell_core::extended_glob::ExtendedGlob;
use fshell_core::glob_utils::parse_glob_qualifiers;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if s.len() > 4096 {
            return;
        }

        // 1. Fuzz extended glob parser and regex generation
        if let Some(glob) = ExtendedGlob::parse(s) {
            let re_str = glob.to_regex();
            let _ = regex::Regex::new(&re_str);
        }

        // 2. Fuzz glob qualifier parser (zsh-style)
        let _ = parse_glob_qualifiers(s);
    }
});
