// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_git::repo::Repository;
use fshell_git::status::Status;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn git_output(directory: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(directory)
        .args(args)
        .env("GIT_AUTHOR_NAME", "fshell-git tests")
        .env("GIT_AUTHOR_EMAIL", "fshell-git@example.invalid")
        .env("GIT_COMMITTER_NAME", "fshell-git tests")
        .env("GIT_COMMITTER_EMAIL", "fshell-git@example.invalid")
        .output()
        .expect("test fixture operation should succeed")
}

fn git(directory: &Path, args: &[&str]) -> Output {
    let output = git_output(directory, args);
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn git_text(directory: &Path, args: &[&str]) -> String {
    String::from_utf8(git(directory, args).stdout)
        .expect("test fixture operation should succeed")
        .trim()
        .to_owned()
}

fn init_repo(directory: &Path) {
    git(directory, &["init", "--initial-branch=main"]);
    git(directory, &["config", "user.name", "fshell-git tests"]);
    git(
        directory,
        &["config", "user.email", "fshell-git@example.invalid"],
    );
}

fn commit_all(directory: &Path, message: &str) {
    git(directory, &["add", "--all"]);
    git(directory, &["commit", "--message", message]);
}

fn status_for(repo: &Repository, path: &str) -> Status {
    repo.status()
        .expect("test fixture operation should succeed")
        .get(Path::new(path))
        .copied()
        .unwrap_or(Status::Untracked)
}

fn status_reported_by_git(directory: &Path) -> Result<HashMap<PathBuf, Status>, String> {
    let output = git(
        directory,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignored=matching",
        ],
    );
    let records = output.stdout.split(|byte| *byte == 0);
    let mut records = records.filter(|record| !record.is_empty()).peekable();
    let mut expected = HashMap::new();

    while let Some(record) = records.next() {
        if record.len() < 4 {
            return Err("Git porcelain record is truncated".into());
        }
        let code = &record[..2];
        let path = PathBuf::from(OsStr::from_bytes(&record[3..]));
        let status = if code == b"??" {
            Status::Untracked
        } else if code == b"!!" {
            Status::Ignored
        } else if code.contains(&b'U') || code == b"AA" || code == b"DD" {
            Status::Conflicted
        } else if code.contains(&b'R') {
            let old_path = records
                .next()
                .ok_or_else(|| "Git rename record has no source path".to_owned())?;
            expected.insert(PathBuf::from(OsStr::from_bytes(old_path)), Status::Deleted);
            Status::Renamed
        } else if code.contains(&b'D') {
            Status::Deleted
        } else if code.contains(&b'T') {
            Status::TypeChange
        } else if code.contains(&b'A') {
            Status::Added
        } else if code.contains(&b'M') {
            Status::Modified
        } else {
            return Err(format!("unexpected Git porcelain status: {code:?}"));
        };
        expected.insert(path, status);
    }

    Ok(expected)
}

fn assert_status_matches_git(repo: &Repository, directory: &Path) {
    let expected =
        status_reported_by_git(directory).expect("Git porcelain status should be parseable");
    let actual = repo
        .status()
        .expect("repository status should be available")
        .into_iter()
        .filter(|(_, status)| *status != Status::Clean)
        .collect::<HashMap<_, _>>();
    assert_eq!(actual, expected, "fshell-git status differs from Git CLI");
}

#[test]
fn status_reports_clean_untracked_and_ignored_paths() {
    let directory = tempfile::tempdir().expect("test fixture operation should succeed");
    init_repo(directory.path());
    fs::write(directory.path().join("tracked.txt"), "hello")
        .expect("test fixture operation should succeed");
    fs::write(directory.path().join(".gitignore"), "*.log\n")
        .expect("test fixture operation should succeed");
    commit_all(directory.path(), "initial");
    fs::write(directory.path().join("untracked.txt"), "world")
        .expect("test fixture operation should succeed");
    fs::write(directory.path().join("debug.log"), "log data")
        .expect("test fixture operation should succeed");

    let repo =
        Repository::discover(directory.path()).expect("test fixture operation should succeed");
    assert_status_matches_git(&repo, directory.path());
    assert_eq!(status_for(&repo, "tracked.txt"), Status::Clean);
    assert_eq!(status_for(&repo, "untracked.txt"), Status::Untracked);
    assert_eq!(status_for(&repo, "debug.log"), Status::Ignored);
}

