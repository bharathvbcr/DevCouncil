#!/usr/bin/env python3
"""Real linked-worktree, watcher and IPC stress. Uses only disposable local data.

Example: python3 tools/worktree_stress.py --binary /absolute/devmap --worktrees 128
Results name actual concurrency, edits, query counts and cold-build comparisons.
A deadline, subprocess failure, missing result or truncated response fails the run.
"""
import argparse
import concurrent.futures as futures
import hashlib
import json
import os
import platform
from pathlib import Path
import signal
import socket
import sqlite3
import subprocess
import tempfile
import threading
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--ipc-probe", type=Path, help="native ipc_probe example; required on Windows")
    parser.add_argument("--worktrees", type=int, default=128)
    parser.add_argument("--rounds", type=int, default=4)
    parser.add_argument("--build-workers", type=int, default=16)
    parser.add_argument("--fixture-files", type=int, default=0,
                        help="additional indexed modules per worktree (0..8192)")
    parser.add_argument("--functions-per-file", type=int, default=8,
                        help="functions per additional module (2..64)")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not (1 <= args.worktrees <= 256 and 1 <= args.rounds <= 100 and 1 <= args.build_workers <= 32):
        parser.error("worktrees must be 1..256, rounds 1..100, build-workers 1..32")
    if not (0 <= args.fixture_files <= 8192 and 2 <= args.functions_per_file <= 64):
        parser.error("fixture-files must be 0..8192, functions-per-file 2..64")
    if os.name not in ("posix", "nt"):
        parser.error("supported process control platforms: POSIX and Windows")
    if os.name == "nt" and args.ipc_probe is None:
        parser.error("Windows requires --ipc-probe (cargo build -p devmap-cli --example ipc_probe)")
    probe = str(args.ipc_probe.resolve(strict=True)) if args.ipc_probe else None
    process_options = (dict(creationflags=subprocess.CREATE_NEW_PROCESS_GROUP)
                       if os.name == "nt" else dict(start_new_session=True))
    binary = str(args.binary.resolve(strict=True))
    with open(binary, "rb") as binary_file:
        binary_hash = hashlib.file_digest(binary_file, "sha256").hexdigest()
    env = dict(os.environ, GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, DEVMAP_AUTOSPAWN="0", DEVMAP_MAX_IDLE_SECS="0",
               TOKIO_WORKER_THREADS="2", RAYON_NUM_THREADS="2")
    env.pop("DEVMAP_HOME", None)
    children = []
    query_ms = []
    logs = []
    started = time.monotonic()
    report = dict(worktrees=args.worktrees, simultaneous_daemons=args.worktrees,
                  simultaneous_editors=2 * args.worktrees, build_workers=args.build_workers,
                  tokio_workers_per_process=2, rayon_workers_per_process=2,
                  binary_sha256=binary_hash, platform=platform.platform(),
                  ipc_transport="named_pipe" if os.name == "nt" else "unix_socket",
                  ipc_probe_process=probe is not None,
                  latency_includes_probe_startup=probe is not None,
                  initial_source_files_per_worktree=1 + args.fixture_files,
                  initial_symbols_per_worktree=3 + args.fixture_files * (1 + args.functions_per_file),
                  initial_functions_per_worktree=2 + args.fixture_files * args.functions_per_file,
                  initial_call_edges_per_worktree=1 + args.fixture_files * (args.functions_per_file - 1),
                  fixture_functions_per_file=args.functions_per_file,
                  rounds=args.rounds, edit_operations=0, ipc_queries=0, metadata_checks=0,
                  cold_comparisons=0, kills=0, daemon_rss_kib_samples=[], passed=False)

    def run(argv, cwd=None):
        with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
            child = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                                     stdout=stdout, stderr=stderr, **process_options)
            try:
                code = child.wait(timeout=90)
            finally:
                if child.poll() is None:
                    stop(child, crash=True)
            for stream in (stdout, stderr):
                if stream.tell() > 2_000_000:
                    raise RuntimeError("subprocess output exceeded harness bound")
                stream.seek(0)
            if code:
                raise subprocess.CalledProcessError(code, argv, stderr=stderr.read(8192))
            return stdout.read(2_000_000)

    def parallel(fn, items):
        with futures.ThreadPoolExecutor(max_workers=args.build_workers) as pool:
            return list(pool.map(fn, items))

    def ipc(endpoint, **command):
        if probe:
            try:
                data = run([probe, str(endpoint), json.dumps(dict(version=1, **command))])
            except subprocess.CalledProcessError as error:
                if error.returncode == 2:
                    raise ConnectionRefusedError(str(endpoint)) from error
                raise
            result = json.loads(data)
            if result.get("ok") is not True:
                raise RuntimeError(f"IPC error: {result}")
            return result["result"]
        with socket.socket(socket.AF_UNIX) as client:
            deadline = time.monotonic() + 30
            client.settimeout(30)
            client.connect(str(endpoint))
            client.sendall(json.dumps(dict(version=1, **command)).encode() + b"\n")
            data = bytearray()
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError("IPC exchange exceeded 30 seconds")
                client.settimeout(remaining)
                part = client.recv(65536)
                if not part:
                    break
                data.extend(part)
                if len(data) > 2_000_000:
                    raise RuntimeError("IPC response exceeded harness bound")
            result = json.loads(data)
            if result.get("ok") is not True:
                raise RuntimeError(f"IPC error: {result}")
            return result["result"]

    def snapshot(db):
        with sqlite3.connect(db.as_uri() + "?mode=ro", uri=True, timeout=10) as conn:
            assert conn.execute("PRAGMA integrity_check").fetchone() == ("ok",)
            assert not conn.execute("PRAGMA foreign_key_check").fetchall()
            generation = conn.execute("SELECT max(id) FROM generations").fetchone()[0]
            assert generation is not None
            nodes = conn.execute("SELECT p.path,n.qualified_name,n.kind,n.span_start,n.span_end FROM generation_nodes n JOIN paths p ON p.id=n.file_id WHERE n.generation_id=? ORDER BY 1,2,3,4,5", (generation,)).fetchall()
            edges = conn.execute("SELECT source_symbol,target_symbol,edge_kind,confidence,resolution FROM generation_edges WHERE generation_id=? ORDER BY 1,2,3,4,5", (generation,)).fetchall()
            pending = conn.execute("SELECT count(*) FROM pending_paths").fetchone()[0]
            assert nodes and edges, "empty graph cannot prove equivalence"
            return nodes, edges, pending

    def stop(child, crash=False):
        if child.poll() is None:
            if os.name == "nt":
                if crash:
                    child.kill()
                else:
                    child.send_signal(signal.CTRL_BREAK_EVENT)
            else:
                os.killpg(child.pid, signal.SIGKILL if crash else signal.SIGTERM)
            try:
                child.wait(timeout=15)
            except subprocess.TimeoutExpired:
                if os.name == "nt":
                    child.kill()
                else:
                    os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=10)
                if not crash:
                    raise RuntimeError("daemon did not shut down within 15 seconds")
        if not crash and child.returncode != 0:
            raise RuntimeError(f"daemon graceful shutdown failed: exit {child.returncode}")

    scratch_scope = tempfile.TemporaryDirectory(prefix="dm-wt-", dir="/tmp" if os.name == "posix" else None)
    try:
        scratch = Path(scratch_scope.name)
        main_tree = scratch / "main"
        main_tree.mkdir()
        hooks = scratch / "empty-hooks"
        hooks.mkdir()
        run(["git", "init", "-q", str(main_tree)])
        run(["git", "config", "core.autocrlf", "false"], main_tree)
        (main_tree / ".gitignore").write_text(".devmap/\n.devcouncil/\nAGENTS.md\n")
        (main_tree / "common.py").write_text("def stable_helper():\n    return 1\ndef stable_caller():\n    return stable_helper()\n")
        # Unique names prevent accidental cross-file ambiguity from turning a
        # size test into an uncontrolled quadratic-resolution fixture.
        for module in range(args.fixture_files):
            body = []
            for leaf in range(args.functions_per_file):
                expression = str(module) if leaf == 0 else f"fixture_{module}_{leaf - 1}()"
                body.append(f"def fixture_{module}_{leaf}():\n    return {expression}\n")
            (main_tree / f"fixture_{module}.py").write_text("".join(body))
        run(["git", "add", "."], main_tree)
        run(["git", "-c", "user.name=DevMap test", "-c", "user.email=devmap-test@invalid", "-c", f"core.hooksPath={hooks}", "commit", "-qm", "fixture"], main_tree)
        roots = [scratch / f"w{i}" for i in range(args.worktrees)]
        for root in roots:
            run(["git", "worktree", "add", "-q", "--detach", str(root), "HEAD"], main_tree)
        exclude = main_tree / ".git/info/exclude"
        exclude.write_text("excluded.py\n")
        for i, root in enumerate(roots):
            (root / "excluded.py").write_text(f"def excluded_w{i}():\n    return 1\n")
        print(f"created {len(roots)} real linked worktrees", flush=True)
        parallel(lambda root: run([binary, "build", str(root), "--json"]), roots)
        dbs = [root / ".devmap/codeintel/devmap.sqlite" for root in roots]
        for db in dbs:
            nodes, edges, pending = snapshot(db)
            assert len(nodes) == report["initial_symbols_per_worktree"], f"fixture symbol coverage mismatch: {len(nodes)}, sample={nodes[:6]}"
            assert sum(row[2] == "File" for row in nodes) == report["initial_source_files_per_worktree"]
            assert sum(row[2] == "Function" for row in nodes) == report["initial_functions_per_worktree"]
            assert sum(row[2] == "Calls" for row in edges) == report["initial_call_edges_per_worktree"], "fixture edge coverage mismatch"
            if "initial_edges_per_worktree" in report:
                assert len(edges) == report["initial_edges_per_worktree"]
            report["initial_edges_per_worktree"] = len(edges)
            assert pending == 0
        endpoints = ([Path(r"\\.\pipe" + f"\\devmap-capacity-{os.getpid()}-{i}") for i in range(args.worktrees)]
                     if os.name == "nt" else [scratch / f"s{i}" for i in range(args.worktrees)])

        def start_daemon(i):
            log = tempfile.TemporaryFile()
            logs.append(log)
            child = subprocess.Popen([binary, "serve", str(roots[i]), "--socket", str(endpoints[i])],
                                     env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                     **process_options)
            children.append(child)
            return child

        active = [start_daemon(i) for i in range(len(roots))]
        def ready(i):
            deadline = time.monotonic() + 60
            while time.monotonic() < deadline:
                if active[i].poll() is not None:
                    raise RuntimeError(f"daemon {i} exited during startup")
                if os.name == "nt" or endpoints[i].exists():
                    try:
                        return ipc(endpoints[i], cmd="status")
                    except (ConnectionRefusedError, FileNotFoundError):
                        # A killed daemon leaves its endpoint for its
                        # successor to reclaim; existence is not readiness.
                        pass
                time.sleep(.05)
            raise RuntimeError(f"daemon {i} failed to bind")
        parallel(ready, range(len(roots)))
        print(f"{len(active)} daemons answering IPC concurrently", flush=True)

        def wait_metadata(i, present=None, head=None):
            deadline = time.monotonic() + 90
            while time.monotonic() < deadline:
                nodes, _, pending = snapshot(dbs[i])
                found = any(row[1].endswith(f"::excluded_w{i}") for row in nodes)
                with sqlite3.connect(dbs[i].as_uri() + "?mode=ro", uri=True, timeout=10) as conn:
                    stored_head = conn.execute("SELECT head_sha FROM generations ORDER BY id DESC LIMIT 1").fetchone()[0]
                if pending == 0 and (present is None or found == present) and (head is None or stored_head == head):
                    return
                time.sleep(.1)
            raise RuntimeError(f"metadata-only change did not converge in worktree {i}: present={present}, head={head}")

        parallel(lambda i: wait_metadata(i, present=False), range(len(roots)))
        exclude.write_text("")
        parallel(lambda i: wait_metadata(i, present=True), range(len(roots)))
        exclude.write_text("excluded.py\n")
        parallel(lambda i: wait_metadata(i, present=False), range(len(roots)))
        def move_head(root):
            run(["git", "-c", "user.name=DevMap test", "-c", "user.email=devmap-test@invalid", "-c", f"core.hooksPath={hooks}", "commit", "--allow-empty", "-qm", "metadata-only edit"], root)
            return run(["git", "rev-parse", "HEAD"], root).decode().strip()
        heads = parallel(move_head, roots)
        parallel(lambda i: wait_metadata(i, head=heads[i]), range(len(roots)))
        report["metadata_checks"] = 4 * len(roots)
        print("shared exclude changes and private HEAD-only commits converged", flush=True)


        for cycle in range(args.rounds):
            barrier = threading.Barrier(2 * len(roots))
            def edit(session):
                i, agent = divmod(session, 2)
                root = roots[i]
                path = root / f"agent{agent}.py"
                barrier.wait(timeout=30)
                for revision in range(8):
                    stage = root / f"agent{agent}.tmp"
                    stage.write_text(f"def owner_w{i}_a{agent}_r{cycle}():\n    return {revision}\n")
                    stage.replace(path)
                renamed = root / f"renamed{agent}.py"
                path.rename(renamed)
                renamed.rename(path)
                transient = root / f"deleted{agent}.py"
                transient.write_text("def must_disappear():\n    return 1\n")
                transient.unlink()
                # Query while other editors and drains are active.
                query_start = time.monotonic()
                result = ipc(endpoints[i], cmd="search", query="stable_helper")
                elapsed = (time.monotonic() - query_start) * 1000
                assert result["total"] == 1, result
                return 12, elapsed
            with futures.ThreadPoolExecutor(max_workers=2 * len(roots)) as pool:
                for operations, elapsed in pool.map(edit, range(2 * len(roots))):
                    report["edit_operations"] += operations
                    query_ms.append(elapsed)
            report["ipc_queries"] += 2 * len(roots)
            # Actual process death, not task cancellation. Restart without
            # manually deleting sockets, locks, databases or pending work.
            victims = list(range(cycle % 4, len(roots), 4))
            for i in victims:
                stop(active[i], crash=True)
                active[i] = start_daemon(i)
                report["kills"] += 1
            parallel(ready, victims)
            def converged(i):
                deadline = time.monotonic() + 120
                while time.monotonic() < deadline:
                    if active[i].poll() is not None:
                        raise RuntimeError(f"daemon {i} exited during resync")
                    nodes, _, pending = snapshot(dbs[i])
                    names = {row[1].split("::")[-1] for row in nodes}
                    expected = {f"owner_w{i}_a{a}_r{cycle}" for a in range(2)}
                    if pending == 0 and expected <= names and "must_disappear" not in names:
                        assert not any(name.startswith("owner_w") and name not in expected for name in names)
                        return
                    time.sleep(.1)
                raise RuntimeError(f"worktree {i} did not converge after round {cycle}")
            parallel(converged, range(len(roots)))
            pids = ",".join(str(child.pid) for child in active)
            if os.name == "nt":
                rss = run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command",
                           f"$ErrorActionPreference='Stop'; Get-Process -Id {pids} | ForEach-Object {{ [math]::Ceiling($_.WorkingSet64 / 1024) }}"]).split()
            else:
                rss = run(["ps", "-o", "rss=", "-p", pids]).split()
            assert len(rss) == len(active), "RSS sample missed a daemon"
            report["daemon_rss_kib_samples"].append(sum(int(value) for value in rss))
            print(f"round {cycle + 1}: {2 * len(roots)} editors converged; {len(victims)} crash/restarts", flush=True)

        for child in active:
            stop(child)
        def compare(i):
            cold = scratch / f"cold{i}.sqlite"
            run([binary, "--db", str(cold), "build", str(roots[i]), "--json"])
            incremental = snapshot(dbs[i])
            expected = snapshot(cold)
            assert incremental == expected, f"incremental/cold mismatch in worktree {i}"
            if os.name == "nt":
                try:
                    ipc(endpoints[i], cmd="status")
                except ConnectionRefusedError:
                    pass
                else:
                    raise RuntimeError(f"endpoint still serves after shutdown: {i}")
            else:
                assert not endpoints[i].exists(), f"stale endpoint after shutdown: {i}"
        parallel(compare, range(len(roots)))
        report["cold_comparisons"] = len(roots)
        report["passed"] = True
    except BaseException as error:
        report["error"] = str(error)
        if isinstance(error, subprocess.CalledProcessError):
            report["subprocess_stderr"] = (error.stderr or b"").decode(errors="replace")
        raise
    finally:
        cleanup_errors = []
        for child in children:
            try:
                stop(child, crash=True)
            except Exception as error:
                cleanup_errors.append(f"child {child.pid}: {error}")
        if not report["passed"]:
            report["daemon_log_samples"] = []
            for log in logs[:4]:
                log.seek(0, 2)
                log.seek(max(0, log.tell() - 8192))
                report["daemon_log_samples"].append(log.read().decode(errors="replace"))
        for log in logs:
            log.close()
        try:
            scratch_scope.cleanup()
        except Exception as error:
            cleanup_errors.append(f"scratch cleanup: {error}")
        if cleanup_errors:
            report["cleanup_errors"] = cleanup_errors
            report["passed"] = False
        report["elapsed_seconds"] = round(time.monotonic() - started, 3)
        if query_ms:
            measured = sorted(query_ms)
            report["query_latency_ms"] = {"samples": len(measured), "p50": round(measured[len(measured)//2], 3), "p95": round(measured[min(len(measured)-1, int(len(measured)*.95))], 3), "max": round(measured[-1], 3)}
        args.output.write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report), flush=True)
        if cleanup_errors:
            raise RuntimeError(f"harness cleanup failed: {cleanup_errors}")


if __name__ == "__main__":
    main()
