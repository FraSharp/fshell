// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use fshell_git::ignore::IgnoreRules;
use fshell_git::index::Index;
use fshell_git::repo::Repository;
use std::fs;
use std::hint::black_box;
use std::path::Path;
// Helpers
fn create_loose_object(git_dir: &Path, oid_hex: &str, obj_type: &str, content: &[u8]) {
    let dir = git_dir.join("objects").join(&oid_hex[..2]);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(&oid_hex[2..]);

    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write;

    let header = format!("{} {}\0", obj_type, content.len());
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(header.as_bytes()).unwrap();
    encoder.write_all(content).unwrap();
    let compressed = encoder.finish().unwrap();
    fs::write(&path, compressed).unwrap();
}

fn oid_from_hex(hex_str: &str) -> [u8; 20] {
    let mut oid = [0u8; 20];
    for i in 0..20 {
        oid[i] = u8::from_str_radix(&hex_str[i * 2..i * 2 + 2], 16).unwrap();
    }
    oid
}

fn setup_repo_with_files(n: usize) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let git_dir = tmp.path().join(".git");
    fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
    fs::create_dir_all(git_dir.join("refs/remotes/origin")).unwrap();

    // Create index entries
    let mut index_entries = Vec::new();
    for i in 0..n {
        let name = format!("file_{i:04}.txt");
        let size = (i * 100 + 50) as u32;
        index_entries.push((name, 0o100644u32, size));
    }

    // Build index bytes
    let mut buf = Vec::new();
    buf.extend_from_slice(b"DIRC");
    buf.extend_from_slice(&3u32.to_be_bytes());
    buf.extend_from_slice(&(n as u32).to_be_bytes());

    for (path, mode, size) in &index_entries {
        buf.extend_from_slice(&0i64.to_be_bytes());
        buf.extend_from_slice(&0i64.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&mode.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&size.to_be_bytes());
        buf.extend_from_slice(&[0u8; 20]);
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(path.as_bytes());
        buf.push(0);
        let entry_len = 62 + path.len() + 1;
        let padded = (entry_len + 7) & !7;
        buf.extend(std::iter::repeat_n(0u8, padded - entry_len));
    }
    fs::write(git_dir.join("index"), &buf).unwrap();

    // Create the actual files with matching sizes
    for (name, _, size) in &index_entries {
        let content = vec![b'x'; *size as usize];
        fs::write(tmp.path().join(name), &content).unwrap();
    }

    tmp
}
// Benchmarks: Index parsing
fn bench_index_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("index_parse");

    for size in [10, 100, 1000, 5000] {
        let mut entries = Vec::new();
        for i in 0..size {
            let name = format!("src/module/file_{i:04}.rs");
            entries.push((name, 0o100644u32, (i * 200) as u32));
        }

        let mut buf = Vec::new();
        buf.extend_from_slice(b"DIRC");
        buf.extend_from_slice(&3u32.to_be_bytes());
        buf.extend_from_slice(&(size as u32).to_be_bytes());
        for (path, mode, size) in &entries {
            buf.extend_from_slice(&0i64.to_be_bytes());
            buf.extend_from_slice(&0i64.to_be_bytes());
            buf.extend_from_slice(&0u32.to_be_bytes());
            buf.extend_from_slice(&0u32.to_be_bytes());
            buf.extend_from_slice(&mode.to_be_bytes());
            buf.extend_from_slice(&0u32.to_be_bytes());
            buf.extend_from_slice(&0u32.to_be_bytes());
            buf.extend_from_slice(&size.to_be_bytes());
            buf.extend_from_slice(&[0u8; 20]);
            buf.extend_from_slice(&0u16.to_be_bytes());
            buf.extend_from_slice(path.as_bytes());
            buf.push(0);
            let entry_len = 62 + path.len() + 1;
            let padded = (entry_len + 7) & !7;
            buf.extend(std::iter::repeat_n(0u8, padded - entry_len));
        }

        group.bench_with_input(BenchmarkId::from_parameter(size), &buf, |b, data| {
            b.iter(|| Index::parse_bytes(black_box(data)).unwrap());
        });
    }
    group.finish();
}
// Benchmarks: Object reading (loose objects)
fn bench_object_read(c: &mut Criterion) {
    let mut group = c.benchmark_group("object_read");

    for size_kb in [1, 10, 100, 1000] {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();

        let oid_hex = "abc123def456789012345678901234567890abcd";
        let content = vec![b'x'; size_kb * 1024];
        create_loose_object(&git_dir, oid_hex, "blob", &content);

        let repo = Repository::discover(tmp.path()).unwrap();
        let oid = oid_from_hex(oid_hex);

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{size_kb}KB")),
            &oid,
            |b, oid| {
                b.iter(|| repo.read_object(black_box(oid)).unwrap());
            },
        );
    }
    group.finish();
}
// Benchmarks: Gitignore matching
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
                let p = Path::new(path);
                black_box(rules.is_ignored(p, *is_dir));
            }
        });
    });

    group.finish();
}
// Benchmarks: Ref resolution
fn bench_ref_resolve(c: &mut Criterion) {
    let mut group = c.benchmark_group("ref_resolve");

    let tmp = tempfile::tempdir().unwrap();
    let git_dir = tmp.path().join(".git");
    fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
    fs::create_dir_all(git_dir.join("refs/remotes/origin")).unwrap();

    // Create 100 loose refs
    for i in 0..100 {
        let name = format!("refs/heads/branch_{i:03}");
        fs::write(git_dir.join(&name), format!("{:040x}\n", i)).unwrap();
    }

    let repo = Repository::discover(tmp.path()).unwrap();

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
// Benchmarks: Status diff
fn bench_status(c: &mut Criterion) {
    let mut group = c.benchmark_group("status_diff");

    for n in [10, 100, 500] {
        let tmp = setup_repo_with_files(n);

        // Make some files modified
        for i in (0..n).step_by(3) {
            let name = format!("file_{i:04}.txt");
            fs::write(tmp.path().join(&name), "modified!").unwrap();
        }

        // Add some untracked files
        for i in (0..n).step_by(5) {
            let name = format!("untracked_{i:04}.txt");
            fs::write(tmp.path().join(&name), "new").unwrap();
        }

        let repo = Repository::discover(tmp.path()).unwrap();

        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| black_box(repo.status().unwrap()));
        });
    }
    group.finish();
}
// Benchmarks: HEAD resolution
fn bench_head(c: &mut Criterion) {
    let mut group = c.benchmark_group("head_resolution");

    let tmp = tempfile::tempdir().unwrap();
    let git_dir = tmp.path().join(".git");
    fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
    fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(
        git_dir.join("refs/heads/main"),
        "abc123def456789012345678901234567890abcd\n",
    )
    .unwrap();

    let repo = Repository::discover(tmp.path()).unwrap();

    group.bench_function("head", |b| {
        b.iter(|| black_box(repo.head().unwrap()));
    });

    group.finish();
}
// Benchmarks: Config parsing
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
        b.iter(|| fshell_git::config::Config::parse(black_box(config_content)).unwrap());
    });

    group.finish();
}
// Benchmark groups
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
