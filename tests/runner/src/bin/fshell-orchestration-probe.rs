//! Deterministic Cargo/rustc stand-in for orchestration fault probes.
//! Never invokes Cargo, rustc, nextest or the shell under test.

use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("orchestration probe: {error}");
            ExitCode::from(17)
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(
        std::env::var_os("FSHELL_ORCHESTRATION_PROBE_ROOT")
            .ok_or_else(|| io::Error::other("missing probe root"))?,
    );
    let mode = fs::read_to_string(root.join("mode"))?;
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| {
            arg.into_string()
                .map_err(|_| io::Error::other("non-Unicode probe argument"))
        })
        .collect::<Result<_, _>>()?;
    let executable = match std::env::var("FSHELL_TEST_EXECUTABLE") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(error) => return Err(error.into()),
    };
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("calls"))?;
    serde_json::to_writer(
        &mut log,
        &json!({"args": args, "executable": executable, "cwd": std::env::current_dir()?}),
    )?;
    log.write_all(b"\n")?;
    let operation = args.iter().map(String::as_str).collect::<Vec<_>>();
    match operation.as_slice() {
        ["-vV"] => {
            if mode == "rustc-fail" {
                return Err(io::Error::other("injected compiler failure").into());
            }
            println!("release: 1.88.0\nhost: aarch64-unknown-linux-gnu");
        }
        ["nextest", "--version"] => {
            if mode == "version-fail" {
                return Err(io::Error::other("injected nextest prerequisite failure").into());
            }
            println!("cargo-nextest 0.9.114");
        }
        ["metadata", ..] => {
            if mode == "metadata-fail" {
                return Err(io::Error::other("injected metadata failure").into());
            }
            let mut targets: Vec<Value> = [
                "builtin_registry",
                "invocation_state",
                "native_evaluation",
                "evaluation_effects",
                "native_parsing",
                "pipeline_values",
                "pipeline_execution",
                "posix_expansion",
                "posix_evaluation",
                "posix_io_control",
                "search_streams",
                "cli_execution",
            ]
            .into_iter()
            .map(|name| json!({"name":name,"kind":["test"]}))
            .collect();
            if mode == "metadata-unknown" {
                targets.push(json!({"name":"unclassified","kind":["test"]}));
            }
            if mode == "metadata-missing" {
                targets.pop();
            }
            let package = json!({"name":"fshell-specifications","targets":targets});
            let packages = if mode == "metadata-ambiguous" {
                vec![package.clone(), package]
            } else {
                vec![package]
            };
            println!("{}", json!({"packages":packages}));
        }
        ["build", ..] => {
            if mode == "build-fail" {
                return Err(io::Error::other("injected build failure").into());
            }
            let path = if mode == "artifact-missing" {
                root.join("absent-fsh")
            } else {
                root.join("fake-fsh")
            };
            println!(
                "{}",
                json!({"reason":"compiler-artifact","target":{"name":"fsh","kind":["bin"]},"executable":path})
            );
        }
        ["nextest", "run", ..] => {
            if mode == "nextest-fail" {
                return Err(io::Error::other("injected test failure").into());
            }
        }
        _ => return Err(io::Error::other(format!("unexpected tool invocation: {args:?}")).into()),
    }
    Ok(())
}
