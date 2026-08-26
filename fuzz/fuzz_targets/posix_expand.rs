#![no_main]

use fshell_posix::expand::{ExpansionConfig, expand_word, split_ifs};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if s.len() > 4096 {
            return;
        }

        let env = fshell_engine::Env::for_command();
        let cfg = ExpansionConfig::default();

        // 1. Fuzz POSIX word expansion
        let positional = vec!["arg1".to_string(), "arg2 with space".to_string()];
        let _ = expand_word(s, &env, &cfg, &positional);

        // 2. Fuzz IFS field splitting with varied delimiters
        let _ = split_ifs(s, " \t\n");
        let _ = split_ifs(s, ":,");
        let _ = split_ifs(s, "");
    }
});
