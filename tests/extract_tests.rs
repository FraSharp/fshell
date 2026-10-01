#![cfg(feature = "extract")]

mod common;
use common::FshCmd;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> &'static [u8] {
    match name {
        "zip" => include_bytes!("fixtures/archives/hello.zip"),
        "7z" => include_bytes!("fixtures/archives/hello.7z"),
        "tar" => include_bytes!("fixtures/archives/hello.tar"),
        "gzip" => include_bytes!("fixtures/archives/hello.tar.gz"),
        "xz" => include_bytes!("fixtures/archives/hello.tar.xz"),
        "zstd" => include_bytes!("fixtures/archives/hello.tar.zst"),
        "raw-xz" => include_bytes!("fixtures/archives/hello.txt.xz"),
        _ => panic!("unknown fixture: {name}"),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Directory,
    IncrementalDirectory,
    Symlink,
    Hardlink,
    Fifo,
}

fn octal(field: &mut [u8], value: u64) {
    let text = format!("{:0width$o}\0", value, width = field.len() - 1);
    field.copy_from_slice(text.as_bytes());
}

/// Construct a tiny ustar archive without depending on system `tar` or on
/// another copy of libarchive (which would collide with the decoder's FFI).
fn make_tar_entries(path: &Path, entries: &[(&str, Kind, Option<&str>)]) {
    let mut archive = Vec::new();
    for &(name, kind, link) in entries {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        octal(
            &mut header[100..108],
            if kind == Kind::Directory {
                0o755
            } else {
                0o644
            },
        );
        octal(&mut header[108..116], 0);
        octal(&mut header[116..124], 0);
        octal(
            &mut header[124..136],
            if matches!(kind, Kind::File | Kind::IncrementalDirectory) {
                4
            } else {
                0
            },
        );
        octal(&mut header[136..148], 0);
        header[148..156].fill(b' ');
        header[156] = match kind {
            Kind::File => b'0',
            Kind::Directory => b'5',
            Kind::IncrementalDirectory => b'D',
            Kind::Symlink => b'2',
            Kind::Hardlink => b'1',
            Kind::Fifo => b'6',
        };
        if let Some(link) = link {
            header[157..157 + link.len()].copy_from_slice(link.as_bytes());
        }
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let checksum: u32 = header.iter().map(|b| u32::from(*b)).sum();
        let text = format!("{checksum:06o}\0 ");
        header[148..156].copy_from_slice(text.as_bytes());
        archive.extend_from_slice(&header);
        if matches!(kind, Kind::File | Kind::IncrementalDirectory) {
            archive.extend_from_slice(b"data");
            archive.resize(archive.len().next_multiple_of(512), 0);
        }
    }
    archive.extend_from_slice(&[0u8; 1024]);
    std::fs::write(path, archive).unwrap();
}

fn run(archive: &Path, output: &Path, extra: &str) -> common::FshOutput {
    let command = format!("extract {extra} -- '{}'", archive.display());
    FshCmd::new()
        .arg("--native")
        .cmd(&command)
        .current_dir(output)
        .run()
        .unwrap()
}

fn setup() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let output = tmp.path().join("output");
    std::fs::create_dir(&output).unwrap();
    (tmp, output)
}

#[test]
fn detects_archives_from_data_not_extension() {
    for (name, format) in [
        ("archive.zip", "zip"),
        ("misleading.data", "gzip"),
        ("archive.tar.xz", "xz"),
        ("archive.tar.zst", "zstd"),
        ("archive.7z", "7z"),
    ] {
        let (tmp, output) = setup();
        let archive = tmp.path().join(name);
        std::fs::write(&archive, fixture(format)).unwrap();
        run(&archive, &output, "").assert_success();
        assert_eq!(
            std::fs::read(output.join("sub/hello.txt")).unwrap(),
            b"hello archive\n",
            "{name}"
        );
    }
}

#[test]
fn decodes_a_standalone_compressed_stream() {
    let (tmp, output) = setup();
    let archive = tmp.path().join("hello.txt.xz");
    std::fs::write(&archive, fixture("raw-xz")).unwrap();
    run(&archive, &output, "").assert_success();
    assert_eq!(
        std::fs::read(output.join("hello.txt")).unwrap(),
        b"hello archive\n"
    );
}

#[test]
fn destination_option_and_existing_files() {
    let (tmp, output) = setup();
    let archive = tmp.path().join("archive.zip");
    std::fs::write(&archive, fixture("zip")).unwrap();
    let target = tmp.path().join("target");
    std::fs::create_dir(&target).unwrap();
    let result = run(&archive, &output, &format!("-C '{}'", target.display()));
    result.assert_success();
    assert!(target.join("sub/hello.txt").exists());
    assert!(!output.join("sub/hello.txt").exists());

    std::fs::write(target.join("sub/hello.txt"), b"preserved").unwrap();
    run(&archive, &output, &format!("-C '{}'", target.display()))
        .assert_failure()
        .assert_stderr_contains("already exists");
    assert_eq!(
        std::fs::read(target.join("sub/hello.txt")).unwrap(),
        b"preserved"
    );
}

