// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#[cfg(not(unix))]
compile_error!("fshell requires a Unix-compatible operating system (Linux or macOS).");

fn main() {
    fshell::setup_panic_hook();

    let args: Vec<String> = std::env::args().collect();
    let mut is_empty_command = false;
    let mut i = 0;
    while i < args.len() {
        if (args[i] == "-c" || args[i] == "--command")
            && i + 1 < args.len()
            && args[i + 1].trim().is_empty()
        {
            is_empty_command = true;
        }
        i += 1;
    }
    if is_empty_command {
        std::process::exit(0);
    }

    let program_name = args
        .first()
        .as_ref()
        .and_then(|p| std::path::Path::new(p).file_name())
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "fshell".to_string());

    if program_name != "fsh" && program_name != "fshell" {
        let utility_args: Vec<String> = args.into_iter().skip(1).collect();
        fshell::run_utility(&program_name, &utility_args);
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| panic!("failed to build runtime: {e}"));
    rt.block_on(fshell::run());
}
