// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use clap::Parser;
use fshell_core::Val;
use fshell_core::diagnostic::FshDiag;
use fshell_engine::profiler::{ProfilerCategory, ProfilerState};
use fshell_engine::trace::{SpanOutcome, TraceMode, TraceSink};
use fshell_engine::{EngineError, Flow, PipelinePayload};
use std::io::IsTerminal;
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "fsh",
    version = env!("FSH_FULL_VERSION"),
    about = "A structured data shell with typed pipelines and capability-based security",
    long_about = "fsh is a next-generation shell that replaces fragile text pipelines \
                  with structured data flows. Every value carries its type through the \
                  pipeline — integers, maps, dates, and object graphs.\n\n\
                  Capability-based security means commands only access what you \
                  explicitly grant. Run with --strict to start with no default permissions.",
    after_help = "DOCUMENTATION:\n\
                  * For builtins: try `help <name>` or `fsh help <name>`\n\
                  * For language reference: `man fsh` or read docs/LANGUAGE.md\n\
                  * For migration from bash/zsh/fish: see docs/MIGRATION.md"
)]
struct Cli {
    /// Path to an fsh script file to execute non-interactively
    script: Option<String>,

    /// Arguments passed to the script ($1, $2, $@)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,

    /// Run an inline fsh command and exit
    #[arg(short = 'c', long = "command", value_name = "COMMAND")]
    command: Option<String>,

    /// Enable strict capability mode — deny all access unless explicitly granted via with caps(...)
    #[arg(short = 's', long = "strict")]
    strict: bool,

    /// Restore session state from a handoff file (internal, used by reload --full)
    #[arg(long = "handoff", value_name = "PATH", hide = true)]
    handoff: Option<String>,

    /// Error output format: graphical, compact, or json
    #[arg(long = "error-format", value_name = "FORMAT")]
    error_format: Option<String>,

    /// Disable colored error output
    #[arg(long = "no-color")]
    no_color: bool,

    /// Disable "did you mean" command suggestions
    #[arg(long = "no-dym")]
    no_dym: bool,

    /// DYM mode: "blocking" or "deferred"
    #[arg(long = "suggestion-mode", value_name = "MODE")]
    suggestion_mode: Option<String>,

    /// Resume a saved session by ID, or show picker if "ask"
    #[arg(short = 'r', long = "resume", num_args = 0..=1, default_missing_value = "ask")]
    resume: Option<String>,

    /// Run as a login shell
    #[arg(short = 'l', long = "login")]
    login: bool,

    /// Run in POSIX compatibility mode (sh/bash execution via fshell-posix)
    #[arg(long = "posix")]
    posix: bool,
}

/// Apply the CLI's error-rendering and suggestion flags to shell options.
/// Shared by the `-c` path and the script/REPL path.
fn apply_cli_render_options(env: &fshell_engine::Env, cli: &Cli) {
    let error_format = cli.error_format.as_deref().and_then(|s| match s {
        "graphical" => Some(fshell_render::RenderFormat::Graphical),
        "compact" => Some(fshell_render::RenderFormat::Compact),
        "json" => Some(fshell_render::RenderFormat::Json),
        _ => None,
    });
    let mut opts = env.options.write();
    if let Some(fmt) = error_format {
        opts.error_format = fmt;
    }
    if cli.no_color {
        opts.error_color = false;
    }
    if cli.no_dym {
        opts.did_you_mean = false;
    }
    if let Some(ref mode) = cli.suggestion_mode {
        match mode.as_str() {
            "blocking" => opts.suggestion_mode = fshell_engine::SuggestionMode::Blocking,
            "deferred" => opts.suggestion_mode = fshell_engine::SuggestionMode::Deferred,
            _ => {
                eprintln!(
                    "\x1b[1;33mWarning: Unknown suggestion-mode '{mode}', expected 'blocking' or 'deferred'\x1b[0m"
                );
            }
        }
    }
}

fn trace_span(
    trace: &Arc<TraceSink>,
    name: &str,
    mode: TraceMode,
) -> Option<fshell_engine::trace::TraceSpan> {
    trace.span(trace.root_context(), name, mode, serde_json::Map::new())
}

fn finish_span(span: &mut Option<fshell_engine::trace::TraceSpan>, outcome: SpanOutcome) {
    if let Some(span) = span.take() {
        span.finish(outcome);
    }
}

