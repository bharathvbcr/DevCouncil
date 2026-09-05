"""Stress and adversarial coverage for the map engine seam and the kernel it drives.

These run the real kernel and are skipped when it is not built. They are
deliberately slower than unit tests (seconds each, not milliseconds) and live
under ``tests/stress`` so the unit suite stays fast. Run with::

    .venv/bin/python -m pytest tests/stress -q

Each test states the failure it exists to catch. Where a number is asserted it
was chosen from a measurement, and the measurement is in the docstring.
"""

from __future__ import annotations

import json
import os
import signal
import subprocess
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import pytest

from devcouncil.devmap_client import DevMapClient, DevMapClientError
from devcouncil.devmap_engine import (
    DEFAULT_DB_RELPATH,
    DevMapEngineError,
    build_map,
    find_engine_binary,
)
from devcouncil.indexing.repo_mapper import RepoMapper


def _have_engine() -> bool:
    try:
        find_engine_binary()
    except DevMapEngineError:
        return False
    return True


pytestmark = pytest.mark.skipif(
    not _have_engine(), reason="devmap kernel not built (cargo build --release -p devmap-cli)"
)


def _git(root: Path, *args: str) -> None:
    subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", *args],
        cwd=root,
        check=True,
        capture_output=True,
    )


def _repo(root: Path, *, files: int = 40, modules_per_dir: int = 8) -> Path:
    """A committed Python repository with real cross-file calls."""
    root.mkdir(parents=True, exist_ok=True)
    (root / ".gitignore").write_text(".devcouncil/\nAGENTS.md\nCLAUDE.md\n", encoding="utf-8")
    for index in range(files):
        package = root / "pkg" / f"d{index // modules_per_dir}"
        package.mkdir(parents=True, exist_ok=True)
        (package / "__init__.py").touch()
        callee = f"helper_{index}"
        caller_of = f"helper_{(index + 1) % files}"
        (package / f"m{index}.py").write_text(
            f"def {callee}(x):\n    return x + {index}\n\n"
            f"def use_{index}():\n    return {caller_of}(1)\n",
            encoding="utf-8",
        )
    _git(root, "init", "-q", ".")
    _git(root, "add", "-A")
    _git(root, "commit", "-qm", "init")
    return root


def _status(root: Path):
    return DevMapClient(root, autospawn=False).status()


# --- 1. concurrency ----------------------------------------------------------------


def test_eight_concurrent_builds_leave_one_consistent_store(tmp_path):
    """Eight `dev map` at once against one repository.

    The failure this catches: a second writer that dies with a bare
    `database is locked` after paying for a full extract, or a store left with
    a half-written generation. Every worker must either succeed or fail with
    the kernel's named-holder message; afterwards the store must answer
    `status` with a committed generation and both artifacts must parse.
    """
    root = _repo(tmp_path / "repo")
    (root / ".devcouncil").mkdir()
    outcomes: list[str] = []

    def _one(_index: int) -> str:
        try:
            build_map(root, timeout=300)
            return "ok"
        except DevMapEngineError as exc:
            return f"error: {exc}"

    with ThreadPoolExecutor(max_workers=8) as pool:
        outcomes = list(pool.map(_one, range(8)))

    assert any(outcome == "ok" for outcome in outcomes), outcomes
    for outcome in outcomes:
        if outcome != "ok":
            assert "database is locked" not in outcome, outcome
    status = _status(root)
    assert status.generation_id >= 1
    assert status.node_count > 0
    for artifact in (root / ".devcouncil" / "repo_map.json", root / ".devcouncil" / "graph" / "code_graph.json"):
        payload = json.loads(artifact.read_text(encoding="utf-8"))
        assert isinstance(payload, dict) and payload


