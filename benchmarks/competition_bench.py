#!/usr/bin/env python3
"""Pinned local DevMap/CodeGraph/CBM comparison, with CLI and persistent MCP.

Run with --config JSON --out NEW_DIRECTORY. Config names absolute executable
paths and repositories [{name, path, revision}]. Only fresh clones are edited.
Raw native outputs and unsuccessful cells are retained; timings are descriptive,
never an accuracy score. No packages, credentials or providers are installed.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import re
import selectors
import signal
import statistics
import subprocess
import threading
import time

from map_bench import BenchError, binary_identity, run

MAX_RESPONSE = 16 * 1024 * 1024
MAX_SESSION_OUTPUT = 64 * 1024 * 1024
TOOLS = ("devmap", "codegraph", "cbm")


def save(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")


def stop(process: subprocess.Popen) -> None:
    # A completed leader can leave workers in its process group. This group
    # was created exclusively for this invocation, so clean it on every exit.
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    if process.poll() is None:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=3)
    try:
        os.killpg(process.pid, 0)
    except ProcessLookupError:
        return
    # Give workers time to flush shutdown state even if the leader exited first.
    time.sleep(0.05)
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass


class Recorder:
    def __init__(self, out: Path):
        self.out = out
        self.records: list[dict] = []

    def command(self, label: str, argv: list[str], cwd: Path, env: dict,
                timeout: float = 180) -> dict:
        stem = f"{len(self.records):05}-{label}"
        begin = time.perf_counter()
        with (self.out / f"{stem}.stdout").open("wb") as stdout, (
            self.out / f"{stem}.stderr"
        ).open("wb") as stderr:
            process = subprocess.Popen(argv, cwd=cwd, env=env, stdout=stdout,
                                       stderr=stderr, start_new_session=True)
            finished = threading.Event()
            finished_at: float | None = None
            def reap() -> None:
                nonlocal finished_at
                process.wait()
                finished_at = time.perf_counter()
                finished.set()
            waiter = threading.Thread(target=reap, daemon=True)
            waiter.start()
            deadline = time.monotonic() + timeout
            output_limited = False
            while not finished.wait(min(0.05, max(0, deadline - time.monotonic()))):
                if stdout.tell() > MAX_RESPONSE or stderr.tell() > MAX_RESPONSE:
                    output_limited = True
                    break
                if time.monotonic() >= deadline:
                    break
            timed_out = not finished.is_set() and not output_limited
            # Measure completion before cleanup; a worker shutdown is recorded
            # separately, not charged to an already completed query.
            elapsed = ((finished_at or time.perf_counter()) - begin) * 1000
            try:
                stop(process)
            finally:
                waiter.join(timeout=3)
            output_sizes = {"stdout": os.fstat(stdout.fileno()).st_size,
                            "stderr": os.fstat(stderr.fileno()).st_size}
            # A fast process can finish before the polling loop observes its
            # output. Budget qualification must include completed processes.
            output_limited = output_limited or any(size > MAX_RESPONSE for size in output_sizes.values())
        record = {"label": label, "command": argv, "cwd": str(cwd),
                  "wall_ms": elapsed,
                  "cleanup_ms": (time.perf_counter() - begin) * 1000 - elapsed,
                  "exit_code": process.returncode, "timed_out": timed_out,
                  "output_limited": output_limited,
                  "output_bytes": output_sizes,
                  "ok": process.returncode == 0 and not timed_out and not output_limited,
                  "stdout": f"{stem}.stdout", "stderr": f"{stem}.stderr"}
        self.record(record)
        return record

    def record(self, record: dict) -> None:
        record["measurement_id"] = len(self.records)
        self.records.append(record)
        with (self.out / "measurements.jsonl").open("a") as stream:
            stream.write(json.dumps(record) + "\n")

    def checkpoint(self) -> None:
        with (self.out / "measurements.jsonl").open("w") as stream:
            for record in self.records:
                stream.write(json.dumps(record) + "\n")

    def read(self, record: dict) -> object:
        if not record["ok"]:
            raise BenchError(f"failed cell: {record['label']}; see {record['stderr']}")
        path = self.out / record["stdout"]
        if path.stat().st_size > MAX_RESPONSE:
            raise BenchError(f"response exceeds {MAX_RESPONSE} bytes: {path}")
        return json.loads(path.read_bytes())


class MCP:
    """One bounded stdio session. Records actual startup separately from queries."""
    def __init__(self, argv: list[str], cwd: Path, env: dict, stderr: Path):
        self.started = time.perf_counter()
        self.stderr = stderr.open("wb")
        self.frames = stderr.with_suffix(".frames.jsonl").open("ab")
        self.process = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=self.stderr,
                                        start_new_session=True)
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)
        self.buffer = b""
        self.serial = 0
        self.received_bytes = 0
        self.tools: dict[str, dict] = {}

    def close(self) -> None:
        if self.process.stdin:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            stop(self.process)
        stop(self.process)
        self.selector.close()
        if self.process.stdout:
            self.process.stdout.close()
        self.stderr.close()
        self.frames.close()

    def request(self, method: str, params: dict, timeout: float = 60) -> object:
        self.serial += 1
        request_id = self.serial
        assert self.process.stdin is not None
        self.process.stdin.write((json.dumps({"jsonrpc": "2.0", "id": request_id,
                                             "method": method, "params": params}) + "\n").encode())
        self.process.stdin.flush()
        deadline = time.monotonic() + timeout
        request_bytes = len(self.buffer)
        while time.monotonic() < deadline:
            if os.fstat(self.stderr.fileno()).st_size > MAX_RESPONSE:
                raise BenchError("MCP stderr exceeds response budget")
            while b"\n" in self.buffer:
                line, self.buffer = self.buffer.split(b"\n", 1)
                self.frames.write(line + b"\n")
                self.frames.flush()
                value = json.loads(line)
                if value.get("id") != request_id:
                    continue
                if "error" in value:
                    raise BenchError(f"MCP {method}: {value['error']}")
                return value["result"]
            # Poll stderr even if a server stops producing stdout. File-backed
            # logs can overshoot between polls, but cannot qualify as a pass.
            if not self.selector.select(min(0.05, max(0, deadline - time.monotonic()))):
                continue
            assert self.process.stdout is not None
            chunk = os.read(self.process.stdout.fileno(), 65536)
            if not chunk:
                raise BenchError("MCP stdout closed before matching response")
            request_bytes += len(chunk)
            self.received_bytes += len(chunk)
            if request_bytes > MAX_RESPONSE or self.received_bytes > MAX_SESSION_OUTPUT:
                raise BenchError("MCP output exceeds aggregate response/session budget")
            self.buffer += chunk
            if len(self.buffer) > MAX_RESPONSE:
                raise BenchError("MCP frame exceeds response budget")
        raise BenchError(f"MCP {method} timed out after {timeout}s")

    def initialize(self) -> object:
        result = self.request("initialize", {"protocolVersion": "2025-11-25",
            "clientInfo": {"name": "devmap-competition-bench", "version": "1"},
            "capabilities": {}})
        assert self.process.stdin is not None
        self.process.stdin.write(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
        self.process.stdin.flush()
        listing = self.request("tools/list", {})
        self.tools = {tool["name"]: tool for tool in listing["tools"]}
        return {"initialize": result, "tools": listing}

    def call(self, name: str, arguments: dict) -> object:
        if name not in self.tools:
            raise BenchError(f"server did not advertise {name}")
        result = self.request("tools/call", {"name": name, "arguments": arguments})
        if result.get("isError"):
            raise BenchError(f"tool error: {result}")
        return result


def unwrap(value: object) -> object:
    if isinstance(value, dict) and "structuredContent" in value:
        return value["structuredContent"]
    if isinstance(value, dict) and "content" in value:
        texts = [x["text"] for x in value["content"] if x.get("type") == "text"]
        if len(texts) == 1:
            try:
                return json.loads(texts[0])
            except json.JSONDecodeError:
                return {"native_text": texts[0]}
    return value


def search_projection(tool: str, value: object) -> tuple[set[tuple[str, str]], bool]:
    """Definition-only projection. Missing/unknown schemas fail, never imply absence."""
    value = unwrap(value)
    if tool == "devmap" and isinstance(value, dict) and "items" in value:
        if value.get("resolution") != "Available":
            raise BenchError("DevMap search resolution is not Available")
        total, shown, hidden = value["total"], value["shown"], value["hidden"]
        if any(type(n) is not int or n < 0 for n in (total, shown, hidden)) or shown != len(value["items"]):
            raise BenchError("inconsistent DevMap search counts")
        return {(x["file_path"], x["symbol_name"]) for x in value["items"]}, bool(value["truncated"] or hidden or total != shown)
    if tool == "codegraph":
        if isinstance(value, list):
            return {(x["node"]["filePath"], x["node"]["name"]) for x in value}, len(value) >= 100
        if isinstance(value, dict) and "native_text" in value:
            # Current native MCP search renders one block per symbol. Keep the
            # parser anchored to its observed schema; changed prose is a refusal.
            text = value["native_text"]
            if re.fullmatch(r'No results found for "[^"\n]+"', text):
                return set(), False
            count = re.match(r"\*\*Search Results \((\d+) found\)\*\*\n", text)
            rows = re.findall(r"(?:^|\n)\*\*([^*\n]+)\*\* \([^\n]+\)\n([^\n]+):\d+\n", text)
            if count and len(rows) == int(count[1]):
                return {(path, name) for name, path in rows}, len(rows) >= 100 or "output truncated" in text
    if tool == "cbm" and isinstance(value, dict) and "groups" in value:
        columns = value["cols"]
        if "name" not in columns:
            raise BenchError("CBM search projection lacks name column")
        col = columns.index("name")
        return {(group["file"], row[col]) for group in value["groups"] for row in group["rows"]}, bool(value["has_more"])
    raise BenchError(f"unrecognized {tool} search response; native output retained")


def manifest(root: Path) -> list[dict]:
    paths = subprocess.check_output(["git", "ls-files", "-z"], cwd=root).split(b"\0")
    return [{"path": os.fsdecode(path), "bytes": (root / os.fsdecode(path)).stat().st_size,
             "sha256": hashlib.sha256((root / os.fsdecode(path)).read_bytes()).hexdigest()}
            for path in paths if path and (root / os.fsdecode(path)).is_file()
            and not (root / os.fsdecode(path)).is_symlink()]


@contextmanager
def source_guard(root: Path, baseline: list[dict], state: Path):
    try:
        yield
    finally:
        mismatches = []
        for row in baseline:
            try:
                current = hashlib.sha256((root / row["path"]).read_bytes()).hexdigest()
                if current != row["sha256"]:
                    mismatches.append({"path": row["path"], "reason": "hash changed"})
            except OSError as exc:
                mismatches.append({"path": row["path"], "reason": str(exc)})
        save(state / "source-verification.json", {"checked": len(baseline), "mismatches": mismatches})
        if mismatches:
            raise BenchError(f"corpus changed unexpectedly: {state}; comparative timings invalid")


def fixture_sources() -> dict[str, str]:
    return {
        "python.py": "def audit_python_leaf():\n    return 1\n\ndef audit_python_caller():\n    return audit_python_leaf()\n",
        "go.go": "package audit\nfunc audit_go_leaf() int { return 1 }\nfunc audit_go_caller() int { return audit_go_leaf() }\n",
        "rust.rs": "fn audit_rust_leaf() -> i32 { 1 }\nfn audit_rust_caller() -> i32 { audit_rust_leaf() }\n",
        "javascript.js": "function audit_javascript_leaf() { return 1; }\nfunction audit_javascript_caller() { return audit_javascript_leaf(); }\n",
        "typescript.ts": "function audit_typescript_leaf(): number { return 1; }\nfunction audit_typescript_caller(): number { return audit_typescript_leaf(); }\n",
        "c.c": "int audit_c_leaf(void) { return 1; }\nint audit_c_caller(void) { return audit_c_leaf(); }\n",
        "cpp.cpp": "int audit_cpp_leaf() { return 1; }\nint audit_cpp_caller() { return audit_cpp_leaf(); }\n",
        "java.java": "class AuditJava { static int audit_java_leaf() { return 1; }\nstatic int audit_java_caller() { return audit_java_leaf(); } }\n",
        "ruby.rb": "def audit_ruby_leaf\n  1\nend\ndef audit_ruby_caller\n  audit_ruby_leaf()\nend\n",
        "swift.swift": "func audit_swift_leaf() -> Int { return 1 }\nfunc audit_swift_caller() -> Int { return audit_swift_leaf() }\n",
    }


def qualify_search(record: dict, tool: str, native: object, expected: set[tuple[str, str]]) -> dict:
    try:
        rows, incomplete = search_projection(tool, native)
        normalized = {(path, name.rsplit(".", 1)[-1]) for path, name in rows}
        status = "incomplete" if incomplete else "passed" if normalized == expected else "incorrect"
        record["semantic_status"] = status
        return {"expected": sorted(expected), "returned": sorted(normalized),
                "incomplete": incomplete, "passed": status == "passed"}
    except (BenchError, ValueError, KeyError, TypeError, IndexError) as exc:
        record["semantic_status"] = "unverified"
        return {"status": "unverified", "error": str(exc)}


class Adapter:
    def __init__(self, tool: str, binary: str, root: Path, state: Path, project: str):
        self.tool, self.binary, self.root, self.state, self.project = tool, binary, root, state, project
        self.db = state / "devmap.sqlite"

    def build(self, cold: bool = False) -> list[str]:
        if self.tool == "devmap":
            return [self.binary, "--db", str(self.db), "--json", "--progress", "never", "build", str(self.root)]
        if self.tool == "codegraph":
            return [self.binary, "init" if cold else "sync", str(self.root), *(["--yes"] if cold else [])]
        return [self.binary, "cli", "index_repository", "--repo-path", str(self.root), "--name", self.project,
                "--mode", "full", "--persistence", "false"]

    def search(self, query: str) -> list[str]:
        if self.tool == "devmap":
            return [self.binary, "--db", str(self.db), "--json", "search", query, "--budget", "10000"]
        if self.tool == "codegraph":
            return [self.binary, "query", query, "--path", str(self.root), "--limit", "100", "--json"]
        return [self.binary, "cli", "search_graph", "--project", self.project,
                "--name-pattern", "^" + re.escape(query) + "$", "--format", "json", "--limit", "100"]

    def server(self) -> list[str]:
        if self.tool == "devmap":
            return [self.binary, "--db", str(self.db), "mcp", "--root", str(self.root)]
        if self.tool == "codegraph":
            return [self.binary, "serve", "--mcp", "--path", str(self.root), "--no-watch"]
        return [self.binary]

    def search_call(self, query: str) -> tuple[str, dict]:
        if self.tool == "devmap":
            return "devmap_search", {"query": query, "repo_path": str(self.root), "budget": 10000}
        if self.tool == "codegraph":
            return "codegraph_search", {"query": query, "projectPath": str(self.root), "limit": 100}
        return "search_graph", {"project": self.project, "name_pattern": "^" + re.escape(query) + "$", "format": "json", "limit": 100}


def measure_search(recorder: Recorder, adapter: Adapter, label: str, env: dict,
                   query: str, expected: set[tuple[str, str]]) -> tuple[dict, dict]:
    sample = recorder.command(label, adapter.search(query), adapter.root, env)
    try:
        checked = qualify_search(sample, adapter.tool, recorder.read(sample), expected)
    except (BenchError, ValueError) as exc:
        sample["semantic_status"] = "unverified"
        checked = {"status": "unverified", "error": str(exc)}
    return sample, checked


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--repeat", type=int, default=3)
    args = parser.parse_args()
    if not 3 <= args.repeat <= 20:
        parser.error("repeat must be between 3 and 20")
    config = json.loads(args.config.read_text())
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    recorder = Recorder(out)
    save(out / "config.json", config)
    identities = {tool: binary_identity(config[tool]) for tool in TOOLS}
    save(out / "binaries.json", identities)
    checks: list[dict] = []
    corpus_manifests: dict[str, list[dict]] = {}
    for repo in config["repositories"]:
        name = repo["name"]
        if not re.fullmatch(r"[a-z0-9_-]{1,40}", name):
            raise BenchError("repository name must be a safe directory component")
        for tool in TOOLS:
            prefix = f"{name}-{tool}"
            state = out / prefix
            state.mkdir()
            root = state / "repo"
            run(["git", "clone", "--quiet", "--no-hardlinks", "--no-checkout", repo["path"], str(root)], cwd=out)
            run(["git", "-c", "core.hooksPath=/dev/null", "checkout", "--quiet", "--detach", repo["revision"]], cwd=root)
            actual_revision = run(["git", "rev-parse", "HEAD"], cwd=root).stdout.strip()
            resolved_revision = run(["git", "rev-parse", repo["revision"] + "^{commit}"], cwd=Path(repo["path"])).stdout.strip()
            if actual_revision != resolved_revision:
                raise BenchError("clone revision differs from resolved source commit")
            save(state / "revision.json", {"actual": actual_revision, "source": resolved_revision})
            baseline = manifest(root)
            if name in corpus_manifests and baseline != corpus_manifests[name]:
                raise BenchError(f"corpus bytes differ across tool clones: {name}")
            corpus_manifests[name] = baseline
            save(state / "source-manifest.json", baseline)
            with source_guard(root, baseline, state):
                fixtures = root / "audit_cases"
                fixtures.mkdir()
                for path, content in fixture_sources().items():
                    (fixtures / path).write_text(content)
                run(["git", "add", "--", "audit_cases"], cwd=root)
                save(state / "fixture-manifest.json", manifest(root))
                env = {key: value for key, value in os.environ.items() if key in (
                    "PATH", "HOME", "TMPDIR", "LANG", "LC_ALL", "USER", "LOGNAME", "SystemRoot")}
                env.update({"XDG_CONFIG_HOME": str(state / "config"), "XDG_CACHE_HOME": str(state / "cache"),
                            "XDG_DATA_HOME": str(state / "data"), "CBM_CACHE_DIR": str(state / "cbm"),
                            "DO_NOT_TRACK": "1", "NO_COLOR": "1", "CODEGRAPH_MCP_TOOLS": "search,callers"})
                adapter = Adapter(tool, config[tool], root, state, prefix)
                if tool == "devmap":
                    paths = recorder.command(prefix + "-paths", [adapter.binary, "--json", "paths", str(root)], root, env)
                    locations = recorder.read(paths)
                    if locations["root"] != str(root):
                        raise BenchError("DevMap resolved a different repository")
                    adapter.db = Path(locations["db_path"])
                cold = recorder.command(prefix + "-cold", adapter.build(True), root, env)
                if not cold["ok"]:
                    checks.append({"cell": prefix, "status": "index_failed"})
                    continue
                for iteration in range(args.repeat):
                    recorder.command(prefix + "-warm", adapter.build(), root, env)
                for langfile in fixture_sources():
                    language = Path(langfile).stem
                    query = f"audit_{language}_leaf"
                    expected = ("audit_cases/" + langfile, query)
                    sample, checked = measure_search(recorder, adapter, prefix + "-cli-search-" + language, env, query, {expected})
                    checks.append({"cell": prefix + "-cli-" + language, **checked})
                # Repeat matched search requests separately from the language sample.
                for _ in range(args.repeat):
                    sample, checked = measure_search(recorder, adapter, prefix + "-cli-search-python-repeat", env,
                                                     "audit_python_leaf", {("audit_cases/python.py", "audit_python_leaf")})
                    checks.append({"cell": prefix + "-cli-repeat", **checked})
                session = MCP(adapter.server(), root, env, state / "mcp.stderr")
                started = session.started
                try:
                    startup = session.initialize()
                    save(state / "mcp-schema.json", startup)
                    recorder.record({"label": prefix + "-mcp-startup", "ok": True,
                                     "wall_ms": (time.perf_counter() - started) * 1000})
                    queries = ["audit_" + Path(f).stem + "_leaf" for f in fixture_sources()] + ["audit_python_leaf"] * args.repeat
                    for query_index, query in enumerate(queries):
                        before = time.perf_counter()
                        native = session.call(*adapter.search_call(query))
                        elapsed = (time.perf_counter() - before) * 1000
                        raw = f"{len(recorder.records):05}-{prefix}-mcp-search.json"
                        save(out / raw, native)
                        label = prefix + "-mcp-search-" + query
                        if query_index >= len(fixture_sources()):
                            label += "-repeat"
                        sample = {"label": label, "ok": True,
                                         "wall_ms": elapsed, "native": raw,
                                         "output_bytes": (out / raw).stat().st_size}
                        expected_file = next("audit_cases/" + f for f in fixture_sources()
                                             if query == "audit_" + Path(f).stem + "_leaf")
                        checked = qualify_search(sample, tool, native, {(expected_file, query)})
                        recorder.record(sample)
                        checks.append({"cell": prefix + "-mcp-" + query, **checked})
                except (BenchError, ValueError, OSError, KeyError) as exc:
                    recorder.record({"label": prefix + "-mcp-failed", "ok": False,
                                     "wall_ms": (time.perf_counter() - started) * 1000,
                                     "error": str(exc), "semantic_status": "unverified"})
                    checks.append({"cell": prefix + "-mcp", "status": "unverified", "error": str(exc)})
                finally:
                    session.close()
                # Success requires observing changed content, moved definitions and
                # removal through the normal refresh path, not merely zero exit codes.
                probe = fixtures / "python.py"
                original = probe.read_bytes()
                for scenario in ("edit", "rename", "delete", "restore"):
                    for iteration in range(args.repeat):
                        moved = fixtures / "renamed.py"
                        probe.write_bytes(original)
                        if moved.exists():
                            moved.unlink()
                        reset = recorder.command(prefix + "-reset", adapter.build(), root, env)
                        _, initial = measure_search(recorder, adapter, prefix + "-check-reset", env,
                                                     "audit_python_leaf", {("audit_cases/python.py", "audit_python_leaf")})
                        setup_ok = reset["ok"] and initial.get("passed") is True
                        expected_path = "audit_cases/python.py"
                        query = "audit_python_leaf"
                        if scenario == "edit":
                            query = "audit_python_edit_" + str(iteration)
                            probe.write_bytes(original + f"\ndef {query}():\n    return audit_python_leaf()\n".encode())
                        elif scenario == "rename":
                            probe.rename(moved)
                            expected_path = "audit_cases/renamed.py"
                        elif scenario == "delete":
                            probe.unlink()
                        else:
                            probe.write_bytes(b"")
                            empty = recorder.command(prefix + "-empty", adapter.build(), root, env)
                            _, absent = measure_search(recorder, adapter, prefix + "-check-empty", env, query, set())
                            setup_ok = setup_ok and empty["ok"] and absent.get("passed") is True
                            probe.write_bytes(original)
                        update = recorder.command(prefix + "-" + scenario, adapter.build(), root, env)
                        sample = recorder.command(prefix + "-check-" + scenario, adapter.search(query), root, env)
                        try:
                            expected = set() if scenario == "delete" else {(expected_path, query)}
                            checked = qualify_search(sample, tool, recorder.read(sample), expected)
                            update["semantic_status"] = sample["semantic_status"]
                            checked["setup_verified"] = setup_ok
                            if not setup_ok or not update["ok"]:
                                checked["passed"] = False
                                checked["status"] = "setup_failed" if not setup_ok else "update_failed"
                                update["semantic_status"] = sample["semantic_status"] = "unverified"
                            checks.append({"cell": prefix + "-" + scenario, "iteration": iteration, **checked})
                        except (BenchError, ValueError, KeyError, TypeError) as exc:
                            update["semantic_status"] = sample["semantic_status"] = "unverified"
                            checks.append({"cell": prefix + "-" + scenario, "status": "unverified", "error": str(exc)})
                probe.write_bytes(original)
                recorder.command(prefix + "-final-restore", adapter.build(), root, env)
                mismatches = [row["path"] for row in baseline
                              if hashlib.sha256((root / row["path"]).read_bytes()).hexdigest() != row["sha256"]]
                checks.append({"cell": prefix + "-source-preservation", "checked": len(baseline), "mismatches": mismatches})
                save(out / "checks.json", checks)
                recorder.checkpoint()
                print(prefix + " complete", flush=True)
    groups: dict[str, list[float]] = {}
    for record in recorder.records:
        if record["ok"] and record.get("semantic_status", "passed") == "passed":
            groups.setdefault(record["label"], []).append(record["wall_ms"])
    save(out / "summary.json", {label: {"n": len(values), "median_ms": statistics.median(values),
                                      "min_ms": min(values), "max_ms": max(values)}
                               for label, values in groups.items()})
    save(out / "checks.json", checks)
    final_identities = {tool: binary_identity(config[tool]) for tool in TOOLS}
    save(out / "binaries-after.json", final_identities)
    changed = [tool for tool in TOOLS if identities[tool]["sha256"] != final_identities[tool]["sha256"]]
    save(out / "coverage.json", {"repositories": len(config["repositories"]), "tools": list(TOOLS),
        "languages": len(fixture_sources()), "samples": len(recorder.records),
        "failed_invocations": sum(not r["ok"] for r in recorder.records),
        "invalid_or_incomplete_answers": sum(r.get("semantic_status") in ("incorrect", "incomplete", "unverified") for r in recorder.records),
        "binary_changes": changed, "provenance_valid": not changed,
        "source_preservation_valid": True,
        "limitations": ["One cold sample per corpus/tool; do not rank cold medians.",
            "Native payloads differ; projected definitions are the comparison contract.",
            "Synthetic direct-function cases do not estimate repository-wide recall.",
            "No hosted credentials passed; bundled local extraction providers still differ.",
            "No population-wide or universally complete-coverage claim."]})
    if changed:
        raise BenchError(f"executable changed during campaign: {changed}; timings invalid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
