// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::error::BuiltinError;
use crate::utils::{get_home_dir, interpret_ansi_escapes, val_to_display_string};
use fshell_core::Val;
use fshell_core::diagnostic::StringError;
use fshell_engine::{CapAction, Env, PipeSender, PipeStream, PipelinePayload};
use nu_ansi_term::Color;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

fn find_workspace_root() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("FSHELL_SOURCE") {
        let p = PathBuf::from(path);
        if p.join("Cargo.toml").exists() {
            return Some(p);
        }
    }

    let mut dir = std::env::current_exe().ok()?;
    while dir.pop() {
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.exists() {
            let contains_fshell = std::fs::read_to_string(&cargo_toml)
                .map(|content| content.contains("fshell"))
                .unwrap_or(false);
            if contains_fshell {
                return Some(dir);
            }
        }
    }

    option_env!("FSHELL_WORKSPACE_ROOT")
        .filter(|p| PathBuf::from(p).join("Cargo.toml").exists())
        .map(PathBuf::from)
}

fn clone_workspace_root() -> Result<PathBuf, StringError> {
    let repo_url = option_env!("FSHELL_REPO_URL").unwrap_or("https://github.com/FraSharp/fshell");

    let home = get_home_dir().ok_or("Could not determine home directory")?;
    let cache_dir = home.join(".cache").join("fsh").join("src");

    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| format!("Failed to create cache dir {}: {e}", cache_dir.display()))?;

    let cargo_toml = cache_dir.join("Cargo.toml");
    if cargo_toml.exists() {
        let status = std::process::Command::new("git")
            .args(["pull", "--ff-only"])
            .current_dir(&cache_dir)
            .status()
            .map_err(|e| format!("Failed to run git pull: {e}"))?;
        if !status.success() {
            return Err(BuiltinError::CommandFailed {
                cmd: "git".into(),
                status: status.code().unwrap_or(-1),
                stderr: "git pull failed".into(),
            }
            .into());
        }
    } else {
        let status = std::process::Command::new("git")
            .args(["clone", "--depth", "1", repo_url, "."])
            .current_dir(&cache_dir)
            .status()
            .map_err(|e| format!("Failed to run git clone: {e}"))?;
        if !status.success() {
            return Err(BuiltinError::CommandFailed {
                cmd: "git".into(),
                status: status.code().unwrap_or(-1),
                stderr: "git clone failed".into(),
            }
            .into());
        }
    }

    Ok(cache_dir)
}

