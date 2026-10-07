// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use fshell_git::ignore::IgnoreRules;
use fshell_git::index::Index;
use fshell_git::repo::Repository;
use std::fs;
use std::hint::black_box;
use std::path::Path;
use std::process::{Command, Output};

fn git(directory: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .current_dir(directory)
        .args(args)
        .env("GIT_AUTHOR_NAME", "fshell-git benchmarks")
        .env("GIT_AUTHOR_EMAIL", "fshell-git@example.invalid")
        .env("GIT_COMMITTER_NAME", "fshell-git benchmarks")
        .env("GIT_COMMITTER_EMAIL", "fshell-git@example.invalid")
        .output()
        .expect("git must be available to prepare benchmark repositories");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn git_text(directory: &Path, args: &[&str]) -> String {
    String::from_utf8(git(directory, args).stdout)
        .expect("git output should be UTF-8")
        .trim()
        .to_owned()
}

fn init_repo(directory: &Path) {
    git(directory, &["init", "-b", "main"]);
    git(directory, &["config", "user.name", "fshell-git benchmarks"]);
    git(
        directory,
        &["config", "user.email", "fshell-git@example.invalid"],
    );
}

fn commit_all(directory: &Path, message: &str) {
    git(directory, &["add", "--all"]);
    git(directory, &["commit", "--message", message]);
}

fn setup_repo_with_files(count: usize) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("temporary directory should be created");
    init_repo(tmp.path());

    for i in 0..count {
        let name = format!("file_{i:04}.txt");
        fs::write(tmp.path().join(name), format!("benchmark file {i}\n"))
            .expect("benchmark worktree file should be written");
    }
    commit_all(tmp.path(), "benchmark baseline");
    tmp
}

fn bench_index_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("index_parse");

    for count in [10, 100, 1000, 5000] {
        let repository = setup_repo_with_files(count);
        let bytes = fs::read(repository.path().join(".git/index"))
            .expect("Git-generated index should be readable");

        group.bench_with_input(BenchmarkId::from_parameter(count), &bytes, |b, data| {
            b.iter(|| Index::parse_bytes(black_box(data)).expect("index should parse"));
        });
    }
    group.finish();
}

fn bench_object_read(c: &mut Criterion) {
    let mut group = c.benchmark_group("object_read");

    for size_kb in [1, 10, 100, 1000] {
        let tmp = tempfile::tempdir().expect("temporary directory should be created");
        init_repo(tmp.path());
        fs::write(tmp.path().join("payload.bin"), vec![b'x'; size_kb * 1024])
            .expect("benchmark object should be written");
        commit_all(tmp.path(), "benchmark object");
        let oid = git_text(tmp.path(), &["rev-parse", "HEAD:payload.bin"]);
        let oid: [u8; 20] = hex::decode(oid)
            .expect("Git should return a hexadecimal object id")
            .try_into()
            .expect("SHA-1 object id should contain 20 bytes");
        let repo = Repository::discover(tmp.path()).expect("repository should be discovered");

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{size_kb}KB")),
            &oid,
            |b, oid| {
                b.iter(|| {
                    repo.read_object(black_box(oid))
                        .expect("object should be readable")
                });
            },
        );
    }
    group.finish();
}

fn bench_gitignore(c: &mut Criterion) {
    let mut group = c.benchmark_group("gitignore_match");
    let patterns = "*.log\nbuild/\n!important.log\n*.o\n*.pyc\n__pycache__/\n.env\n*.tmp\nnode_modules/\n*.swp\n";

    group.bench_function("parse_patterns", |b| {
        b.iter(|| IgnoreRules::parse(black_box(patterns)));
    });

    let rules = IgnoreRules::parse(patterns);
    let test_paths = [
        ("debug.log", false),
        ("important.log", false),
        ("main.o", false),
        ("build", true),
        ("src/main.rs", false),
        ("__pycache__", true),
        (".env", false),
        ("tmp_file.tmp", false),
        ("node_modules", true),
        ("file.swp", false),
    ];

    group.bench_function("match_10_paths", |b| {
        b.iter(|| {
            for (path, is_dir) in &test_paths {
                black_box(rules.is_ignored(Path::new(path), *is_dir));
            }
        });
    });
    group.finish();
}

fn bench_ref_resolve(c: &mut Criterion) {
    let mut group = c.benchmark_group("ref_resolve");
    let tmp = setup_repo_with_files(1);

    for i in 0..100 {
        let name = format!("refs/heads/branch_{i:03}");
        git(tmp.path(), &["update-ref", &name, "HEAD"]);
    }
    let repo = Repository::discover(tmp.path()).expect("repository should be discovered");

    group.bench_function("resolve_100_refs", |b| {
        b.iter(|| {
            for i in 0..100 {
                let name = format!("refs/heads/branch_{i:03}");
                black_box(repo.resolve_ref(&name).ok());
            }
        });
    });
    group.bench_function("list_refs", |b| {
        b.iter(|| black_box(repo.list_refs("refs/heads/")));
    });
    group.finish();
}

fn bench_status(c: &mut Criterion) {
    let mut group = c.benchmark_group("status_diff");

    for count in [10, 100, 500] {
        let tmp = setup_repo_with_files(count);
        for i in (0..count).step_by(3) {
            let name = format!("file_{i:04}.txt");
            fs::write(tmp.path().join(name), "modified!")
                .expect("modified benchmark file should be written");
        }
        for i in (0..count).step_by(5) {
            let name = format!("untracked_{i:04}.txt");
            fs::write(tmp.path().join(name), "new")
                .expect("untracked benchmark file should be written");
        }

        let repo = Repository::discover(tmp.path()).expect("repository should be discovered");
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, _| {
            b.iter(|| black_box(repo.status().expect("status should be computed")));
        });
    }
    group.finish();
}

fn bench_head(c: &mut Criterion) {
    let mut group = c.benchmark_group("head_resolution");
    let tmp = setup_repo_with_files(1);
    let repo = Repository::discover(tmp.path()).expect("repository should be discovered");

    group.bench_function("head", |b| {
        b.iter(|| black_box(repo.head().expect("HEAD should resolve")));
    });
    group.finish();
}

fn bench_config(c: &mut Criterion) {
    let mut group = c.benchmark_group("config_parse");
    let config_content = r#"[core]
    repositoryformatversion = 0
    filemode = true
    bare = false
    logallrefupdates = true
    ignorecase = true
    precomposeunicode = false
[remote "origin"]
    url = https://github.com/user/repo.git
    fetch = +refs/heads/*:refs/remotes/origin/*
[branch "main"]
    remote = origin
    merge = refs/heads/main
[branch "feature"]
    remote = origin
    merge = refs/heads/feature
[user]
    name = John Doe
    email = john@example.com
"#;

    group.bench_function("parse_config", |b| {
        b.iter(|| {
            fshell_git::config::Config::parse(black_box(config_content))
                .expect("Git config should parse")
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_index_parse,
    bench_object_read,
    bench_gitignore,
    bench_ref_resolve,
    bench_status,
    bench_head,
    bench_config,
);

criterion_main!(benches);
