// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#[cfg(not(unix))]
compile_error!("fshell requires a Unix-compatible operating system (Linux or macOS).");

fn main() {
    let trace = fshell_engine::trace::TraceSink::from_env();
    let mut entry = trace.span(
        trace.root_context(),
        "process.rust_entry",
        fshell_engine::trace::TraceMode::Startup,
        serde_json::Map::new(),
    );
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
        if let Some(span) = entry.take() {
            span.finish(fshell_engine::trace::SpanOutcome::Ok);
        }
        fshell_engine::trace::TraceSink::exit_process(
            &trace,
            trace.root_context(),
            fshell_engine::trace::TraceMode::Startup,
            0,
            fshell_engine::trace::SpanOutcome::Exit,
        );
    }

    let program_name = args
        .first()
        .as_ref()
        .and_then(|p| std::path::Path::new(p).file_name())
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "fshell".to_string());

    // login(1) starts login shells with a leading '-' in argv[0] (for
    // example, `-fsh`). Keep the original argv[0] for `fshell::run()` so it
    // can detect login mode, but remove that marker when deciding whether
    // this multicall binary was invoked as a utility.
    let dispatch_name = program_name.strip_prefix('-').unwrap_or(&program_name);
    if dispatch_name != "fsh" && dispatch_name != "fshell" {
        if let Some(mut span) = entry.take() {
            span.add_attr("route", "utility");
            span.finish(fshell_engine::trace::SpanOutcome::Ok);
        }
        let utility_args: Vec<String> = args.into_iter().skip(1).collect();
        fshell::run_utility_with_trace(dispatch_name, &utility_args, trace.clone());
    }

    let runtime_span = trace.span(
        trace.root_context(),
        "process.runtime_init",
        fshell_engine::trace::TraceMode::Startup,
        serde_json::Map::new(),
    );
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| panic!("failed to build runtime: {e}"));
    if let Some(mut span) = entry.take() {
        span.add_attr("route", "shell");
        span.finish(fshell_engine::trace::SpanOutcome::Ok);
    }
    if let Some(span) = runtime_span {
        span.finish(fshell_engine::trace::SpanOutcome::Ok);
    }
    rt.block_on(fshell::run_with_trace(trace.clone()));
    trace.flush();
}