pub fn reload_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    _tx: PipeSender,
) -> Result<(), StringError> {
    let is_build_debug = args.iter().any(|arg| match arg {
        Val::String(s) => s == "--build-debug" || s == "-bd",
        _ => false,
    });
    let is_build_release = args.iter().any(|arg| match arg {
        Val::String(s) => s == "--build-release" || s == "-br",
        _ => false,
    });
    let is_build = is_build_debug
        || is_build_release
        || args.iter().any(|arg| match arg {
            Val::String(s) => s == "--build" || s == "-b",
            _ => false,
        });

    let is_full = is_build
        || args.iter().any(|arg| match arg {
            Val::String(s) => s == "--full",
            _ => false,
        });

    if !is_full {
        let env_clone = env.clone();
        tokio::spawn(async move {
            match fshell_engine::load_config_script(&env_clone).await {
                Ok(()) => {
                    println!("  {}  configuration re-sourced", Color::Green.paint("[ok]"));
                }
                Err(e) => {
                    eprintln!("\x1b[1;31mreload error:\x1b[0m {e}");
                    println!("  {}  config load failed", Color::Red.paint("[!]"));
                }
            }
        });
        return Ok(());
    }

    if is_build {
        let workspace_root = match find_workspace_root() {
            Some(root) => root,
            None => clone_workspace_root()?,
        };

        let exe_str = env.exe_path.to_string_lossy();
        let is_release = if is_build_debug {
            false
        } else if is_build_release {
            true
        } else {
            exe_str.contains("/release/")
        };
        let profile = if is_release { "release" } else { "debug" };

        eprintln!(
            "  {}  rebuilding fshell ({profile})...",
            Color::Cyan.paint("•")
        );

        let mut cmd = std::process::Command::new("cargo");
        cmd.args(["build"]).current_dir(&workspace_root);
        if is_release {
            cmd.arg("--release");
        }
        cmd.stdout(std::process::Stdio::inherit());
        cmd.stderr(std::process::Stdio::inherit());

        let start = Instant::now();
        let build_result = match cmd.status() {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(format!(
                "compilation failed (exit code {})",
                status.code().unwrap_or(-1)
            )),
            Err(e) => Err(format!("failed to execute cargo build: {e}")),
        };
        let elapsed = start.elapsed();

        match build_result {
            Ok(()) => {
                println!(
                    "  {}  rebuilt fshell ({profile}, {:.1}s)",
                    Color::Green.paint("[ok]"),
                    elapsed.as_secs_f64()
                );
            }
            Err(e) => {
                eprintln!("  {}  build failed", Color::Red.paint("[!]"));
                return Err(BuiltinError::InternalError {
                    cmd: "reload".into(),
                    message: e.to_string(),
                    span: None,
                }
                .into());
            }
        }
    }

    let caps_guard = env.caps.caps.read();
    let vars_guard = env.vars.read();
    let fns_guard = env.fns.read();
    let reactive_guard = env.reactive.pipelines.read();
    let options_guard = env.options.read();
    let hooks_guard = env.hooks.registry.read();
    let exit_code_guard = env.prompt.last_exit_code.read();
    let duration_guard = env.prompt.last_duration.read();

    let state = fshell_engine::handoff::HandoffState {
        vars: vars_guard.clone(),
        fns: fns_guard.clone(),
        caps_held: caps_guard.held.clone(),
        caps_strict_mode: caps_guard.strict_mode,
        reactive_pipelines: reactive_guard.clone(),
        session_id: vars_guard
            .get("FSH_SESSION_ID")
            .and_then(|v| {
                if let Val::String(s) = v {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "unknown".to_string()),
        cwd: std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "/".to_string()),
        options: options_guard.clone(),
        hooks: hooks_guard.clone(),
        last_exit_code: *exit_code_guard,
        last_duration_secs: duration_guard.as_secs_f64(),
    };

    drop(duration_guard);
    drop(exit_code_guard);
    drop(hooks_guard);
    drop(options_guard);
    drop(reactive_guard);
    drop(fns_guard);
    drop(vars_guard);
    drop(caps_guard);

    let handoff_path = fshell_engine::handoff::save_handoff(&state)?;
    let exe_str = env.exe_path.to_string_lossy().to_string();
    let handoff_str = handoff_path.to_string_lossy().to_string();

    let c_exe =
        std::ffi::CString::new(exe_str.as_bytes()).map_err(|_| "Null byte in executable path")?;
    let c_flag = std::ffi::CString::new("--handoff".as_bytes())
        .map_err(|_| "Null byte in --handoff flag")?;
    let c_path =
        std::ffi::CString::new(handoff_str.as_bytes()).map_err(|_| "Null byte in handoff path")?;

    let argv: Vec<*const libc::c_char> = vec![
        c_exe.as_ptr(),
        c_flag.as_ptr(),
        c_path.as_ptr(),
        std::ptr::null(),
    ];

    println!("  {}  restarting...", Color::Cyan.paint("->"));

    {
        let jobs = env.job_control.jobs.read();
        for job in jobs.values() {
            for &pid in &job.pids {
                // SAFETY: SIGTERM is sent to children to clean them up before process replacement
                // to avoid leaving zombie or orphaned background processes.
                unsafe { libc::kill(pid, libc::SIGTERM) };
            }
        }
    }
    // Sleep for 2 seconds to allow background processes to terminate and let the
    // background job waiter tasks clean them up. The jobs read lock was dropped
    // above, so background thread tasks are free to progress during this sleep.
    std::thread::sleep(std::time::Duration::from_secs(2));

    fshell_engine::suspend_session_logging();
    // SAFETY: execvp replaces the current process image. It is safe because we have
    // stopped the session logger and cleaned up active child processes. If it fails,
    // we return the OS error back to the caller.
    unsafe {
        libc::execvp(c_exe.as_ptr(), argv.as_ptr());
    }

    Err(format!("execvp failed: {}", std::io::Error::last_os_error()).into())
}

pub fn which_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), StringError> {
    if args.is_empty() {
        return Err("which: missing argument".to_string().into());
    }

    let mut paths = Vec::new();
    let current_path = Some(env.vars.read())
        .and_then(|vars| {
            vars.get("PATH").and_then(|v| {
                if let Val::String(s) = v {
                    if s.is_empty() { None } else { Some(s.clone()) }
                } else {
                    None
                }
            })
        })
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();

    for arg in args {
        let name = match arg {
            Val::String(s) => s,
            other => other.to_text(),
        };

        let mut found = false;
        for dir in current_path.split(':') {
            let path = std::path::Path::new(dir).join(&name);
            if path.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(metadata) = path.metadata()
                        && metadata.permissions().mode() & 0o111 != 0
                    {
                        paths.push(path.to_string_lossy().to_string());
                        found = true;
                        break;
                    }
                }
                #[cfg(not(unix))]
                {
                    paths.push(path.to_string_lossy().to_string());
                    found = true;
                    break;
                }
            }
        }
        if !found {
            return Err(BuiltinError::NotFound {
                cmd: "which".into(),
                what: name.to_string(),
                span: None,
            }
            .into());
        }
    }

    tokio::spawn(async move {
        for p in paths {
            let _ = tx
                .send(PipelinePayload::Data(Arc::new(Val::String(p))))
                .await;
        }
    });

    Ok(())
}

