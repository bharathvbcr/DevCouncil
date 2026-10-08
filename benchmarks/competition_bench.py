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
import ground_truth

MAX_RESPONSE = 16 * 1024 * 1024
MAX_SESSION_OUTPUT = 64 * 1024 * 1024
TOOLS = ("devmap", "codegraph", "cbm")
# Edit-latency arms: config key -> tool kind. `devmap_b` is a second DevMap
# executable timed against the first in the same interleaved schedule.
ARMS = (("devmap", "devmap"), ("devmap_b", "devmap"), ("codegraph", "codegraph"), ("cbm", "cbm"))
EDIT_LATENCY_REPEAT = (2, 60)
CALLERS_LIMIT = 1000


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

    def callers(self, symbol: str) -> list[str] | None:
        """Direct callers of `symbol`, or None where this harness has not
        verified a callers interface for the tool."""
        if self.tool == "devmap":
            return [self.binary, "--db", str(self.db), "--json", "impact", symbol,
                    "--depth", "1", "--budget", "100000"]
        if self.tool == "codegraph":
            return [self.binary, "callers", symbol, "--path", str(self.root),
                    "--limit", str(CALLERS_LIMIT), "--json"]
        return None

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


def callers_projection(tool: str, symbol: str, value: object) -> tuple[set[tuple[str, str]], bool, dict]:
    """Function-level direct callers as (file, name), whether the answer was
    capped, and what was set aside. Unknown schemas fail, never imply absence."""
    value = unwrap(value)
    if tool == "devmap" and isinstance(value, dict) and "items" in value:
        if value.get("resolution") != "Available":
            raise BenchError("DevMap impact resolution is not Available")
        rows, other = set(), 0
        for item in value["items"]:
            if item["edge_kind"] == "Calls" and item["target_symbol"].rsplit("::", 1)[-1].rsplit(".", 1)[-1] == symbol:
                rows.add((item["source_file"], item["source_symbol"].rsplit("::", 1)[-1].rsplit(".", 1)[-1]))
            else:
                other += 1
        namesakes = value.get("unresolved_namesakes") or {}
        return rows, bool(value["truncated"] or value.get("hidden")), {
            "non_call_edges": other, "unresolved_namesake_sites": len(namesakes.get("sites") or [])}
    if tool == "codegraph" and isinstance(value, dict) and isinstance(value.get("callers"), list):
        callable_kinds = {"function", "method"}
        rows = {(row["filePath"], row["name"]) for row in value["callers"] if row["kind"] in callable_kinds}
        other = sum(row["kind"] not in callable_kinds for row in value["callers"])
        return rows, len(value["callers"]) >= CALLERS_LIMIT, {"non_function_rows": other}
    raise BenchError(f"unrecognized {tool} callers response; native output retained")


def caller_check(recorder: Recorder, adapter: Adapter, arm: str, env: dict, truth: dict) -> dict:
    """Score one arm's callers answers against the generated truth."""
    if adapter.callers("probe") is None:
        return {"arm": arm, "status": "not_measured",
                "reason": f"no callers interface verified for {adapter.tool} in this harness"}
    per_symbol, totals = [], {"expected": 0, "returned": 0, "true_positives": 0}
    capped = unverified = 0
    for symbol, rows in truth["callers"].items():
        expected = {tuple(row) for row in rows}
        sample = recorder.command(f"{arm}-callers-{symbol}", adapter.callers(symbol), adapter.root, env)
        try:
            returned, incomplete, set_aside = callers_projection(adapter.tool, symbol, recorder.read(sample))
        except (BenchError, ValueError, KeyError, TypeError) as exc:
            unverified += 1
            per_symbol.append({"symbol": symbol, "status": "unverified", "error": str(exc),
                               "native": sample["stdout"]})
            continue
        scored = ground_truth.score(expected, returned)
        capped += incomplete
        per_symbol.append({"symbol": symbol, "incomplete": incomplete, "native": sample["stdout"],
                           **set_aside, **scored})
        for key in totals:
            totals[key] += scored[key]
    return {"arm": arm, "status": "measured" if not unverified else "partially_unverified",
            "symbols": len(truth["callers"]), "unverified_symbols": unverified, "capped_answers": capped,
            **totals,
            "precision": totals["true_positives"] / totals["returned"] if totals["returned"] else None,
            "recall": totals["true_positives"] / totals["expected"] if totals["expected"] else None,
            "per_symbol": per_symbol}