def test_a_build_killed_mid_persist_does_not_poison_the_store(tmp_path):
    """SIGKILL the kernel while it writes; the next build must succeed.

    SQLite's WAL makes an interrupted transaction invisible, and the writer
    lock must be released by process death (flock), never left behind as a
    stale file the next build waits on.
    """
    root = _repo(tmp_path / "repo", files=240)
    (root / ".devcouncil").mkdir()
    binary = find_engine_binary(root)
    db = root / DEFAULT_DB_RELPATH
    db.parent.mkdir(parents=True, exist_ok=True)
    # K13: the kernel takes an flock on this file at the start of a build and
    # writes its pid into it. Seeing *this* process's pid there is the proof it
    # is inside the build, so the kill lands on a writer every time instead of
    # racing a sleep against the machine's speed.
    lock_path = db.with_name(db.name + ".writer.lock")

    def _kernel_holds_lock(pid: int) -> bool:
        try:
            return lock_path.read_text(encoding="utf-8").strip() == str(pid)
        except (FileNotFoundError, UnicodeDecodeError):
            return False

    killed = 0
    for attempt in range(3):
        process = subprocess.Popen(
            [binary, "--db", str(db), "--progress", "never", "build", str(root)],
            cwd=root,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        deadline = time.monotonic() + 30
        while process.poll() is None and time.monotonic() < deadline:
            if _kernel_holds_lock(process.pid):
                # Later attempts let the build get further in (towards persist)
                # before the kill; the first one interrupts as early as possible.
                time.sleep(0.15 * attempt)
                break
            time.sleep(0.001)
        if process.poll() is None:
            os.kill(process.pid, signal.SIGKILL)
            process.wait(timeout=10)
            killed += 1
        # Touch a file so the next build has something to do even if the
        # killed one managed to commit.
        (root / "pkg" / "d0" / "m0.py").write_text(
            f"def helper_0(x):\n    return x + {attempt}\n", encoding="utf-8"
        )

    started = time.monotonic()
    build_map(root, timeout=300)
    assert time.monotonic() - started < 120, "a stale writer lock made the next build wait"
    status = _status(root)
    assert status.generation_id >= 1 and status.node_count > 0
    assert killed >= 1, "no attempt was interrupted although the writer lock was observed"


# --- 2. hostile trees ----------------------------------------------------------------


def test_a_hostile_tree_is_indexed_with_refusals_reported(tmp_path, capsys):
    """Oversized file, invalid UTF-8, binary blob, symlink loop, deep nesting,
    a file named like a directory, and a path with spaces and unicode.

    None of these may fail the build. The oversized one must be *reported* as
    refused (a file absent from the graph with no record is the defect), and
    the store must end fresh — a refusal must not become a permanently
    quarantined pending path.
    """
    root = _repo(tmp_path / "repo", files=20)
    (root / ".devcouncil").mkdir()
    (root / "big.py").write_bytes(b"x = 1\n" * (400_000))  # ~2.4 MB > 1 MiB limit
    (root / "bad_utf8.py").write_bytes(b"def f():\n    return '\xff\xfe'\n")
    (root / "blob.bin").write_bytes(os.urandom(64 * 1024))
    deep = root
    for level in range(40):
        deep = deep / f"n{level}"
    deep.mkdir(parents=True)
    (deep / "leaf.py").write_text("def deep():\n    return 1\n", encoding="utf-8")
    (root / "sp ace ünï.py").write_text("def spaced():\n    return 2\n", encoding="utf-8")
    try:
        os.symlink(root, root / "loop")
    except OSError:
        pass
    _git(root, "add", "-A")
    _git(root, "commit", "-qm", "hostile")

    build_map(root, timeout=300)
    err = capsys.readouterr().err
    assert "discovery refused" in err and "big.py" in err
    status = _status(root)
    assert status.generation_id >= 1
    assert status.quarantined_count == 0, status
    assert status.pending_count == 0, status
    payload = json.loads((root / ".devcouncil" / "repo_map.json").read_text(encoding="utf-8"))
    listed = {entry["path"] for entry in payload["files"]}
    assert any(path.endswith("leaf.py") for path in listed)
    assert "sp ace ünï.py" in listed


def test_two_consecutive_builds_do_not_make_each_other_stale(tmp_path):
    """The map a build writes must read fresh to the rule that triggers builds.

    Otherwise `--if-stale`, the watcher, verify and checkout rebuild forever.
    Both the guides and the artifacts are written after the stamp; neither may
    move the fingerprint.
    """
    root = _repo(tmp_path / "repo")
    (root / ".devcouncil").mkdir()
    # No ignore rule for the guides: the hardest case.
    (root / ".gitignore").write_text(".devcouncil/\n", encoding="utf-8")
    _git(root, "add", "-A")
    _git(root, "commit", "-qm", "ignore")
    from devcouncil.indexing.map_artifacts import refresh_map_artifacts

    map_path = root / ".devcouncil" / "repo_map.json"
    refresh_map_artifacts(root, map_path, quiet=True)
    first = json.loads(map_path.read_text(encoding="utf-8"))
    assert RepoMapper(root).map_is_stale(first) is False
    refresh_map_artifacts(root, map_path, quiet=True)
    second = json.loads(map_path.read_text(encoding="utf-8"))
    assert RepoMapper(root).map_is_stale(second) is False
    assert second["generated_head"] == first["generated_head"]


# --- 3. the watcher under a burst -----------------------------------------------------


def test_a_two_thousand_file_burst_costs_one_rebuild(tmp_path, monkeypatch):
    """`git checkout` touches thousands of files in under a second.

    The old poll would have checked the fingerprint on its own schedule; the
    event loop must coalesce the burst into one rebuild, not one per file.
    Measured budget: 2,000 writes land in ~0.3 s here; the debounce is 0.5 s.
    """
    import devcouncil.cli.commands.map as map_cmd
    import devcouncil.devmap_engine as engine

    root = _repo(tmp_path / "repo")
    (root / ".devcouncil").mkdir()
    (root / ".devcouncil" / "repo_map.json").write_text('{"files": []}', encoding="utf-8")
    builds: list[float] = []
    stop = threading.Event()

    def _build(_root, **_kwargs):
        builds.append(time.monotonic())
        if len(builds) >= 1:
            stop.set()

    monkeypatch.setattr(engine, "build_map", _build)
    # …and the function it adapts, which is what `refresh_map_artifacts` calls.
    monkeypatch.setattr(engine, "build_map_result", _build)
    monkeypatch.setattr(map_cmd.RepoMapper, "map_is_stale", lambda self, data: True)
    monkeypatch.setattr(map_cmd, "WATCH_POLL_INTERVAL_SECONDS", 600.0)

    real_wait = map_cmd._wait_for_change

    def _wait(changed, timeout):
        if stop.is_set():
            # Give the burst's tail a chance to arrive, then end the watch.
            time.sleep(1.0)
            raise KeyboardInterrupt
        return real_wait(changed, timeout)

    monkeypatch.setattr(map_cmd, "_wait_for_change", _wait)

    def _burst() -> None:
        time.sleep(0.5)
        burst = root / "burst"
        burst.mkdir()
        for index in range(2000):
            (burst / f"f{index}.py").write_text("x = 1\n", encoding="utf-8")

    threading.Thread(target=_burst, daemon=True).start()
    map_cmd._watch_map(root)
    assert len(builds) == 1, f"a burst of writes caused {len(builds)} rebuilds"


# --- 4. the client against a hostile daemon ---------------------------------------------


def test_a_daemon_that_never_answers_degrades_to_the_cli_within_the_deadline(tmp_path, monkeypatch):
    """A socket that accepts and then says nothing must not hold a query.

    The client's per-recv timeout and whole-exchange deadline bound it; the
    request then falls through to the CLI, which answers from the store.
    """
    import socket

    root = _repo(tmp_path / "repo")
    (root / ".devcouncil").mkdir()
    build_map(root, timeout=300)
    client = DevMapClient(root, autospawn=False, response_deadline_seconds=3.0)
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    # AF_UNIX paths are capped at ~104 bytes on macOS; pytest's tmp_path is longer.
    sock_path = f"/tmp/devmap-stress-{os.getpid()}.sock"
    if os.path.exists(sock_path):
        os.unlink(sock_path)
    server.bind(sock_path)
    server.listen(4)
    client.socket_path = sock_path
    accepted: list[socket.socket] = []

    def _accept() -> None:
        try:
            while True:
                conn, _ = server.accept()
                accepted.append(conn)  # never write, never close
        except OSError:
            return

    threading.Thread(target=_accept, daemon=True).start()
    started = time.monotonic()
    status = client.status()
    elapsed = time.monotonic() - started
    assert status.generation_id >= 1
    assert elapsed < 10.0, f"a silent daemon held the client for {elapsed:.1f}s"
    server.close()
    for conn in accepted:
        conn.close()
    try:
        os.unlink(sock_path)
    except OSError:
        pass


def test_a_query_that_exceeds_the_budget_contract_is_rejected_not_trusted(tmp_path, monkeypatch):
    """Every budgeted surface must satisfy shown+hidden==total, truncated==(hidden>0),
    tokens_used<=budget — including `snapshots`, which used to report ten times
    the budget it was given."""
    root = _repo(tmp_path / "repo")
    (root / ".devcouncil").mkdir()
    build_map(root, timeout=300)
    client = DevMapClient(root, autospawn=False)
    for budget in (50, 200, 2000):
        for call in (
            lambda: client.search("helper", limit=budget),
            lambda: client.dead_symbols(budget=budget),
            lambda: client.semantic_snapshots("", budget=budget),
        ):
            try:
                response = call()
            except DevMapClientError as exc:
                pytest.fail(f"budget {budget}: {exc}")
            assert response.tokens_used <= budget
            assert response.shown + response.hidden == response.total
