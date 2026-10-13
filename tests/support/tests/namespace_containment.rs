//! Explicit Linux containment prerequisites: unavailable namespaces fail; they
//! are not skipped and this target is not part of the portable support profile.

#![cfg(target_os = "linux")]

use fshell_test_support::fixture::ShellFixture;
use fshell_test_support::namespace::{NamespaceError, NamespaceStage};
use fshell_test_support::process::{
    ExecutionLimits, ProcessCommand, ProcessError, ProcessErrorKind, ProcessOutput,
};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::ExitStatusExt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

type Outcome = Result<ProcessOutput, Box<ProcessError>>;

fn command(fixture: &ShellFixture, operation: &str) -> ProcessCommand {
    ProcessCommand::new(
        env!("CARGO_BIN_EXE_fshell-process-probe"),
        fixture.workspace(),
    )
    .in_pid_namespace(env!("CARGO_BIN_EXE_fshell-process-supervisor"))
    .arg(operation)
}

fn pidfd(pid: i32) -> io::Result<OwnedFd> {
    // SAFETY: probe identities are held alive behind a fixture release handshake;
    // the syscall creates a stable handle before allowing cleanup to begin.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: one fresh descriptor is adopted exactly once.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn dead(fd: &OwnedFd, timeout: i32) -> io::Result<bool> {
    let mut poll = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one live pollfd refers to an owned pidfd.
    let result = unsafe { libc::poll(&mut poll, 1, timeout) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    assert_eq!(poll.revents & libc::POLLNVAL, 0);
    Ok(result > 0 && poll.revents & (libc::POLLIN | libc::POLLHUP) != 0)
}

struct Session {
    fixture: ShellFixture,
    worker: Option<JoinHandle<Outcome>>,
    cancellation: Arc<AtomicBool>,
}

impl Session {
    fn start(mode: &str, limits: ExecutionLimits, separate_owner: bool) -> io::Result<Self> {
        let fixture = ShellFixture::new()?;
        let cancellation = Arc::new(AtomicBool::new(false));
        let command = if separate_owner {
            ProcessCommand::new(
                env!("CARGO_BIN_EXE_fshell-namespace-probe"),
                fixture.workspace(),
            )
            .arg("own-domain")
            .arg(env!("CARGO_BIN_EXE_fshell-process-supervisor"))
            .arg(fixture.workspace())
        } else {
            ProcessCommand::new(
                env!("CARGO_BIN_EXE_fshell-namespace-probe"),
                fixture.workspace(),
            )
            .in_pid_namespace(env!("CARGO_BIN_EXE_fshell-process-supervisor"))
            .arg(mode)
            .arg(fixture.workspace())
        }
        .limits(limits)
        .cancellation(cancellation.clone());
        let worker = std::thread::spawn(move || command.run());
        Ok(Self {
            fixture,
            worker: Some(worker),
            cancellation,
        })
    }

    fn ready(&mut self) -> io::Result<(Vec<OwnedFd>, [i32; 3])> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.fixture.workspace().join("ready").exists() {
            if self.worker.as_ref().expect("active worker").is_finished() {
                let outcome = self.finish();
                return Err(io::Error::other(format!(
                    "probe exited before readiness: {outcome:?}"
                )));
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other("probe readiness deadline"));
            }
            std::thread::yield_now();
        }
        let parse = |data: Vec<u8>| -> io::Result<Vec<i32>> {
            std::str::from_utf8(&data)
                .map_err(io::Error::other)?
                .split_whitespace()
                .map(|text| text.parse::<i32>().map_err(io::Error::other))
                .collect()
        };
        let ids: [i32; 3] = parse(self.fixture.read_file("ready")?)?
            .try_into()
            .map_err(|_| io::Error::other("invalid readiness frame"))?;
        let mut fds = ids
            .iter()
            .map(|pid| pidfd(*pid))
            .collect::<io::Result<Vec<_>>>()?;
        let mut descendants = Vec::new();
        for name in ["session", "group", "nested-parent", "nested"] {
            let ids = parse(self.fixture.read_file(name)?)?;
            assert_eq!(ids.len(), 1, "{name}");
            descendants.push(ids[0]);
            fds.push(pidfd(ids[0])?);
        }
        for fd in &fds {
            assert!(
                !dead(fd, 0)?,
                "handshake must keep every observed process alive"
            );
        }
        // SAFETY: all probe processes are held alive at the release barrier.
        unsafe {
            assert_ne!(
                libc::getsid(descendants[0]),
                libc::getsid(ids[0]),
                "new session must escape the payload session"
            );
            assert_ne!(
                libc::getpgid(descendants[1]),
                libc::getpgid(ids[0]),
                "new group must escape the payload group"
            );
        }
        Ok((fds, ids))
    }

    fn release(&self) -> io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut fifo = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(self.fixture.workspace().join("release"))?;
        fifo.write_all(b"S")
    }

    fn finish(&mut self) -> Outcome {
        self.worker
            .take()
            .expect("active worker")
            .join()
            .expect("transport worker")
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.cancellation.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        // The fixture is dropped only after transport and domain cleanup join.
    }
}

