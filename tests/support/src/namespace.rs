//! Linux PID-namespace lifecycle containment, not a filesystem/network sandbox.
//!
//! The supervisor creates an identity-mapped user namespace and a PID namespace.
//! Payload execution starts only after the caller owns a stable init pidfd. Init
//! death kills the namespace's descendants, including new groups/sessions. The
//! supervisor and init have parent-death guards; no migration to an ancestor PID
//! namespace is permitted by the new user namespace. Delegated host services are
//! not descendants and are outside this boundary. UID/GID numbers are preserved;
//! the user namespace still changes the capability context. Only the caller's
//! primary UID/GID are mapped; supplementary group numbers are not preserved.

mod supervisor;

use std::fmt;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixDatagram;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};

pub use supervisor::supervisor_main;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum NamespaceStage {
    Control = 1,
    ParentPidfd = 2,
    Unshare = 3,
    UidMap = 4,
    Setgroups = 5,
    GidMap = 6,
    InitPipe = 7,
    ForkInit = 8,
    InitPidfd = 9,
    ParentDeath = 10,
    PayloadSpawn = 11,
    PayloadWait = 12,
    InitWait = 13,
}

impl NamespaceStage {
    fn from_wire(value: u32) -> Option<Self> {
        Some(match value {
            1 => Self::Control,
            2 => Self::ParentPidfd,
            3 => Self::Unshare,
            4 => Self::UidMap,
            5 => Self::Setgroups,
            6 => Self::GidMap,
            7 => Self::InitPipe,
            8 => Self::ForkInit,
            9 => Self::InitPidfd,
            10 => Self::ParentDeath,
            11 => Self::PayloadSpawn,
            12 => Self::PayloadWait,
            13 => Self::InitWait,
            _ => return None,
        })
    }
}

#[derive(Debug)]
pub enum NamespaceError {
    Io {
        stage: NamespaceStage,
        source: io::Error,
    },
    Protocol(&'static str),
}

impl fmt::Display for NamespaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { stage, source } => write!(f, "namespace {stage:?}: {source}"),
            Self::Protocol(message) => write!(f, "namespace protocol: {message}"),
        }
    }
}

impl std::error::Error for NamespaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Protocol(_) => None,
        }
    }
}

fn at(stage: NamespaceStage, source: io::Error) -> NamespaceError {
    NamespaceError::Io { stage, source }
}

const READY: u32 = 1;
const FINISHED: u32 = 2;
const FAILED: u32 = 3;
const START: u32 = 4;
const FRAME_BYTES: usize = 12;

#[derive(Debug)]
struct Frame {
    tag: u32,
    first: u32,
    second: u32,
}

impl Frame {
    fn bytes(&self) -> [u8; FRAME_BYTES] {
        let mut bytes = [0; FRAME_BYTES];
        bytes[..4].copy_from_slice(&self.tag.to_be_bytes());
        bytes[4..8].copy_from_slice(&self.first.to_be_bytes());
        bytes[8..].copy_from_slice(&self.second.to_be_bytes());
        bytes
    }

    fn parse(bytes: &[u8]) -> Result<Self, NamespaceError> {
        if bytes.len() != FRAME_BYTES {
            return Err(NamespaceError::Protocol("invalid frame length"));
        }
        let word = |offset| {
            u32::from_be_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ])
        };
        Ok(Self {
            tag: word(0),
            first: word(4),
            second: word(8),
        })
    }
}

