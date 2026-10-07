use std::path::Path;
use std::process::{Command, Output};

pub fn git(directory: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .current_dir(directory)
        .args(args)
        .env("GIT_AUTHOR_NAME", "fshell-git tests")
        .env("GIT_AUTHOR_EMAIL", "fshell-git@example.invalid")
        .env("GIT_COMMITTER_NAME", "fshell-git tests")
        .env("GIT_COMMITTER_EMAIL", "fshell-git@example.invalid")
        .output()
        .expect("test fixture operation should succeed");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

pub fn init_repo(directory: &Path) {
    git(directory, &["init", "-b", "main"]);
    git(directory, &["config", "user.name", "fshell-git tests"]);
    git(
        directory,
        &["config", "user.email", "fshell-git@example.invalid"],
    );
}

pub fn commit_all(directory: &Path, message: &str) {
    git(directory, &["add", "--all"]);
    git(directory, &["commit", "--message", message]);
}
