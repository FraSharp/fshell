// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::error::BuiltinError;
use fshell_core::ShellError;
use fshell_core::Val;
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use miette::SourceSpan;
use std::io::BufRead;
use std::sync::Arc;
use ustr::ustr;

pub fn ps_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    _span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    // Check process spawn capability (we delegate to ps command)
    env.enforce_capability("ps", fshell_engine::CapAction::ProcessSpawn)?;

    let all_users = args.iter().any(|a| {
        let s = a.to_text();
        s == "-a" || s == "-A" || s == "-u"
    });

    let pids: Vec<String> = args
        .iter()
        .filter_map(|a| {
            let s = a.to_text();
            s.strip_prefix("-p").map(|p| p.trim().to_string())
        })
        .collect();

    let tx_clone = tx.clone();
    tokio::spawn(async move {
        if let Err(e) = run_ps(all_users, &pids, tx_clone).await {
            eprintln!("ps error: {}", e);
        }
    });

    Ok(())
}

async fn run_ps(all_users: bool, pids: &[String], tx: PipeSender) -> Result<(), ShellError> {
    // Use native ps command with structured output.
    // `args` must be last (otherwise BSD truncates it to column width) and we
    // force wide output (`ww`) so long command lines are not clipped at $COLUMNS.
    let mut cmd = std::process::Command::new("ps");
    let fmt = "pid,ppid,user,%cpu,rss,vsize,state,comm,nice,args";

    if !pids.is_empty() {
        // -p <pid> -ww -o fmt
        cmd.arg("-p");
        cmd.arg(pids.join(","));
        cmd.arg("-ww");
        cmd.arg("-o");
        cmd.arg(fmt);
    } else if all_users {
        // BSD: axww -o fmt,  GNU: ax -ww -o fmt — both spellings work on macOS,
        // the extra -ww is harmless on Linux.
        #[cfg(target_os = "macos")]
        {
            cmd.arg("axww");
            cmd.arg("-o");
        }
        #[cfg(not(target_os = "macos"))]
        {
            cmd.arg("ax");
            cmd.arg("-ww");
            cmd.arg("-o");
        }
        cmd.arg(fmt);
    } else {
        #[cfg(target_os = "macos")]
        {
            cmd.arg("-xww");
            cmd.arg("-o");
        }
        #[cfg(not(target_os = "macos"))]
        {
            cmd.arg("-x");
            cmd.arg("-ww");
            cmd.arg("-o");
        }
        cmd.arg(fmt);
    }
    // NOTE: --no-headers is GNU-ps only (Linux); BSD ps (macOS) doesn't support it.
    // We skip the header line below instead.
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("ps: failed to spawn: {}", e))?;
    let stdout = child.stdout.take().ok_or("ps: failed to capture stdout")?;
    let reader = std::io::BufReader::new(stdout);

    let mut is_first_line = true;

    for line in reader.lines() {
        let line = line.map_err(|e| format!("ps: read error: {}", e))?;
        let trimmed = line.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }

        // Skip the header line (contains column names like "PID PPID USER ...")
        if is_first_line {
            is_first_line = false;
            continue;
        }

        let fields = split_ps_output(&trimmed);
        if fields.len() < 9 {
            continue;
        }

        let pid_str = fields[0];
        let ppid_str = fields[1];
        let user = fields[2].to_string();
        let cpu_str = fields[3];
        let rss_str = fields[4];
        let vsize_str = fields[5];
        let state = fields[6].to_string();
        let comm = fields[7].to_string();
        let nice_str = fields[8];
        let args_str = if fields.len() > 9 {
            fields[9].to_string()
        } else {
            String::new()
        };

        let pid: i64 = pid_str.parse().unwrap_or(0);
        let ppid: i64 = ppid_str.parse().unwrap_or(0);
        let cpu: f64 = cpu_str.parse().unwrap_or(0.0);
        let rss: i64 = rss_str.parse().unwrap_or(0);
        let vsize: i64 = vsize_str.parse().unwrap_or(0);
        let nice: i64 = nice_str.parse().unwrap_or(0);

        // `comm` from `ps` is truncated to 15 chars on BSD (macOS) and is useless
        // for tables. Prefer the full `args` (full command line) for `command`,
        // fallback to `comm` only when `args` is empty (kernel threads).
        let command_val = if !args_str.is_empty() {
            args_str.clone()
        } else {
            comm.clone()
        };
        let mut map = fshell_core::FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        map.insert(ustr("pid"), Val::Int(pid));
        map.insert(ustr("ppid"), Val::Int(ppid));
        map.insert(ustr("user"), Val::String(user));
        map.insert(ustr("cpu"), Val::Float(cpu));
        map.insert(ustr("rss"), Val::Int(rss));
        map.insert(ustr("vsize"), Val::Int(vsize));
        map.insert(ustr("state"), Val::String(state));
        map.insert(ustr("command"), Val::String(command_val));
        map.insert(ustr("args"), Val::String(args_str));
        map.insert(ustr("comm"), Val::String(comm));
        map.insert(ustr("nice"), Val::Int(nice));

        if tx
            .send(PipelinePayload::Data(Arc::new(Val::Map(map))))
            .await
            .is_err()
        {
            break;
        }
    }

    // Wait for the child to fully exit
    let status = child.wait().map_err(|e| format!("ps: wait error: {}", e))?;
    if !status.success() {
        // stderr is not captured in streaming mode; we just signal non-zero exit
        return Err(BuiltinError::CommandFailed {
            cmd: "ps".into(),
            status: status.code().unwrap_or(-1),
            stderr: String::new(),
        }
        .into());
    }

    Ok(())
}