#[test]
fn status_hashes_when_stat_data_changes_and_detects_same_size_content_changes() {
    let directory = tempfile::tempdir().expect("test fixture operation should succeed");
    init_repo(directory.path());
    let path = directory.path().join("tracked.txt");
    fs::write(&path, "first").expect("test fixture operation should succeed");
    commit_all(directory.path(), "initial");

    let repo =
        Repository::discover(directory.path()).expect("test fixture operation should succeed");
    let index = fshell_git::index::Index::parse(repo.git_dir())
        .expect("test fixture operation should succeed");
    let entry = index
        .get(Path::new("tracked.txt"))
        .expect("test fixture operation should succeed");

    filetime::set_file_mtime(
        &path,
        filetime::FileTime::from_unix_time(entry.mtime_secs + 3_600, 0),
    )
    .expect("test fixture operation should succeed");
    assert_status_matches_git(&repo, directory.path());
    assert_eq!(status_for(&repo, "tracked.txt"), Status::Clean);

    std::thread::sleep(std::time::Duration::from_secs(1));
    fs::write(&path, "other").expect("test fixture operation should succeed");
    filetime::set_file_mtime(
        &path,
        filetime::FileTime::from_unix_time(entry.mtime_secs, entry.mtime_nanos),
    )
    .expect("test fixture operation should succeed");
    assert_status_matches_git(&repo, directory.path());
    assert_eq!(status_for(&repo, "tracked.txt"), Status::Modified);
}

#[test]
fn status_reports_staged_rename_and_deletion() {
    let directory = tempfile::tempdir().expect("test fixture operation should succeed");
    init_repo(directory.path());
    fs::write(directory.path().join("old.txt"), "stable content")
        .expect("test fixture operation should succeed");
    fs::write(directory.path().join("delete.txt"), "remove me")
        .expect("test fixture operation should succeed");
    commit_all(directory.path(), "initial");

    git(directory.path(), &["mv", "old.txt", "new.txt"]);
    git(directory.path(), &["rm", "delete.txt"]);

    let repo =
        Repository::discover(directory.path()).expect("test fixture operation should succeed");
    assert_status_matches_git(&repo, directory.path());
    assert_eq!(status_for(&repo, "old.txt"), Status::Deleted);
    assert_eq!(status_for(&repo, "new.txt"), Status::Renamed);
    assert_eq!(status_for(&repo, "delete.txt"), Status::Deleted);
}

#[test]
fn status_matches_git_for_unmerged_paths() {
    let directory = tempfile::tempdir().expect("test fixture operation should succeed");
    init_repo(directory.path());
    fs::write(directory.path().join("conflict.txt"), "base\n")
        .expect("test fixture operation should succeed");
    commit_all(directory.path(), "base");

    git(directory.path(), &["switch", "-c", "side"]);
    fs::write(directory.path().join("conflict.txt"), "side\n")
        .expect("test fixture operation should succeed");
    commit_all(directory.path(), "side change");
    git(directory.path(), &["switch", "main"]);
    fs::write(directory.path().join("conflict.txt"), "main\n")
        .expect("test fixture operation should succeed");
    commit_all(directory.path(), "main change");

    let merge = git_output(directory.path(), &["merge", "side"]);
    assert!(!merge.status.success(), "merge should stop on a conflict");

    let repo =
        Repository::discover(directory.path()).expect("test fixture operation should succeed");
    assert_status_matches_git(&repo, directory.path());
    assert_eq!(status_for(&repo, "conflict.txt"), Status::Conflicted);
}

#[test]
fn status_matches_git_for_file_type_changes() {
    let directory = tempfile::tempdir().expect("test fixture operation should succeed");
    init_repo(directory.path());
    fs::write(directory.path().join("tracked.txt"), "regular file")
        .expect("test fixture operation should succeed");
    fs::write(directory.path().join("target.txt"), "symlink target")
        .expect("test fixture operation should succeed");
    commit_all(directory.path(), "initial");
    fs::remove_file(directory.path().join("tracked.txt"))
        .expect("tracked file should be removed before replacement");
    std::os::unix::fs::symlink("target.txt", directory.path().join("tracked.txt"))
        .expect("symlink replacement should be created");

    let repo =
        Repository::discover(directory.path()).expect("test fixture operation should succeed");
    assert_status_matches_git(&repo, directory.path());
    assert_eq!(status_for(&repo, "tracked.txt"), Status::TypeChange);
}