pub fn caps_profile_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), StringError> {
    let mut curr =
        std::env::current_dir().map_err(|e| format!("Failed to get current dir: {e}"))?;
    let mut caps_yaml_path = None;
    let mut base_dir = None;

    loop {
        let path = curr.join(".fsh/caps.yaml");
        if path.is_file() {
            caps_yaml_path = Some(path);
            base_dir = Some(curr.clone());
            break;
        }
        if !curr.pop() {
            break;
        }
    }

    let yaml_path = match caps_yaml_path {
        Some(p) => p,
        None => return Err("caps: No local capability profile found at .fsh/caps.yaml in current or parent directories".to_string().into()),
    };

    let content = std::fs::read_to_string(&yaml_path)
        .map_err(|e| format!("Failed to read {}: {}", yaml_path.display(), e))?;

    let base = base_dir.ok_or_else(|| "caps: no base directory found".to_string())?;

    let yaml_val: serde_yaml::Value = serde_yaml::from_str(&content)
        .map_err(|e| format!("Invalid YAML profile format: {}", e))?;

    let profiles_map = yaml_val
        .get("profiles")
        .and_then(|v| v.as_mapping())
        .ok_or_else(|| "Missing or invalid 'profiles' map in caps.yaml".to_string())?;

    let available_profiles: Vec<String> = profiles_map
        .keys()
        .filter_map(|k| k.as_str())
        .map(|s| s.to_string())
        .collect();

    if args.is_empty() {
        let msg = format!(
            "Available capability profiles in {}:\n{}",
            yaml_path.display(),
            available_profiles
                .iter()
                .map(|p| format!("  - {}", p))
                .collect::<Vec<_>>()
                .join("\n")
        );
        tokio::spawn(async move {
            let _ = tx
                .send(PipelinePayload::Data(Arc::new(Val::String(msg))))
                .await;
        });
        return Ok(());
    }

    let target_profile = match &args[0] {
        Val::String(s) => s,
        _ => return Err("caps: profile name must be a string".to_string().into()),
    };

    let profile_handles =
        fshell_capabilities::CapsRegistry::load_profile_from_yaml(&content, &base, target_profile)?;

    let is_strict = {
        let caps = env.caps.caps.read();
        caps.strict_mode
    };

    if is_strict {
        println!(
            "[!] Profile '{}' is requesting the following capabilities:",
            target_profile
        );
        for h in &profile_handles {
            println!("   - {:?}", h);
        }

        print!("   Approve profile? [y] Yes  [n] No (Default): ");
        use std::io::Write;
        let _ = std::io::stdout().flush();

        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err()
            || !input.trim().eq_ignore_ascii_case("y")
        {
            return Err(BuiltinError::Cancelled { cmd: "caps".into() }.into());
        }
    }

    {
        let mut caps = env.caps.caps.write();
        for handle in profile_handles {
            caps.grant(handle);
        }
    }

    let success_msg = format!(
        "Capability profile '{}' successfully applied.",
        target_profile
    );
    tokio::spawn(async move {
        let _ = tx
            .send(PipelinePayload::Data(Arc::new(Val::String(success_msg))))
            .await;
    });

    Ok(())
}

struct TestParser<'a> {
    args: &'a [Val],
    pos: usize,
    env: &'a Env,
}

impl<'a> TestParser<'a> {
    fn new(args: &'a [Val], env: &'a Env) -> Self {
        Self { args, pos: 0, env }
    }

    fn peek(&self) -> Option<&'a Val> {
        self.args.get(self.pos)
    }

    fn next(&mut self) -> Option<&'a Val> {
        if self.pos < self.args.len() {
            let res = &self.args[self.pos];
            self.pos += 1;
            Some(res)
        } else {
            None
        }
    }

    fn parse_or(&mut self) -> Result<bool, String> {
        let mut val = self.parse_and()?;
        while let Some(arg) = self.peek() {
            if val_to_display_string(arg) == "-o" {
                self.next();
                let right = self.parse_and()?;
                val = val || right;
            } else {
                break;
            }
        }
        Ok(val)
    }

    fn parse_and(&mut self) -> Result<bool, String> {
        let mut val = self.parse_primary()?;
        while let Some(arg) = self.peek() {
            if val_to_display_string(arg) == "-a" {
                self.next();
                let right = self.parse_primary()?;
                val = val && right;
            } else {
                break;
            }
        }
        Ok(val)
    }

    fn parse_primary(&mut self) -> Result<bool, String> {
        let Some(first) = self.next() else {
            return Ok(false);
        };

        let first_str = val_to_display_string(first);
        let first_ref = first_str.as_str();

        if first_ref == "(" {
            let val = self.parse_or()?;
            match self.next() {
                Some(p) if val_to_display_string(p) == ")" => {}
                _ => return Err("test: expected ')'".to_string()),
            }
            return Ok(val);
        }

        if first_ref == "!" {
            let val = self.parse_primary()?;
            return Ok(!val);
        }

        if first_ref.starts_with('-') && first_ref.len() == 2 {
            let path_val = self
                .next()
                .ok_or_else(|| format!("test: missing argument after {}", first_ref))?;
            let path_str = val_to_display_string(path_val);
            let path = std::path::Path::new(&path_str);
            match first_ref {
                "-e" => {
                    return Ok(path.exists());
                }
                "-f" => {
                    return Ok(path.is_file());
                }
                "-d" => {
                    return Ok(path.is_dir());
                }
                "-s" => {
                    return Ok(std::fs::metadata(path)
                        .map(|m| m.len() > 0)
                        .unwrap_or(false));
                }
                "-L" | "-h" => {
                    return Ok(std::fs::symlink_metadata(path)
                        .map(|m| m.file_type().is_symlink())
                        .unwrap_or(false));
                }
                "-r" => {
                    let has_cap = self
                        .env
                        .enforce_capability("test", CapAction::ReadFile(path.to_path_buf()))
                        .is_ok();
                    let is_readable = std::fs::metadata(path).is_ok();
                    return Ok(has_cap && is_readable);
                }
                "-w" => {
                    let has_cap = self
                        .env
                        .enforce_capability("test", CapAction::WriteFile(path.to_path_buf()))
                        .is_ok();
                    let is_writable = std::fs::metadata(path)
                        .map(|m| !m.permissions().readonly())
                        .unwrap_or(false);
                    return Ok(has_cap && is_writable);
                }
                "-x" => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let is_executable = std::fs::metadata(path)
                            .map(|m| m.permissions().mode() & 0o111 != 0)
                            .unwrap_or(false);
                        return Ok(is_executable);
                    }
                    #[cfg(not(unix))]
                    {
                        return Ok(path.exists());
                    }
                }
                "-z" => {
                    return Ok(path_str.is_empty());
                }
                "-n" => {
                    return Ok(!path_str.is_empty());
                }
                _ => {}
            }
        }

        if let Some(op) = self.peek() {
            let op_str = val_to_display_string(op);
            let op_ref = op_str.as_str();
            if op_ref == "="
                || op_ref == "=="
                || op_ref == "!="
                || op_ref == "-eq"
                || op_ref == "-ne"
                || op_ref == "-lt"
                || op_ref == "-le"
                || op_ref == "-gt"
                || op_ref == "-ge"
            {
                self.next();
                let second = self
                    .next()
                    .ok_or_else(|| format!("test: missing argument after {}", op_ref))?;
                let second_str = val_to_display_string(second);
                match op_ref {
                    "=" | "==" => {
                        return Ok(first_str == second_str);
                    }
                    "!=" => {
                        return Ok(first_str != second_str);
                    }
                    "-eq" => {
                        let a = parse_int_str(&first_str)?;
                        let b = parse_int_str(&second_str)?;
                        return Ok(a == b);
                    }
                    "-ne" => {
                        let a = parse_int_str(&first_str)?;
                        let b = parse_int_str(&second_str)?;
                        return Ok(a != b);
                    }
                    "-lt" => {
                        let a = parse_int_str(&first_str)?;
                        let b = parse_int_str(&second_str)?;
                        return Ok(a < b);
                    }
                    "-le" => {
                        let a = parse_int_str(&first_str)?;
                        let b = parse_int_str(&second_str)?;
                        return Ok(a <= b);
                    }
                    "-gt" => {
                        let a = parse_int_str(&first_str)?;
                        let b = parse_int_str(&second_str)?;
                        return Ok(a > b);
                    }
                    "-ge" => {
                        let a = parse_int_str(&first_str)?;
                        let b = parse_int_str(&second_str)?;
                        return Ok(a >= b);
                    }
                    _ => unreachable!(),
                }
            }
        }

        Ok(!first_str.is_empty())
    }
}