/// Split ps output line respecting the fixed-width columns.
/// ps axo format uses fixed-width columns separated by whitespace.
/// `args` is last so it can contain spaces and is not truncated.
fn split_ps_output(line: &str) -> Vec<&str> {
    let mut fields: Vec<&str> = Vec::new();
    let mut remaining = line.trim();

    // PID, PPID, USER, %CPU, RSS, VSIZE, STATE are whitespace-separated
    for _ in 0..7 {
        remaining = remaining.trim_start();
        if let Some(pos) = remaining.find(char::is_whitespace) {
            fields.push(remaining[..pos].trim());
            remaining = &remaining[pos..];
        } else {
            fields.push(remaining);
            return fields;
        }
    }

    // Remaining fields: COMM, NI, ARGS — args is last and can contain spaces
    remaining = remaining.trim_start();
    if remaining.is_empty() {
        return fields;
    }
    // COMM (no spaces)
    if let Some(pos) = remaining.find(char::is_whitespace) {
        fields.push(&remaining[..pos]);
        remaining = remaining[pos..].trim_start();
    } else {
        fields.push(remaining);
        return fields;
    }
    if remaining.is_empty() {
        fields.push("0");
        fields.push("");
        return fields;
    }
    // NI
    if let Some(pos) = remaining.find(char::is_whitespace) {
        fields.push(&remaining[..pos]);
        remaining = remaining[pos..].trim_start();
        // ARGS — remainder of line (may be empty, may contain spaces)
        fields.push(remaining);
    } else {
        // No ARGS, just NI
        fields.push(remaining);
        fields.push("");
    }

    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_ps_output_basic() {
        let line = " 1234  5678 ariel           0.0   12345   67890 S   zsh     0 zsh -l";
        let fields = split_ps_output(line);
        assert_eq!(fields[0], "1234", "pid");
        assert_eq!(fields[1], "5678", "ppid");
        assert_eq!(fields[2], "ariel", "user");
        assert_eq!(fields[3], "0.0", "cpu");
        assert_eq!(fields[7], "zsh", "comm");
        assert_eq!(fields[8], "0", "nice");
        assert_eq!(fields[9], "zsh -l", "args");
    }

    #[test]
    fn test_split_ps_output_root() {
        let line = "     1     0 root            0.0   24680   98765 Ss  launchd 0 launchd";
        let fields = split_ps_output(line);
        assert_eq!(fields[0], "1", "pid");
        assert_eq!(fields[2], "root", "user");
        assert_eq!(fields[8], "0", "nice");
        assert!(
            fields.len() >= 9,
            "should have at least 9 fields, got {}",
            fields.len()
        );
    }

    #[test]
    fn test_split_ps_output_vsized_cpu() {
        // Verify rss and vsize parsing
        let line =
            "  9876  1234 ariel          12.5  204800 1048576 R   cargo  0 cargo build --release";
        let fields = split_ps_output(line);
        assert_eq!(fields[3], "12.5");
        assert_eq!(fields[4], "204800");
        assert_eq!(fields[5], "1048576");
        assert_eq!(fields[6], "R");
        assert_eq!(fields[7], "cargo");
        assert_eq!(fields[8], "0");
        assert_eq!(fields[9], "cargo build --release");
    }
}
