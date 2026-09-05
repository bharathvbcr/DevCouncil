"""devmap_health.py — what `dev map status` and `dev map doctor` report.

Both commands used to describe the *Python* engine: its `index.sqlite`
(schema 2, generation 76), its grammar wheels, its writer lease. `dev map` had
moved to the Rust kernel, so the commands the operator reaches for when a map
looks wrong reported on an engine the map no longer came from — "index STALE"
against a fresh map, "grammars 6/35" against a kernel that links 32.

Everything here is read from the surfaces the kernel actually owns: the binary
that will run, its store, the daemon socket, and the two artifacts. A probe
that cannot run reports ``None``/an error, never a healthy value.
"""

from __future__ import annotations

import datetime as _dt
import json
import os
import shutil
import time
import signal
import sqlite3
import subprocess
from pathlib import Path
from typing import Any, Dict, List, Optional

from devcouncil.devmap_engine import (
    DEFAULT_DB_RELPATH,
    DEFAULT_GRAPH_RELPATH,
    DEFAULT_MAP_RELPATH,
    DevMapEngineError,
    find_engine_binary,
)

#: Free-page share above which the store is worth reclaiming. The kernel's own
#: threshold is 5%; doctor warns well above it because steady state after a
#: build legitimately sits near 50% until the next reclaim lands.
FREELIST_WARN_RATIO = 0.30
#: A WAL this large means checkpoints are not landing (a long-lived reader,
#: usually a daemon, kept every TRUNCATE busy).
WAL_WARN_BYTES = 64 * 1024 * 1024


def _iso(epoch: float) -> str:
    return _dt.datetime.fromtimestamp(epoch).isoformat(timespec="seconds")


def engine_info(root: Path) -> Dict[str, Any]:
    """Which kernel will run for ``root``, when it was built, and what it reports."""
    info: Dict[str, Any] = {
        "binary": None,
        "built_at": None,
        "version": None,
        "schema_version": None,
        "error": None,
    }
    try:
        binary = find_engine_binary(root)
    except DevMapEngineError as exc:
        info["error"] = str(exc)
        return info
    info["binary"] = binary
    try:
        info["built_at"] = _iso(Path(binary).stat().st_mtime)
    except OSError:
        pass
    try:
        probe = subprocess.run(
            [binary, "--version"], capture_output=True, text=True, timeout=15
        )
        text = (probe.stdout or probe.stderr or "").strip()
        info["version"] = text or None
        # `devmap 0.1.0 (store schema 13, code graph schema 2)` — the *first*
        # number after the word `schema` is the store schema, the one that
        # decides whether this binary can open the store. The kernel prints it
        # first for that reason; the package version does not move on a schema
        # bump, and the code-graph number is a different thing entirely.
        if "schema" in text:
            tail = text.split("schema", 1)[1]
            digits = "".join(ch for ch in tail if ch.isdigit() or ch == " ").split()
            if digits:
                info["schema_version"] = int(digits[0])
    except (OSError, subprocess.SubprocessError):
        pass
    return info


def store_info(root: Path) -> Dict[str, Any]:
    """Size, schema and reclaim state of the kernel store, read directly.

    Uses the standard-library sqlite3 module. It cannot open the FTS5 virtual
    tables (the system SQLite here lacks the module) but pragmas and
    `sqlite_master` do not need them.
    """
    path = root / DEFAULT_DB_RELPATH
    info: Dict[str, Any] = {
        "path": str(path),
        "exists": path.is_file(),
        "bytes": None,
        "wal_bytes": None,
        "schema_version": None,
        "page_count": None,
        "freelist_pages": None,
        "freelist_ratio": None,
        "error": None,
    }
    if not path.is_file():
        return info
    try:
        info["bytes"] = path.stat().st_size
        wal = path.with_name(path.name + "-wal")
        info["wal_bytes"] = wal.stat().st_size if wal.is_file() else 0
    except OSError:
        pass
    try:
        conn = sqlite3.connect(f"file:{path}?mode=ro", uri=True, timeout=1.0)
        try:
            info["schema_version"] = int(conn.execute("PRAGMA user_version").fetchone()[0])
            pages = int(conn.execute("PRAGMA page_count").fetchone()[0])
            free = int(conn.execute("PRAGMA freelist_count").fetchone()[0])
            info["page_count"] = pages
            info["freelist_pages"] = free
            info["freelist_ratio"] = (free / pages) if pages else 0.0
        finally:
            conn.close()
    except sqlite3.Error as exc:
        info["error"] = f"{type(exc).__name__}: {exc}"
    return info