#[test]
fn nested_ignore_rules_match_git_path_scopes_and_negation() {
    let directory = tempfile::tempdir().expect("test fixture operation should succeed");
    init_repo(directory.path());
    fs::write(
        directory.path().join(".gitignore"),
        "*.log\n!important.log\n/anchored/*.tmp\n",
    )
    .expect("test fixture operation should succeed");
    fs::create_dir_all(directory.path().join("nested/sub"))
        .expect("test fixture operation should succeed");
    fs::write(
        directory.path().join("nested/.gitignore"),
        "*.tmp\n!keep.tmp\n",
    )
    .expect("test fixture operation should succeed");
    commit_all(directory.path(), "ignore rules");

    fs::write(directory.path().join("nested/debug.log"), "ignored")
        .expect("test fixture operation should succeed");
    fs::write(directory.path().join("important.log"), "visible")
        .expect("test fixture operation should succeed");
    fs::create_dir_all(directory.path().join("anchored"))
        .expect("test fixture operation should succeed");
    fs::write(directory.path().join("anchored/match.tmp"), "ignored")
        .expect("test fixture operation should succeed");
    fs::create_dir_all(directory.path().join("other/anchored"))
        .expect("test fixture operation should succeed");
    fs::write(directory.path().join("other/anchored/match.tmp"), "visible")
        .expect("test fixture operation should succeed");
    fs::write(directory.path().join("nested/drop.tmp"), "ignored")
        .expect("test fixture operation should succeed");
    fs::write(directory.path().join("nested/keep.tmp"), "visible")
        .expect("test fixture operation should succeed");
    fs::write(directory.path().join("nested/sub/drop.tmp"), "ignored")
        .expect("test fixture operation should succeed");

    let repo =
        Repository::discover(directory.path()).expect("test fixture operation should succeed");
    assert_status_matches_git(&repo, directory.path());
    assert_eq!(status_for(&repo, "nested/debug.log"), Status::Ignored);
    assert_eq!(status_for(&repo, "important.log"), Status::Untracked);
    assert_eq!(status_for(&repo, "anchored/match.tmp"), Status::Ignored);
    assert_eq!(
        status_for(&repo, "other/anchored/match.tmp"),
        Status::Untracked
    );
    assert_eq!(status_for(&repo, "nested/drop.tmp"), Status::Ignored);
    assert_eq!(status_for(&repo, "nested/keep.tmp"), Status::Untracked);
    assert_eq!(status_for(&repo, "nested/sub/drop.tmp"), Status::Ignored);
}

#[test]
fn linked_worktree_uses_private_head_and_shared_objects_and_refs() {
    let directory = tempfile::tempdir().expect("test fixture operation should succeed");
    init_repo(directory.path());
    fs::write(directory.path().join("tracked.txt"), "initial")
        .expect("test fixture operation should succeed");
    commit_all(directory.path(), "initial");

    let worktree_path = directory.path().join("feature-worktree");
    git(
        directory.path(),
        &[
            "worktree",
            "add",
            "-b",
            "feature",
            worktree_path
                .to_str()
                .expect("test fixture operation should succeed"),
        ],
    );
    fs::write(worktree_path.join("tracked.txt"), "changed")
        .expect("test fixture operation should succeed");

    let repo = Repository::discover(&worktree_path).expect("test fixture operation should succeed");
    assert_status_matches_git(&repo, &worktree_path);
    let head = repo.head().expect("test fixture operation should succeed");
    assert_eq!(head.branch.as_deref(), Some("feature"));
    assert_eq!(
        repo.read_commit(&head.oid)
            .expect("test fixture operation should succeed")
            .message,
        "initial"
    );
    assert_eq!(status_for(&repo, "tracked.txt"), Status::Modified);
    assert_eq!(repo.list_refs("refs/heads/").len(), 2);
    assert_ne!(repo.git_dir(), directory.path().join(".git"));
}

#[test]
fn packed_objects_and_packed_refs_are_read_by_gitoxide() {
    let directory = tempfile::tempdir().expect("test fixture operation should succeed");
    init_repo(directory.path());
    fs::write(directory.path().join("tracked.txt"), "packed content")
        .expect("test fixture operation should succeed");
    commit_all(directory.path(), "packed commit");
    let oid = git_text(directory.path(), &["rev-parse", "HEAD"]);

    git(directory.path(), &["gc", "--aggressive", "--prune=now"]);
    git(directory.path(), &["pack-refs", "--all", "--prune"]);

    let repo =
        Repository::discover(directory.path()).expect("test fixture operation should succeed");
    let oid = hex::decode(oid)
        .expect("test fixture operation should succeed")
        .try_into()
        .expect("test fixture operation should succeed");
    assert_eq!(
        repo.read_commit(&oid)
            .expect("test fixture operation should succeed")
            .message,
        "packed commit"
    );
    assert_eq!(
        repo.resolve_ref("refs/heads/main")
            .expect("test fixture operation should succeed"),
        oid
    );
    assert!(
        repo.list_refs("refs/heads/")
            .iter()
            .any(|reference| reference.name == "refs/heads/main")
    );
}