def counterbalanced_orders(arms: list[str], rounds: int) -> list[list[str]]:
    """One arm order per round: a rotated Latin square, every other block
    reversed. Over each block of len(arms) rounds every arm takes every position
    once; reversing alternate blocks also cancels a linear drift, which for two
    arms is the ABBA design. A partial block cannot balance, so it is refused."""
    count = len(arms)
    if count == 0:
        raise BenchError("no arms configured")
    if rounds <= 0 or rounds % count:
        raise BenchError(f"rounds ({rounds}) must be a positive multiple of the arm count ({count}) "
                         "so every arm takes every position equally often")
    orders = []
    for block in range(rounds // count):
        rotations = [arms[shift:] + arms[:shift] for shift in range(count)]
        orders.extend(reversed(rotations) if block % 2 else rotations)
    return orders


def run_rounds(orders: list[list[str]], step) -> list[list[str]]:
    """Call step(arm, round, position) in schedule order; return the order
    actually executed, so the record proves what ran rather than what was planned."""
    executed = []
    for round_index, order in enumerate(orders):
        executed.append([])
        for position, arm in enumerate(order):
            step(arm, round_index, position)
            executed[-1].append(arm)
    return executed


def summarize_latency(samples: list[dict]) -> dict:
    """Per arm and scenario: n, min, median, max over verified samples only,
    the failures beside them, and the same split by position in the round."""
    summary: dict[str, dict] = {}
    for sample in samples:
        cell = summary.setdefault(f"{sample['arm']}/{sample['scenario']}",
                                  {"verified_ms": [], "failed": 0, "by_position": {}})
        if not sample["verified"]:
            cell["failed"] += 1
            continue
        cell["verified_ms"].append(sample["wall_ms"])
        cell["by_position"].setdefault(str(sample["position"]), []).append(sample["wall_ms"])

    def stats(values: list[float]) -> dict:
        if not values:
            return {"n": 0, "min_ms": None, "median_ms": None, "max_ms": None}
        return {"n": len(values), "min_ms": min(values), "median_ms": statistics.median(values),
                "max_ms": max(values)}

    return {key: {**stats(cell["verified_ms"]), "failed": cell["failed"],
                  "by_position": {position: stats(values) for position, values in sorted(cell["by_position"].items())}}
            for key, cell in sorted(summary.items())}


def bench_env(state: Path) -> dict:
    env = {key: value for key, value in os.environ.items() if key in (
        "PATH", "HOME", "TMPDIR", "LANG", "LC_ALL", "USER", "LOGNAME", "SystemRoot")}
    env.update({"XDG_CONFIG_HOME": str(state / "config"), "XDG_CACHE_HOME": str(state / "cache"),
                "XDG_DATA_HOME": str(state / "data"), "CBM_CACHE_DIR": str(state / "cbm"),
                "DO_NOT_TRACK": "1", "NO_COLOR": "1", "CODEGRAPH_MCP_TOOLS": "search,callers"})
    return env


def clone_revision(out: Path, repo: dict, root: Path, state: Path) -> None:
    run(["git", "clone", "--quiet", "--no-hardlinks", "--no-checkout", repo["path"], str(root)], cwd=out)
    run(["git", "-c", "core.hooksPath=/dev/null", "checkout", "--quiet", "--detach", repo["revision"]], cwd=root)
    actual_revision = run(["git", "rev-parse", "HEAD"], cwd=root).stdout.strip()
    resolved_revision = run(["git", "rev-parse", repo["revision"] + "^{commit}"], cwd=Path(repo["path"])).stdout.strip()
    if actual_revision != resolved_revision:
        raise BenchError("clone revision differs from resolved source commit")
    save(state / "revision.json", {"actual": actual_revision, "source": resolved_revision})


def edit_latency(args: argparse.Namespace, config: dict, out: Path, recorder: Recorder) -> int:
    """Interleaved, counterbalanced one-file edit latency, plus the caller check.

    Every arm indexes its own clone of the same revision with the same planted
    fixtures. Each round, in that round's counterbalanced order, every arm edits
    one file, rebuilds, and must find the new symbol; then restores, rebuilds,
    and must no longer find it. Only verified samples are timed."""
    arms = [(key, tool) for key, tool in ARMS if config.get(key)]
    skipped = [key for key, _ in ARMS if not config.get(key)]
    names = [key for key, _ in arms]
    low, high = EDIT_LATENCY_REPEAT
    if not low <= args.repeat <= high:
        raise BenchError(f"edit-latency repeat must be between {low} and {high}")
    orders = counterbalanced_orders(names, args.repeat)
    identities = {key: binary_identity(config[key]) for key in names}
    save(out / "binaries.json", identities)
    planted = ground_truth.truth(args.seed)
    save(out / "ground_truth.json", planted)
    samples: list[dict] = []
    report: dict = {"mode": "edit-latency", "repeat": args.repeat, "seed": args.seed,
                    "arms": names, "skipped_arms": skipped, "repositories": []}
    for repo in config["repositories"]:
        name = repo["name"]
        if not re.fullmatch(r"[a-z0-9_-]{1,40}", name):
            raise BenchError("repository name must be a safe directory component")
        adapters, envs, baselines = {}, {}, {}
        caller_results, cold_failed = [], []
        for key, tool in arms:
            prefix = f"{name}-{key}"
            state = out / prefix
            state.mkdir()
            root = state / "repo"
            clone_revision(out, repo, root, state)
            ground_truth.generate(root, args.seed)
            run(["git", "add", "--", *sorted({Path(p).parts[0] for p in ground_truth.sources(args.seed)})], cwd=root)
            baselines[key] = manifest(root)
            save(state / "source-manifest.json", baselines[key])
            env = bench_env(state)
            adapter = Adapter(tool, config[key], root, state, prefix)
            if tool == "devmap":
                paths = recorder.command(prefix + "-paths", [adapter.binary, "--json", "paths", str(root)], root, env)
                locations = recorder.read(paths)
                if locations["root"] != str(root):
                    raise BenchError("DevMap resolved a different repository")
                adapter.db = Path(locations["db_path"])
            cold = recorder.command(prefix + "-cold", adapter.build(True), root, env)
            if not cold["ok"]:
                cold_failed.append(key)
                continue
            caller_results.append(caller_check(recorder, adapter, key, env, planted))
            adapters[key], envs[key] = adapter, env
        if set(baselines) != set(names) or len({json.dumps(b) for b in baselines.values()}) != 1:
            raise BenchError(f"corpus bytes differ across arm clones: {name}")
        live_orders = [[arm for arm in order if arm in adapters] for order in orders]
        if cold_failed:
            # A schedule with a missing arm no longer balances; say so rather
            # than letting the surviving arms' numbers read as counterbalanced.
            report.setdefault("unbalanced", []).append({"repository": name, "cold_failed": cold_failed})
        probe_rel = ground_truth.module_path("python", 0)

        def step(arm: str, round_index: int, position: int) -> None:
            adapter, env = adapters[arm], envs[arm]
            probe = adapter.root / probe_rel
            original = probe.read_bytes()
            symbol = f"gt_python_edit_r{round_index}"
            load = os.getloadavg()
            try:
                probe.write_bytes(original + f"\n\ndef {symbol}():\n    return gt_python_leaf_0()\n".encode())
                for scenario, expected in (("edit", {(probe_rel, symbol)}), ("restore", set())):
                    if scenario == "restore":
                        probe.write_bytes(original)
                    update = recorder.command(f"{name}-{arm}-{scenario}", adapter.build(), adapter.root, env)
                    check = recorder.command(f"{name}-{arm}-check-{scenario}", adapter.search(symbol), adapter.root, env)
                    try:
                        checked = qualify_search(check, adapter.tool, recorder.read(check), expected)
                    except (BenchError, ValueError) as exc:
                        checked = {"passed": False, "error": str(exc)}
                    verified = bool(update["ok"] and checked.get("passed") is True)
                    update.update({"arm": arm, "tool": adapter.tool, "scenario": scenario, "round": round_index,
                                   "position": position, "loadavg": load, "verified": verified,
                                   "semantic_status": "passed" if verified else "unverified"})
                    samples.append({key: update[key] for key in ("arm", "scenario", "round", "position",
                                                                 "wall_ms", "verified", "loadavg", "measurement_id")})
            finally:
                probe.write_bytes(original)

        verifications = {}
        guards = [source_guard(adapters[key].root, baselines[key], out / f"{name}-{key}") for key in adapters]
        for guard in guards:
            guard.__enter__()
        try:
            executed = run_rounds(live_orders, step)
        finally:
            for key, guard in zip(list(adapters), guards):
                try:
                    guard.__exit__(None, None, None)
                    verifications[key] = "unchanged"
                except BenchError as exc:
                    verifications[key] = str(exc)
        if executed != live_orders:
            raise BenchError("executed order differs from the planned schedule")
        report["repositories"].append({"name": name, "planned_orders": live_orders, "executed_orders": executed,
                                       "source_verification": verifications, "callers": caller_results})
        recorder.checkpoint()
        print(f"{name} edit-latency complete", flush=True)
    report["summary"] = summarize_latency(samples)
    save(out / "samples.json", samples)
    save(out / "edit-latency.json", report)
    final = {key: binary_identity(config[key]) for key in names}
    save(out / "binaries-after.json", final)
    changed = [key for key in names if identities[key]["sha256"] != final[key]["sha256"]]
    if changed:
        raise BenchError(f"executable changed during campaign: {changed}; timings invalid")
    if any(value != "unchanged" for repo in report["repositories"] for value in repo["source_verification"].values()):
        raise BenchError("a corpus changed during the edit-latency rounds; timings invalid")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--repeat", type=int, default=3)
    parser.add_argument("--mode", choices=("campaign", "edit-latency"), default="campaign")
    parser.add_argument("--seed", type=int, default=20261007,
                        help="edit-latency: seed for the generated caller ground truth")
    args = parser.parse_args()
    if args.mode == "campaign" and not 3 <= args.repeat <= 20:
        parser.error("repeat must be between 3 and 20")
    config = json.loads(args.config.read_text())
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    recorder = Recorder(out)
    save(out / "config.json", config)
    if args.mode == "edit-latency":
        return edit_latency(args, config, out, recorder)
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
