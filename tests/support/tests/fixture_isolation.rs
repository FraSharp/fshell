use fshell_test_support::fixture::ShellFixture;
use std::ffi::OsString;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

const PROBE: &str = env!("CARGO_BIN_EXE_fshell-process-probe");

#[test]
fn nested_files_preserve_bytes_and_existing_contents() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let bytes = [0, 255, b'\n'];
    fixture.create_file("nested/input", &bytes)?;
    assert_eq!(fixture.read_file("nested/input")?, bytes);
    assert_eq!(
        fixture
            .create_file("nested/input", b"overwrite")
            .expect_err("existing file")
            .kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(fixture.read_file("nested/input")?, bytes);
    fixture.close()
}

#[test]
fn absolute_parent_empty_and_nul_paths_are_rejected() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    for path in [
        Path::new(""),
        Path::new("."),
        Path::new("../escape"),
        Path::new("a/../../escape"),
        Path::new("/absolute"),
    ] {
        assert_eq!(
            fixture
                .create_file(path, b"data")
                .expect_err("unsafe path")
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            fixture.read_file(path).expect_err("unsafe path").kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            fixture
                .create_directory(path)
                .expect_err("unsafe path")
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }
    let nul = OsString::from_vec(vec![b'a', 0, b'b']);
    assert_eq!(
        fixture
            .create_file(&nul, b"data")
            .expect_err("NUL path")
            .kind(),
        io::ErrorKind::InvalidInput
    );
    fixture.close()
}

#[test]
fn non_unicode_paths_are_supported() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let name = OsString::from_vec(vec![b'f', 255]);
    fixture.create_file(&name, b"contents")?;
    assert_eq!(fixture.read_file(&name)?, b"contents");
    fixture.close()
}

#[test]
fn symlink_parents_cannot_escape_preparation_or_observation() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let outside = tempfile::tempdir()?;
    std::fs::write(outside.path().join("existing"), b"outside")?;
    std::os::unix::fs::symlink(outside.path(), fixture.workspace().join("link"))?;
    assert!(fixture.create_file("link/new", b"bad").is_err());
    assert!(fixture.create_directory("link/new-directory").is_err());
    assert!(fixture.read_file("link/existing").is_err());
    assert!(!outside.path().join("new").exists());
    assert!(!outside.path().join("new-directory").exists());
    assert_eq!(std::fs::read(outside.path().join("existing"))?, b"outside");
    fixture.close()?;
    outside.close()
}

#[test]
fn final_symlinks_cannot_overwrite_or_read_external_files() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let outside = tempfile::tempdir()?;
    let target = outside.path().join("target");
    std::fs::write(&target, b"outside")?;
    std::os::unix::fs::symlink(&target, fixture.workspace().join("link"))?;
    assert!(fixture.create_file("link", b"bad").is_err());
    assert!(fixture.read_file("link").is_err());
    assert_eq!(std::fs::read(&target)?, b"outside");
    fixture.close()?;
    outside.close()
}

#[test]
fn missing_files_are_errors_not_sentinel_contents() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    assert_eq!(
        fixture
            .read_file("absent")
            .expect_err("missing file")
            .kind(),
        io::ErrorKind::NotFound
    );
    fixture.create_file("literal", b"<missing>")?;
    assert_eq!(fixture.read_file("literal")?, b"<missing>");
    fixture.close()
}

#[test]
fn non_regular_files_are_rejected_without_blocking_on_a_fifo() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let path = std::ffi::CString::new(fixture.workspace().join("fifo").as_os_str().as_bytes())
        .expect("fixture path contains no NUL");
    // SAFETY: valid fixture-only pathname; no host resource is affected.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    assert_eq!(
        fixture.read_file("fifo").expect_err("not regular").kind(),
        io::ErrorKind::InvalidInput
    );
    fixture.create_directory("directory")?;
    assert_eq!(
        fixture
            .read_file("directory")
            .expect_err("not regular")
            .kind(),
        io::ErrorKind::InvalidInput
    );
    fixture.close()
}

#[test]
fn fixture_observation_is_bounded() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    fixture.create_file("large", b"")?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(fixture.workspace().join("large"))?
        .set_len(16 * 1024 * 1024 + 1)?;
    assert_eq!(
        fixture
            .read_file("large")
            .expect_err("observation limit")
            .kind(),
        io::ErrorKind::InvalidInput
    );
    fixture.close()
}

#[test]
fn fixture_environment_is_explicit_and_uses_production_noninteractive_behavior() -> io::Result<()> {
    let parent_cwd = std::env::current_dir()?;
    let parent_home = std::env::var_os("HOME");
    let fixture = ShellFixture::new()?;
    for key in [
        "CI",
        "FSH_TEST_ENV",
        "FSH_ENGINE_TRACE",
        "FSH_TRACE_FILE",
        "FSH_SESSION_LOG",
    ] {
        let output = fixture
            .command(PROBE)
            .args(["environment", key])
            .run()
            .expect("environment probe");
        assert!(output.status.success());
        assert_eq!(output.capture.stdout, [0], "{key} should be absent");
        assert_eq!(output.capture.stderr, []);
    }
    for (key, value) in [
        ("HOME", fixture.home()),
        ("FSH_CONFIG_DIR", fixture.config()),
        ("FSH_TEST_DB_PATH", fixture.state().join("history.db")),
        ("FSH_Z_DB_PATH", fixture.state().join("frecency.json")),
        ("TMPDIR", fixture.temporary()),
    ] {
        let output = fixture
            .command(PROBE)
            .args(["environment", key])
            .run()
            .expect("environment probe");
        assert!(output.status.success());
        let mut expected = vec![1];
        expected.extend_from_slice(value.as_os_str().as_bytes());
        assert_eq!(output.capture.stdout, expected, "{key}");
    }
    let output = fixture
        .command(PROBE)
        .args(["environment", "TERM"])
        .run()
        .expect("terminal profile probe");
    assert_eq!(output.capture.stdout, b"\x01xterm-256color");
    assert_eq!(std::env::current_dir()?, parent_cwd);
    assert_eq!(std::env::var_os("HOME"), parent_home);
    fixture.close()
}

#[test]
fn concurrent_fixtures_do_not_share_files_or_state_paths() -> io::Result<()> {
    let workers: Vec<_> = (0..8)
        .map(|n| {
            std::thread::spawn(move || -> io::Result<_> {
                let fixture = ShellFixture::new()?;
                let value = n.to_string();
                fixture.create_file("same-name", value.as_bytes())?;
                assert_eq!(fixture.read_file("same-name")?, value.as_bytes());
                let state = fixture.state();
                fixture.close()?;
                Ok(state)
            })
        })
        .collect();
    let mut states = Vec::new();
    // Join every worker even if one failed; no detached fixture lifetimes.
    let mut errors = Vec::new();
    for worker in workers {
        match worker.join() {
            Ok(Ok(state)) => states.push(state),
            Ok(Err(error)) => errors.push(error.to_string()),
            Err(_) => errors.push("fixture worker panicked".into()),
        }
    }
    assert!(errors.is_empty(), "{errors:?}");
    states.sort();
    states.dedup();
    assert_eq!(states.len(), 8);
    Ok(())
}

#[test]
fn explicit_close_removes_the_entire_fixture() -> io::Result<()> {
    let fixture = ShellFixture::new()?;
    let root = fixture.root().to_path_buf();
    fixture.create_file("nested/file", b"contents")?;
    fixture.close()?;
    assert!(!root.exists());
    Ok(())
}
