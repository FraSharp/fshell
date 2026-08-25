// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use std::fs;
use std::io::{self, Read};
use std::path::Path;

fn print_usage(program: &str) {
    eprintln!("Usage: {program} [OPTIONS] [FILE...]");
    eprintln!("Compute fhash digests.");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  -a <256|512|xof>   Algorithm (default: 256)");
    eprintln!("  -o <N>             Output length for XOF mode (default: 32)");
    eprintln!("  -h, --help         Show this help");
}

enum Mode {
    Sum256,
    Sum512,
    Xof(usize),
}

fn hash_reader<R: Read>(mut reader: R, mode: &Mode) -> io::Result<String> {
    let mut hasher = match mode {
        Mode::Sum256 => fshell_hash::Hasher::new(0x00, 16),
        Mode::Sum512 => fshell_hash::Hasher::new(0x04, 16),
        Mode::Xof(_) => fshell_hash::Hasher::new(0x02, 16),
    };

    let mut buf = [0u8; 4096];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    let output_len = match mode {
        Mode::Sum256 => 32,
        Mode::Sum512 => 64,
        Mode::Xof(len) => *len,
    };

    let digest = hasher.finalize(output_len);
    Ok(hex_encode(&digest))
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

fn hash_stdin(mode: &Mode) -> io::Result<()> {
    let hash = hash_reader(io::stdin(), mode)?;
    println!("{hash}");
    Ok(())
}

fn hash_file(path: &Path, mode: &Mode) -> io::Result<()> {
    let file = fs::File::open(path)?;
    let hash = hash_reader(file, mode)?;
    println!("{hash}  {}", path.display());
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let program = args.first().map(|s| s.as_str()).unwrap_or("fhash");

    let mut algo = String::from("256");
    let mut xof_len: usize = 32;
    let mut files: Vec<&str> = Vec::new();
    let mut i = 1;

    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_usage(program);
                return;
            }
            "-a" => {
                i += 1;
                let a = args.get(i).unwrap_or_else(|| {
                    eprintln!("error: -a requires an argument");
                    std::process::exit(1);
                });
                if a != "256" && a != "512" && a != "xof" {
                    eprintln!("error: unknown algorithm '{a}' (use 256, 512, or xof)");
                    std::process::exit(1);
                }
                algo = a.clone();
            }
            "-o" => {
                i += 1;
                let len_str = args.get(i).unwrap_or_else(|| {
                    eprintln!("error: -o requires a number");
                    std::process::exit(1);
                });
                xof_len = len_str.parse().unwrap_or_else(|_| {
                    eprintln!("error: -o expected a number, got '{len_str}'");
                    std::process::exit(1);
                });
                algo = String::from("xof");
            }
            arg if arg.starts_with('-') => {
                eprintln!("error: unknown option '{arg}'");
                std::process::exit(1);
            }
            file => {
                files.push(file);
            }
        }
        i += 1;
    }

    let mode = match algo.as_str() {
        "256" => Mode::Sum256,
        "512" => Mode::Sum512,
        "xof" => Mode::Xof(xof_len),
        _ => unreachable!(),
    };

    if files.is_empty() {
        if let Err(e) = hash_stdin(&mode) {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    } else {
        for file in files {
            if let Err(e) = hash_file(Path::new(file), &mode) {
                eprintln!("{file}: {e}");
            }
        }
    }
}