fn all_dead(fds: &[OwnedFd]) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(2);
    for fd in fds {
        loop {
            if dead(fd, 20)? {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "owned namespace process survived cleanup"
            );
        }
    }
    Ok(())
}

#[test]
fn payload_exit_signal_and_binary_streams_remain_exact_observations() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let output = command(&fixture, "bytes")
        .run()
        .expect("namespace prerequisite and payload execution");
    assert!(output.status.success());
    assert_eq!(output.capture.stdout, [0, 255, 128, b'\n']);
    assert_eq!(output.capture.stderr, [254, b'\n']);
    let output = command(&fixture, "exit")
        .arg("37")
        .run()
        .expect("payload exit");
    assert_eq!(output.status.code(), Some(37));
    let output = command(&fixture, "signal").run().expect("payload signal");
    assert_eq!(output.status.signal(), Some(libc::SIGTERM));
    assert_eq!(output.status.code(), None);
    fixture.close()
}

#[test]
fn namespace_transport_preserves_large_duplex_input_and_both_streams() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let size = 1024 * 1024;
    let output = command(&fixture, "duplex")
        .arg(size.to_string())
        .stdin(vec![b'I'; size])
        .run()
        .expect("namespace duplex execution");
    assert!(output.status.success());
    assert_eq!(output.capture.stdout, vec![b'O'; size]);
    assert_eq!(output.capture.stderr, vec![b'E'; size]);
    assert_eq!(output.capture.stdin_written, size);
    fixture.close()
}

#[test]
fn namespace_identity_mapping_preserves_uid_and_gid_and_internal_fds_do_not_escape_exec()
-> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let make = |operation| {
        ProcessCommand::new(
            env!("CARGO_BIN_EXE_fshell-namespace-probe"),
            fixture.workspace(),
        )
        .in_pid_namespace(env!("CARGO_BIN_EXE_fshell-process-supervisor"))
        .arg(operation)
    };
    let output = make("identity").run().expect("identity probe");
    // SAFETY: identity queries have no preconditions.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    assert!(output.status.success());
    assert_eq!(
        output.capture.stdout,
        format!("uid={uid} gid={gid} pid=2 ppid=1\n").as_bytes()
    );
    assert_eq!(output.capture.stderr, []);
    let output = make("fds").run().expect("descriptor probe");
    assert!(output.status.success());
    // /proc/self/fd's directory iterator owns the fourth descriptor.
    assert_eq!(output.capture.stdout, b"[0, 1, 2, 3]\n");
    assert_eq!(output.capture.stderr, []);
    fixture.close()
}

#[test]
fn successful_payload_exit_removes_new_sessions_groups_and_nested_pid_namespaces() -> io::Result<()>
{
    for mode in ["tree-hold", "tree-close"] {
        let mut session = Session::start(mode, ExecutionLimits::default(), false)?;
        let (fds, _) = session.ready()?;
        session.release()?;
        let output = session.finish().expect("namespace payload completion");
        assert_eq!(output.status.code(), Some(37));
        assert_eq!(output.capture.stdout, b"READY\n");
        assert_eq!(output.capture.stderr, []);
        all_dead(&fds)?;
    }
    Ok(())
}

#[test]
fn cancellation_after_readiness_confirms_namespace_death() -> io::Result<()> {
    let mut session = Session::start("tree-hold", ExecutionLimits::default(), false)?;
    let (fds, _) = session.ready()?;
    session.cancellation.store(true, Ordering::Release);
    let error = session.finish().expect_err("cancellation must fail");
    assert!(matches!(error.kind, ProcessErrorKind::Cancelled), "{error}");
    assert!(error.cleanup_errors.is_empty(), "{error}");
    all_dead(&fds)
}

#[test]
fn deadline_cleans_descendants_even_after_they_close_capture_pipes() -> io::Result<()> {
    let mut session = Session::start(
        "tree-close",
        ExecutionLimits {
            deadline: Duration::from_secs(2),
            ..ExecutionLimits::default()
        },
        false,
    )?;
    let (fds, _) = session.ready()?;
    let error = session
        .finish()
        .expect_err("unreleased payload must time out");
    assert!(matches!(error.kind, ProcessErrorKind::Deadline), "{error}");
    assert_eq!(error.capture.stdout, b"READY\n");
    assert!(error.cleanup_errors.is_empty(), "{error}");
    all_dead(&fds)
}

