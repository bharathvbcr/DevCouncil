#!/usr/bin/env python3
"""Bounded, dependency-free CLI output stress. All writes use a scratch repository.

Build first with the project installers or cargo/go build; this script never
installs software. PTY tests require macOS/Linux and report that limit explicitly.
"""
import argparse
import concurrent.futures
import errno
import fcntl
import json
import os
from pathlib import Path
import platform
import pty
import re
import select
import signal
import socket
import struct
import subprocess
import tempfile
import termios
import time

ANSI = re.compile(rb"\x1b\[[0-9;?]*[A-Za-z]")
MAX_OUTPUT = 8 * 1024 * 1024


def run(argv, root, **kwargs):
    result = subprocess.run(argv, cwd=root, input=b"", capture_output=True, timeout=10, **kwargs)
    assert len(result.stdout) + len(result.stderr) < MAX_OUTPUT, "output budget exceeded"
    assert b"panicked at" not in result.stderr, result.stderr[:1000]
    return result


def receipt(result, expected=0):
    assert result.returncode == expected, (result.args, result.returncode, result.stderr[:1500], result.stdout[:1500])
    assert b"\x1b" not in result.stdout, result.stdout[:1000]
    return json.loads(result.stdout)


def terminal(argv, root, width=80, mode="auto", locale="en_US.UTF-8", term="xterm-256color", no_color=True, pipe=False, paused=False):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, width, 0, 0))
    # Darwin marks the first terminal write in F_GETFL even for /bin/echo.
    # Establish that kernel state before comparing all inherited flags.
    os.write(slave, b".")
    assert os.read(master, 1) == b"."
    before_flags, before_modes = fcntl.fcntl(slave, fcntl.F_GETFL), termios.tcgetattr(slave)
    if paused:
        termios.tcflow(slave, termios.TCOOFF)
    env = dict(os.environ, TERM=term, LC_ALL=locale, NO_COLOR="1" if no_color else "")
    child = subprocess.Popen([*argv, "--progress", mode], cwd=root, stdin=subprocess.DEVNULL,
                             stdout=subprocess.PIPE if pipe else slave, stderr=slave, env=env)
    stdout, screen = bytearray(), bytearray()
    streams = {master: screen}
    if pipe:
        streams[child.stdout.fileno()] = stdout
    started = time.monotonic()
    try:
        while child.poll() is None or streams:
            assert time.monotonic() - started < 6, f"terminal command timed out: {argv}"
            if child.poll() is not None and slave is not None:
                assert fcntl.fcntl(slave, fcntl.F_GETFL) == before_flags, ("inherited descriptor flags changed", argv, before_flags, fcntl.fcntl(slave, fcntl.F_GETFL))
                assert termios.tcgetattr(slave) == before_modes, "terminal modes changed"
                if paused:
                    termios.tcflow(slave, termios.TCOON)
                os.close(slave)
                slave = None
            ready, _, _ = select.select(list(streams), [], [], .03)
            for fd in ready:
                try:
                    data = os.read(fd, 65536)
                except OSError as exc:
                    if exc.errno != errno.EIO:
                        raise
                    data = b""
                if not data:
                    streams.pop(fd)
                else:
                    streams[fd].extend(data)
                    assert len(stdout) + len(screen) < MAX_OUTPUT, "terminal output budget exceeded"
        return child.returncode, bytes(stdout), bytes(screen), time.monotonic() - started
    finally:
        if child.poll() is None:
            child.kill()
        child.wait(timeout=2)
        if child.stdout:
            child.stdout.close()
        if slave is not None:
            if paused:
                termios.tcflow(slave, termios.TCOON)
            os.close(slave)
        os.close(master)


def full_stderr(argv, root):
    reader, writer = os.pipe()
    original = fcntl.fcntl(writer, fcntl.F_GETFL)
    fcntl.fcntl(writer, fcntl.F_SETFL, original | os.O_NONBLOCK)
    try:
        for _ in range(4096):
            try:
                os.write(writer, b"x" * 4096)
            except BlockingIOError:
                break
        else:
            raise AssertionError("could not fill diagnostic pipe within budget")
        fcntl.fcntl(writer, fcntl.F_SETFL, original)
        original = fcntl.fcntl(writer, fcntl.F_GETFL)
        started = time.monotonic()
        child = subprocess.run(argv, cwd=root, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=writer, timeout=3)
        assert fcntl.fcntl(writer, fcntl.F_GETFL) == original
        assert time.monotonic() - started < 2, "diagnostics delayed the result"
        assert child.returncode != 0
        result = json.loads(child.stdout)
        assert result.get("error") or result.get("diagnostics"), result
    finally:
        os.close(reader)
        os.close(writer)


