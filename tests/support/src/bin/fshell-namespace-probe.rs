//! Adversarial lifecycle probes. Escaping descendants are only created as PID 2
//! under the private PID-namespace init, never under launch-group-only transport.

#[cfg(target_os = "linux")]
mod linux {
    use fshell_test_support::process::ProcessCommand;
    use std::io::{self, Read, Write};
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::net::UnixDatagram;
    use std::path::Path;

    fn required(args: &[std::ffi::OsString], index: usize) -> io::Result<&std::ffi::OsStr> {
        args.get(index)
            .map(|arg| arg.as_os_str())
            .ok_or_else(|| io::Error::other("missing probe argument"))
    }

    pub fn run() -> io::Result<u8> {
        let args: Vec<_> = std::env::args_os().collect();
        if required(&args, 1)? == "--control-fd" {
            let fd: i32 = required(&args, 2)?
                .to_str()
                .and_then(|arg| arg.parse().ok())
                .filter(|fd| *fd > 2)
                .ok_or_else(|| io::Error::other("invalid control fd"))?;
            // SAFETY: this fake supervisor was explicitly given its owned socket.
            let socket = unsafe { UnixDatagram::from_raw_fd(fd) };
            match required(&args, 5)?.to_str() {
                Some("supervisor-silent") => {}
                Some("supervisor-short") => {
                    socket.send(b"bad")?;
                }
                Some("supervisor-finished-first") => {
                    socket.send(&[0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0])?;
                }
                Some("supervisor-denied") => {
                    io::stderr().write_all(b"DENIED\n")?;
                    socket.send(&[0, 0, 0, 3, 0, 0, 0, 3, 0, 0, 0, libc::EPERM as u8])?;
                }
                _ => return Err(io::Error::other("unknown supervisor fault")),
            }
            return Ok(0);
        }
        if required(&args, 1)? == "own-domain" {
            let output = ProcessCommand::new(std::env::current_exe()?, required(&args, 3)?)
                .in_pid_namespace(required(&args, 2)?)
                .args([std::ffi::OsStr::new("tree-hold"), required(&args, 3)?])
                .run()
                .map_err(io::Error::other)?;
            io::stdout().write_all(&output.capture.stdout)?;
            io::stderr().write_all(&output.capture.stderr)?;
            return output
                .status
                .code()
                .and_then(|code| u8::try_from(code).ok())
                .ok_or_else(|| io::Error::other("unexpected payload signal"));
        }
        // SAFETY: identity queries have no preconditions. This protects the host
        // if the probe is accidentally executed without the namespace adapter.
        if unsafe { libc::getpid() != 2 || libc::getppid() != 1 } {
            return Err(io::Error::other(
                "probe requires the first payload of a private PID namespace",
            ));
        }
        match required(&args, 1)?.to_str() {
            Some("identity") => {
                // SAFETY: identity queries have no preconditions.
                let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
                writeln!(io::stdout(), "uid={uid} gid={gid} pid=2 ppid=1")?;
                Ok(0)
            }
            Some("fds") => {
                let mut fds: Vec<u32> = std::fs::read_dir("/proc/self/fd")?
                    .map(|entry| {
                        entry?
                            .file_name()
                            .to_str()
                            .and_then(|name| name.parse().ok())
                            .ok_or_else(|| io::Error::other("invalid descriptor name"))
                    })
                    .collect::<io::Result<_>>()?;
                fds.sort_unstable();
                writeln!(io::stdout(), "{fds:?}")?;
                Ok(0)
            }
            Some(mode @ ("tree-hold" | "tree-close" | "tree-flood")) => {
                tree(Path::new(required(&args, 2)?), mode)
            }
            _ => Err(io::Error::other("unknown namespace probe")),
        }
    }

    fn publish(root: &Path, name: &str, data: &[u8]) -> io::Result<()> {
        let pending = root.join(format!("{name}.pending"));
        std::fs::write(&pending, data)?;
        std::fs::rename(pending, root.join(name))
    }

