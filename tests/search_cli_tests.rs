use std::process::{Command, Output};

fn native(script: &str, config: &std::path::Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fsh"))
        .args(["--native", "--no-color", "-c", script])
        .env("FSH_CONFIG_DIR", config)
        .output()
        .unwrap()
}

#[test]
fn flagless_grep_consumes_files_and_expands_directory_components() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("executed");
    for policy in ["policy0", "policy4"] {
        std::fs::create_dir(tmp.path().join(policy)).unwrap();
        let file = tmp.path().join(policy).join("scaling_cur_freq");
        std::fs::write(&file, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let result = native(
        &format!(
            "grep touch {}/policy*/scaling_cur_freq",
            tmp.path().display()
        ),
        tmp.path(),
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        result
            .stdout
            .split(|b| *b == b'\n')
            .filter(|s| !s.is_empty())
            .count(),
        2
    );
    assert!(!marker.exists(), "grep executed its filename argument");
}

#[test]
fn supported_text_modes_agree_with_system_grep() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("input");
    std::fs::write(&file, "before\nAlpha\nother\nalpha\nafter\n\nfinal").unwrap();
    for flags in [
        "",
        "-n",
        "-c",
        "-q",
        "-i",
        "-v",
        "-F",
        "-w",
        "-x",
        "-C 1",
        "-m 1",
        "-m 1 -A 1",
        "-m 1 -C 1",
        "-i -m 1 -A 2",
        "-Hn",
    ] {
        let native = native(
            &format!("grep {flags} alpha '{}'", file.display()),
            tmp.path(),
        );
        let mut oracle = Command::new("/usr/bin/grep");
        oracle
            .args(flags.split_whitespace())
            .arg("alpha")
            .arg(&file);
        let oracle = oracle.output().unwrap();
        assert_eq!(
            native.status.code(),
            oracle.status.code(),
            "status for {flags}: {}",
            String::from_utf8_lossy(&native.stderr)
        );
        assert_eq!(native.stdout, oracle.stdout, "output for {flags}");
    }
}

#[test]
fn grep_reports_distinct_no_match_and_error_statuses() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("input");
    std::fs::write(&file, "alpha\n").unwrap();
    for (pattern, status) in [("alpha", 0), ("absent", 1), ("[", 2)] {
        let output = native(
            &format!("grep '{pattern}' '{}'", file.display()),
            tmp.path(),
        );
        assert_eq!(
            output.status.code(),
            Some(status),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = native(
        &format!("grep alpha '{}/missing'", tmp.path().display()),
        tmp.path(),
    );
    assert_eq!(output.status.code(), Some(2));
    let output = native(
        &format!("grep --unknown alpha '{}'", file.display()),
        tmp.path(),
    );
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn byte_stream_grep_preserves_long_line_boundaries() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("input");
    let mut line = b"A".to_vec();
    line.extend(std::iter::repeat_n(b'x', 200000));
    line.extend_from_slice(b"Z\n");
    std::fs::write(&file, line).unwrap();
    let output = native(
        &format!("/usr/bin/cat '{}' | grep '^Ax*Z$' | count", file.display()),
        tmp.path(),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"1\n");
}

#[test]
fn external_command_prefix_preserves_the_child_status() {
    let tmp = tempfile::tempdir().unwrap();
    let output = native("command sh -c 'exit 7'", tmp.path());
    assert_eq!(output.status.code(), Some(7));
    let output = native("command grep -E '[' /dev/null", tmp.path());
    assert_eq!(output.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("FSH-BLT"));
}

#[test]
fn explicit_symlinks_are_searched_without_following_recursive_symlinks() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("tree")).unwrap();
    std::fs::write(tmp.path().join("tree/value"), "alpha\n").unwrap();
    std::os::unix::fs::symlink(tmp.path().join("tree/value"), tmp.path().join("file-link"))
        .unwrap();
    std::os::unix::fs::symlink(tmp.path().join("tree"), tmp.path().join("directory-link")).unwrap();
    for command in [
        format!("grep alpha '{}/file-link'", tmp.path().display()),
        format!("grep -r alpha '{}/directory-link'", tmp.path().display()),
    ] {
        let output = native(&command, tmp.path());
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("alpha"));
    }
}