def http_with_full_stderr(devmap, root):
    reader, writer = os.pipe()
    flags = fcntl.fcntl(writer, fcntl.F_GETFL)
    fcntl.fcntl(writer, fcntl.F_SETFL, flags | os.O_NONBLOCK)
    for _ in range(4096):
        try:
            os.write(writer, b"x" * 4096)
        except BlockingIOError:
            break
    else:
        raise AssertionError("diagnostic pipe did not fill")
    fcntl.fcntl(writer, fcntl.F_SETFL, flags)
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    child = subprocess.Popen([devmap, "--root", str(root), "mcp", "--http", f"127.0.0.1:{port}"],
                             stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=writer)
    try:
        deadline = time.monotonic() + 2
        while True:
            try:
                connection = socket.create_connection(("127.0.0.1", port), timeout=.5)
                break
            except ConnectionRefusedError:
                assert time.monotonic() < deadline, "HTTP server did not bind"
                time.sleep(.01)
        with connection:
            connection.settimeout(2)
            connection.sendall(f"GET /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n".encode())
            response = connection.recv(4096)
            assert response.startswith(b"HTTP/1.1 "), response
    finally:
        child.kill()
        child.communicate(timeout=2)
        os.close(reader)
        os.close(writer)


def interrupt_forwarded_command(host, root):
    program = root / "slow-devmap"
    pid_file = root / "child.pid"
    program.write_text("#!/bin/sh\nprintf '%s' \"$$\" > \"$UI_TEST_PID_FILE\"\nexec /bin/sleep 30\n")
    program.chmod(0o755)
    for sig in [signal.SIGINT, signal.SIGTERM]:
        pid_file.unlink(missing_ok=True)
        child = subprocess.Popen([host, "map"], cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 env=dict(os.environ, DEVMAP_BIN=str(program), UI_TEST_PID_FILE=str(pid_file)))
        started = time.monotonic()
        pid = None
        try:
            while not pid_file.exists() or not pid_file.read_text():
                assert time.monotonic() - started < 2, "forwarded child did not start"
                time.sleep(.01)
            pid = int(pid_file.read_text())
            child.send_signal(sig)
            stdout, stderr = child.communicate(timeout=3)
            assert child.returncode != 0
            assert b"10m" not in stderr, "cancellation was reported as a ten-minute timeout"
            for _ in range(100):
                try:
                    os.kill(pid, 0)
                except ProcessLookupError:
                    break
                time.sleep(.01)
            else:
                raise AssertionError("forwarded child survived cancellation")
        finally:
            if child.poll() is None:
                child.kill()
            child.communicate(timeout=2)
            if pid is not None:
                try:
                    os.kill(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--devmap", required=True, type=Path)
    parser.add_argument("--host", required=True, type=Path)
    parser.add_argument("--components", type=Path, help="directory with dcstore, dcverify and dcgrep")
    parser.add_argument("--iterations", type=int, default=25)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    if not 1 <= args.iterations <= 1000:
        parser.error("iterations must be between 1 and 1000")
    devmap, host = str(args.devmap.resolve()), str(args.host.resolve())
    started = time.monotonic()
    counts = dict(json_commands=0, terminal_runs=0, pipe_failures=0, concurrent_queries=0, component_checks=0)
    samples = {}
    with tempfile.TemporaryDirectory(prefix="devcouncil-ui-stress-") as scratch:
        root = Path(scratch)
        source = "def needle():\n    return 42\n\ndef caller():\n    return needle()\n"
        (root / "example.py").write_text(source)
        base = [devmap, "--root", str(root)]
        receipt(run([*base, "build", "--manifest", "--json"], root))
        queries = [
            ["search", "needle"], ["deps", "example.py"], ["impact", "needle"],
            ["neighbors", "needle"], ["trace", "needle"], ["dead"], ["explore", "needle"],
            ["affected", "needle"], ["preview", "--file", "example.py", "--content", str(root / "example.py")],
            ["workspace", "list"], ["savings"], ["clones"], ["freshness"], ["status"],
            ["doctor"], ["session-report"], ["paths"], ["history"], ["snapshots"],
            ["pdg", str(root / "example.py")], ["cypher", "MATCH (a) RETURN a.name LIMIT 5"],
            ["ast", "needle"], ["routes"], ["shape-check"], ["api-impact", "GET /x"],
            ["claude", "events"], ["manifest"], ["map-html", "--force"], ["html"],
            ["export", "--out", str(root / "graph.graphml")],
            ["skills", "install", "--dry-run", "--project-root", str(root)],
            ["integrate", "codex", "--dry-run", "--project-root", str(root)],
            ["repair", "--fts", "--pending", "--page-size"],
        ]
        for query in queries:
            receipt(run([*base, *query, "--json", "--progress", "always"], root))
            counts["json_commands"] += 1
        host_queries = [["skills", "list"], ["install", "devmap", "--dry-run"], ["install", "--list"],
                        ["gate", "status", "--project-root", str(root)]]
        for query in host_queries:
            receipt(run([host, *query, "--json", "--progress", "always"], root))
            counts["json_commands"] += 1
        # Real PTYs, including a terminal stderr with redirected stdout.
        for binary, query, title in [(devmap, ["--root", str(root), "search", "needle"], b"devmap / Search"),
                                     (host, ["skills", "list"], b"devcouncil / Skills")]:
            for width in [16, 40, 80, 120]:
                for mode in ["auto", "always", "never"]:
                    code, _, screen, _ = terminal([binary, *query], root, width, mode)
                    assert code == 0, screen
                    assert b"\x1b[36m" not in screen, "NO_COLOR violated"
                    assert (b"\x1b" not in screen if mode == "never" else b"\x1b[2K" in screen), (binary, query, mode, width, screen)
                    if width >= 40 and mode != "never":
                        assert title in ANSI.sub(b"", screen), screen
                    counts["terminal_runs"] += 1
                    if width == 80 and mode == "auto":
                        samples[Path(binary).name] = screen.decode("utf-8")
            for locale, term in [("C", "xterm-256color"), ("en_US.UTF-8", "dumb")]:
                code, _, screen, _ = terminal([binary, *query], root, locale=locale, term=term)
                assert code == 0
                if locale == "C":
                    assert screen.isascii(), screen
                if term == "dumb":
                    assert b"\x1b" not in screen
                counts["terminal_runs"] += 1
            code, stdout, screen, _ = terminal([binary, *query], root, pipe=True, no_color=False)
            assert code == 0 and b"\x1b" not in stdout, "terminal stderr decorated redirected stdout"
            counts["terminal_runs"] += 1
            code, stdout, _, elapsed = terminal([binary, *query, "--json"], root, mode="always", pipe=True, paused=True)
            assert code == 0 and elapsed < 2
            json.loads(stdout)
            counts["terminal_runs"] += 1
        # Configuration exports are pure even on a live terminal.
        for query in [["mcp", "--print-config"], ["claude", "hooks"]]:
            code, _, screen, _ = terminal([*base, *query], root, mode="always")
            assert code == 0
            json.loads(screen)
            counts["terminal_runs"] += 1
        code, _, screen, _ = terminal([*base, "export", "--out", "-"], root, mode="always")
        assert code == 0 and screen.startswith(b"<?xml") and screen.rstrip().endswith(b"</graphml>")
        counts["terminal_runs"] += 1
        full_stderr([devmap, "--db", str(root / "missing.sqlite"), "search", "needle", "--json", "--progress", "always"], root)
        full_stderr([host, "install", "not-a-component", "--json", "--progress", "always"], root)
        http_with_full_stderr(devmap, root)
        interrupt_forwarded_command(host, root)
        counts["pipe_failures"] += 3
        counts["interruptions"] = 2
        # Repeated concurrent readers check output framing and deterministic data.
        def reader(i):
            return receipt(run([*base, "search", "needle", "--json", "--progress", ["auto", "always", "never"][i % 3]], root))
        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
            answers = list(pool.map(reader, range(args.iterations * 8)))
        assert all(answer == answers[0] for answer in answers), "query output drift under concurrency"
        counts["concurrent_queries"] = len(answers)
        if args.components:
            for name in ["dcstore", "dcverify", "dcgrep"]:
                binary = str((args.components / name).resolve())
                prefix = [binary, "--db", str(root / "state.sqlite")] if name == "dcstore" else [binary]
                if name == "dcstore":
                    receipt(run([*prefix, "ready"], root))
                receipt(run([*prefix, "health"], root))
                for command in ["health", "not-a-command"]:
                    for _ in range(args.iterations):
                        read_fd, write_fd = os.pipe()
                        os.close(read_fd)
                        try:
                            result = subprocess.run([*prefix, command], stdout=write_fd, stderr=subprocess.PIPE, timeout=3)
                            assert result.returncode == 1 and result.stderr == b"", (name, result.returncode, result.stderr)
                        finally:
                            os.close(write_fd)
                        counts["component_checks"] += 1
    report = dict(ok=True, platform=platform.platform(), counts=counts, elapsed_seconds=round(time.monotonic() - started, 3), terminal_samples=samples)
    args.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({key: value for key, value in report.items() if key != "terminal_samples"}, indent=2))


if __name__ == "__main__":
    main()