#[test]
fn index_v4_split_index_and_extended_flags_are_supported() {
    let directory = tempfile::tempdir().expect("test fixture operation should succeed");
    init_repo(directory.path());
    fs::write(directory.path().join("tracked.txt"), "initial")
        .expect("test fixture operation should succeed");
    commit_all(directory.path(), "initial");
    git(directory.path(), &["update-index", "--index-version=4"]);

    let repo =
        Repository::discover(directory.path()).expect("test fixture operation should succeed");
    let index = fshell_git::index::Index::parse(repo.git_dir())
        .expect("test fixture operation should succeed");
    assert_eq!(index.version(), 4);
    assert!(index.get(Path::new("tracked.txt")).is_some());

    git(
        directory.path(),
        &["update-index", "--skip-worktree", "tracked.txt"],
    );
    git(directory.path(), &["update-index", "--split-index"]);
    let split_index = fshell_git::index::Index::parse(repo.git_dir())
        .expect("test fixture operation should succeed");
    assert_eq!(split_index.len(), 1);
    assert_ne!(
        split_index
            .get(Path::new("tracked.txt"))
            .expect("test fixture operation should succeed")
            .flags
            & 0x4000,
        0
    );

    fs::write(directory.path().join("tracked.txt"), "changed")
        .expect("test fixture operation should succeed");
    assert_eq!(status_for(&repo, "tracked.txt"), Status::Clean);
}

#[test]
fn sparse_index_status_fails_explicitly_when_gitoxide_cannot_compare_it() {
    let directory = tempfile::tempdir().expect("test fixture operation should succeed");
    init_repo(directory.path());
    fs::create_dir_all(directory.path().join("included"))
        .expect("test fixture operation should succeed");
    fs::create_dir_all(directory.path().join("excluded"))
        .expect("test fixture operation should succeed");
    fs::write(directory.path().join("included/file.txt"), "included")
        .expect("test fixture operation should succeed");
    fs::write(directory.path().join("excluded/file.txt"), "excluded")
        .expect("test fixture operation should succeed");
    commit_all(directory.path(), "initial");
    git(
        directory.path(),
        &["sparse-checkout", "init", "--cone", "--sparse-index"],
    );
    git(directory.path(), &["sparse-checkout", "set", "included"]);

    let repo =
        Repository::discover(directory.path()).expect("test fixture operation should succeed");
    let index = fshell_git::index::Index::parse(repo.git_dir())
        .expect("test fixture operation should succeed");
    assert!(index.is_sparse());
    assert!(matches!(
        repo.status(),
        Err(fshell_git::repo::Error::UnsupportedFeature(message))
            if message.contains("sparse indexes")
    ));
}