    fn host_identity(path: &Path) -> io::Result<(i32, i32)> {
        let stat = std::fs::read_to_string(path)?;
        let pid = stat
            .split_whitespace()
            .next()
            .and_then(|pid| pid.parse().ok())
            .ok_or_else(|| io::Error::other("missing host PID"))?;
        let parent = stat
            .rsplit_once(") ")
            .and_then(|(_, fields)| fields.split_whitespace().nth(1))
            .and_then(|pid| pid.parse().ok())
            .ok_or_else(|| io::Error::other("missing host parent PID"))?;
        Ok((pid, parent))
    }

    fn tree(root: &Path, mode: &str) -> io::Result<u8> {
        let release_path = root.join("release");
        let release_name = std::ffi::CString::new(release_path.as_os_str().as_bytes())
            .map_err(io::Error::other)?;
        // SAFETY: a unique fixture owns this path; mkfifo only creates its leaf.
        if unsafe { libc::mkfifo(release_name.as_ptr(), 0o600) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // An owned read/write FIFO blocks on the start byte without busy polling
        // or requiring a writer to open first. No sleeps synchronize the probe.
        let mut release = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(release_path)?;
        for (name, session) in [("session", true), ("group", false), ("nested", false)] {
            // SAFETY: this private binary is single-threaded, creates descendants
            // only inside the PID namespace and has no held stdio locks.
            let pid = unsafe { libc::fork() };
            if pid < 0 {
                return Err(io::Error::last_os_error());
            }
            if pid == 0 {
                // SAFETY: only this child changes its session/group.
                let rc = unsafe {
                    if session {
                        libc::setsid()
                    } else {
                        libc::setpgid(0, 0)
                    }
                };
                if rc < 0 {
                    return Err(io::Error::last_os_error());
                }
                if name == "nested" {
                    // SAFETY: this single-threaded descendant changes only its
                    // own user namespace and the PID namespace of future children.
                    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
                    if unsafe { libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWPID) } < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    std::fs::write("/proc/self/uid_map", format!("{uid} {uid} 1\n"))?;
                    std::fs::write("/proc/self/setgroups", "deny\n")?;
                    std::fs::write("/proc/self/gid_map", format!("{gid} {gid} 1\n"))?;
                    // SAFETY: this private probe remains single-threaded.
                    let nested = unsafe { libc::fork() };
                    if nested < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    if nested > 0 {
                        let (host_pid, _) = host_identity(Path::new("/proc/self/stat"))?;
                        publish(root, "nested-parent", format!("{host_pid}\n").as_bytes())?;
                        loop {
                            unsafe {
                                libc::pause();
                            }
                        }
                    }
                }
                let (host_pid, _) = host_identity(Path::new("/proc/self/stat"))?;
                publish(root, name, format!("{host_pid}\n").as_bytes())?;
                if mode == "tree-close" {
                    // SAFETY: the child owns its stdio and never accesses it again.
                    unsafe {
                        libc::close(0);
                        libc::close(1);
                        libc::close(2);
                    }
                }
                loop {
                    unsafe {
                        libc::pause();
                    }
                }
            }
        }
        while ["session", "group", "nested-parent", "nested"]
            .iter()
            .any(|name| !root.join(name).exists())
        {
            std::thread::yield_now();
        }
        let (payload, init) = host_identity(Path::new("/proc/self/stat"))?;
        let (_, supervisor) =
            host_identity(&std::path::PathBuf::from(format!("/proc/{init}/stat")))?;
        publish(
            root,
            "ready",
            format!("{payload} {init} {supervisor}\n").as_bytes(),
        )?;
        io::stdout().write_all(b"READY\n")?;
        let mut start = [0];
        release.read_exact(&mut start)?;
        if start != *b"S" {
            return Err(io::Error::other("invalid release handshake"));
        }
        if mode == "tree-flood" {
            loop {
                io::stdout().write_all(&[b'O'; 8192])?;
            }
        }
        Ok(37)
    }
}

fn main() -> std::process::ExitCode {
    #[cfg(target_os = "linux")]
    match linux::run() {
        Ok(code) => std::process::ExitCode::from(code),
        Err(error) => {
            eprintln!("namespace probe: {error}");
            std::process::ExitCode::from(125)
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("namespace probes require Linux");
        std::process::ExitCode::from(125)
    }
}