#[test]
fn output_overflow_retains_the_prefix_and_confirms_descendant_cleanup() -> io::Result<()> {
    let mut session = Session::start(
        "tree-flood",
        ExecutionLimits {
            output_bytes: 1024,
            ..ExecutionLimits::default()
        },
        false,
    )?;
    let (fds, _) = session.ready()?;
    session.release()?;
    let error = session.finish().expect_err("output overflow must fail");
    assert!(
        matches!(error.kind, ProcessErrorKind::OutputLimit(_)),
        "{error}"
    );
    assert_eq!(
        error.capture.stdout,
        [b"READY\n".as_slice(), vec![b'O'; 1018].as_slice()].concat()
    );
    assert!(error.cleanup_errors.is_empty(), "{error}");
    all_dead(&fds)
}

#[test]
fn killing_the_transport_owner_also_kills_its_supervisor_and_namespace() -> io::Result<()> {
    let mut session = Session::start("tree-hold", ExecutionLimits::default(), true)?;
    let (fds, ids) = session.ready()?;
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", ids[2]))?;
    let owner: i32 = stat
        .rsplit_once(") ")
        .expect("supervisor stat")
        .1
        .split_whitespace()
        .nth(1)
        .expect("owner PID")
        .parse()
        .expect("owner integer");
    let owner_fd = pidfd(owner)?;
    // SAFETY: signal a stable handle to the fixture's dedicated transport owner,
    // not a numeric PID or a process outside this probe's ownership.
    assert_eq!(
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                owner_fd.as_raw_fd(),
                libc::SIGKILL,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        },
        0
    );
    let output = session
        .finish()
        .expect("outer owner termination observation");
    assert_eq!(output.status.signal(), Some(libc::SIGKILL));
    all_dead(&fds)
}

#[test]
fn payload_exec_failure_is_an_infrastructure_error_after_namespace_teardown() -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let fixture = ShellFixture::new()?;
    let executable = fixture.create_file("invalid-executable", b"not an executable image\n")?;
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))?;
    let error = ProcessCommand::new(executable, fixture.workspace())
        .in_pid_namespace(env!("CARGO_BIN_EXE_fshell-process-supervisor"))
        .run()
        .expect_err("invalid payload must not become an ordinary exit");
    assert!(
        matches!(&error.kind, ProcessErrorKind::Namespace(NamespaceError::Io { stage: NamespaceStage::PayloadSpawn, source }) if source.raw_os_error() == Some(libc::ENOEXEC)),
        "{error}"
    );
    assert!(error.cleanup_errors.is_empty(), "{error}");
    fixture.close()
}

#[test]
fn invalid_supervisor_paths_fail_without_running_the_payload() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    for supervisor in [
        std::path::PathBuf::from("relative-supervisor"),
        fixture.workspace().join("absent-supervisor"),
    ] {
        let error = command(&fixture, "ready-file")
            .arg(fixture.workspace().join("should-not-exist"))
            .in_pid_namespace(supervisor)
            .run()
            .expect_err("invalid helper must not fall back to launch groups");
        assert!(
            matches!(
                error.kind,
                ProcessErrorKind::InvalidInvocation(_)
                    | ProcessErrorKind::Io {
                        operation: "validate namespace supervisor",
                        ..
                    }
            ),
            "{error}"
        );
        assert!(error.status.is_none());
        assert_eq!(
            fixture
                .read_file("should-not-exist")
                .expect_err("payload not executed")
                .kind(),
            io::ErrorKind::NotFound
        );
    }
    fixture.close()
}

#[test]
fn missing_malformed_and_denied_supervisor_handshakes_are_infrastructure_failures() -> io::Result<()>
{
    let fixture = ShellFixture::new()?;
    for mode in [
        "supervisor-silent",
        "supervisor-short",
        "supervisor-finished-first",
        "supervisor-denied",
    ] {
        let error = ProcessCommand::new(
            env!("CARGO_BIN_EXE_fshell-process-probe"),
            fixture.workspace(),
        )
        .in_pid_namespace(env!("CARGO_BIN_EXE_fshell-namespace-probe"))
        .arg(mode)
        .run()
        .expect_err("fault must never become an ordinary exit observation");
        if mode == "supervisor-denied" {
            assert_eq!(error.capture.stderr, b"DENIED\n");
            assert!(
                matches!(error.kind, ProcessErrorKind::Namespace(NamespaceError::Io { stage: NamespaceStage::Unshare, source }) if source.raw_os_error() == Some(libc::EPERM))
            );
        } else {
            assert!(
                matches!(
                    error.kind,
                    ProcessErrorKind::Namespace(NamespaceError::Protocol(_))
                ),
                "{error}"
            );
        }
        assert!(error.cleanup_errors.is_empty());
    }
    fixture.close()
}