#[test]
fn configured_fetch_refspec_controls_upstream_and_ahead_behind_counts() {
    let directory = tempfile::tempdir().expect("test fixture operation should succeed");
    let local = directory.path().join("local");
    let remote = directory.path().join("remote.git");
    let second_clone = directory.path().join("second-clone");
    fs::create_dir(&local).expect("test fixture operation should succeed");
    git(
        directory.path(),
        &[
            "init",
            "--bare",
            "--initial-branch=main",
            remote
                .to_str()
                .expect("test fixture operation should succeed"),
        ],
    );
    init_repo(&local);
    fs::write(local.join("base.txt"), "base").expect("test fixture operation should succeed");
    commit_all(&local, "base");
    git(
        &local,
        &[
            "remote",
            "add",
            "origin",
            remote
                .to_str()
                .expect("test fixture operation should succeed"),
        ],
    );
    git(&local, &["push", "--set-upstream", "origin", "main"]);
    git(
        &local,
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/custom/*",
        ],
    );
    git(&local, &["fetch", "origin"]);

    git(
        directory.path(),
        &[
            "clone",
            remote
                .to_str()
                .expect("test fixture operation should succeed"),
            second_clone
                .to_str()
                .expect("test fixture operation should succeed"),
        ],
    );
    git(&second_clone, &["config", "user.name", "fshell-git tests"]);
    git(
        &second_clone,
        &["config", "user.email", "fshell-git@example.invalid"],
    );
    fs::write(second_clone.join("remote.txt"), "remote")
        .expect("test fixture operation should succeed");
    commit_all(&second_clone, "remote commit");
    git(&second_clone, &["push"]);
    git(&local, &["fetch", "origin"]);

    fs::write(local.join("local.txt"), "local").expect("test fixture operation should succeed");
    commit_all(&local, "local commit");

    let repo = Repository::discover(&local).expect("test fixture operation should succeed");
    let upstream = repo
        .find_upstream("main")
        .expect("test fixture operation should succeed")
        .expect("test fixture operation should succeed");
    let expected_upstream: [u8; 20] =
        hex::decode(git_text(&local, &["rev-parse", "refs/remotes/custom/main"]))
            .expect("test fixture operation should succeed")
            .try_into()
            .expect("test fixture operation should succeed");
    assert_eq!(upstream.0, "origin");
    assert_eq!(upstream.1, expected_upstream);
    assert_eq!(
        repo.ahead_behind()
            .expect("test fixture operation should succeed"),
        (1, 1)
    );
}

#[test]
fn ahead_behind_propagates_missing_commit_errors() {
    let directory = tempfile::tempdir().expect("test fixture operation should succeed");
    let local = directory.path().join("local");
    let remote = directory.path().join("remote.git");
    fs::create_dir(&local).expect("test fixture operation should succeed");
    git(
        directory.path(),
        &[
            "init",
            "--bare",
            "--initial-branch=main",
            remote
                .to_str()
                .expect("test fixture operation should succeed"),
        ],
    );
    init_repo(&local);
    fs::write(local.join("tracked.txt"), "content").expect("test fixture operation should succeed");
    commit_all(&local, "initial");
    git(
        &local,
        &[
            "remote",
            "add",
            "origin",
            remote
                .to_str()
                .expect("test fixture operation should succeed"),
        ],
    );
    git(&local, &["push", "--set-upstream", "origin", "main"]);
    fs::write(
        local.join(".git/refs/remotes/origin/main"),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
    )
    .expect("test fixture operation should succeed");

    let repo = Repository::discover(&local).expect("test fixture operation should succeed");
    assert!(repo.ahead_behind().is_err());
}

#[test]
fn gitlink_status_is_not_misclassified_as_a_directory_type_change() {
    let directory = tempfile::tempdir().expect("test fixture operation should succeed");
    let module = directory.path().join("module-source");
    let superproject = directory.path().join("superproject");
    fs::create_dir(&module).expect("test fixture operation should succeed");
    fs::create_dir(&superproject).expect("test fixture operation should succeed");
    init_repo(&module);
    fs::write(module.join("file.txt"), "module").expect("test fixture operation should succeed");
    commit_all(&module, "module commit");
    init_repo(&superproject);
    git(
        &superproject,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            module
                .to_str()
                .expect("test fixture operation should succeed"),
            "vendor/module",
        ],
    );
    commit_all(&superproject, "add submodule");

    let repo = Repository::discover(&superproject).expect("test fixture operation should succeed");
    assert_eq!(status_for(&repo, "vendor/module"), Status::Clean);
}

#[test]
fn malformed_pack_data_returns_an_error_without_panicking() {
    let directory = tempfile::tempdir().expect("test fixture operation should succeed");
    init_repo(directory.path());
    fs::write(directory.path().join("tracked.txt"), "packed content")
        .expect("test fixture operation should succeed");
    commit_all(directory.path(), "packed commit");
    let oid = git_text(directory.path(), &["rev-parse", "HEAD"]);
    git(directory.path(), &["gc", "--prune=now"]);

    let pack_directory = directory.path().join(".git/objects/pack");
    let pack_path: PathBuf = fs::read_dir(&pack_directory)
        .expect("test fixture operation should succeed")
        .map(|entry| entry.expect("test fixture operation should succeed").path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "pack")
        })
        .expect("test fixture operation should succeed");
    let pack = fs::read(&pack_path).expect("test fixture operation should succeed");
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(&pack_path)
        .expect("test fixture operation should succeed")
        .permissions();
    permissions.set_mode(permissions.mode() | 0o200);
    fs::set_permissions(&pack_path, permissions).expect("test fixture operation should succeed");
    fs::write(&pack_path, &pack[..pack.len() / 2]).expect("test fixture operation should succeed");

    let repo =
        Repository::discover(directory.path()).expect("test fixture operation should succeed");
    let oid = hex::decode(oid)
        .expect("test fixture operation should succeed")
        .try_into()
        .expect("test fixture operation should succeed");
    assert!(repo.read_commit(&oid).is_err());
}
