//! Child-only filesystem and environment policy for executable specifications.
//!
//! This is an isolated fixture, not a kernel sandbox. Preparation helpers use
//! descriptor-relative paths and refuse symlink traversal. They create new files
//! only; expected effects must be read as bytes or explicit I/O errors.

use crate::process::ProcessCommand;
use std::ffi::{CString, OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use tempfile::TempDir;

const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug)]
pub struct ShellFixture {
    directory: TempDir,
    root: PathBuf,
}

impl ShellFixture {
    pub fn new() -> io::Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("fshell-specification-")
            .tempdir()?;
        let root = directory.path().canonicalize()?;
        for name in ["work", "home", "config", "cache", "data", "state", "tmp"] {
            std::fs::create_dir(root.join(name))?;
        }
        Ok(Self { directory, root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn workspace(&self) -> PathBuf {
        self.root.join("work")
    }
    pub fn home(&self) -> PathBuf {
        self.root.join("home")
    }
    pub fn config(&self) -> PathBuf {
        self.root.join("config")
    }
    pub fn state(&self) -> PathBuf {
        self.root.join("state")
    }
    pub fn temporary(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// Use production noninteractive behavior: no CI/FSH_TEST_ENV/TERM=dumb.
    /// A fresh process receives no ambient exports, tracing, aliases or history
    /// overrides. Override individual variables explicitly when testing them.
    pub fn command(&self, executable: impl AsRef<Path>) -> ProcessCommand {
        ProcessCommand::new(executable.as_ref(), self.workspace())
            .env("HOME", self.home())
            .env("PWD", self.workspace())
            .env("XDG_CONFIG_HOME", self.config())
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_STATE_HOME", self.state())
            .env("TMPDIR", self.temporary())
            .env("FSH_CONFIG_DIR", self.config())
            .env("FSH_TEST_DB_PATH", self.state().join("history.db"))
            .env("FSH_Z_DB_PATH", self.state().join("frecency.json"))
            .env("FSH_BINARY_PATH", executable.as_ref())
            .env("PATH", "/usr/bin:/bin")
            .env("SHELL", "/bin/sh")
            .env("USER", "fshell-specification")
            .env("LOGNAME", "fshell-specification")
            .env("HOSTNAME", "fshell-specification")
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .env("TZ", "UTC")
            .env("TERM", "xterm-256color")
    }

    /// Create a new workspace file, creating parents without following symlinks.
    /// Existing files are rejected rather than accidentally overwriting effects.
    pub fn create_file(&self, relative: impl AsRef<Path>, bytes: &[u8]) -> io::Result<PathBuf> {
        let components = relative_components(relative.as_ref())?;
        let (parents, filename) = split_filename(&components)?;
        let parent = self.open_directory(parents, true)?;
        let filename = c_string(filename)?;
        // SAFETY: parent and filename remain live; O_EXCL refuses all existing
        // entries, including symlinks. The returned descriptor is owned once.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                filename.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600 as libc::mode_t,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        file.write_all(bytes)?;
        Ok(self.workspace().join(relative))
    }

    pub fn create_directory(&self, relative: impl AsRef<Path>) -> io::Result<PathBuf> {
        let components = relative_components(relative.as_ref())?;
        self.open_directory(&components, true)?;
        Ok(self.workspace().join(relative))
    }

    /// Read a regular workspace file as bytes, refusing symlinks and unbounded
    /// reads. Missing, unreadable and non-regular files remain distinct errors.
    pub fn read_file(&self, relative: impl AsRef<Path>) -> io::Result<Vec<u8>> {
        let components = relative_components(relative.as_ref())?;
        let (parents, filename) = split_filename(&components)?;
        let parent = self.open_directory(parents, false)?;
        let filename = c_string(filename)?;
        // SAFETY: descriptor-relative open with a live directory/name. NONBLOCK
        // prevents a malicious FIFO from blocking before metadata validation.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                filename.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(invalid("fixture observation requires a regular file"));
        }
        if metadata.len() > MAX_FILE_BYTES {
            return Err(invalid("fixture file exceeds 16 MiB observation limit"));
        }
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(invalid("fixture file exceeds 16 MiB observation limit"));
        }
        Ok(bytes)
    }

    /// Report ordinary teardown errors. Drop still provides best-effort cleanup
    /// during unwinding; successful specifications should explicitly close.
    pub fn close(self) -> io::Result<()> {
        self.directory.close()
    }

    fn open_directory(&self, components: &[OsString], create: bool) -> io::Result<File> {
        let mut directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(self.workspace())?;
        for component in components {
            let name = c_string(component)?;
            if create {
                // SAFETY: live parent/name; an existing entry is validated by
                // the subsequent directory-only, no-follow open, not trusted.
                let rc = unsafe {
                    libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700 as libc::mode_t)
                };
                if rc < 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::AlreadyExists {
                        return Err(error);
                    }
                }
            }
            let fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: successful openat transfers one new owned descriptor.
            directory = unsafe { File::from_raw_fd(fd) };
        }
        Ok(directory)
    }
}

fn invalid(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, reason)
}

fn relative_components(path: &Path) -> io::Result<Vec<OsString>> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => {
                c_string(name)?;
                parts.push(name.to_owned());
            }
            _ => {
                return Err(invalid(
                    "fixture paths must contain only relative normal components",
                ));
            }
        }
    }
    if parts.is_empty() {
        return Err(invalid("fixture path must not be empty"));
    }
    Ok(parts)
}

fn split_filename(components: &[OsString]) -> io::Result<(&[OsString], &OsStr)> {
    components
        .split_last()
        .map(|(last, parents)| (parents, last.as_os_str()))
        .ok_or_else(|| invalid("fixture path must name a file"))
}

fn c_string(value: impl AsRef<OsStr>) -> io::Result<CString> {
    CString::new(value.as_ref().as_bytes())
        .map_err(|_| invalid("fixture paths must not contain NUL"))
}
