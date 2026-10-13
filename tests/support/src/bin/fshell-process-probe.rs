//! Deterministic adversarial child for testing the process transport itself.
//! Never shipped and never a stand-in for fshell behavior.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::process::{Command, ExitCode, Stdio};

fn required(args: &[OsString], index: usize) -> io::Result<&std::ffi::OsStr> {
    args.get(index)
        .map(OsString::as_os_str)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing probe argument"))
}

fn number(args: &[OsString], index: usize) -> io::Result<usize> {
    required(args, index)?
        .to_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid probe integer"))
}

fn run(args: &[OsString]) -> io::Result<u8> {
    match required(args, 1)?.as_bytes() {
        b"bytes" => {
            io::stdout().write_all(&[0, 255, 128, b'\n'])?;
            io::stderr().write_all(&[254, b'\n'])?;
        }
        b"argv" => {
            for arg in args.iter().skip(2) {
                let bytes = arg.as_bytes();
                io::stdout().write_all(&(bytes.len() as u64).to_be_bytes())?;
                io::stdout().write_all(bytes)?;
            }
        }
        b"environment" => match std::env::var_os(required(args, 2)?) {
            None => io::stdout().write_all(&[0])?,
            Some(value) => {
                io::stdout().write_all(&[1])?;
                io::stdout().write_all(value.as_bytes())?;
            }
        },
        b"cwd" => io::stdout().write_all(std::env::current_dir()?.as_os_str().as_bytes())?,
        b"echo-input" => {
            io::copy(&mut io::stdin(), &mut io::stdout())?;
        }
        b"duplex" => {
            let size = number(args, 2)?;
            let stdout = std::thread::spawn(move || io::stdout().write_all(&vec![b'O'; size]));
            let stderr = std::thread::spawn(move || io::stderr().write_all(&vec![b'E'; size]));
            let mut input = Vec::new();
            io::stdin().read_to_end(&mut input)?;
            stdout
                .join()
                .map_err(|_| io::Error::other("stdout producer panicked"))??;
            stderr
                .join()
                .map_err(|_| io::Error::other("stderr producer panicked"))??;
            if input != vec![b'I'; size] {
                return Err(io::Error::other("incorrect duplex input"));
            }
        }
        b"exit" => {
            return u8::try_from(number(args, 2)?)
                .map_err(|_| io::Error::other("exit status out of range"));
        }
        b"signal" => {
            // SAFETY: raise sends a known termination signal to this probe only.
            unsafe {
                libc::raise(libc::SIGTERM);
            }
            return Err(io::Error::other("SIGTERM did not terminate probe"));
        }
        b"block-input" => {
            io::stdout().write_all(b"READY\n")?;
            io::stdout().flush()?;
            loop {
                std::thread::park();
            }
        }
        b"close-input" => {
            // SAFETY: the probe owns its stdin and will not access it afterwards.
            unsafe {
                libc::close(libc::STDIN_FILENO);
            }
            io::stdout().write_all(b"READY\n")?;
            io::stdout().flush()?;
            loop {
                std::thread::park();
            }
        }
        b"ready-file" => {
            std::fs::write(required(args, 2)?, b"ready")?;
            loop {
                std::thread::park();
            }
        }
        b"hold-pipes" => {
            let child = Command::new(std::env::current_exe()?)
                .arg("idle")
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()?;
            writeln!(io::stdout(), "DESCENDANT:{}", child.id())?;
            io::stdout().flush()?;
            // Intentionally exit without waiting: the transport must distinguish
            // direct-child exit from EOF and terminate the owned launch group.
        }
        b"idle" => loop {
            std::thread::park();
        },
        b"stdout" => io::stdout().write_all(&vec![b'O'; number(args, 2)?])?,
        b"stderr" => io::stderr().write_all(&vec![b'E'; number(args, 2)?])?,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unknown probe operation",
            ));
        }
    }
    Ok(0)
}

fn main() -> ExitCode {
    match run(&std::env::args_os().collect::<Vec<_>>()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("process probe: {error}");
            ExitCode::FAILURE
        }
    }
}
