// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use fshell_ls::{Config, SortMode, list_dir, render_to_with_width};
use std::fs;
use std::hint::black_box;
use std::io;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;

const COUNTS: &[usize] = &[1, 10, 100, 1_000, 10_000];
const DEEP_COUNTS: &[usize] = &[10, 100, 1_000];
const BENCH_WIDTH: usize = 120;

fn config(path: &Path) -> Config {
    Config {
        path: path.to_path_buf(),
        show_all: false,
        list_dirs: false,
        long_listing: false,
        one_per_line: false,
        human_readable: false,
        raw: false,
        show_inode: false,
        sort_mode: SortMode::Name,
        reverse_sort: false,
        use_color: false,
        tree: false,
        tree_depth: None,
        group_directories_first: false,
        show_icons: false,
        git: false,
        dereference: false,
        recursive: false,
        verbose: false,
    }
}

fn flat_fixture(file_count: usize, git_repo: bool) -> TempDir {
    let temp = tempfile::tempdir().expect("create ls benchmark fixture");
    for index in 0..file_count {
        let name = format!("entry_{index:05}.dat");
        let size = 32 + index % 4_096;
        fs::write(temp.path().join(name), vec![b'x'; size]).expect("populate ls benchmark fixture");
    }

    if git_repo {
        let git_dir = temp.path().join(".git");
        fs::create_dir_all(&git_dir).expect("create synthetic git directory");
        let mut index = b"DIRC".to_vec();
        index.extend_from_slice(&2u32.to_be_bytes());
        index.extend_from_slice(&0u32.to_be_bytes());
        fs::write(git_dir.join("index"), index).expect("write empty synthetic git index");
    }
    temp
}

fn nested_fixture(file_count: usize) -> TempDir {
    assert_eq!(file_count % 10, 0);
    let temp = tempfile::tempdir().expect("create tree benchmark fixture");
    for index in 0..file_count {
        let directory = temp.path().join(format!("group_{:04}", index / 10));
        fs::create_dir_all(&directory).expect("create tree benchmark directory");
        let name = format!("entry_{index:05}.dat");
        fs::write(directory.join(name), vec![b'y'; 128 + index % 1_024])
            .expect("populate tree benchmark fixture");
    }
    temp
}

fn configure_group(group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>) {
    group.sample_size(30);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(2));
}

fn bench_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("ls/scan");
    configure_group(&mut group);

    for &count in COUNTS {
        let fixture = flat_fixture(count, false);
        let base = config(fixture.path());

        group.bench_with_input(BenchmarkId::new("default", count), &base, |b, cfg| {
            b.iter(|| black_box(list_dir(black_box(cfg)).expect("scan fixture")));
        });

        let mut long = base.clone();
        long.long_listing = true;
        group.bench_with_input(BenchmarkId::new("long_metadata", count), &long, |b, cfg| {
            b.iter(|| black_box(list_dir(black_box(cfg)).expect("scan fixture")));
        });
    }
    group.finish();
}

fn bench_render(c: &mut Criterion) {
    let mut group = c.benchmark_group("ls/render_to_sink");
    configure_group(&mut group);

    for &count in COUNTS {
        let fixture = flat_fixture(count, false);
        let base = config(fixture.path());
        let default_result = list_dir(&base).expect("prepare column rendering fixture");
        group.bench_with_input(
            BenchmarkId::new("columns", count),
            &(&base, &default_result),
            |b, (cfg, result)| {
                b.iter(|| {
                    let mut sink = io::sink();
                    black_box(
                        render_to_with_width(
                            black_box(result),
                            black_box(cfg),
                            &mut sink,
                            BENCH_WIDTH,
                            |_| true,
                        )
                        .expect("render listing"),
                    );
                });
            },
        );

        let mut one_per_line = base.clone();
        one_per_line.one_per_line = true;
        group.bench_with_input(
            BenchmarkId::new("one_per_line", count),
            &(&one_per_line, &default_result),
            |b, (cfg, result)| {
                b.iter(|| {
                    let mut sink = io::sink();
                    black_box(
                        render_to_with_width(
                            black_box(result),
                            black_box(cfg),
                            &mut sink,
                            BENCH_WIDTH,
                            |_| true,
                        )
                        .expect("render listing"),
                    );
                });
            },
        );

        let mut long = base.clone();
        long.long_listing = true;
        let long_result = list_dir(&long).expect("prepare long rendering fixture");
        group.bench_with_input(
            BenchmarkId::new("long", count),
            &(&long, &long_result),
            |b, (cfg, result)| {
                b.iter(|| {
                    let mut sink = io::sink();
                    black_box(
                        render_to_with_width(
                            black_box(result),
                            black_box(cfg),
                            &mut sink,
                            BENCH_WIDTH,
                            |_| true,
                        )
                        .expect("render listing"),
                    );
                });
            },
        );
    }
    group.finish();
}

fn bench_end_to_end(c: &mut Criterion) {
    let mut group = c.benchmark_group("ls/end_to_end");
    configure_group(&mut group);

    for &count in DEEP_COUNTS {
        let fixture = flat_fixture(count, false);
        let cfg = config(fixture.path());
        group.bench_with_input(BenchmarkId::new("default", count), &cfg, |b, cfg| {
            b.iter(|| {
                let result = list_dir(black_box(cfg)).expect("scan fixture");
                let mut sink = io::sink();
                black_box(
                    render_to_with_width(
                        black_box(&result),
                        black_box(cfg),
                        &mut sink,
                        BENCH_WIDTH,
                        |_| true,
                    )
                    .expect("render listing"),
                );
            });
        });

        let tree_fixture = nested_fixture(count);
        let mut tree = config(tree_fixture.path());
        tree.tree = true;
        group.bench_with_input(BenchmarkId::new("tree", count), &tree, |b, cfg| {
            b.iter(|| {
                let result = list_dir(black_box(cfg)).expect("scan tree root");
                let mut sink = io::sink();
                black_box(
                    render_to_with_width(
                        black_box(&result),
                        black_box(cfg),
                        &mut sink,
                        BENCH_WIDTH,
                        |_| true,
                    )
                    .expect("render tree"),
                );
            });
        });

        let git_fixture = flat_fixture(count, true);
        let mut git = config(git_fixture.path());
        git.git = true;
        group.bench_with_input(BenchmarkId::new("git_untracked", count), &git, |b, cfg| {
            b.iter(|| {
                let result = list_dir(black_box(cfg)).expect("scan git fixture");
                let mut sink = io::sink();
                black_box(
                    render_to_with_width(
                        black_box(&result),
                        black_box(cfg),
                        &mut sink,
                        BENCH_WIDTH,
                        |_| true,
                    )
                    .expect("render git listing"),
                );
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_scan, bench_render, bench_end_to_end);
criterion_main!(benches);