fn parse_int_str(s: &str) -> Result<i64, String> {
    s.parse::<i64>()
        .map_err(|_| format!("test: integer expression expected: '{}'", s))
}

pub fn test_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), StringError> {
    if args.is_empty() {
        tokio::spawn(async move {
            let _ = tx
                .send(PipelinePayload::Data(Arc::new(Val::Bool(false))))
                .await;
        });
        return Err(StringError::condition_false());
    }

    let mut parser = TestParser::new(&args, env);
    let res = parser.parse_or().map_err(StringError::from)?;

    if parser.pos < args.len() {
        return Err(BuiltinError::UnexpectedArgument {
            cmd: "test".into(),
            arg: val_to_display_string(&args[parser.pos]),
            span: None,
        }
        .into());
    }

    tokio::spawn(async move {
        let _ = tx
            .send(PipelinePayload::Data(Arc::new(Val::Bool(res))))
            .await;
    });

    if res {
        Ok(())
    } else {
        Err(StringError::condition_false())
    }
}

pub fn bracket_builtin(
    input: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), StringError> {
    if args.last().is_none_or(|a| val_to_display_string(a) != "]") {
        return Err("[: expected ']' to close bracket test".into());
    }
    let len = args.len();
    let inner_args: Vec<Val> = args.into_iter().take(len - 1).collect();
    test_builtin(input, inner_args, env, tx)
}

