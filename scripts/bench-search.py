#!/usr/bin/env python3
"""Warm-cache, end-to-end native search comparisons with output/status oracles.

Build first: cargo build --release
No cache dropping, shell configuration changes, or external search subprocesses
inside fsh. All fixtures and configuration are isolated in a temporary directory.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import shlex
import shutil
import statistics
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fsh", default="target/release/fsh")
    parser.add_argument("--fd", default="fd")
    parser.add_argument("--rg", default="rg")
    parser.add_argument("--files", type=int, default=20000)
    parser.add_argument("--lines", type=int, default=1000000)
    parser.add_argument("--runs", type=int, default=7)
    parser.add_argument("--threads", type=int, nargs="+", default=[1, 8])
    parser.add_argument("--cpus", help="Linux CPU IDs, comma-separated; inherited by both tools and all worker threads")
    parser.add_argument("--only", help="substring selecting benchmark cases")
    parser.add_argument("--rg-io", choices=["default", "buffered"], default="default", help="default ripgrep I/O, or explicitly disable mmap for a buffered-reader comparison")
    parser.add_argument("--json", type=Path)
    parser.add_argument("--max-ratio", type=float, help="fail if any median fsh/reference ratio exceeds this gate")
    args = parser.parse_args()
    if min(args.files, args.lines, args.runs, *args.threads) < 1:
        parser.error("fixture sizes, runs, and worker counts must be positive")
    if args.max_ratio is not None and (not math.isfinite(args.max_ratio) or args.max_ratio <= 0):
        parser.error("--max-ratio must be positive and finite")
    if args.cpus:
        if not hasattr(os, "sched_setaffinity"):
            parser.error("--cpus requires Linux CPU affinity support")
        try:
            cpus = {int(cpu) for cpu in args.cpus.split(",")}
        except ValueError:
            parser.error("--cpus requires comma-separated integer CPU IDs")
        if not cpus.issubset(os.sched_getaffinity(0)):
            parser.error("--cpus must select currently allowed CPU IDs")
        os.sched_setaffinity(0, cpus)
    tools = {"fsh": str(Path(args.fsh).resolve()), "fd": shutil.which(args.fd), "rg": shutil.which(args.rg)}
    if not all(tools.values()) or not os.access(tools["fsh"], os.X_OK):
        parser.error("release fsh, fd, and rg executables are required")

    with tempfile.TemporaryDirectory(prefix="fshell-search-bench-") as directory:
        base = Path(directory)
        tree = base / "tree"
        tree.mkdir()
        for i in range(args.files):
            parent = tree / f"d{i // 200:04}"
            parent.mkdir(exist_ok=True)
            (parent / f"file{i:06}.rs").write_bytes(b"needle\nplain\n")
        for name in ["ignored", ".hidden"]:
            parent = tree / name
            parent.mkdir()
            for i in range(max(1, args.files // 10)):
                (parent / f"extra{i:06}.rs").write_bytes(b"needle\nplain\n")
        (tree / ".ignore").write_text("ignored/\n")
        large = base / "large.txt"
        hit = b"needle " + b"x" * 56 + b"\n"
        miss = b"plain  " + b"x" * 56 + b"\n"
        with large.open("wb") as file:
            for start in range(0, args.lines, 100):
                length = min(100, args.lines - start)
                file.write(hit + miss * (length - 1))
        config = base / "config"
        config.mkdir()
        home = base / "home"
        home.mkdir()
        env = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(config), FSH_CONFIG_DIR=str(config), LC_ALL="C", NO_COLOR="1")
        for key in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "RIPGREP_CONFIG_PATH"]:
            env.pop(key, None)

        def native(script):
            return [tools["fsh"], "--native", "-c", script]

        def run(command, capture):
            started = time.perf_counter_ns()
            result = subprocess.run(command, cwd=base, env=env, stdout=subprocess.PIPE if capture else subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=60)
            elapsed = (time.perf_counter_ns() - started) / 1e6
            if result.returncode not in (0, 1) or result.stderr:
                raise RuntimeError(f"command failed: {command!r}\nstatus={result.returncode}, stderr={result.stderr!r}")
            return elapsed, result

        io_flags = ["--no-mmap"] if args.rg_io == "buffered" else []
        cases = []
        for pattern, label, expected in [("x", "dense", args.lines), ("needle", "sparse", (args.lines + 99) // 100), ("absenttoken", "none", 0)]:
            command = f"grep -F -c {shlex.quote(pattern)} {shlex.quote(str(large))}"
            cases.append((f"grep/file-count/{label}", native(command), [tools["rg"], "--no-config", *io_flags, "-F", "-c", pattern, str(large)], "count", expected))
            # Both sides spawn cat and search a pipe; fsh negotiates byte input.
            command = f"{shlex.quote(shutil.which('cat'))} {shlex.quote(str(large))} | grep -F -c {shlex.quote(pattern)}"
            reference = f"{shlex.quote(shutil.which('cat'))} {shlex.quote(str(large))} | {shlex.quote(tools['rg'])} --no-config -F -c {shlex.quote(pattern)}"
            cases.append((f"grep/pipe-count/{label}", native(command), ["/bin/sh", "-c", reference], "count", expected))
        # Use a selected matching line mode as well as count: output bytes must
        # agree, rather than comparing typed records with ripgrep's text printer.
        cases.append(("grep/file-lines/sparse", native(f"grep --text-output -F needle {shlex.quote(str(large))}"), [tools["rg"], "--no-config", *io_flags, "--color", "never", "-F", "needle", str(large)], "bytes", hit * ((args.lines + 99) // 100)))
        for threads in args.threads:
            for ignore in (False, True):
                policy = "ignored" if ignore else "all"
                flags = "" if ignore else "--hidden --no-ignore"
                fd_flags = ["--no-ignore-parent"] if ignore else ["--hidden", "--no-ignore"]
                command = f"ff {shlex.quote(str(tree))} name = '*.rs' type = file --print0 -j {threads} {flags}"
                reference = [tools["fd"], "--case-sensitive", "--color", "never", "--absolute-path", "--print0", "--type", "f", "--glob", "--threads", str(threads), *fd_flags, "*.rs", str(tree)]
                expected = args.files if ignore else args.files + 2 * max(1, args.files // 10)
                cases.append((f"ff/paths/{policy}/j{threads}", native(command), reference, "paths", expected))
                for pattern in ("needle", "absenttoken"):
                    command = f"grep -r -l -F --threads {threads} {flags} {pattern} {shlex.quote(str(tree))}"
                    reference = [tools["rg"], "--no-config", *io_flags, "--color", "never", "--threads", str(threads), *([] if ignore else ["--hidden", "--no-ignore"]), "-l", "-F", pattern, str(tree)]
                    cases.append((f"grep/tree-files/{policy}/{pattern}/j{threads}", native(command), reference, "lines", expected if pattern == "needle" else 0))

        report = {"cpus": sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None, "rg_io": args.rg_io, "cache": "warm (validation plus one warmup per command)", "platform": platform.platform(), "threads": args.threads, "files": args.files, "lines": args.lines, "content_bytes": large.stat().st_size, "runs": args.runs, "fsh_sha256": hashlib.sha256(Path(tools["fsh"]).read_bytes()).hexdigest(), "versions": {name: subprocess.check_output([path, "--version"], text=True).splitlines()[0] for name, path in tools.items()}, "cases": []}
        for name, ours, reference, kind, expected in cases:
            if args.only and args.only not in name:
                continue
            _, actual = run(ours, True)
            _, oracle = run(reference, True)
            if actual.returncode != oracle.returncode:
                raise AssertionError(f"{name}: status mismatch {actual.returncode}/{oracle.returncode}")
            if kind == "count":
                a, b = int(actual.stdout.strip() or b"0"), int(oracle.stdout.strip() or b"0")
                valid = a == b == expected
            elif kind in ("paths", "lines"):
                delimiter = b"\0" if kind == "paths" else b"\n"
                a = sorted(x for x in actual.stdout.split(delimiter) if x)
                b = sorted(x for x in oracle.stdout.split(delimiter) if x)
                valid = a == b and len(a) == expected and len(set(a)) == expected
            else:
                valid = actual.stdout == oracle.stdout == expected
            if not valid:
                raise AssertionError(f"{name}: output mismatch (fsh={len(actual.stdout)} bytes, reference={len(oracle.stdout)} bytes)")
            run(ours, False)
            run(reference, False)
            timings = [[], []]
            for iteration in range(args.runs):
                for side in ([0, 1] if iteration % 2 == 0 else [1, 0]):
                    elapsed, result = run([ours, reference][side], False)
                    if result.returncode != actual.returncode:
                        raise AssertionError(f"{name}: timed status changed")
                    timings[side].append(elapsed)
            medians = [statistics.median(samples) for samples in timings]
            ratio = medians[0] / medians[1]
            report["cases"].append({"name": name, "commands": [ours, reference], "milliseconds": timings, "median_ms": medians, "ratio": ratio})
            print(f"{name:43} fsh {medians[0]:8.2f} ms  reference {medians[1]:8.2f} ms  ratio {ratio:5.2f}", flush=True)
        if not report["cases"]:
            parser.error("--only selected no cases")
        if args.json:
            args.json.write_text(json.dumps(report, indent=2) + "\n")
        if args.max_ratio is not None and any(case["ratio"] > args.max_ratio for case in report["cases"]):
            raise SystemExit("performance ratio gate failed")


if __name__ == "__main__":
    main()