fn exit_with_trace(trace: &Arc<TraceSink>, code: i32, mode: TraceMode, outcome: SpanOutcome) -> ! {
    TraceSink::exit_process(trace, trace.root_context(), mode, code, outcome)
}

pub async fn run() {
    run_with_trace(TraceSink::from_env()).await;
}

pub async fn run_with_trace(trace: Arc<TraceSink>) {
    // Multicall detection: if invoked as a utility name (e.g. `ls` via symlink
    // to the `fshell` binary), run in utility mode instead of the REPL.
    let raw_program_name = std::env::args()
        .next()
        .and_then(|p| {
            std::path::Path::new(&p)
                .file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "fsh".to_string());

    let is_login_argv0 = raw_program_name.starts_with('-');
    let program_name = if is_login_argv0 {
        raw_program_name
            .strip_prefix('-')
            .unwrap_or(&raw_program_name)
            .to_string()
    } else {
        raw_program_name.clone()
    };

    if program_name != "fsh" && program_name != "fshell" {
        let args: Vec<String> = std::env::args().skip(1).collect();
        // Run utility logic within the existing tokio runtime (avoid creating
        // a nested runtime, which panics).  `run_utility_inner` returns the
        // exit code — exit the process immediately.
        let mut route = trace_span(&trace, "cli.utility_route", TraceMode::Utility);
        if let Some(span) = route.take() {
            span.finish(SpanOutcome::Ok);
        }
        let exit_code = run_utility_inner(&program_name, &args, trace.clone()).await;
        exit_with_trace(&trace, exit_code, TraceMode::Utility, SpanOutcome::Exit);
    }

    let mut cli_parse = trace_span(&trace, "cli.parse", TraceMode::Startup);
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            exit_with_trace(&trace, code, TraceMode::Startup, SpanOutcome::Exit);
        }
    };
    finish_span(&mut cli_parse, SpanOutcome::Ok);
    let is_login = fshell_engine::login::detect_login(&raw_program_name, cli.login);

    // Command mode: lightweight startup — skip handoff, strict, reactive, etc.
    if let Some(cmd) = &cli.command {
        if cmd.trim().is_empty() {
            exit_with_trace(&trace, 0, TraceMode::Command, SpanOutcome::Exit);
        }

        let boot_profiler = Arc::new(fshell_core::RwLock::new(ProfilerState::new(true)));
        {
            let _g = ProfilerState::guard(&boot_profiler, "core init", ProfilerCategory::Init);
            let mut timing = trace_span(&trace, "startup.core_init", TraceMode::Command);
            fshell_core::init();
            finish_span(&mut timing, SpanOutcome::Ok);
        }
        {
            let _g = ProfilerState::guard(&boot_profiler, "caps init", ProfilerCategory::Init);
            let mut timing = trace_span(&trace, "startup.capabilities_init", TraceMode::Command);
            fshell_capabilities::init();
            finish_span(&mut timing, SpanOutcome::Ok);
        }

        let mut timing = trace_span(&trace, "startup.env_init", TraceMode::Command);
        let env = fshell_engine::Env::for_command_with_trace(trace.clone());
        finish_span(&mut timing, SpanOutcome::Ok);

        // Merge boot profiler entries into env's profiler
        {
            let mut target = env.profiler.write();
            let mut source = boot_profiler.write();
            target.merge(&mut source);
        }

        {
            let _g = ProfilerState::guard(&env.profiler, "builtins init", ProfilerCategory::Init);
            let mut timing = trace_span(&trace, "startup.builtins_init", TraceMode::Command);
            fshell_builtins::init(&env);
            finish_span(&mut timing, SpanOutcome::Ok);
        }
        {
            let _g = ProfilerState::guard(&env.profiler, "bridge init", ProfilerCategory::Init);
            let mut timing = trace_span(&trace, "startup.bridge_init", TraceMode::Command);
            fshell_bridge::init(&env);
            finish_span(&mut timing, SpanOutcome::Ok);
        }
        init_posix_handler();
        fshell_engine::setup_signal_handlers(env.clone());
        if !cmd.trim().is_empty() {
            let _g = ProfilerState::guard(&env.profiler, "populate env", ProfilerCategory::Init);
            let mut timing = trace_span(&trace, "startup.host_environment", TraceMode::Command);
            fshell_engine::populate_env_from_host(&env);
            finish_span(&mut timing, SpanOutcome::Ok);
        }

        // Login / interactive semantics: even `fsh -c` can be run as
        // `fsh --login -c '...'` (e.g. via `su -` or `ssh host cmd`).
        // In that case the host login profiles must be visible to the
        // one-shot command — otherwise `$PATH` from ~/.zprofile is missing.
        let is_interactive_cmd = false; // `-c` is always non-interactive
        let mut timing = trace_span(&trace, "startup.login_environment", TraceMode::Command);
        let login_result =
            fshell_engine::login::load_login_environment(&env, is_login, is_interactive_cmd).await;
        finish_span(
            &mut timing,
            if login_result.is_ok() {
                SpanOutcome::Ok
            } else {
                SpanOutcome::Error
            },
        );

        let mut timing = trace_span(&trace, "startup.path_warmup", TraceMode::Command);
        fshell_engine::warmup_path_cache(Some(&env));
        finish_span(&mut timing, SpanOutcome::Ok);
        apply_cli_render_options(&env, &cli);

        // POSIX mode: evaluate via fshell-posix engine
        if cli.posix {
            let mut timing = trace_span(&trace, "command.posix_execute", TraceMode::Posix);
            let parsed = fshell_posix::parser::parse_posix_script(cmd).unwrap_or_else(|e| {
                eprintln!("POSIX parse error: {e}");
                exit_with_trace(&trace, 2, TraceMode::Posix, SpanOutcome::Error);
            });
            let code = fshell_posix::eval::eval_source(
                &parsed,
                &env,
                &fshell_posix::eval::EvalConfig::default(),
            )
            .await
            .unwrap_or_else(|e| {
                eprintln!("POSIX execution error: {e}");
                exit_with_trace(&trace, 1, TraceMode::Posix, SpanOutcome::Error);
            });
            finish_span(&mut timing, SpanOutcome::Ok);
            exit_with_trace(&trace, code, TraceMode::Posix, SpanOutcome::Exit);
        }

        let mut timing = trace.command_span(
            trace.root_context(),
            "command",
            TraceMode::Command,
            serde_json::Map::new(),
        );
        let command_env = if let Some(span) = &timing {
            env.with_trace_context(span.context())
        } else {
            env.clone()
        };
        match fshell_engine::run_script(cmd, &command_env).await {
            Ok(Flow::Exit(code)) => {
                finish_span(&mut timing, SpanOutcome::Exit);
                exit_with_trace(&trace, code, TraceMode::Command, SpanOutcome::Exit)
            }
            Ok(Flow::Break) | Ok(Flow::Continue) | Ok(Flow::Return(_)) => {
                eprintln!("error: stray control flow at top level");
                finish_span(&mut timing, SpanOutcome::Error);
                exit_with_trace(&trace, 1, TraceMode::Command, SpanOutcome::Error);
            }
            Ok(_) => {
                let code = env.exit_code() as i32;
                finish_span(&mut timing, SpanOutcome::Ok);
                exit_with_trace(&trace, code, TraceMode::Command, SpanOutcome::Exit);
            }
            Err(e) => {
                finish_span(&mut timing, SpanOutcome::Error);
                render_and_exit(e, cmd, "command", &env)
            }
        }
    }

    // Full initialization for REPL and scripts
    let boot_profiler = Arc::new(fshell_core::RwLock::new(ProfilerState::new(true)));
    {
        let _g = ProfilerState::guard(&boot_profiler, "core init", ProfilerCategory::Init);
        let mut timing = trace_span(&trace, "startup.core_init", TraceMode::Startup);
        fshell_core::init();
        finish_span(&mut timing, SpanOutcome::Ok);
    }
    {
        let _g = ProfilerState::guard(&boot_profiler, "caps init", ProfilerCategory::Init);
        let mut timing = trace_span(&trace, "startup.capabilities_init", TraceMode::Startup);
        fshell_capabilities::init();
        finish_span(&mut timing, SpanOutcome::Ok);
    }

    let mut timing = trace_span(&trace, "startup.env_init", TraceMode::Startup);
    let env = fshell_engine::Env::new_with_trace(trace.clone());
    finish_span(&mut timing, SpanOutcome::Ok);

    // Merge boot profiler entries into env's profiler
    {
        let mut target = env.profiler.write();
        let mut source = boot_profiler.write();
        target.merge(&mut source);
    }

    {
        let _g = ProfilerState::guard(&env.profiler, "builtins init", ProfilerCategory::Init);
        let mut timing = trace_span(&trace, "startup.builtins_init", TraceMode::Startup);
        fshell_builtins::init(&env);
        finish_span(&mut timing, SpanOutcome::Ok);
    }
    {
        let _g = ProfilerState::guard(&env.profiler, "bridge init", ProfilerCategory::Init);
        let mut timing = trace_span(&trace, "startup.bridge_init", TraceMode::Startup);
        fshell_bridge::init(&env);
        finish_span(&mut timing, SpanOutcome::Ok);
    }
    init_posix_handler();

    if let Some(ref handoff_path) = cli.handoff {
        let path = std::path::Path::new(handoff_path);
        match fshell_engine::handoff::load_handoff(path) {
            Ok(state) => {
                restore_handoff_state(&env, state);
            }
            Err(e) => {
                eprintln!("\x1b[1;33mWarning: Handoff state incompatible — starting fresh.\x1b[0m");
                eprintln!("  {e}");
            }
        }
    }

    if cli.strict {
        let mut caps = env.caps.caps.write();
        caps.strict_mode = true;
        caps.held.clear();
    }

    // For the REPL path the login environment is loaded by ftui's
    // login-aware init (see fshell-repl/src/lib.rs).  For the script
    // path we must still bump $SHLVL and set $FSH_LOGIN now — scripts
    // may inspect them and `load_config_script` below may need
    // SHLVL in env.  No sourcing here: scripts are non-interactive
    // unless explicitly `--login`.
    //
    // Kept in one place (`login::bump_shlvl` + FSH_LOGIN) so semantics
    // match the REPL.  The REPL will see the same values because it
    // runs with the pre-populated env.
    let env = if cli.script.is_some() {
        env.with_trace_mode(TraceMode::Script)
    } else {
        env.with_trace_mode(TraceMode::Interactive)
    };
    if cli.script.is_some() {
        if fshell_engine::login::is_interactive() {
            fshell_engine::login::bump_shlvl(&env);
        }
        {
            let mut vars = env.vars.write();
            vars.insert("FSH_LOGIN".to_string(), Val::Bool(is_login));
        }
        // Non-interactive `fsh script.fsh --login` should source login
        // profiles before the script runs.  Best-effort: if this fails
        // the script still runs with host env.
        if is_login {
            let mut timing = trace_span(&trace, "startup.login_environment", env.trace_mode);
            let result = fshell_engine::login::load_login_environment(&env, true, false).await;
            finish_span(
                &mut timing,
                if result.is_ok() {
                    SpanOutcome::Ok
                } else {
                    SpanOutcome::Error
                },
            );
        }
    } else if !is_login {
        // Non-login REPL will do its own login env loading inside
        // ftui's init — but set FSH_LOGIN now so early code (handoff
        // etc.) can inspect it.
        {
            let mut vars = env.vars.write();
            vars.insert("FSH_LOGIN".to_string(), Val::Bool(false));
        }
    } else {
        let mut vars = env.vars.write();
        vars.insert("FSH_LOGIN".to_string(), Val::Bool(true));
    }

    let mut timing = trace_span(&trace, "startup.host_environment", env.trace_mode);
    fshell_engine::populate_env_from_host(&env);
    finish_span(&mut timing, SpanOutcome::Ok);

    let mut timing = trace_span(&trace, "startup.path_warmup", env.trace_mode);
    fshell_engine::warmup_path_cache(Some(&env));
    finish_span(&mut timing, SpanOutcome::Ok);

    apply_cli_render_options(&env, &cli);

    if let Some(script_path) = &cli.script {
        {
            let mut vars = env.vars.write();
            vars.insert("0".to_string(), Val::String(script_path.clone()));
            for (i, arg) in cli.args.iter().enumerate() {
                vars.insert((i + 1).to_string(), Val::String(arg.clone()));
            }
            vars.insert(
                "@".to_string(),
                Val::List(cli.args.iter().map(|a| Val::String(a.clone())).collect()),
            );
            vars.insert(
                "*".to_string(),
                Val::List(cli.args.iter().map(|a| Val::String(a.clone())).collect()),
            );
            vars.insert("#".to_string(), Val::Int(cli.args.len() as i64));
        }
        let mut command_span = trace.command_span(
            trace.root_context(),
            "command",
            TraceMode::Script,
            serde_json::Map::new(),
        );
        let command_env = if let Some(span) = &command_span {
            env.with_trace_context(span.context())
        } else {
            env.clone()
        };
        let mut read_span = command_env.trace.span(
            command_env.trace_context,
            "script.read",
            TraceMode::Script,
            serde_json::Map::new(),
        );
        let script_content = std::fs::read_to_string(script_path);
        finish_span(
            &mut read_span,
            if script_content.is_ok() {
                SpanOutcome::Ok
            } else {
                SpanOutcome::Error
            },
        );
        match script_content {
            Ok(content) => {
                // POSIX file dispatch: shebang auto-detect or --posix flag
                let use_posix = cli.posix || fshell_posix::parser::is_posix_shebang(&content);
                if use_posix {
                    let mut timing = trace_span(&trace, "script.posix_parse", TraceMode::Posix);
                    match fshell_posix::parser::parse_posix_script(&content) {
                        Ok(parsed) => {
                            finish_span(&mut timing, SpanOutcome::Ok);
                            let cfg = fshell_posix::eval::EvalConfig {
                                positional: cli.args.clone(),
                                ..Default::default()
                            };
                            let mut execution =
                                trace_span(&trace, "script.posix_execute", TraceMode::Posix);
                            let code = fshell_posix::eval::eval_source(&parsed, &command_env, &cfg)
                                .await
                                .unwrap_or_else(|e| {
                                    eprintln!("POSIX execution error: {e}");
                                    finish_span(&mut command_span, SpanOutcome::Error);
                                    exit_with_trace(
                                        &trace,
                                        1,
                                        TraceMode::Posix,
                                        SpanOutcome::Error,
                                    );
                                });
                            finish_span(&mut execution, SpanOutcome::Ok);
                            finish_span(&mut command_span, SpanOutcome::Ok);
                            exit_with_trace(&trace, code, TraceMode::Posix, SpanOutcome::Exit);
                        }
                        Err(e) => {
                            eprintln!("POSIX parse error in '{script_path}': {e}");
                            finish_span(&mut timing, SpanOutcome::Error);
                            exit_with_trace(&trace, 2, TraceMode::Posix, SpanOutcome::Error);
                        }
                    }
                }
                let mut timing = trace_span(&trace, "script.execute", TraceMode::Script);
                match fshell_engine::run_script(&content, &command_env).await {
                    Ok(Flow::Exit(code)) => {
                        finish_span(&mut timing, SpanOutcome::Exit);
                        finish_span(&mut command_span, SpanOutcome::Exit);
                        exit_with_trace(&trace, code, TraceMode::Script, SpanOutcome::Exit)
                    }
                    Ok(Flow::Break) | Ok(Flow::Continue) | Ok(Flow::Return(_)) => {
                        eprintln!("error: stray control flow at top level in '{script_path}'");
                        finish_span(&mut timing, SpanOutcome::Error);
                        finish_span(&mut command_span, SpanOutcome::Error);
                        exit_with_trace(&trace, 1, TraceMode::Script, SpanOutcome::Error);
                    }
                    Ok(_) => {
                        let code = env.exit_code() as i32;
                        finish_span(&mut timing, SpanOutcome::Ok);
                        finish_span(&mut command_span, SpanOutcome::Ok);
                        exit_with_trace(&trace, code, TraceMode::Script, SpanOutcome::Exit);
                    }
                    Err(e) => {
                        finish_span(&mut timing, SpanOutcome::Error);
                        finish_span(&mut command_span, SpanOutcome::Error);
                        render_and_exit(e, &content, script_path, &env)
                    }
                }
            }
            Err(e) => {
                eprintln!("Error reading script '{}': {}", script_path, e);
                finish_span(&mut command_span, SpanOutcome::Error);
                exit_with_trace(&trace, 1, TraceMode::Script, SpanOutcome::Error);
            }
        }
    } else {
        let mut timing = trace_span(
            &trace,
            "startup.history_session_init",
            TraceMode::Interactive,
        );
        fshell_repl::init(&env);
        finish_span(&mut timing, SpanOutcome::Ok);
        fshell_repl::run_repl_with_env(env, cli.resume).await;
        let _ = crossterm::execute!(std::io::stdout(), crossterm::cursor::EnableBlinking);
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

fn restore_handoff_state(env: &fshell_engine::Env, state: fshell_engine::handoff::HandoffState) {
    {
        let mut vars = env.vars.write();
        for (k, v) in state.vars {
            // Never restore a stale build stamp from handoff — the new process's
            // exe_path/build is canonical. Handoff may be from a different binary location.
            if k == "FSH_EXE"
                || k == "FSH_VERSION"
                || k == "FSH_FULL_VERSION"
                || k == "FSH_BUILD_DATETIME"
                || k == "FSH_BUILD_DATETIME_ISO"
                || k == "FSH_BUILD_TIMESTAMP"
                || k == "FSH_GIT_COMMIT"
            {
                continue;
            }
            vars.insert(k, v);
        }
        vars.insert("FSH_SESSION_ID".to_string(), Val::String(state.session_id));
        vars.insert("FSH_HANDOFF".to_string(), Val::Bool(true));
        // Re-assert current exe/version so scripts see the live binary, not the handoff's
        vars.insert(
            "FSH_EXE".to_string(),
            Val::String(env.exe_path.to_string_lossy().to_string()),
        );
        vars.insert(
            "FSH_VERSION".to_string(),
            Val::String(fshell_engine::exe::version().to_string()),
        );
        vars.insert(
            "FSH_FULL_VERSION".to_string(),
            Val::String(fshell_engine::exe::full_version()),
        );
        if let Some(dt) = fshell_engine::exe::build_datetime() {
            vars.insert(
                "FSH_BUILD_DATETIME".to_string(),
                Val::String(dt.to_string()),
            );
        }
        if let Some(iso) = fshell_engine::exe::build_datetime_iso() {
            vars.insert(
                "FSH_BUILD_DATETIME_ISO".to_string(),
                Val::String(iso.to_string()),
            );
        }
        if let Some(commit) = fshell_engine::exe::git_commit() {
            vars.insert(
                "FSH_GIT_COMMIT".to_string(),
                Val::String(commit.to_string()),
            );
        }
    }
    {
        let mut fns = env.fns.write();
        for (k, v) in state.fns {
            fns.insert(k, v);
        }
    }
    {
        let mut caps = env.caps.caps.write();
        caps.held = state.caps_held;
        caps.strict_mode = state.caps_strict_mode;
    }
    {
        let mut pipes = env.reactive.pipelines.write();
        for (k, v) in state.reactive_pipelines {
            pipes.insert(k, v);
        }
    }
    env.set_cwd(std::path::PathBuf::from(&state.cwd));
    {
        let mut opts = env.options.write();
        *opts = state.options;
    }
    {
        let mut hooks = env.hooks.registry.write();
        for (k, v) in state.hooks {
            hooks.insert(k, v);
        }
    }
    env.set_published_exit_code(state.last_exit_code);
    {
        let mut dur = env.prompt.last_duration.write();
        *dur = std::time::Duration::from_secs_f64(state.last_duration_secs);
    }
}

fn render_and_exit(e: EngineError, input: &str, src_path: &str, env: &fshell_engine::Env) -> ! {
    let config = {
        let opts = env.options.read();
        fshell_render::RenderConfig {
            format: opts.error_format,
            color: opts.error_color,
            is_interactive: false,
        }
    };
    let diag = FshDiag::new(e);
    let err_str = fshell_render::render(diag, Some(input), src_path, &config);
    eprintln!("{}", err_str);
    exit_with_trace(&env.trace, 1, env.trace_mode, SpanOutcome::Error);
}

/// Entry point for `fsh` binary (called before any tokio runtime exists).
/// Creates its own current_thread runtime and drives the utility to completion.
pub fn run_utility(name: &str, args: &[String]) -> ! {
    run_utility_with_trace(name, args, TraceSink::from_env())
}

pub fn run_utility_with_trace(name: &str, args: &[String], trace: Arc<TraceSink>) -> ! {
    let runtime_span = trace_span(&trace, "process.runtime_init", TraceMode::Utility);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build utility runtime");
    let mut runtime_span = runtime_span;
    finish_span(&mut runtime_span, SpanOutcome::Ok);

    let exit_code = runtime.block_on(run_utility_inner(name, args, trace.clone()));
    exit_with_trace(&trace, exit_code, TraceMode::Utility, SpanOutcome::Exit);
}

/// Core utility logic — runs within any existing tokio runtime.
/// Returns the exit code (caller is responsible for process::exit).
async fn run_utility_inner(name: &str, args: &[String], trace: Arc<TraceSink>) -> i32 {
    let mut route = trace_span(&trace, "utility.execute", TraceMode::Utility);
    match name {
        "ls" => {
            let result = run_ls_utility(args, trace).await;
            finish_span(&mut route, SpanOutcome::Ok);
            result
        }
        _ => {
            eprintln!("fsh: '{name}' is not available as a standalone utility");
            finish_span(&mut route, SpanOutcome::Error);
            1
        }
    }
}

async fn run_ls_utility(args: &[String], trace: Arc<TraceSink>) -> i32 {
    let mut env = fshell_engine::Env::for_command_with_trace(trace);
    env.trace_mode = TraceMode::Utility;
    env.is_last_stage = true;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<PipelinePayload>(32);
    let converted: Vec<Val> = args.iter().map(|s| Val::String(s.clone())).collect();
    let color_always = args.iter().any(|a| a == "--color=always");

    let consumer = tokio::spawn(async move {
        while let Some(payload) = rx.recv().await {
            match payload {
                PipelinePayload::Data(v) => {
                    let text = v.to_text();
                    if !std::io::stdout().is_terminal() && !color_always {
                        let clean = strip_ansi_escapes::strip_str(&text);
                        println!("{}", clean);
                    } else {
                        println!("{}", text);
                    }
                }
                PipelinePayload::Bytes(b) => {
                    let text = String::from_utf8_lossy(&b).into_owned();
                    println!("{}", text);
                }
                PipelinePayload::Structured(d) => {
                    eprintln!("{}", d.report);
                }
            }
        }
    });

    match fshell_builtins::ls_builtin(None, converted, &env, tx, None) {
        Ok(_) => match consumer.await {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("fsh: ls output consumer failed: {e}");
                1
            }
        },
        Err(e) => {
            eprintln!("ls: {}", e.message);
            1
        }
    }
}

