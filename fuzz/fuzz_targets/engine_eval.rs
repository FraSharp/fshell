#![no_main]

use fshell_core::Parser;
use fshell_engine::{Env, eval_stmt};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if s.len() > 4096 {
            return;
        }

        let mut parser = Parser::new(s);
        if let Ok(stmts) = parser.parse_statements() {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(_) => return,
            };

            let env = Env::for_command();
            let _ = rt.block_on(async {
                for stmt in stmts {
                    let _ = tokio::select! {
                        res = eval_stmt(&stmt, &env, false) => res.ok(),
                        _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => None,
                    };
                }
            });
        }
    }
});
