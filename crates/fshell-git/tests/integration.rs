// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_git::repo::Repository;
use std::fs;

fn git(dir: &std::path::Path, args: &[&str]) {
    std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .unwrap();
}

#[test]
fn real_repo_status() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    git(dir, &["init"]);
    fs::write(dir.join("tracked.txt"), "hello").unwrap();
    fs::write(dir.join("untracked.txt"), "world").unwrap();
    fs::write(dir.join(".gitignore"), "*.log\n").unwrap();
    fs::write(dir.join("debug.log"), "log data").unwrap();

    git(dir, &["add", "tracked.txt"]);
    git(dir, &["commit", "-m", "init"]);

    let repo = Repository::discover(dir).unwrap();
    let status = repo.status().unwrap();

    assert_eq!(
        status.get(std::path::Path::new("tracked.txt")),
        Some(&fshell_git::status::Status::Clean)
    );
    assert_eq!(
        status.get(std::path::Path::new("untracked.txt")),
        Some(&fshell_git::status::Status::Added)
    );
    assert_eq!(
        status.get(std::path::Path::new("debug.log")),
        Some(&fshell_git::status::Status::Ignored)
    );
}

#[test]
fn real_repo_modified() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    git(dir, &["init"]);
    fs::write(dir.join("file.txt"), "original content here").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", "init"]);

    fs::write(dir.join("file.txt"), "modified").unwrap();

    let repo = Repository::discover(dir).unwrap();
    let status = repo.status().unwrap();
    assert_eq!(
        status.get(std::path::Path::new("file.txt")),
        Some(&fshell_git::status::Status::Modified)
    );
}

#[test]
fn real_repo_branches() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    git(dir, &["init", "-b", "main"]);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", "first"]);
    git(dir, &["branch", "feature"]);

    let repo = Repository::discover(dir).unwrap();
    let branches = repo.branches().unwrap();
    assert!(branches.iter().any(|b| b.name == "main" && b.head));
    assert!(branches.iter().any(|b| b.name == "feature" && !b.head));
}

#[test]
fn real_repo_branch_info() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    git(dir, &["init", "-b", "main"]);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", "init"]);

    let repo = Repository::discover(dir).unwrap();
    let head = repo.head().unwrap();
    assert_eq!(head.branch.as_deref(), Some("main"));
    assert!(!head.detached);
}

#[test]
fn real_repo_detached_head() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    git(dir, &["init"]);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", "init"]);

    let output = std::process::Command::new("git")
        .current_dir(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let hash = String::from_utf8(output.stdout).unwrap().trim().to_string();
    git(dir, &["checkout", &hash]);

    let repo = Repository::discover(dir).unwrap();
    let head = repo.head().unwrap();
    assert!(head.detached);
    assert_eq!(hex::encode(head.oid), hash);
}

#[test]
fn real_repo_ahead_behind() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    git(dir, &["init"]);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", "init"]);

    let remote_dir = tmp.path().join("remote");
    git(
        dir,
        &["remote", "add", "origin", remote_dir.to_str().unwrap()],
    );
    git(dir, &["fetch", "origin"]);

    let repo = Repository::discover(dir).unwrap();
    let (ahead, behind) = repo.ahead_behind().unwrap();
    assert_eq!(ahead, 0);
    assert_eq!(behind, 0);
}