pub fn setup_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::cursor::EnableBlinking);
        let _ = crossterm::terminal::disable_raw_mode();
        if std::env::var("RUST_BACKTRACE").is_ok() {
            default_hook(info);
        } else {
            eprintln!("\n\x1b[1;31merror:\x1b[0m fshell encountered an internal panic:");
            if let Some(s) = info.payload().downcast_ref::<&str>() {
                eprintln!("  {}", s);
            } else if let Some(s) = info.payload().downcast_ref::<String>() {
                eprintln!("  {}", s);
            } else {
                eprintln!("  Unknown panic reason");
            }
            if let Some(location) = info.location() {
                eprintln!(
                    "  Location: {}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                );
            }
        }
    }));
}

fn init_posix_handler() {
    fshell_engine::register_posix_handler(
        |content: String, args: Vec<String>, env: fshell_engine::Env, capture: bool| async move {
            let parsed = fshell_posix::parser::parse_posix_script(&content)?;
            let cfg = fshell_posix::eval::EvalConfig {
                positional: args,
                ..Default::default()
            };
            fshell_posix::eval::eval_source_stream(&parsed, &env, &cfg, capture).await
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn test_cli_help_includes_hints() {
        let mut cmd = Cli::command();
        let mut buf = Vec::new();
        cmd.write_help(&mut buf).unwrap();
        let help_text = String::from_utf8(buf).unwrap();
        assert!(
            help_text.contains("For builtins: try `help <name>`"),
            "Help text should contain builtin hint"
        );
        assert!(
            help_text.contains("For language reference: `man fsh`"),
            "Help text should contain language reference hint"
        );
        assert!(
            help_text.contains("For migration from bash/zsh/fish: see docs/MIGRATION.md"),
            "Help text should contain migration guide hint"
        );
    }
}