// Ancillary storage is word-aligned for cmsghdr. Truncated/extra rights are
// rejected after every delivered descriptor has been adopted into an owner.
fn receive(socket: &UnixDatagram) -> Result<(Frame, Vec<OwnedFd>), NamespaceError> {
    let mut bytes = [0; FRAME_BYTES + 1];
    let mut ancillary = [0usize; 16];
    let mut iov = libc::iovec {
        iov_base: bytes.as_mut_ptr().cast(),
        iov_len: bytes.len(),
    };
    // SAFETY: zero is the empty state for msghdr; pointers reference live storage.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = ancillary.as_mut_ptr().cast();
    message.msg_controllen = std::mem::size_of_val(&ancillary);
    // SAFETY: all buffers are owned and valid for the duration of recvmsg.
    let size = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC) };
    if size < 0 {
        return Err(at(NamespaceStage::Control, io::Error::last_os_error()));
    }
    let mut rights = Vec::new();
    let mut invalid = false;
    // SAFETY: recvmsg initialized the bounded ancillary buffer; CMSG_NXTHDR
    // validates lengths. SCM_RIGHTS contains newly owned integer descriptors.
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(&message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let base = libc::CMSG_LEN(0) as usize;
                let len = (*header).cmsg_len.saturating_sub(base);
                if !len.is_multiple_of(std::mem::size_of::<RawFd>()) {
                    invalid = true;
                }
                for index in 0..len / std::mem::size_of::<RawFd>() {
                    let fd = std::ptr::read_unaligned(
                        libc::CMSG_DATA(header).cast::<RawFd>().add(index),
                    );
                    rights.push(OwnedFd::from_raw_fd(fd));
                }
            } else {
                invalid = true;
            }
            header = libc::CMSG_NXTHDR(&message, header);
        }
    }
    if invalid || message.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0 {
        return Err(NamespaceError::Protocol(
            "invalid or truncated ancillary data",
        ));
    }
    Ok((Frame::parse(&bytes[..size as usize])?, rights))
}

fn send(socket: &UnixDatagram, frame: Frame, right: Option<RawFd>) -> Result<(), NamespaceError> {
    let bytes = frame.bytes();
    let mut ancillary = [0usize; 16];
    let mut iov = libc::iovec {
        iov_base: bytes.as_ptr().cast_mut().cast(),
        iov_len: bytes.len(),
    };
    // SAFETY: zero is the empty state for msghdr; pointers reference live storage.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    if let Some(fd) = right {
        message.msg_control = ancillary.as_mut_ptr().cast();
        // SAFETY: storage is aligned and large enough for a single integer right.
        unsafe {
            message.msg_controllen = libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) as usize;
            let header = libc::CMSG_FIRSTHDR(&message);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as usize;
            std::ptr::write_unaligned(libc::CMSG_DATA(header).cast::<RawFd>(), fd);
        }
    }
    // SAFETY: storage is live; MSG_NOSIGNAL makes a dead peer an I/O error.
    let size = unsafe { libc::sendmsg(socket.as_raw_fd(), &message, libc::MSG_NOSIGNAL) };
    if size < 0 {
        return Err(at(NamespaceStage::Control, io::Error::last_os_error()));
    }
    if size as usize != bytes.len() {
        return Err(NamespaceError::Protocol("partial datagram write"));
    }
    Ok(())
}