def kernel_status(root: Path) -> Dict[str, Any]:
    """The kernel's own status, or an error — never a fabricated healthy row."""
    from devcouncil.devmap_client import DevMapClient, DevMapClientError

    try:
        # A probe must not start a 30-minute daemon as a side effect.
        status = DevMapClient(root, autospawn=False).status()
    except DevMapClientError as exc:
        return {"error": str(exc)}
    return {
        "generation_id": status.generation_id,
        "pending_count": status.pending_count,
        "quarantined_count": status.quarantined_count,
        "node_count": status.node_count,
        "edge_count": status.edge_count,
        "is_fresh": status.is_fresh,
        "degraded_reason": status.degraded_reason,
    }


def daemon_info(root: Path) -> Dict[str, Any]:
    from devcouncil.devmap_client import DevMapClient

    socket_path = DevMapClient(root, autospawn=False).socket_path
    return {"socket": socket_path, "running": os.path.exists(socket_path)}


#: A build whose marker has not been touched for this long while its pid is
#: still alive is reported as stuck. The kernel writes a progress line for every
#: stage and every 5% of extraction; a quiet marker this long is a hang, not a
#: slow phase — measured: the slowest stage on this repository is 2.7 s.
BUILD_STUCK_AFTER_SECONDS = 600.0