fn format_printf(format: &str, args: &[Val]) -> Result<String, String> {
    let mut result = String::new();
    let mut arg_idx = 0;
    let mut chars = format.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '%' {
            if chars.peek() == Some(&'%') {
                result.push('%');
                chars.next();
                continue;
            }

            let mut flag_left_align = false;
            let mut flag_show_sign = false;
            let mut flag_space_sign = false;
            let mut flag_zero_pad = false;
            let mut flag_alternate = false;

            while let Some(&next_c) = chars.peek() {
                match next_c {
                    '-' => {
                        flag_left_align = true;
                        chars.next();
                    }
                    '+' => {
                        flag_show_sign = true;
                        chars.next();
                    }
                    ' ' => {
                        flag_space_sign = true;
                        chars.next();
                    }
                    '0' => {
                        flag_zero_pad = true;
                        chars.next();
                    }
                    '#' => {
                        flag_alternate = true;
                        chars.next();
                    }
                    _ => break,
                }
            }

            let mut width: Option<usize> = None;
            let mut width_val = 0;
            let mut has_width = false;
            while let Some(&next_c) = chars.peek() {
                if let Some(digit) = next_c.to_digit(10) {
                    width_val = width_val * 10 + digit as usize;
                    has_width = true;
                    chars.next();
                } else {
                    break;
                }
            }
            if has_width {
                width = Some(width_val);
            }

            let mut precision: Option<usize> = None;
            if chars.peek() == Some(&'.') {
                chars.next();
                let mut prec_val = 0;
                let mut has_prec = false;
                while let Some(&next_c) = chars.peek() {
                    if let Some(digit) = next_c.to_digit(10) {
                        prec_val = prec_val * 10 + digit as usize;
                        has_prec = true;
                        chars.next();
                    } else {
                        break;
                    }
                }
                if has_prec {
                    precision = Some(prec_val);
                } else {
                    precision = Some(0);
                }
            }

            let spec_type = match chars.next() {
                Some(t) => t,
                None => return Err("printf: incomplete format specifier".to_string()),
            };

            let arg = if arg_idx < args.len() {
                &args[arg_idx]
            } else {
                &Val::Null
            };
            arg_idx += 1;

            let formatted = match spec_type {
                's' => {
                    let s = match arg {
                        Val::String(st) => st.clone(),
                        Val::Null => String::new(),
                        other => val_to_display_string(other),
                    };
                    let s_prec = if let Some(p) = precision {
                        if p < s.len() { s[..p].to_string() } else { s }
                    } else {
                        s
                    };
                    if let Some(w) = width {
                        if s_prec.len() < w {
                            let pad = " ".repeat(w - s_prec.len());
                            if flag_left_align {
                                format!("{}{}", s_prec, pad)
                            } else {
                                format!("{}{}", pad, s_prec)
                            }
                        } else {
                            s_prec
                        }
                    } else {
                        s_prec
                    }
                }
                'd' | 'i' => {
                    let val = match arg {
                        Val::Int(i) => *i,
                        Val::Float(f) => *f as i64,
                        Val::String(s) => s.parse().unwrap_or(0),
                        _ => 0,
                    };
                    let sign = if val < 0 {
                        "-"
                    } else if flag_show_sign {
                        "+"
                    } else if flag_space_sign {
                        " "
                    } else {
                        ""
                    };
                    let abs_val = val.abs();
                    let mut num_str = abs_val.to_string();
                    if let Some(p) = precision
                        && num_str.len() < p
                    {
                        num_str = format!("{}{}", "0".repeat(p - num_str.len()), num_str);
                    }
                    if let Some(w) = width {
                        let total_len = sign.len() + num_str.len();
                        if total_len < w {
                            let pad_len = w - total_len;
                            if flag_left_align {
                                format!("{}{}{}", sign, num_str, " ".repeat(pad_len))
                            } else if flag_zero_pad && precision.is_none() {
                                format!("{}{}{}", sign, "0".repeat(pad_len), num_str)
                            } else {
                                format!("{}{}{}", " ".repeat(pad_len), sign, num_str)
                            }
                        } else {
                            format!("{}{}", sign, num_str)
                        }
                    } else {
                        format!("{}{}", sign, num_str)
                    }
                }
                'x' | 'X' => {
                    let val = match arg {
                        Val::Int(i) => *i,
                        Val::Float(f) => *f as i64,
                        Val::String(s) => s.parse().unwrap_or(0),
                        _ => 0,
                    };
                    let mut num_str = if spec_type == 'x' {
                        format!("{:x}", val)
                    } else {
                        format!("{:X}", val)
                    };
                    if let Some(p) = precision
                        && num_str.len() < p
                    {
                        num_str = format!("{}{}", "0".repeat(p - num_str.len()), num_str);
                    }
                    let prefix = if flag_alternate && val != 0 {
                        if spec_type == 'x' { "0x" } else { "0X" }
                    } else {
                        ""
                    };
                    if let Some(w) = width {
                        let total_len = prefix.len() + num_str.len();
                        if total_len < w {
                            let pad_len = w - total_len;
                            if flag_left_align {
                                format!("{}{}{}", prefix, num_str, " ".repeat(pad_len))
                            } else if flag_zero_pad && precision.is_none() {
                                format!("{}{}{}", prefix, "0".repeat(pad_len), num_str)
                            } else {
                                format!("{}{}{}", " ".repeat(pad_len), prefix, num_str)
                            }
                        } else {
                            format!("{}{}", prefix, num_str)
                        }
                    } else {
                        format!("{}{}", prefix, num_str)
                    }
                }
                'o' => {
                    let val = match arg {
                        Val::Int(i) => *i,
                        Val::Float(f) => *f as i64,
                        Val::String(s) => s.parse().unwrap_or(0),
                        _ => 0,
                    };
                    let mut num_str = format!("{:o}", val);
                    if let Some(p) = precision
                        && num_str.len() < p
                    {
                        num_str = format!("{}{}", "0".repeat(p - num_str.len()), num_str);
                    }
                    let prefix = if flag_alternate && val != 0 && !num_str.starts_with('0') {
                        "0"
                    } else {
                        ""
                    };
                    if let Some(w) = width {
                        let total_len = prefix.len() + num_str.len();
                        if total_len < w {
                            let pad_len = w - total_len;
                            if flag_left_align {
                                format!("{}{}{}", prefix, num_str, " ".repeat(pad_len))
                            } else if flag_zero_pad && precision.is_none() {
                                format!("{}{}{}", prefix, "0".repeat(pad_len), num_str)
                            } else {
                                format!("{}{}{}", " ".repeat(pad_len), prefix, num_str)
                            }
                        } else {
                            format!("{}{}", prefix, num_str)
                        }
                    } else {
                        format!("{}{}", prefix, num_str)
                    }
                }
                'f' => {
                    let val = match arg {
                        Val::Float(f) => *f,
                        Val::Int(i) => *i as f64,
                        Val::String(s) => s.parse().unwrap_or(0.0),
                        _ => 0.0,
                    };
                    let sign = if val < 0.0 {
                        "-"
                    } else if flag_show_sign {
                        "+"
                    } else if flag_space_sign {
                        " "
                    } else {
                        ""
                    };
                    let abs_val = val.abs();
                    let prec = precision.unwrap_or(6);
                    let mut num_str = format!("{:.*}", prec, abs_val);
                    if flag_alternate && prec == 0 && !num_str.contains('.') {
                        num_str.push('.');
                    }
                    if let Some(w) = width {
                        let total_len = sign.len() + num_str.len();
                        if total_len < w {
                            let pad_len = w - total_len;
                            if flag_left_align {
                                format!("{}{}{}", sign, num_str, " ".repeat(pad_len))
                            } else if flag_zero_pad {
                                format!("{}{}{}", sign, "0".repeat(pad_len), num_str)
                            } else {
                                format!("{}{}{}", " ".repeat(pad_len), sign, num_str)
                            }
                        } else {
                            format!("{}{}", sign, num_str)
                        }
                    } else {
                        format!("{}{}", sign, num_str)
                    }
                }
                _ => {
                    return Err(BuiltinError::InvalidArgument {
                        cmd: "printf".into(),
                        arg: format!("unknown format type '{spec_type}'"),
                        span: None,
                    }
                    .to_string());
                }
            };

            result.push_str(&formatted);
        } else {
            result.push(c);
        }
    }

    Ok(result)
}

