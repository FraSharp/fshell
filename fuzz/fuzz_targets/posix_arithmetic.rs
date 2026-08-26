#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if s.len() > 4096 {
            return;
        }

        let env = fshell_engine::Env::for_command();
        // Fuzz arithmetic expression parser and evaluator
        let _ = fshell_posix::arithmetic::eval_arithmetic_expr(s, &env);
    }
});