def _pid_alive(pid: Optional[int]) -> bool:
    if not pid:
        return False
    try:
        os.kill(int(pid), 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    except OSError:
        return False
    return True


def _process_command(pid: int) -> str:
    """The command line of *pid* as `ps` reports it, or "" when unavailable."""
    try:
        probe = subprocess.run(
            ["ps", "-o", "command=", "-p", str(int(pid))],
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return ""
    return (probe.stdout or "").strip()


def build_activity(root: Path) -> Dict[str, Any]:
    """Is a kernel build running right now, and is it making progress?

    Reads the live marker the seam writes for the duration of a build and the
    kernel's writer lock. A marker whose pid is dead is a build that crashed or
    was killed: reported, never silently discarded, because a build that
    vanished is exactly the event an agent needs to know about.
    """
    from devcouncil.devmap_engine import DEFAULT_DB_RELPATH, LIVE_BUILD_RELPATH

    root = Path(root).expanduser().resolve()
    result: Dict[str, Any] = {
        "in_progress": False,
        "stale_marker": False,
        "stuck": False,
        "pid": None,
        "run_id": None,
        "stage": "",
        "elapsed_s": None,
        "since_progress_s": None,
        "writer_lock": None,
    }
    marker = root / LIVE_BUILD_RELPATH
    if marker.is_file():
        try:
            data = json.loads(marker.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            data = {}
        pid = data.get("pid")
        result["pid"] = pid
        result["run_id"] = data.get("run_id")
        result["stage"] = str(data.get("stage") or "")
        now = time.time()
        started = float(data.get("started_at") or now)
        updated = float(data.get("updated_at") or started)
        result["elapsed_s"] = round(now - started, 1)
        result["since_progress_s"] = round(now - updated, 1)
        alive = _pid_alive(pid) and "devmap" in _process_command(int(pid))
        if alive:
            result["in_progress"] = True
            result["stuck"] = (now - updated) > BUILD_STUCK_AFTER_SECONDS
        else:
            result["stale_marker"] = True
    lock = root / (DEFAULT_DB_RELPATH + ".writer.lock")
    if lock.is_file():
        try:
            holder = int((lock.read_text(encoding="utf-8") or "0").strip() or 0)
        except (OSError, ValueError):
            holder = 0
        result["writer_lock"] = {
            "path": str(lock),
            "pid": holder or None,
            # The flock is released by the OS on exit, so a dead pid in the
            # file is not a held lock — the file is just the last holder's note.
            "held": bool(holder) and _pid_alive(holder),
        }
    return result


def last_build(root: Path) -> Dict[str, Any]:
    """The most recent kernel build run, and the most recent failed one."""
    from devcouncil.devmap_engine import read_runs

    try:
        runs = read_runs(root, limit=50)
    except Exception as exc:  # the trace log is best-effort input
        return {"error": str(exc)}
    builds = [run for run in runs if run.get("stage") == "build"]
    failed = [run for run in builds if not run.get("ok", True)]
    return {
        "last": builds[-1] if builds else None,
        "last_failed": failed[-1] if failed else None,
        "recorded": len(builds),
    }


def _artifact(path: Path, *, engine_key: str) -> Dict[str, Any]:
    info: Dict[str, Any] = {
        "path": str(path),
        "exists": path.is_file(),
        "bytes": None,
        "written_at": None,
        "map_engine": None,
        "generated_head": None,
    }
    if not path.is_file():
        return info
    try:
        stat = path.stat()
        info["bytes"] = stat.st_size
        info["written_at"] = _iso(stat.st_mtime)
    except OSError:
        pass
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        info["error"] = f"{type(exc).__name__}: {exc}"
        return info
    if isinstance(payload, dict):
        if engine_key == "meta":
            meta = payload.get("meta") if isinstance(payload.get("meta"), dict) else {}
            info["map_engine"] = meta.get("map_engine")
        else:
            info["map_engine"] = payload.get("map_engine")
        info["generated_head"] = payload.get("generated_head")
    return info


def artifacts_info(root: Path) -> Dict[str, Any]:
    return {
        "repo_map": _artifact(root / DEFAULT_MAP_RELPATH, engine_key="top"),
        "code_graph": _artifact(root / DEFAULT_GRAPH_RELPATH, engine_key="meta"),
    }


def map_freshness(root: Path) -> Dict[str, Any]:
    """Is `repo_map.json` the map of the tree as it stands? Same rule as `--if-stale`."""
    from devcouncil.indexing.repo_mapper import RepoMapper

    map_path = root / DEFAULT_MAP_RELPATH
    result: Dict[str, Any] = {"fresh": None, "reason": "", "map_head": "", "current_head": ""}
    if not map_path.is_file():
        result["reason"] = "no repo_map.json; run `dev map`"
        return result
    try:
        payload = json.loads(map_path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        result["reason"] = f"repo_map.json unreadable: {exc}"
        return result
    if not isinstance(payload, dict):
        result["reason"] = "repo_map.json is not an object"
        return result
    mapper = RepoMapper(root)
    result["map_head"] = str(payload.get("generated_head") or "")
    try:
        result["current_head"] = mapper._git_head()
    except Exception:  # noqa: BLE001 - freshness must not raise
        result["current_head"] = ""
    try:
        stale = bool(mapper.map_is_stale(payload))
    except Exception as exc:  # noqa: BLE001
        result["reason"] = f"freshness check failed: {exc}"
        return result
    result["fresh"] = not stale
    if stale:
        if result["map_head"] and result["current_head"] and result["map_head"] != result["current_head"]:
            result["reason"] = (
                f"map was built from {result['map_head'][:12]} but HEAD is "
                f"{result['current_head'][:12]}"
            )
        elif not payload.get("generated_head") and not payload.get("indexed_hash"):
            result["reason"] = "map carries no freshness stamps"
        else:
            result["reason"] = "tracked files changed since the map was written"
    return result


def python_query_cache_info(root: Path) -> Dict[str, Any]:
    """The Python `index.sqlite`, now a read cache for the graph JSON.

    It is not the engine and never written by `dev map`; `load_code_graph`
    imports the kernel's `code_graph.json` into it on first use after a build so
    the Python-only query commands answer from the current generation.
    """
    path = root / ".devcouncil" / "codeintel" / "index.sqlite"
    info: Dict[str, Any] = {"path": str(path), "exists": path.is_file(), "generation": None}
    if not path.is_file():
        return info
    try:
        from devcouncil.codeintel import get_codeintel_service

        state = get_codeintel_service(root).status()
        info["generation"] = state.get("generation")
        info["state"] = state.get("state")
    except Exception as exc:  # noqa: BLE001 - informational only
        info["error"] = f"{type(exc).__name__}: {exc}"
    return info


def collect_map_status(root: Path) -> Dict[str, Any]:
    """Everything `dev map status` prints, as one JSON-able dict.

    Top-level ``state`` / ``generation`` / ``node_count`` / ``edge_count`` /
    ``index_freshness`` / ``sync`` / ``source`` keep the names earlier
    consumers read; the engine-specific sections are additive.
    """
    root = Path(root).expanduser().resolve()
    engine = engine_info(root)
    store = store_info(root)
    kernel = kernel_status(root)
    daemon = daemon_info(root)
    artifacts = artifacts_info(root)
    freshness = map_freshness(root)
    cache = python_query_cache_info(root)
    build = build_activity(root)
    runs = last_build(root)

    generation = kernel.get("generation_id") if "error" not in kernel else None
    if not store["exists"]:
        state = "uninitialized"
    elif "error" in kernel:
        state = "unreadable"
    elif not generation:
        state = "empty"
    else:
        state = "committed"
    pending_reason = kernel.get("degraded_reason") if "error" not in kernel else kernel["error"]
    return {
        "state": state,
        "source": "devmap-rust",
        "generation": generation,
        "node_count": kernel.get("node_count", 0) if "error" not in kernel else 0,
        "edge_count": kernel.get("edge_count", 0) if "error" not in kernel else 0,
        "index_freshness": freshness,
        "engine": engine,
        "store": store,
        "kernel": kernel,
        "daemon": daemon,
        "artifacts": artifacts,
        "python_query_cache": cache,
        "build": build,
        "last_build": runs,
        "sync": {
            "state": "daemon" if daemon["running"] else "disabled",
            "backend": "devmap serve" if daemon["running"] else None,
            "pending": kernel.get("pending_count", 0) if "error" not in kernel else 0,
            "quarantined": kernel.get("quarantined_count", 0) if "error" not in kernel else 0,
            "degraded_reason": pending_reason,
        },
    }


def _mb(value: Optional[int]) -> str:
    return "n/a" if value is None else f"{value / (1024 * 1024):.1f} MB"


def render_status(result: Dict[str, Any]) -> List[str]:
    """Human lines for `dev map status`."""
    lines: List[str] = []
    engine = result["engine"]
    if engine.get("binary"):
        version = engine.get("version") or "version unknown"
        lines.append(f"engine: {engine['binary']} ({version}, built {engine.get('built_at') or '?'})")
    else:
        lines.append(f"engine: UNAVAILABLE — {engine.get('error')}")
    store = result["store"]
    if store["exists"]:
        ratio = store.get("freelist_ratio")
        lines.append(
            f"store: {store['path']} schema {store.get('schema_version')} "
            f"{_mb(store.get('bytes'))} (+{_mb(store.get('wal_bytes'))} WAL, "
            f"{(ratio or 0) * 100:.0f}% free)"
        )
    else:
        lines.append(f"store: none at {store['path']} (run `dev map`)")
    lines.append(f"state: {result['state']}")
    lines.append(f"generation: {result.get('generation') or '(none)'}")
    lines.append(f"nodes/edges: {result.get('node_count', 0)}/{result.get('edge_count', 0)}")
    freshness = result["index_freshness"]
    if freshness.get("fresh") is True:
        lines.append(f"map: fresh (HEAD {str(freshness.get('map_head') or '')[:12]})")
    elif freshness.get("fresh") is False:
        lines.append(f"map: STALE — {freshness.get('reason')}")
    else:
        lines.append(f"map: unknown — {freshness.get('reason')}")
    kernel = result["kernel"]
    if "error" in kernel:
        lines.append(f"kernel: unreadable — {kernel['error']}")
    else:
        verdict = "fresh" if kernel.get("is_fresh") else "NOT FRESH"
        lines.append(
            f"kernel: {verdict}; pending {kernel.get('pending_count', 0)}, "
            f"quarantined {kernel.get('quarantined_count', 0)}"
        )
        if kernel.get("degraded_reason"):
            lines.append(f"degraded: {kernel['degraded_reason']}")
            lines.append("hint: `dev map repair --pending` drops entries the kernel cannot index")
    daemon = result["daemon"]
    lines.append(
        f"daemon: {'running' if daemon['running'] else 'not running'} ({daemon['socket']})"
    )
    if not daemon["running"]:
        lines.append("hint: `dev map --watch` keeps the map fresh on edits")
    for name, artifact in result["artifacts"].items():
        if artifact["exists"]:
            engine_tag = artifact.get("map_engine") or "unknown writer"
            lines.append(
                f"{name}: {_mb(artifact.get('bytes'))} written {artifact.get('written_at')} "
                f"by {engine_tag}"
            )
        else:
            lines.append(f"{name}: missing ({artifact['path']})")
    cache = result["python_query_cache"]
    if cache["exists"]:
        lines.append(
            f"python query cache: generation {cache.get('generation') or '(none)'} "
            "(imported from code_graph.json on first read; not the engine)"
        )
    lines.extend(render_build_lines(result))
    return lines


def render_build_lines(result: Dict[str, Any]) -> List[str]:
    """The `build:` and `last build:` lines shared by status and doctor output."""
    lines: List[str] = []
    build = result.get("build") or {}
    if build.get("in_progress"):
        state = "STUCK" if build.get("stuck") else "running"
        lines.append(
            f"build: {state} (pid {build.get('pid')}, run {build.get('run_id')}, "
            f"{build.get('elapsed_s')} s elapsed, last progress {build.get('since_progress_s')} s ago: "
            f"{build.get('stage') or '(no progress line yet)'})"
        )
        if build.get("stuck"):
            lines.append("hint: `dev map abort` stops it; the store rolls back to the prior generation")
    elif build.get("stale_marker"):
        lines.append(
            f"build: marker left by a build that is no longer running (pid {build.get('pid')}, "
            f"run {build.get('run_id')}) — `dev map doctor --fix` clears it"
        )
    runs = result.get("last_build") or {}
    last = runs.get("last")
    if last:
        when = _iso(float(last.get("started_at") or 0)) if last.get("started_at") else last.get("timestamp")
        if last.get("ok"):
            lines.append(
                f"last build: ok in {last.get('duration_s')} s at {when} (run {last.get('run_id')})"
            )
        else:
            lines.append(
                f"last build: FAILED {last.get('code')} at {when} (run {last.get('run_id')}) — "
                "`dev map runs --last 1 --json` has the record"
            )
    return lines


def run_doctor(root: Path) -> Dict[str, Any]:
    """Checks with verdicts. ``ok`` is False when any *critical* check fails.

    Critical: a usable kernel, a store the kernel can open (schema no newer
    than the binary), artifacts written by the kernel and not by a foreign
    writer. Warnings do not fail: reclaim pressure, a large WAL, a stale map,
    quarantined paths — each names its remedy.
    """
    root = Path(root).expanduser().resolve()
    status = collect_map_status(root)
    checks: List[Dict[str, Any]] = []

    def check(
        name: str,
        ok: Optional[bool],
        detail: str,
        *,
        critical: bool = True,
        fix: str = "",
        code: str = "",
        fix_command: str = "",
    ) -> None:
        # `code` is what an agent branches on; `fix_command` is what it runs.
        # `fix` stays the sentence a person reads. A failing check without a
        # code would be a check nobody can act on programmatically, so it is
        # derived from the name when the caller gave none.
        if ok is False and not code:
            code = name
        checks.append(
            {
                "name": name,
                "ok": ok,
                "detail": detail,
                "critical": critical,
                "fix": fix,
                "code": code,
                "fix_command": fix_command or (fix.split(" (")[0].split(";")[0].strip() if fix else ""),
            }
        )

    engine = status["engine"]
    if engine.get("binary"):
        check("engine", True, f"{engine['binary']} ({engine.get('version') or 'version unknown'})")
    else:
        check(
            "engine",
            False,
            engine.get("error") or "no kernel binary",
            fix="cargo build --release -p devmap-cli (in rust-port/) or set DEVMAP_BINARY",
            code="engine_missing",
            fix_command="cd rust-port && cargo build --release -p devmap-cli",
        )

    store = status["store"]
    if not store["exists"]:
        check("store", None, "no store yet", critical=False, fix="dev map")
    elif store.get("error"):
        check(
            "store",
            False,
            store["error"],
            fix="dev map doctor --fix (quarantines the store and rebuilds)",
            code="store_unreadable",
            fix_command="dev map doctor --fix",
        )
    else:
        binary_schema = engine.get("schema_version")
        store_schema = store.get("schema_version")
        if binary_schema is not None and store_schema is not None:
            if store_schema > binary_schema:
                check(
                    "schema",
                    False,
                    f"store is at schema {store_schema}, the kernel supports {binary_schema}",
                    fix="rebuild the kernel: cargo build --release -p devmap-cli (or DEVMAP_BINARY)",
                    code="schema_newer_than_kernel",
                    fix_command="cd rust-port && cargo build --release -p devmap-cli",
                )
            elif store_schema < binary_schema:
                check(
                    "schema",
                    True,
                    f"store at schema {store_schema}, kernel at {binary_schema}; `dev map` migrates it",
                    critical=False,
                    fix="dev map",
                    code="schema_older_than_kernel",
                    fix_command="dev map",
                )
            else:
                check("schema", True, f"store and kernel both at schema {store_schema}")
        else:
            check(
                "schema",
                None,
                "kernel does not report its schema version; compatibility unverified",
                critical=False,
            )
        ratio = store.get("freelist_ratio") or 0.0
        if ratio > FREELIST_WARN_RATIO:
            check(
                "reclaim",
                False,
                f"{ratio * 100:.0f}% of the store is free pages ({_mb(store.get('bytes'))})",
                critical=False,
                fix="dev map (a build reclaims); stop a long-lived daemon if it keeps the WAL busy",
                code="reclaim_pressure",
                fix_command="dev map",
            )
        else:
            check("reclaim", True, f"{ratio * 100:.0f}% free pages", critical=False)
        wal = store.get("wal_bytes") or 0
        if wal > WAL_WARN_BYTES:
            check(
                "wal",
                False,
                f"WAL is {_mb(wal)}; checkpoints are not landing",
                critical=False,
                fix="stop the daemon (`dev map status` shows its socket) and run `dev map`",
                code="wal_large",
                fix_command="dev map",
            )
        else:
            check("wal", True, f"WAL {_mb(wal)}", critical=False)

    kernel = status["kernel"]
    if store["exists"]:
        if "error" in kernel:
            check(
                "kernel",
                False,
                kernel["error"],
                fix="dev map doctor --fix",
                code="kernel_unreadable",
                fix_command="dev map doctor --fix",
            )
        else:
            if kernel.get("is_fresh") and not kernel.get("degraded_reason"):
                check("kernel", True, f"generation {kernel.get('generation_id')}, no pending paths")
            else:
                check(
                    "kernel",
                    False,
                    kernel.get("degraded_reason")
                    or f"{kernel.get('pending_count')} pending path(s)",
                    critical=False,
                    fix="dev map repair --pending, then dev map",
                    code="pending_paths",
                    fix_command="dev map doctor --fix",
                )

    for name, artifact in status["artifacts"].items():
        if not artifact["exists"]:
            check(
                name,
                None if not store["exists"] else False,
                "missing",
                critical=False,
                fix="dev map",
                code="artifact_missing",
                fix_command="dev map",
            )
        elif artifact.get("error"):
            check(name, False, artifact["error"], fix="dev map", code="artifact_unreadable", fix_command="dev map")
        elif artifact.get("map_engine") != "devmap-rust":
            check(
                name,
                False,
                f"written by {artifact.get('map_engine') or 'an unknown writer'}, not the kernel",
                fix="dev map (the kernel overwrites foreign artifacts)",
                code="foreign_writer",
                fix_command="dev map",
            )
        else:
            check(name, True, f"{_mb(artifact.get('bytes'))} by devmap-rust", critical=False)

    freshness = status["index_freshness"]
    if freshness.get("fresh") is False:
        check(
            "freshness",
            False,
            freshness.get("reason") or "stale",
            critical=False,
            fix="dev map",
            code="stale_map",
            fix_command="dev map",
        )
    elif freshness.get("fresh") is True:
        check("freshness", True, "map matches the tree", critical=False)
    else:
        check("freshness", None, freshness.get("reason") or "unknown", critical=False)

    daemon = status["daemon"]
    check(
        "daemon",
        True,
        "running" if daemon["running"] else "not running",
        critical=False,
        fix="" if daemon["running"] else "dev map --watch",
    )

    build = status["build"]
    if build.get("in_progress"):
        if build.get("stuck"):
            check(
                "build",
                False,
                f"pid {build['pid']} has reported no progress for {build['since_progress_s']} s "
                f"(last: {build.get('stage') or 'none'})",
                critical=False,
                fix="dev map abort (the store rolls back to the prior generation), then dev map",
                code="build_stuck",
                fix_command="dev map abort",
            )
        else:
            check(
                "build",
                True,
                f"running: pid {build['pid']}, {build['elapsed_s']} s, {build.get('stage') or 'starting'}",
                critical=False,
                code="build_in_progress",
            )
    elif build.get("stale_marker"):
        check(
            "build",
            False,
            f"a build (pid {build['pid']}, run {build['run_id']}) ended without cleaning up; "
            "it crashed or was killed",
            critical=False,
            fix="dev map doctor --fix (clears the marker); dev map runs --last 1 --json for the record",
            code="stale_build_marker",
            fix_command="dev map doctor --fix",
        )
    else:
        check("build", True, "none running", critical=False)

    runs = status["last_build"]
    last = runs.get("last") if isinstance(runs, dict) else None
    if last and not last.get("ok", True):
        check(
            "last_build",
            False,
            f"{last.get('code')} (run {last.get('run_id')}): "
            + (last.get("stderr_tail") or ["no output"])[-1],
            critical=False,
            fix=f"dev map runs --last 1 --json; then the run's own fix for {last.get('code')}",
            code=f"last_build_failed:{last.get('code')}",
            fix_command="dev map runs --last 1 --json",
        )
    elif last:
        check("last_build", True, f"ok in {last.get('duration_s')} s (run {last.get('run_id')})", critical=False)

    ok = all(item["ok"] is not False for item in checks if item["critical"])
    return {"ok": ok, "checks": checks, "status": status}


#: Doctor codes a build resolves, so `--fix` runs one build for all of them.
_BUILD_RESOLVES = {
    "pending_paths",
    "foreign_writer",
    "stale_map",
    "artifact_missing",
    "artifact_unreadable",
    "reclaim_pressure",
    "wal_large",
    "schema_older_than_kernel",
    "store_unreadable",
    "kernel_unreadable",
}
#: Codes no command inside the repository can fix.
_NEEDS_A_PERSON = {"engine_missing", "schema_newer_than_kernel"}


def _quarantine_store(root: Path) -> Dict[str, Any]:
    """Move an unreadable store aside, keeping it as evidence, so a build starts clean."""
    from devcouncil.devmap_engine import DEFAULT_DB_RELPATH

    store = root / DEFAULT_DB_RELPATH
    stamp = time.strftime("%Y%m%dT%H%M%S")
    moved: List[str] = []
    for suffix in ("", "-wal", "-shm"):
        candidate = Path(str(store) + suffix)
        if candidate.exists():
            target = Path(f"{store}.corrupt-{stamp}{suffix}")
            shutil.move(str(candidate), str(target))
            moved.append(str(target))
    return {"action": "quarantine_store", "ok": True, "moved": moved}


def apply_fixes(root: Path) -> Dict[str, Any]:
    """Apply every fix the doctor can apply from inside the repository.

    Refuses to touch anything while a build is running. Each action is
    reported with its outcome; codes that need a person (a missing or outdated
    kernel binary) are listed, not attempted. Ends with a fresh doctor pass so
    the caller sees the state it left behind, not the state it found.
    """
    from devcouncil.devmap_engine import DevMapEngineError, LIVE_BUILD_RELPATH, repair_pending
    from devcouncil.indexing.map_artifacts import refresh_map_artifacts

    root = Path(root).expanduser().resolve()
    before = run_doctor(root)
    failing = [item for item in before["checks"] if item["ok"] is False]
    codes = {item["code"] for item in failing}
    actions: List[Dict[str, Any]] = []
    not_applied: List[Dict[str, Any]] = []

    if before["status"]["build"].get("in_progress"):
        return {
            "ok": before["ok"],
            "applied": [],
            "not_applied": [{"code": "build_in_progress", "reason": "a build is running; wait or `dev map abort`"}],
            "before": before,
            "after": before,
        }

    for item in failing:
        if item["code"] in _NEEDS_A_PERSON or item["code"].startswith("last_build_failed"):
            not_applied.append({"code": item["code"], "fix": item["fix"], "fix_command": item["fix_command"]})
    if "stale_build_marker" in codes:
        marker = root / LIVE_BUILD_RELPATH
        try:
            marker.unlink()
            actions.append({"action": "clear_build_marker", "ok": True, "path": str(marker)})
        except OSError as exc:
            actions.append({"action": "clear_build_marker", "ok": False, "error": str(exc)})
    if "store_unreadable" in codes or "kernel_unreadable" in codes:
        try:
            actions.append(_quarantine_store(root))
        except OSError as exc:
            actions.append({"action": "quarantine_store", "ok": False, "error": str(exc)})
    if "pending_paths" in codes and "store_unreadable" not in codes:
        try:
            output = repair_pending(root)
            actions.append({"action": "repair_pending", "ok": True, "output": output[-400:]})
        except DevMapEngineError as exc:
            actions.append({"action": "repair_pending", "ok": False, **exc.to_dict()})
    if codes & _BUILD_RESOLVES and not codes & _NEEDS_A_PERSON:
        try:
            refresh = refresh_map_artifacts(root, root / ".devcouncil" / "repo_map.json", quiet=True)
            actions.append({"action": "build", "ok": True, "generation": refresh.generation})
        except DevMapEngineError as exc:
            actions.append({"action": "build", "ok": False, **exc.to_dict()})

    after = run_doctor(root)
    return {
        "ok": after["ok"],
        "applied": actions,
        "not_applied": not_applied,
        "before": before,
        "after": after,
    }


def abort_build(root: Path, *, grace_seconds: float = 5.0) -> Dict[str, Any]:
    """Stop the kernel build the live marker names.

    SIGTERM first; SIGKILL if it is still alive after *grace_seconds*. Safe
    because a generation is one transaction: an interrupted build leaves the
    store on the prior generation and the OS releases the writer lock. Refuses
    a pid whose command line is not a devmap process — a stale marker must
    never become a signal to whatever process inherited the number.
    """
    from devcouncil.devmap_engine import LIVE_BUILD_RELPATH

    root = Path(root).expanduser().resolve()
    activity = build_activity(root)
    marker = root / LIVE_BUILD_RELPATH
    if activity.get("stale_marker"):
        try:
            marker.unlink()
        except OSError:
            pass
        return {"ok": True, "aborted": False, "code": "stale_marker_cleared", "pid": activity.get("pid")}
    if not activity.get("in_progress"):
        return {"ok": True, "aborted": False, "code": "no_build_in_progress"}
    pid = int(activity["pid"])
    command = _process_command(pid)
    if "devmap" not in command:
        return {"ok": False, "aborted": False, "code": "not_a_devmap_process", "pid": pid, "command": command}
    sent = "SIGTERM"
    try:
        os.kill(pid, signal.SIGTERM)
        deadline = time.monotonic() + grace_seconds
        while _pid_alive(pid) and time.monotonic() < deadline:
            time.sleep(0.1)
        if _pid_alive(pid):
            os.kill(pid, signal.SIGKILL)
            sent = "SIGKILL"
            time.sleep(0.2)
    except ProcessLookupError:
        pass
    except PermissionError as exc:
        return {"ok": False, "aborted": False, "code": "permission_denied", "pid": pid, "error": str(exc)}
    try:
        marker.unlink()
    except OSError:
        pass
    return {
        "ok": True,
        "aborted": True,
        "pid": pid,
        "signal": sent,
        "run_id": activity.get("run_id"),
        "still_alive": _pid_alive(pid),
    }


def render_doctor(result: Dict[str, Any]) -> List[str]:
    lines: List[str] = []
    for item in result["checks"]:
        if item["ok"] is True:
            mark = "ok  "
        elif item["ok"] is False:
            mark = "FAIL" if item["critical"] else "warn"
        else:
            mark = "?   "
        line = f"{mark} {item['name']}: {item['detail']}"
        if item["ok"] is not True and item.get("fix"):
            line += f" — fix: {item['fix']}"
        lines.append(line)
    lines.append("verdict: " + ("healthy" if result["ok"] else "NOT healthy"))
    return lines


def render_fixes(result: Dict[str, Any]) -> List[str]:
    lines: List[str] = []
    for action in result.get("applied") or []:
        mark = "ok  " if action.get("ok") else "FAIL"
        detail = action.get("error") or action.get("output") or (
            f"generation {action.get('generation')}" if action.get("generation") else ""
        )
        lines.append(f"{mark} {action.get('action')}: {detail}".rstrip(": "))
    for item in result.get("not_applied") or []:
        lines.append(f"skip {item.get('code')}: {item.get('fix') or item.get('reason')}")
    if not result.get("applied") and not result.get("not_applied"):
        lines.append("nothing to fix")
    lines.append("--- after ---")
    lines.extend(render_doctor(result["after"]))
    return lines