pub(crate) fn pidfd(pid: libc::pid_t) -> io::Result<OwnedFd> {
    // SAFETY: the kernel validates the PID and returns a new CLOEXEC descriptor.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as RawFd;
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: one successful syscall transferred one new descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn signal_pidfd(fd: RawFd, signal: i32) -> io::Result<()> {
    // SAFETY: pidfd_send_signal validates the descriptor; no siginfo is supplied.
    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            fd,
            signal,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(crate) fn dead(fd: RawFd, timeout_ms: i32) -> io::Result<bool> {
    let mut poll = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: poll points to one live element referring to an owned pidfd.
    let result = unsafe { libc::poll(&mut poll, 1, timeout_ms) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    if poll.revents & libc::POLLNVAL != 0 {
        return Err(io::Error::other("invalid pidfd"));
    }
    Ok(result > 0 && poll.revents & (libc::POLLIN | libc::POLLHUP) != 0)
}

fn terminal_status(raw: u32) -> Result<ExitStatus, NamespaceError> {
    // Linux terminal wait statuses encode either an 8-bit exit code or a
    // signal (1..=64) with an optional core bit. Stops and reserved bits fail.
    let exit = raw <= 0xff00 && raw & 0xff == 0;
    let signal = raw & 0x7f;
    let signalled = raw <= 0xff && (1..=64).contains(&signal);
    if !exit && !signalled {
        return Err(NamespaceError::Protocol("invalid terminal payload status"));
    }
    Ok(ExitStatus::from_raw(raw as i32))
}

pub(crate) struct NamespaceControl {
    socket: UnixDatagram,
    launch_socket: Option<UnixDatagram>,
    init: Option<OwnedFd>,
    status: Option<ExitStatus>,
    cleanup_attempted: bool,
}

impl NamespaceControl {
    pub(crate) fn new() -> io::Result<Self> {
        let (socket, child) = UnixDatagram::pair()?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            launch_socket: Some(child),
            init: None,
            status: None,
            cleanup_attempted: false,
        })
    }

    pub(crate) fn configure(&self, command: &mut Command) -> io::Result<()> {
        let fd = self
            .launch_socket
            .as_ref()
            .ok_or_else(|| io::Error::other("missing launch socket"))?
            .as_raw_fd();
        command.args(["--control-fd", &fd.to_string(), "--"]);
        // SAFETY: getpid has no preconditions; the child must retain this parent.
        let parent = unsafe { libc::getpid() };
        // SAFETY: only async-signal-safe syscalls execute in this pre-exec hook.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(io::Error::from_raw_os_error(libc::EPIPE));
                }
                if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(())
    }

    pub(crate) fn spawned(&mut self) {
        self.launch_socket = None;
    }

    pub(crate) fn fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }

    pub(crate) fn require_finished(&self) -> Result<(), NamespaceError> {
        self.status.map(|_| ()).ok_or(NamespaceError::Protocol(
            "supervisor exited without a complete handshake",
        ))
    }

    pub(crate) fn pump(&mut self) -> Result<(), NamespaceError> {
        // A bounded number of protocol records cannot starve stream deadlines.
        for _ in 0..4 {
            let (frame, mut rights) = match receive(&self.socket) {
                Err(NamespaceError::Io { source, .. })
                    if source.kind() == io::ErrorKind::WouldBlock =>
                {
                    return Ok(());
                }
                Err(NamespaceError::Io { source, .. })
                    if source.kind() == io::ErrorKind::Interrupted =>
                {
                    continue;
                }
                result => result?,
            };
            match frame.tag {
                READY
                    if self.init.is_none()
                        && self.status.is_none()
                        && rights.len() == 1
                        && frame.first == 0
                        && frame.second == 0 =>
                {
                    let fd = rights.remove(0);
                    signal_pidfd(fd.as_raw_fd(), 0)
                        .map_err(|source| at(NamespaceStage::InitPidfd, source))?;
                    self.init = Some(fd);
                    send(
                        &self.socket,
                        Frame {
                            tag: START,
                            first: 0,
                            second: 0,
                        },
                        None,
                    )?;
                }
                FINISHED
                    if self.init.is_some()
                        && self.status.is_none()
                        && rights.is_empty()
                        && frame.second == 0 =>
                {
                    self.status = Some(terminal_status(frame.first)?);
                }
                FAILED if rights.is_empty() && frame.second != 0 => {
                    let stage = NamespaceStage::from_wire(frame.first)
                        .ok_or(NamespaceError::Protocol("unknown failure stage"))?;
                    return Err(at(stage, io::Error::from_raw_os_error(frame.second as i32)));
                }
                _ => {
                    return Err(NamespaceError::Protocol(
                        "unexpected protocol record or descriptor count",
                    ));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn completed_status(
        &self,
        supervisor: ExitStatus,
    ) -> Result<ExitStatus, NamespaceError> {
        if !supervisor.success() {
            return Err(NamespaceError::Protocol(
                "supervisor terminated without successful cleanup",
            ));
        }
        self.status.ok_or(NamespaceError::Protocol(
            "supervisor exited without a complete handshake",
        ))
    }

    pub(crate) fn cleanup(&mut self) -> io::Result<()> {
        self.cleanup_attempted = true;
        if let Some(fd) = &self.init {
            if let Err(error) = signal_pidfd(fd.as_raw_fd(), libc::SIGKILL)
                && error.raw_os_error() != Some(libc::ESRCH)
            {
                return Err(error);
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                match dead(fd.as_raw_fd(), 20) {
                    Ok(true) => return Ok(()),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error),
                    Ok(false) => {}
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::other(
                        "namespace init death was not confirmed within cleanup deadline",
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Drop for NamespaceControl {
    fn drop(&mut self) {
        // Covers unwinding too; normal run() separately reports cleanup errors.
        if !self.cleanup_attempted
            && let Err(error) = self.cleanup()
        {
            eprintln!("namespace drop cleanup failed: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_statuses_reject_stops_reserved_bits_and_invalid_signal_numbers() {
        assert_eq!(terminal_status(37 << 8).expect("exit").code(), Some(37));
        assert_eq!(
            terminal_status(libc::SIGTERM as u32)
                .expect("signal")
                .signal(),
            Some(libc::SIGTERM)
        );
        for raw in [0x137f, 0xffff, 0x10000, u32::MAX, 128, 65, 127] {
            assert!(terminal_status(raw).is_err(), "{raw:#x}");
        }
    }

    #[test]
    fn protocol_rejects_short_long_unknown_and_out_of_order_records() {
        assert!(Frame::parse(&[0; 11]).is_err());
        assert!(Frame::parse(&[0; 13]).is_err());
        for tag in [READY, FINISHED, START, 99] {
            let mut control = NamespaceControl::new().expect("control pair");
            let peer = control.launch_socket.take().expect("peer");
            send(
                &peer,
                Frame {
                    tag,
                    first: 0,
                    second: 0,
                },
                None,
            )
            .expect("fault record");
            assert!(matches!(control.pump(), Err(NamespaceError::Protocol(_))));
        }
    }

    #[test]
    fn failure_records_preserve_the_stage_and_kernel_errno() {
        let mut control = NamespaceControl::new().expect("control pair");
        let peer = control.launch_socket.take().expect("peer");
        send(
            &peer,
            Frame {
                tag: FAILED,
                first: NamespaceStage::Unshare as u32,
                second: libc::EPERM as u32,
            },
            None,
        )
        .expect("error record");
        assert!(
            matches!(control.pump(), Err(NamespaceError::Io { stage: NamespaceStage::Unshare, source }) if source.raw_os_error() == Some(libc::EPERM))
        );
    }

    #[test]
    fn transferred_rights_are_cloexec_and_invalid_pidfds_are_rejected() {
        let mut control = NamespaceControl::new().expect("control pair");
        let peer = control.launch_socket.take().expect("peer");
        let file = std::fs::File::open("/dev/null").expect("non-pidfd descriptor");
        send(
            &peer,
            Frame {
                tag: READY,
                first: 0,
                second: 0,
            },
            Some(file.as_raw_fd()),
        )
        .expect("rights transfer");
        let (frame, rights) = receive(&control.socket).expect("received rights");
        assert_eq!(frame.tag, READY);
        assert_eq!(rights.len(), 1);
        // SAFETY: inspect the owned received descriptor's flags.
        assert_ne!(
            unsafe { libc::fcntl(rights[0].as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        drop(rights);
        send(
            &peer,
            Frame {
                tag: READY,
                first: 0,
                second: 0,
            },
            Some(file.as_raw_fd()),
        )
        .expect("invalid pidfd");
        assert!(matches!(
            control.pump(),
            Err(NamespaceError::Io {
                stage: NamespaceStage::InitPidfd,
                ..
            })
        ));
    }
}