pub fn printf_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    _env: &Env,
    tx: PipeSender,
) -> Result<(), StringError> {
    if args.is_empty() {
        return Err("printf: missing format string".into());
    }

    let format_str = val_to_display_string(&args[0]);
    let format_args = &args[1..];

    let (interpreted_format, _) = interpret_ansi_escapes(&format_str);
    let output = format_printf(&interpreted_format, format_args).map_err(StringError::from)?;

    tokio::spawn(async move {
        let _ = tx
            .send(PipelinePayload::Data(Arc::new(Val::String(output))))
            .await;
    });

    Ok(())
}

pub fn true_builtin(
    _in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    _env: &Env,
    tx: PipeSender,
) -> Result<(), StringError> {
    tokio::spawn(async move {
        let _ = tx
            .send(PipelinePayload::Data(Arc::new(Val::Bool(true))))
            .await;
    });
    Ok(())
}

pub fn false_builtin(
    _in_rx: Option<PipeStream>,
    _args: Vec<Val>,
    _env: &Env,
    tx: PipeSender,
) -> Result<(), StringError> {
    tokio::spawn(async move {
        let _ = tx
            .send(PipelinePayload::Data(Arc::new(Val::Bool(false))))
            .await;
    });
    Err(StringError::condition_false())
}

fn parse_duration(s: &str) -> Result<std::time::Duration, StringError> {
    let s = s.trim();
    if let Some(ms) = s.strip_suffix("ms") {
        let val: f64 = ms
            .parse()
            .map_err(|_| format!("sleep: invalid duration '{}'", s))?;
        Ok(std::time::Duration::from_secs_f64(val / 1000.0))
    } else if let Some(sec) = s.strip_suffix('s') {
        let val: f64 = sec
            .parse()
            .map_err(|_| format!("sleep: invalid duration '{}'", s))?;
        Ok(std::time::Duration::from_secs_f64(val))
    } else if let Some(min) = s.strip_suffix('m') {
        let val: f64 = min
            .parse()
            .map_err(|_| format!("sleep: invalid duration '{}'", s))?;
        Ok(std::time::Duration::from_secs_f64(val * 60.0))
    } else if let Some(hr) = s.strip_suffix('h') {
        let val: f64 = hr
            .parse()
            .map_err(|_| format!("sleep: invalid duration '{}'", s))?;
        Ok(std::time::Duration::from_secs_f64(val * 3600.0))
    } else {
        let val: f64 = s
            .parse()
            .map_err(|_| format!("sleep: invalid duration '{}'", s))?;
        Ok(std::time::Duration::from_secs_f64(val))
    }
}

