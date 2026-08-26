#![no_main]

use fshell_posix::eval::{EvalConfig, eval_source};
use fshell_posix::parser::parse_posix_script;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if s.len() > 4096 {
            return;
        }

        if let Ok(parsed) = parse_posix_script(s) {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(_) => return,
            };

            let env = fshell_engine::Env::for_command();
            let cfg = EvalConfig::default();

            // Run with a timeout or quick execution
            let _ = rt.block_on(async {
                tokio::select! {
                    res = eval_source(&parsed, &env, &cfg) => res.ok(),
                    _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => None,
                }
            });
        }
    }
});