#[test]
fn refuses_traversal_and_outside_links_without_publishing() {
    for (name, kind, link) in [
        ("../escape.txt", Kind::File, None),
        ("/tmp/escape.txt", Kind::File, None),
        ("link", Kind::Symlink, Some("../../outside")),
        ("link", Kind::Symlink, Some("/tmp")),
        ("alias", Kind::Hardlink, Some("../outside")),
        ("device", Kind::Fifo, None),
    ] {
        let (tmp, output) = setup();
        let archive = tmp.path().join("malicious.tar");
        make_tar_entries(
            &archive,
            &[("good.txt", Kind::File, None), (name, kind, link)],
        );
        let result = run(&archive, &output, "");
        result.assert_failure();
        assert!(
            !output.join("good.txt").exists(),
            "published good.txt before rejecting {name}: {result:?}"
        );
        assert!(!tmp.path().join("escape.txt").exists());
    }
}

#[test]
fn corrupt_archive_never_publishes_earlier_entries() {
    let (tmp, output) = setup();
    let archive = tmp.path().join("truncated.tar");
    make_tar_entries(
        &archive,
        &[
            ("first.txt", Kind::File, None),
            ("second.txt", Kind::File, None),
        ],
    );
    let bytes = std::fs::read(&archive).unwrap();
    std::fs::write(&archive, &bytes[..512 + 512 + 512 + 2]).unwrap();
    run(&archive, &output, "").assert_failure();
    assert!(!output.join("first.txt").exists());
    assert!(!output.join("second.txt").exists());
}

#[test]
fn internal_symlinks_and_hardlinks_preserve_contents() {
    let (tmp, output) = setup();
    let archive = tmp.path().join("links.tar");
    make_tar_entries(
        &archive,
        &[
            ("dir/", Kind::Directory, None),
            ("dir/file", Kind::File, None),
            ("dir/alias", Kind::Symlink, Some("file")),
            ("dir/hard", Kind::Hardlink, Some("dir/file")),
        ],
    );
    run(&archive, &output, "").assert_success();
    assert_eq!(std::fs::read(output.join("dir/alias")).unwrap(), b"data");
    assert_eq!(std::fs::read(output.join("dir/hard")).unwrap(), b"data");
}

#[test]
fn refuses_symlinks_in_existing_destination_and_bad_input() {
    let (tmp, output) = setup();
    let archive = tmp.path().join("archive.zip");
    std::fs::write(&archive, fixture("zip")).unwrap();
    let outside = tmp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, output.join("sub")).unwrap();
    run(&archive, &output, "").assert_failure();
    assert!(!outside.join("hello.txt").exists());

    let not_archive = tmp.path().join("junk.7z");
    std::fs::write(&not_archive, b"this is not an archive").unwrap();
    run(&not_archive, &output, "").assert_failure();
}

#[test]
fn enforces_decoded_size_and_entry_limits() {
    let (tmp, output) = setup();
    let archive = tmp.path().join("archive.tar");
    std::fs::write(&archive, fixture("tar")).unwrap();
    run(&archive, &output, "--max-bytes 3")
        .assert_failure()
        .assert_stderr_contains("limit exceeded");
    assert!(!output.join("sub/hello.txt").exists());
    run(&archive, &output, "--max-entries 0")
        .assert_failure()
        .assert_stderr_contains("limit exceeded");
    assert!(!output.join("sub/hello.txt").exists());
}

#[test]
fn counts_payload_on_non_regular_entries() {
    let (tmp, output) = setup();
    let archive = tmp.path().join("directory-with-data.tar");
    // GNU incremental directory records may carry data. Unlike ordinary tar
    // directory entries, libarchive preserves their payload size for reading.
    make_tar_entries(&archive, &[("empty/", Kind::IncrementalDirectory, None)]);
    run(&archive, &output, "--max-bytes 3")
        .assert_failure()
        .assert_stderr_contains("limit exceeded");
    assert!(!output.join("empty").exists());
}

#[test]
fn rejects_extra_paths_instead_of_ignoring_them() {
    let (tmp, output) = setup();
    let archive = tmp.path().join("archive.zip");
    std::fs::write(&archive, fixture("zip")).unwrap();
    run(&archive, &output, "another.zip").assert_failure();
    assert!(!output.join("sub/hello.txt").exists());
}
