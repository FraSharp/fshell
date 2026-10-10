#!/usr/bin/env python3
"""Measure a real interactive fsh through an isolated Unix PTY (stdlib only)."""

import argparse
import errno
import fcntl
import json
import math
import os
from pathlib import Path
import pty
import select
import signal
import statistics
import struct
import tempfile
import termios
import time


SHOW_CURSOR = b"\x1b[?25h"
ANCHOR = b"\x1b7"
PROMPT = b"AUDIT>"


def stats(values):
    ordered = sorted(values)
    return {
        "p50": round(statistics.median(ordered), 3),
        "p95": round(ordered[math.ceil(len(ordered) * 0.95) - 1], 3),
        "max": round(ordered[-1], 3),
    }


class Session:
    def __init__(self, binary, cwd, config, trace, status_bar):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(cwd)
            os.environ.update(
                HOME=str(config),
                FSH_CONFIG_DIR=str(config),
                FSH_TEST_DB_PATH=str(config / "history.db"),
                TERM="xterm-256color",
                FSH_STATUS_BAR="1" if status_bar else "0",
                FSH_SESSION_LOG="0",
            )
            # Test mode skips real terminal handoff, which is part of this audit.
            for name in ("CI", "FSH_TEST_ENV", "FSH_TRACE_FILE"):
                os.environ.pop(name, None)
            if trace:
                os.environ["FSH_TRACE_FILE"] = str(trace)
            os.execv(str(binary), [str(binary)])
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))

    def read_frame(self, quiet, new_prompt=False, timeout=10):
        start = time.perf_counter()
        last = start
        output = b""
        while time.perf_counter() - start < timeout:
            # Enter must reach a NEW saved prompt anchor, not merely the cooked
            # handoff's show-cursor sequence or the command's stdout.
            ready = SHOW_CURSOR in output and (not new_prompt or ANCHOR in output)
            wait = quiet if ready else min(0.1, timeout)
            if not select.select([self.fd], [], [], wait)[0]:
                if ready:
                    # Quiet time delimits a frame but is NOT included in latency.
                    return (last - start) * 1000, output
                continue
            try:
                chunk = os.read(self.fd, 65536)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                chunk = b""
            if not chunk:
                raise RuntimeError(f"fsh exited before frame completion: {output!r}")
            output += chunk
            last = time.perf_counter()
        raise RuntimeError(f"timed out waiting for frame: {output[-1000:]!r}")

    def send(self, text, quiet, new_prompt=False):
        # These workloads are small enough for one PTY write.
        payload = text.encode()
        if os.write(self.fd, payload) != len(payload):
            raise RuntimeError("short PTY input write")
        return self.read_frame(quiet, new_prompt)

    def idle_bytes(self, duration):
        deadline = time.perf_counter() + duration
        output = b""
        while time.perf_counter() < deadline:
            if select.select([self.fd], [], [], max(0, deadline - time.perf_counter()))[0]:
                output += os.read(self.fd, 65536)
        return len(output)

    def close(self):
        # Let the shell exit normally to flush its trace, but bound cleanup.
        try:
            os.write(self.fd, b"\x15exit\r")
            deadline = time.perf_counter() + 5
            while time.perf_counter() < deadline:
                if os.waitpid(self.pid, os.WNOHANG)[0]:
                    return
                if select.select([self.fd], [], [], 0.05)[0]:
                    try:
                        os.read(self.fd, 65536)
                    except OSError:
                        pass
            os.kill(self.pid, signal.SIGKILL)
            os.waitpid(self.pid, 0)
        finally:
            os.close(self.fd)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--cwd", type=Path, default=Path.cwd())
    parser.add_argument("--samples", type=int, default=20)
    parser.add_argument("--quiet-ms", type=float, default=30)
    parser.add_argument("--default-prompt", action="store_true")
    parser.add_argument("--status-bar", action="store_true")
    parser.add_argument("--trace", type=Path, help="new JSONL trace file (adds overhead)")
    args = parser.parse_args()
    if args.samples < 1 or args.quiet_ms <= 0:
        parser.error("samples and quiet-ms must be positive")
    binary = args.binary.resolve(strict=True)
    cwd = args.cwd.resolve(strict=True)
    trace = args.trace.resolve() if args.trace else None
    if trace and trace.exists():
        parser.error("trace file already exists; choose a new path")
    report = {
        "binary": str(binary), "cwd": str(cwd), "samples": args.samples,
        "default_prompt": args.default_prompt, "status_bar": args.status_bar,
        "tracing": bool(trace), "workloads": [],
    }
    with tempfile.TemporaryDirectory(prefix="fshell-perf-") as temp:
        config = Path(temp)
        (config / ".splash_disabled").touch()
        (config / "init.fsh").write_text(
            "" if args.default_prompt else 'set prompt "AUDIT> "\nset prompt_right ""\n'
        )
        session = Session(binary, cwd, config, trace, args.status_bar)
        try:
            session.read_frame(args.quiet_ms / 1000, new_prompt=True)
            report["idle_bytes_1_2s"] = session.idle_bytes(1.2)
            for command in ("echo audit", "pwd", "/bin/true", "/bin/echo audit", "echo audit | /bin/cat"):
                keys, enters, key_bytes, enter_bytes, rewrites, erases = [], [], [], [], [], []
                for _ in range(args.samples):
                    for char in command:
                        latency, output = session.send(char, args.quiet_ms / 1000)
                        keys.append(latency)
                        key_bytes.append(len(output))
                        rewrites.append(output.count(PROMPT))
                        erases.append(output.count(b"\x1b[K") + output.count(b"\x1b[2K"))
                    latency, output = session.send("\r", args.quiet_ms / 1000, new_prompt=True)
                    enters.append(latency)
                    enter_bytes.append(len(output))
                workload = {
                    "command": command, "key_ms": stats(keys), "enter_ms": stats(enters),
                    "key_bytes": stats(key_bytes), "enter_bytes": stats(enter_bytes),
                    "erases_per_key": round(statistics.mean(erases), 3),
                }
                if not args.default_prompt:
                    workload["ps1_writes_per_key"] = round(statistics.mean(rewrites), 3)
                report["workloads"].append(workload)
        finally:
            session.close()
    if trace:
        spans = [json.loads(line) for line in trace.read_text().splitlines()]
        report["trace_ms"] = {
            name: stats([span["duration_ns"] / 1e6 for span in spans if span["name"] == name])
            for name in sorted({span["name"] for span in spans})
        }
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
