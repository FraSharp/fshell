// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! fshell-panesd — the fshell-panes daemon process.
//!
//! This binary starts the persistent daemon that manages sessions,
//! runs PTY actors, and accepts client connections via Unix socket.
//!
//! Usage:
//!   fshell-panesd              # Start the daemon
//!   fshell-panesd --help       # Show help

use clap::Parser;

#[derive(Parser)]
#[command(
    name = "fshell-panesd",
    about = "fshell-panes daemon — persistent terminal multiplexer backend",
    version
)]
struct Args {
    /// Print verbose output.
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Args::parse();

    if args.verbose {
        eprintln!("fshell-panesd: starting daemon...");
    }

    fshell_panes::daemon::run_daemon().await
}
