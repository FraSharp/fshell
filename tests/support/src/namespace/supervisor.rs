use super::*;
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{ExitCode, Stdio};

/// Entry point for the private, single-threaded namespace supervisor binary.
/// This process, never a test-runner thread, changes its namespace membership.
#[doc(hidden)]
pub fn supervisor_main() -> ExitCode {
    match arguments() {
        Ok((control, program, args)) => match supervise(&control, program, args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                let (stage, errno) = match error {
                    NamespaceError::Io { stage, source } => {
                        (stage, source.raw_os_error().unwrap_or(libc::EIO))
                    }
                    NamespaceError::Protocol(_) => (NamespaceStage::Control, libc::EPROTO),
                };
                let _ = send(
                    &control,
                    Frame {
                        tag: FAILED,
                        first: stage as u32,
                        second: errno as u32,
                    },
                    None,
                );
                ExitCode::from(125)
            }
        },
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(125)
        }
    }
}

fn arguments() -> Result<(UnixDatagram, PathBuf, Vec<std::ffi::OsString>), NamespaceError> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--control-fd")) {
        return Err(NamespaceError::Protocol("missing control descriptor"));
    }
    let fd: RawFd = args
        .next()
        .and_then(|value| value.into_string().ok())
        .and_then(|value| value.parse().ok())
        .filter(|fd| *fd > 2)
        .ok_or(NamespaceError::Protocol("invalid control descriptor"))?;
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--")) {
        return Err(NamespaceError::Protocol("missing payload separator"));
    }
    let program = args
        .next()
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or(NamespaceError::Protocol(
            "payload executable must be absolute",
        ))?;
    // SAFETY: the launcher transferred this live socket through exec. Marking
    // it CLOEXEC before any fork prevents transfer to the payload executable.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(at(NamespaceStage::Control, io::Error::last_os_error()));
    }
    // SAFETY: this process owns the descriptor transferred by the launcher.
    let socket = unsafe { UnixDatagram::from_raw_fd(fd) };
    Ok((socket, program, args.collect()))
}

fn pipe() -> io::Result<(File, File)> {
    let mut fds = [-1; 2];
    // SAFETY: pipe2 writes two newly owned CLOEXEC descriptors into valid storage.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: each fresh descriptor is adopted exactly once.
    Ok(unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) })
}

struct InitChild {
    pid: libc::pid_t,
    reaped: bool,
}