pub fn sleep_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    _tx: PipeSender,
) -> Result<(), StringError> {
    if args.is_empty() {
        return Err("sleep: missing duration".into());
    }

    let duration_str = val_to_display_string(&args[0]);
    let duration = parse_duration(&duration_str)?;

    let start = std::time::Instant::now();
    while start.elapsed() < duration {
        if env
            .job_control
            .sigint_pending
            .load(std::sync::atomic::Ordering::Acquire)
        {
            env.job_control
                .sigint_pending
                .store(false, std::sync::atomic::Ordering::SeqCst);
            return Err("sleep: interrupted".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Ok(())
}

pub fn lint_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    _env: &Env,
    tx: PipeSender,
) -> Result<(), StringError> {
    let source = if args.is_empty() {
        return Err("lint: requires a file path or inline source".into());
    } else {
        match &args[0] {
            Val::String(s) => {
                if std::path::Path::new(s).exists() {
                    std::fs::read_to_string(s)
                        .map_err(|e| format!("lint: failed to read {}: {}", s, e))?
                } else {
                    s.clone()
                }
            }
            _ => return Err("lint: argument must be a string".into()),
        }
    };

    let diagnostics = fshell_core::linter::lint(&source);

    let send_result = move |msg: String, tx: PipeSender| {
        tokio::spawn(async move {
            let _ = tx
                .send(PipelinePayload::Data(Arc::new(Val::String(msg))))
                .await;
        });
    };
    if diagnostics.is_empty() {
        send_result("No issues found".to_string(), tx);
    } else {
        for diag in &diagnostics {
            let level = match diag.level {
                fshell_core::linter::LintLevel::Error => "error",
                fshell_core::linter::LintLevel::Warning => "warning",
                fshell_core::linter::LintLevel::Info => "info",
            };
            let line_info = match &diag.span {
                Some(span) => {
                    let (line, _) = fshell_core::linter::offset_to_line_col(&source, span.offset());
                    if line > 0 {
                        format!(" (line {})", line)
                    } else {
                        String::new()
                    }
                }
                None => String::new(),
            };
            let msg = format!("[{}] {}{}: {}", level, diag.code, line_info, diag.message);
            send_result(msg, tx.clone());
        }
    }

    Ok(())
}

fn prompt_config_path() -> Option<PathBuf> {
    fshell_engine::resolve_config_dir().map(|p| p.join("prompt.toml"))
}

fn spec_display_short(spec: &fshell_core::ColorSpec) -> String {
    match spec {
        fshell_core::ColorSpec::Named(n) => n.clone(),
        fshell_core::ColorSpec::Hex(h) => format!("#{}", h),
        fshell_core::ColorSpec::Conditional { ok, err } => format!("{}|{}", ok, err),
    }
}

pub fn prompt_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), StringError> {
    let cmd = args.first().and_then(|v| match v {
        Val::String(s) if !s.is_empty() => Some(s.as_str()),
        _ => None,
    });

    match cmd {
        None | Some("tui") => {
            env.is_customizer_active
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let msg = "Opening prompt customizer...".to_string();
            tokio::spawn(async move {
                let _ = tx
                    .send(PipelinePayload::Data(Arc::new(Val::String(msg))))
                    .await;
            });
            Ok(())
        }
        Some("list") => {
            let config = env.prompt_config.read();
            let mut lines = vec![format!("Left separator: {:?}", config.left_separator)];
            lines.push(format!("Right separator: {:?}", config.right_separator));
            lines.push("".to_string());
            lines.push("Left segments:".to_string());
            for (i, seg) in config.left.iter().enumerate() {
                lines.push(format!(
                    "  {}. type={} fg={:?} bg={:?} bold={} italic={}",
                    i + 1,
                    seg.r#type.name(),
                    seg.fg,
                    seg.bg,
                    seg.bold,
                    seg.italic,
                ));
            }
            lines.push("".to_string());
            lines.push("Right segments:".to_string());
            for (i, seg) in config.right.iter().enumerate() {
                lines.push(format!(
                    "  {}. type={} fg={:?} bg={:?} bold={} italic={}",
                    i + 1,
                    seg.r#type.name(),
                    seg.fg,
                    seg.bg,
                    seg.bold,
                    seg.italic,
                ));
            }
            let result = lines.join("\n");
            tokio::spawn(async move {
                let _ = tx
                    .send(PipelinePayload::Data(Arc::new(Val::String(result))))
                    .await;
            });
            Ok(())
        }
        Some("export") => {
            let config = env.prompt_config.read();
            let toml = toml_edit::ser::to_string_pretty(&*config)
                .map_err(|e| format!("serialization error: {}", e))?;
            tokio::spawn(async move {
                let _ = tx
                    .send(PipelinePayload::Data(Arc::new(Val::String(toml))))
                    .await;
            });
            Ok(())
        }
        Some("show") => {
            let config = env.prompt_config.read();
            let mut lines = vec![];
            lines.push(format!("Separator: {:?}", config.separator_style));
            if let Some(ref p) = config.preset {
                lines.push(format!("Preset: {}", p));
            }
            lines.push("".to_string());
            lines.push("Left:".to_string());
            if config.left.is_empty() {
                lines.push("  (empty)".to_string());
            }
            for (i, seg) in config.left.iter().enumerate() {
                let fg = seg.fg.as_ref().map(spec_display_short).unwrap_or_else(|| "-".into());
                let bg = seg.bg.as_ref().map(spec_display_short).unwrap_or_else(|| "-".into());
                let mut attrs = vec![];
                if seg.bold { attrs.push("bold"); }
                if seg.italic { attrs.push("italic"); }
                if seg.shorten { attrs.push("shorten"); }
                if seg.hide_on_zero { attrs.push("hide_on_zero"); }
                if seg.hide_when_clean { attrs.push("hide_when_clean"); }
                if seg.show_only_in_repo { attrs.push("repo_only"); }
                let attr_str = if attrs.is_empty() { String::new() } else { format!(" [{}]", attrs.join(",")) };
                lines.push(format!(
                    "  {}. {} fg={} bg={}{}",
                    i + 1,
                    seg.r#type.display_name(),
                    fg,
                    bg,
                    attr_str,
                ));
            }
            lines.push("".to_string());
            lines.push("Right:".to_string());
            if config.right.is_empty() {
                lines.push("  (empty)".to_string());
            }
            for (i, seg) in config.right.iter().enumerate() {
                let fg = seg.fg.as_ref().map(spec_display_short).unwrap_or_else(|| "-".into());
                let bg = seg.bg.as_ref().map(spec_display_short).unwrap_or_else(|| "-".into());
                let mut attrs = vec![];
                if seg.bold { attrs.push("bold"); }
                if seg.hide_on_zero { attrs.push("hide_on_zero"); }
                let attr_str = if attrs.is_empty() { String::new() } else { format!(" [{}]", attrs.join(",")) };
                lines.push(format!(
                    "  {}. {} fg={} bg={}{}",
                    i + 1,
                    seg.r#type.display_name(),
                    fg,
                    bg,
                    attr_str,
                ));
            }
            let result = lines.join("\n");
            tokio::spawn(async move {
                let _ = tx
                    .send(PipelinePayload::Data(Arc::new(Val::String(result))))
                    .await;
            });
            Ok(())
        }
        Some("add") => {
            let type_name = args.get(1).and_then(|v| match v {
                Val::String(s) => Some(s.as_str()),
                _ => None,
            }).ok_or_else(|| StringError::from("prompt add: usage: prompt add <segment_type>"))?;

            let st = match type_name {
                "cargo_run" | "crun" | "fsh_crun" => fshell_core::SegmentType::CargoRun,
                "user" => fshell_core::SegmentType::User,
                "host" => fshell_core::SegmentType::Host,
                "pwd" => fshell_core::SegmentType::Pwd,
                "git_branch" => fshell_core::SegmentType::GitBranch,
                "git_status" => fshell_core::SegmentType::GitStatus,
                "exit_code" => fshell_core::SegmentType::ExitCode,
                "duration" => fshell_core::SegmentType::Duration,
                "jobs" => fshell_core::SegmentType::Jobs,
                "char" => fshell_core::SegmentType::Char,
                "time" => fshell_core::SegmentType::Time,
                "date" => fshell_core::SegmentType::Date,
                "timestamp" => fshell_core::SegmentType::Timestamp,
                "shlvl" => fshell_core::SegmentType::Shlvl,
                "shell" => fshell_core::SegmentType::Shell,
                "line" => fshell_core::SegmentType::Line,
                "aws" => fshell_core::SegmentType::Aws,
                "kube" => fshell_core::SegmentType::Kube,
                "venv" => fshell_core::SegmentType::Venv,
                "ssh" => fshell_core::SegmentType::Ssh,
                "text" => fshell_core::SegmentType::Text,
                "separator" => fshell_core::SegmentType::Separator,
                "newline" => fshell_core::SegmentType::Newline,
                "custom" => fshell_core::SegmentType::Custom,
                _ => return Err(format!("prompt add: unknown segment type '{}'. Run 'prompt show' to list configured segments and their types.", type_name).into()),
            };

            let side = args.get(2).and_then(|v| match v {
                Val::String(s) => Some(s.as_str()),
                _ => None,
            }).unwrap_or("left");

            let mut config = env.prompt_config.write();
            let new_seg = fshell_core::SegmentConfig::new(st, None, false);
            match side {
                "right" | "r" => config.right.push(new_seg),
                _ => config.left.push(new_seg),
            }
            let msg = format!("Added {} to {} side", type_name, side);
            tokio::spawn(async move {
                let _ = tx
                    .send(PipelinePayload::Data(Arc::new(Val::String(msg))))
                    .await;
            });
            Ok(())
        }
        Some("remove") => {
            let target = args.get(1).and_then(|v| match v {
                Val::String(s) => Some(s.as_str()),
                _ => None,
            }).ok_or_else(|| StringError::from("prompt remove: usage: prompt remove <index|type> [left|right]"))?;

            let side = args.get(2).and_then(|v| match v {
                Val::String(s) => Some(s.as_str()),
                _ => None,
            }).unwrap_or("left");

            let mut config = env.prompt_config.write();
            let segs = match side {
                "right" | "r" => &mut config.right,
                _ => &mut config.left,
            };

            if let Ok(idx) = target.parse::<usize>() {
                if idx == 0 || idx > segs.len() {
                    return Err(format!("prompt remove: index {} out of range (1-{})", idx, segs.len()).into());
                }
                let removed = segs.remove(idx - 1);
                let msg = format!("Removed {} (index {}) from {} side", removed.r#type.display_name(), idx, side);
                tokio::spawn(async move {
                    let _ = tx.send(PipelinePayload::Data(Arc::new(Val::String(msg)))).await;
                });
                return Ok(());
            }

            let pos = segs.iter().position(|s| s.r#type.name() == target);
            match pos {
                Some(i) => {
                    let removed = segs.remove(i);
                    let msg = format!("Removed {} from {} side", removed.r#type.display_name(), side);
                    tokio::spawn(async move {
                        let _ = tx.send(PipelinePayload::Data(Arc::new(Val::String(msg)))).await;
                    });
                    Ok(())
                }
                None => Err(format!("prompt remove: no segment named '{}' found on {} side", target, side).into()),
            }
        }
        Some("reset") => {
            let mut config = env.prompt_config.write();
            *config = fshell_core::PromptConfig::default();
            let msg = "Prompt config reset to defaults".to_string();
            tokio::spawn(async move {
                let _ = tx
                    .send(PipelinePayload::Data(Arc::new(Val::String(msg))))
                    .await;
            });
            Ok(())
        }
        Some("reload") => {
            let path = prompt_config_path();
            let new_config = path
                .and_then(|p| std::fs::read_to_string(&p).ok())
                .and_then(|s| toml_edit::de::from_str(&s).ok())
                .unwrap_or_default();
            let mut config = env.prompt_config.write();
            *config = new_config;
            let msg = "Prompt config reloaded from disk".to_string();
            tokio::spawn(async move {
                let _ = tx
                    .send(PipelinePayload::Data(Arc::new(Val::String(msg))))
                    .await;
            });
            Ok(())
        }
        Some(other) => Err(format!(
            "prompt: unknown subcommand '{}'. Available: tui, list, export, reset, reload, show, add, remove",
            other
        )
        .into()),
    }
}

pub fn exec_builtin(
    in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), StringError> {
    if args.is_empty() {
        // `exec` with no arguments is a no-op (handles fd manipulation in bash).
        return Ok(());
    }
    // `exec cmd args...` — run the command via the fallback handler.
    // Unlike a native shell's exec(2), the process is NOT replaced; the
    // command runs as a regular external job and its exit code becomes $?.
    let cmd_name = args[0].to_text();
    let cmd_args: Vec<Val> = args[1..].to_vec();
    let env_clone = env.clone();
    let tx_clone = tx.clone();
    if let Some(handler) = env.get_fallback_handler() {
        tokio::spawn(async move {
            if let Err(e) = handler(&cmd_name, cmd_args, in_rx, &env_clone, tx_clone, false) {
                let _ = tx
                    .send(PipelinePayload::Structured(e.to_string().into()))
                    .await;
            }
        });
    } else {
        return Err("exec: no fallback handler available".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_workspace_root() {
        let root = find_workspace_root();
        assert!(root.is_some(), "Should find the cargo workspace root");
        let root_path = root.unwrap();
        assert!(
            root_path.join("Cargo.toml").exists(),
            "Workspace root should contain Cargo.toml"
        );
        assert!(
            root_path.join("crates").exists(),
            "Workspace root should contain crates directory"
        );
    }
}
