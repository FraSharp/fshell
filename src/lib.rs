// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use clap::Parser;
use fshell_core::Val;
use fshell_core::diagnostic::FshDiag;
use fshell_engine::profiler::{ProfilerCategory, ProfilerState};
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

pub async fn run() {
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
        raw_program_name.trim_start_matches('-').to_string()
    } else {
        raw_program_name.clone()
    };

    if program_name != "fsh" && program_name != "fshell" {
        let args: Vec<String> = std::env::args().skip(1).collect();
        // Run utility logic within the existing tokio runtime (avoid creating
        // a nested runtime, which panics).  `run_utility_inner` returns the
        // exit code — exit the process immediately.
        let exit_code = run_utility_inner(&program_name, &args).await;
        std::process::exit(exit_code);
    }

    let cli = Cli::parse();
    let is_login = fshell_engine::login::detect_login(&raw_program_name, cli.login);

    // Command mode: lightweight startup — skip handoff, strict, reactive, etc.
    if let Some(cmd) = &cli.command {
        if cmd.trim().is_empty() {
            std::process::exit(0);
        }

        let boot_profiler = Arc::new(fshell_core::RwLock::new(ProfilerState::new(true)));
        {
            let _g = ProfilerState::guard(&boot_profiler, "core init", ProfilerCategory::Init);
            fshell_core::init();
        }
        {
            let _g = ProfilerState::guard(&boot_profiler, "caps init", ProfilerCategory::Init);
            fshell_capabilities::init();
        }

        let env = fshell_engine::Env::for_command();

        // Merge boot profiler entries into env's profiler
        {
            let mut target = env.profiler.write();
            let mut source = boot_profiler.write();
            target.merge(&mut source);
        }

        {
            let _g = ProfilerState::guard(&env.profiler, "builtins init", ProfilerCategory::Init);
            fshell_builtins::init(&env);
        }
        {
            let _g = ProfilerState::guard(&env.profiler, "bridge init", ProfilerCategory::Init);
            fshell_bridge::init(&env);
        }
        init_posix_handler();
        fshell_engine::setup_signal_handlers(env.clone());
        if !cmd.trim().is_empty() {
            let _g = ProfilerState::guard(&env.profiler, "populate env", ProfilerCategory::Init);
            fshell_engine::populate_env_from_host(&env);
        }

        // Login / interactive semantics: even `fsh -c` can be run as
        // `fsh --login -c '...'` (e.g. via `su -` or `ssh host cmd`).
        // In that case the host login profiles must be visible to the
        // one-shot command — otherwise `$PATH` from ~/.zprofile is missing.
        let is_interactive_cmd = false; // `-c` is always non-interactive
        let _ =
            fshell_engine::login::load_login_environment(&env, is_login, is_interactive_cmd).await;

        fshell_engine::warmup_path_cache(Some(&env));
        apply_cli_render_options(&env, &cli);

        // POSIX mode: evaluate via fshell-posix engine
        if cli.posix {
            let parsed = fshell_posix::parser::parse_posix_script(cmd).unwrap_or_else(|e| {
                eprintln!("POSIX parse error: {e}");
                std::process::exit(2);
            });
            let code = fshell_posix::eval::eval_source(
                &parsed,
                &env,
                &fshell_posix::eval::EvalConfig::default(),
            )
            .await
            .unwrap_or_else(|e| {
                eprintln!("POSIX execution error: {e}");
                std::process::exit(1);
            });
            std::process::exit(code);
        }

        match fshell_engine::run_script(cmd, &env).await {
            Ok(Flow::Exit(code)) => std::process::exit(code),
            Ok(Flow::Break) | Ok(Flow::Continue) | Ok(Flow::Return(_)) => {
                eprintln!("error: stray control flow at top level");
                std::process::exit(1);
            }
            Ok(_) => {
                let code = *env.prompt.last_exit_code.read() as i32;
                std::process::exit(code);
            }
            Err(e) => render_and_exit(e, cmd, "command", &env),
        }
    }

    // Full initialization for REPL and scripts
    let boot_profiler = Arc::new(fshell_core::RwLock::new(ProfilerState::new(true)));
    {
        let _g = ProfilerState::guard(&boot_profiler, "core init", ProfilerCategory::Init);
        fshell_core::init();
    }
    {
        let _g = ProfilerState::guard(&boot_profiler, "caps init", ProfilerCategory::Init);
        fshell_capabilities::init();
    }

    let env = fshell_engine::Env::new();

    // Merge boot profiler entries into env's profiler
    {
        let mut target = env.profiler.write();
        let mut source = boot_profiler.write();
        target.merge(&mut source);
    }

    {
        let _g = ProfilerState::guard(&env.profiler, "builtins init", ProfilerCategory::Init);
        fshell_builtins::init(&env);
    }
    {
        let _g = ProfilerState::guard(&env.profiler, "bridge init", ProfilerCategory::Init);
        fshell_bridge::init(&env);
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
            let _ = fshell_engine::login::load_login_environment(&env, true, false).await;
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

    fshell_engine::populate_env_from_host(&env);

    fshell_engine::warmup_path_cache(Some(&env));

    apply_cli_render_options(&env, &cli);

    if let Some(script_path) = &cli.script {
        match std::fs::read_to_string(script_path) {
            Ok(content) => {
                // POSIX file dispatch: shebang auto-detect or --posix flag
                let use_posix = cli.posix || fshell_posix::parser::is_posix_shebang(&content);
                if use_posix {
                    match fshell_posix::parser::parse_posix_script(&content) {
                        Ok(parsed) => {
                            let code = fshell_posix::eval::eval_source(
                                &parsed,
                                &env,
                                &fshell_posix::eval::EvalConfig::default(),
                            )
                            .await
                            .unwrap_or_else(|e| {
                                eprintln!("POSIX execution error: {e}");
                                std::process::exit(1);
                            });
                            std::process::exit(code);
                        }
                        Err(e) => {
                            eprintln!("POSIX parse error in '{script_path}': {e}");
                            std::process::exit(2);
                        }
                    }
                }
                match fshell_engine::run_script(&content, &env).await {
                    Ok(Flow::Exit(code)) => std::process::exit(code),
                    Ok(Flow::Break) | Ok(Flow::Continue) | Ok(Flow::Return(_)) => {
                        eprintln!("error: stray control flow at top level in '{script_path}'");
                        std::process::exit(1);
                    }
                    Ok(_) => {
                        let code = *env.prompt.last_exit_code.read() as i32;
                        std::process::exit(code);
                    }
                    Err(e) => render_and_exit(e, &content, script_path, &env),
                }
            }
            Err(e) => {
                eprintln!("Error reading script '{}': {}", script_path, e);
                std::process::exit(1);
            }
        }
    } else {
        fshell_repl::init(&env);
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
    let _ = std::env::set_current_dir(&state.cwd);
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
    {
        let mut code = env.prompt.last_exit_code.write();
        *code = state.last_exit_code;
    }
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
    std::process::exit(1);
}

/// Entry point for `fsh` binary (called before any tokio runtime exists).
/// Creates its own current_thread runtime and drives the utility to completion.
pub fn run_utility(name: &str, args: &[String]) -> ! {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build utility runtime");

    let exit_code = runtime.block_on(run_utility_inner(name, args));
    std::process::exit(exit_code);
}

/// Core utility logic — runs within any existing tokio runtime.
/// Returns the exit code (caller is responsible for process::exit).
async fn run_utility_inner(name: &str, args: &[String]) -> i32 {
    match name {
        "ls" => run_ls_utility(args).await,
        _ => {
            eprintln!("fsh: '{name}' is not available as a standalone utility");
            1
        }
    }
}

async fn run_ls_utility(args: &[String]) -> i32 {
    let mut env = fshell_engine::Env::for_command();
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
        Ok(_) => {
            consumer.await.ok();
            0
        }
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