impl InitChild {
    fn wait(&mut self) -> io::Result<i32> {
        loop {
            let mut status = 0;
            // SAFETY: this is our unreaped direct child, so its PID cannot recycle.
            let result = unsafe { libc::waitpid(self.pid, &mut status, 0) };
            if result == self.pid {
                self.reaped = true;
                return Ok(status);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

impl Drop for InitChild {
    fn drop(&mut self) {
        if !self.reaped {
            // SAFETY: the unreaped direct-child identity is still reserved.
            unsafe {
                libc::kill(self.pid, libc::SIGKILL);
            }
            let _ = self.wait();
        }
    }
}

fn supervise(
    control: &UnixDatagram,
    program: PathBuf,
    args: Vec<std::ffi::OsString>,
) -> Result<(), NamespaceError> {
    // SAFETY: identity queries have no preconditions. Preserve visible UID/GID
    // numbers instead of mapping the payload to UID 0.
    let (uid, gid, pid) = unsafe { (libc::getuid(), libc::getgid(), libc::getpid()) };
    let parent = pidfd(pid).map_err(|error| at(NamespaceStage::ParentPidfd, error))?;
    // SAFETY: getppid has no preconditions; the launcher installed its guard
    // before exec, so it cannot disappear unobserved during this capture.
    let launcher = pidfd(unsafe { libc::getppid() })
        .map_err(|error| at(NamespaceStage::ParentPidfd, error))?;
    // SAFETY: this private binary is single-threaded and owns its process state.
    if unsafe { libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWPID) } < 0 {
        return Err(at(NamespaceStage::Unshare, io::Error::last_os_error()));
    }
    std::fs::write("/proc/self/uid_map", format!("{uid} {uid} 1\n"))
        .map_err(|error| at(NamespaceStage::UidMap, error))?;
    std::fs::write("/proc/self/setgroups", "deny\n")
        .map_err(|error| at(NamespaceStage::Setgroups, error))?;
    std::fs::write("/proc/self/gid_map", format!("{gid} {gid} 1\n"))
        .map_err(|error| at(NamespaceStage::GidMap, error))?;
    // Reinstate the guard after credential/namespace setup, with a stable handle
    // covering any parent-death race during that setup.
    guard_parent(&launcher)?;
    let (mut start_read, mut start_write) =
        pipe().map_err(|error| at(NamespaceStage::InitPipe, error))?;
    let (mut result_read, mut result_write) =
        pipe().map_err(|error| at(NamespaceStage::InitPipe, error))?;
    // SAFETY: the private supervisor is single-threaded. Both branches close
    // their unused endpoints, and the init branch exits without returning here.
    let child = unsafe { libc::fork() };
    if child < 0 {
        return Err(at(NamespaceStage::ForkInit, io::Error::last_os_error()));
    }
    if child == 0 {
        drop(start_write);
        drop(result_read);
        // Init must not retain the caller socket or pass it to the payload.
        // SAFETY: this fork branch owns its copy of the descriptor.
        unsafe {
            libc::close(control.as_raw_fd());
        }
        let result = init(&parent, &mut start_read, program, args);
        let frame = match result {
            Ok(status) => Frame {
                tag: FINISHED,
                first: status as u32,
                second: 0,
            },
            Err(NamespaceError::Io { stage, source }) => Frame {
                tag: FAILED,
                first: stage as u32,
                second: source.raw_os_error().unwrap_or(libc::EIO) as u32,
            },
            Err(NamespaceError::Protocol(_)) => Frame {
                tag: FAILED,
                first: NamespaceStage::Control as u32,
                second: libc::EPROTO as u32,
            },
        };
        let code = if result_write.write_all(&frame.bytes()).is_ok() && frame.tag == FINISHED {
            0
        } else {
            125
        };
        // SAFETY: init death triggers kernel teardown of every PID descendant,
        // including nested PID namespaces. No Rust destructors should run here.
        unsafe {
            libc::_exit(code);
        }
    }
    let mut owner = InitChild {
        pid: child,
        reaped: false,
    };
    drop(start_read);
    drop(result_write);
    let init_fd = pidfd(child).map_err(|error| at(NamespaceStage::InitPidfd, error))?;
    send(
        control,
        Frame {
            tag: READY,
            first: 0,
            second: 0,
        },
        Some(init_fd.as_raw_fd()),
    )?;
    let (ack, rights) = receive(control)?;
    if ack.tag != START || ack.first != 0 || ack.second != 0 || !rights.is_empty() {
        return Err(NamespaceError::Protocol("invalid start acknowledgement"));
    }
    // Payload execution is now safe: the caller already owns the init pidfd.
    start_write
        .write_all(b"S")
        .map_err(|error| at(NamespaceStage::Control, error))?;
    drop(start_write);
    let mut bytes = [0; FRAME_BYTES];
    result_read
        .read_exact(&mut bytes)
        .map_err(|error| at(NamespaceStage::Control, error))?;
    let frame = Frame::parse(&bytes)?;
    let status = owner
        .wait()
        .map_err(|error| at(NamespaceStage::InitWait, error))?;
    if frame.tag == FAILED {
        return send(control, frame, None);
    }
    if frame.tag != FINISHED || status != 0 {
        return Err(NamespaceError::Protocol(
            "init did not complete namespace teardown",
        ));
    }
    send(control, frame, None)
}

fn guard_parent(parent: &OwnedFd) -> Result<(), NamespaceError> {
    // SAFETY: the supervisor/init is single-threaded. The pidfd covers the
    // PR_SET_PDEATHSIG race, including getppid()==0 across PID namespaces.
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) } < 0 {
        return Err(at(NamespaceStage::ParentDeath, io::Error::last_os_error()));
    }
    if dead(parent.as_raw_fd(), 0).map_err(|error| at(NamespaceStage::ParentDeath, error))? {
        return Err(NamespaceError::Protocol(
            "parent died before guard installation",
        ));
    }
    Ok(())
}

fn init(
    parent: &OwnedFd,
    start: &mut File,
    program: PathBuf,
    args: Vec<std::ffi::OsString>,
) -> Result<i32, NamespaceError> {
    guard_parent(parent)?;
    let mut byte = [0];
    start
        .read_exact(&mut byte)
        .map_err(|error| at(NamespaceStage::Control, error))?;
    if byte != *b"S" {
        return Err(NamespaceError::Protocol("invalid init start handshake"));
    }
    let mut payload = Command::new(program);
    payload
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .process_group(0);
    let mut child = payload
        .spawn()
        .map_err(|error| at(NamespaceStage::PayloadSpawn, error))?;
    let status = child
        .wait()
        .map_err(|error| at(NamespaceStage::PayloadWait, error))?;
    Ok(status.into_raw())
}
