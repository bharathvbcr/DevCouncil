"""devmap_engine.py — run the Rust devmap kernel as the repository map engine.

`dev map` used to call the Python indexer in `devcouncil.indexing`. The Rust
kernel (`rust-port/`) now owns extraction, resolution, liveness and the graph
store, and this module is the seam that runs it: locate the binary, build the
store, write `repo_map.json` and `code_graph.json`, and stamp the freshness
fields the Rust side cannot compute.

**Fail closed.** Every failure here raises `DevMapEngineError`. There is no
silent fall back to the Python indexer: two engines answering the same question
differently, with no signal which one answered, is how a stale or partial map
comes to look like a fresh one.
"""

from __future__ import annotations

import json
import logging
import os
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from pathlib import Path
from typing import Any, Dict, Iterator, List, Optional

logger = logging.getLogger(__name__)

DEFAULT_DB_RELPATH = ".devcouncil/codeintel/devmap.sqlite"
DEFAULT_MAP_RELPATH = ".devcouncil/repo_map.json"
DEFAULT_GRAPH_RELPATH = ".devcouncil/graph/code_graph.json"


class DevMapEngineError(RuntimeError):
    """The Rust kernel could not produce a map. Never downgraded to a warning.

    Carries a diagnosis an agent can act on without reading a traceback:

    - ``code`` — a stable identifier (``schema_newer_than_kernel``,
      ``store_locked``, ``store_corrupt``, ``kernel_timeout``, ``binary_missing``,
      ``kernel_failed`` …);
    - ``fix`` — the exact command or step that resolves it;
    - ``run_id`` — the trace record of the kernel run that failed
      (``dev map runs --last 1 --json``);
    - ``stage`` — ``build`` / ``manifest`` / ``repair``;
    - ``evidence`` — the kernel's last lines.

    The message alone used to be the whole contract, and a message is what an
    agent has to parse with a regex and guess at. The code is what it branches
    on; the fix is what it runs.
    """

    def __init__(
        self,
        message: str,
        *,
        code: str = "kernel_failed",
        fix: str = "",
        run_id: Optional[str] = None,
        stage: str = "",
        evidence: Optional[List[str]] = None,
    ) -> None:
        super().__init__(message)
        self.code = code
        self.fix = fix
        self.run_id = run_id
        self.stage = stage
        self.evidence = list(evidence or [])

    def to_dict(self) -> Dict[str, Any]:
        return {
            "ok": False,
            "code": self.code,
            "error": str(self),
            "fix": self.fix,
            "run_id": self.run_id,
            "stage": self.stage,
            "evidence": self.evidence,
        }


#: Environment override for the kernel binary. Explicit beats every search
#: location, but is still probed: an override that cannot do the job is refused
#: by name rather than silently replaced by a search result nobody asked for.
BINARY_ENV_VAR = "DEVMAP_BINARY"


def _binary_candidates(root: Optional[Path]) -> List[Path]:
    """Every place a kernel may live, most specific first, de-duplicated.

    Order matters only for the *override*, which wins outright. Among the rest
    the newest capable build wins (see `find_engine_binary`), so listing is
    about coverage, not priority.
    """
    import shutil

    candidates: List[Path] = []
    override = os.environ.get(BINARY_ENV_VAR, "").strip()
    if override:
        candidates.append(Path(override).expanduser())
    bases: List[Path] = []
    if root is not None:
        bases.append(Path(root).expanduser().resolve())
    bases.append(Path(__file__).resolve().parent.parent.parent)
    for base in bases:
        for profile in ("release", "debug"):
            candidates.append(base / "rust-port" / "target" / profile / "devmap")
    found = shutil.which("devmap")
    if found:
        candidates.append(Path(found))

    unique: List[Path] = []
    seen: set[str] = set()
    for candidate in candidates:
        try:
            key = str(candidate.resolve())
        except OSError:
            key = str(candidate)
        if key in seen:
            continue
        seen.add(key)
        unique.append(candidate)
    return unique


def find_engine_binary(root: Optional[Path] = None) -> str:
    """Locate a devmap binary that can actually do what this module asks of it.

    `root` is the repository being mapped. It is searched first because a
    repository that builds its own kernel (this one) must be able to use it
    regardless of where the Python package was installed from.

    Four failures are guarded here, all observed on this machine.

    **Location, installed package.** A `uv tool install` puts this file under
    site-packages, where `rust-port/` does not exist. Searching only relative
    to the package silently fell through to `PATH` — and `~/.cargo/bin/devmap`
    was a day older than the store. Every `dev map` then ended with
    "unsupported future schema version 12" while a freshly built kernel sat in
    the repository the whole time.

    **Location, other repositories.** `DevMapClient` used to search
    `<root>/rust-port/target` and nothing else, which is right for DevCouncil
    and wrong everywhere else. Both rules now live here: the repository, then
    the package, then `PATH`, and an explicit `DEVMAP_BINARY` beats all three.

    **Capability, not version.** `~/.cargo/bin/devmap` reports `devmap 0.1.0`,
    exactly what the freshly built binary reports, and does not support
    `--graph-output`. A version string is not evidence of a capability when both
    builds carry the same one, so the probe asks the binary what it supports and
    refuses anything that cannot write the graph companion — rather than
    discovering it mid-build and leaving a map with no `code_graph.json`.

    **Age, among the capable.** A release build made before a schema bump and a
    debug build made after it both pass the capability probe — the flags did
    not change, the schema did. Preferring `release` by position picked the one
    that refuses the store. Among candidates that pass the probe the newest
    build wins, because a schema bump is exactly the kind of change that moves
    the build time and not the help text.
    """
    override = os.environ.get(BINARY_ENV_VAR, "").strip()
    capable: List[tuple[int, str]] = []
    rejected: List[str] = []
    for candidate in _binary_candidates(root):
        if not (candidate.is_file() and os.access(candidate, os.X_OK)):
            if override and str(candidate) == str(Path(override).expanduser()):
                rejected.append(f"{candidate} (from {BINARY_ENV_VAR}: not an executable file)")
            continue
        # Through the memoised probe, so the later stamp-capability check reuses
        # this process launch instead of spending its own.
        help_text = _manifest_help(str(candidate))
        is_override = bool(override) and str(candidate) == str(Path(override).expanduser())
        if not help_text:
            rejected.append(f"{candidate} (did not respond to --help)")
        elif "--graph-output" not in help_text:
            rejected.append(f"{candidate} (too old: no --graph-output)")
        else:
            if is_override:
                return str(candidate)
            try:
                mtime = candidate.stat().st_mtime_ns
            except OSError:
                mtime = 0
            capable.append((mtime, str(candidate)))
            continue
        if is_override:
            raise DevMapEngineError(
                f"{BINARY_ENV_VAR} points at a kernel that cannot run this map engine: "
                f"{rejected[-1]}. Unset it, or point it at a build that supports "
                "`manifest --graph-output`."
            )

    if capable:
        capable.sort(key=lambda item: item[0], reverse=True)
        return capable[0][1]

    detail = "; ".join(rejected) if rejected else "none found"
    raise DevMapEngineError(
        "no devmap binary supports this map engine — "
        f"checked: {detail}. Build it with "
        "`cargo build --release -p devmap-cli` in rust-port/, or set "
        f"{BINARY_ENV_VAR} to a built kernel."
    )


def _manifest_help(binary: str) -> str:
    """`devmap manifest --help`, memoised per binary path.

    `find_engine_binary` already runs this probe to reject a kernel too old to
    write the graph companion, and asking a second time to test a second
    capability would spend another process launch (~140 ms measured) answering a
    question the first answer contains. Keyed by path *and* mtime so a rebuilt
    binary is re-probed rather than judged on its predecessor's capabilities —
    every build of this workspace reports the same `devmap 0.1.0`, so the path
    alone is not an identity.
    """
    try:
        stat = Path(binary).stat()
        key = (binary, stat.st_mtime_ns, stat.st_size)
    except OSError:
        key = (binary, 0, 0)
    cached = _MANIFEST_HELP_CACHE.get(key)
    if cached is not None:
        return cached
    try:
        probe = subprocess.run(
            [binary, "manifest", "--help"], capture_output=True, text=True, timeout=30
        )
        text = probe.stdout or ""
    except (OSError, subprocess.SubprocessError):
        # An unprobeable binary is treated as lacking the capability, never as
        # having it: the fallback path still produces a correctly stamped map.
        text = ""
    _MANIFEST_HELP_CACHE[key] = text
    return text


_MANIFEST_HELP_CACHE: dict[tuple[str, int, int], str] = {}


def _manifest_accepts_stamp_flags(binary: str) -> bool:
    """Whether this kernel can be handed the freshness digests directly.

    All three are required. A kernel accepting only some of them would need the
    read-modify-write path for the rest, and running both is strictly worse than
    running one — so the capability is all-or-nothing.
    """
    help_text = _manifest_help(binary)
    return all(
        flag in help_text
        for flag in ("--generated-head", "--indexed-hash", "--content-fingerprint")
    )


_FUTURE_SCHEMA_MARKER = "unsupported future schema version"

#: Written while a kernel build runs, removed when it ends. `dev map status`
#: reads it from another process to say "building: stage X, pid N, 12 s"; a
#: marker whose pid is dead is a crashed or killed build and is reported as such.
LIVE_BUILD_RELPATH = ".devcouncil/codeintel/devmap-build.live.json"
#: Trace event type of one kernel run (build / manifest / repair). They go to
#: the same `.devcouncil/logs/traces.jsonl` every other DevCouncil stage uses,
#: so `devcouncil_tail_trace` and `dev map runs` read the same record.
RUN_EVENT_TYPE = "devmap_run"
RUN_TAIL_LINES = 40

#: (code, markers, fix). The first rule whose marker appears in the kernel's
#: output names the failure. Order matters only where markers overlap.
_FAILURE_RULES: List[tuple] = [
    (
        "schema_newer_than_kernel",
        (_FUTURE_SCHEMA_MARKER,),
        "cargo build --release -p devmap-cli (in rust-port/), or set DEVMAP_BINARY to a newer build",
    ),
    (
        "store_locked",
        ("another devmap writer holds",),
        "dev map status (shows the build in progress); dev map abort if it is stuck",
    ),
    (
        "store_corrupt",
        ("database disk image is malformed", "file is not a database", "malformed"),
        "dev map doctor --fix (quarantines the store and rebuilds)",
    ),
    (
        "store_unwritable",
        ("readonly database", "unable to open database", "disk I/O error", "Permission denied"),
        "check permissions and free space under .devcouncil/codeintel, then dev map",
    ),
    (
        "kernel_flag_unsupported",
        ("unexpected argument", "unrecognized subcommand"),
        "cargo build --release -p devmap-cli (the kernel predates a flag the seam passes)",
    ),
]


def classify_kernel_failure(output: str) -> tuple:
    """``(code, fix)`` for a kernel's non-zero exit, from its output."""
    for code, markers, fix in _FAILURE_RULES:
        if any(marker in output for marker in markers):
            return code, fix
    return "kernel_failed", "dev map runs --last 1 --json (the full record of the failed run)"


def _stage_of(argv: List[str]) -> str:
    """The kernel subcommand in *argv*: the first token that is not a global flag."""
    skip = 0
    for token in argv[1:]:
        if skip:
            skip -= 1
            continue
        if token in ("--db", "--progress"):
            skip = 1
            continue
        if token.startswith("--"):
            continue
        return token
    return ""


def _record_run(root: Path, payload: Dict[str, Any], *, run_id: str, summary: str) -> None:
    """Append one kernel run to the project trace log. Never raises."""
    try:
        from devcouncil.telemetry.traces import TraceLogger

        TraceLogger(root).log_event(RUN_EVENT_TYPE, payload, run_id=run_id, summary=summary)
    except Exception:  # pragma: no cover - tracing is strictly best-effort
        logger.debug("could not record devmap run %s", run_id, exc_info=True)


def read_runs(root: Path, *, limit: int = 20, failed_only: bool = False) -> List[Dict[str, Any]]:
    """The last *limit* kernel runs, oldest first, as plain dicts."""
    from devcouncil.telemetry.traces import read_trace_events

    root = Path(root).expanduser().resolve()
    runs: List[Dict[str, Any]] = []
    for event in read_trace_events(root):
        if event.type != RUN_EVENT_TYPE:
            continue
        if failed_only and event.details.get("ok", True):
            continue
        runs.append({"run_id": event.run_id, "timestamp": event.timestamp, **event.details})
    return runs[-limit:] if limit else runs


#: The kernel indents a `--progress` line with six spaces
#: (`rust-port/crates/devmap-cli/src/main.rs:195,234,253`) and a continuation of the
#: discovery-refusal block with exactly four (`main.rs:1061,1064`). Four-space-prefix
#: alone therefore matches both, which is why the block is tracked by state below
#: rather than by prefix: the build runs with `--progress always`, so a prefix test
#: silently swallows the progress stream along with the refusals.
_REFUSAL_INDENT = "    "
_PROGRESS_INDENT = "     "  # 5+: anything indented deeper than a refusal entry


def _is_refusal_continuation(line: str) -> bool:
    """True for `    <path>: <reason>` / `    … and N more`, not for progress."""
    return line.startswith(_REFUSAL_INDENT) and not line.startswith(_PROGRESS_INDENT)


def iter_refusal_lines(stderr: str) -> Iterator[str]:
    """The discovery-refusal block — its header and continuation lines — and nothing else.

    A refused file is absent from the graph, so a caller who never sees this cannot
    tell "not in this repository" from "refused by the indexer". Everything else on
    the kernel's stderr, the progress stream included, is dropped: callers print this
    to their own stderr, and `dev … --json` is read by machines.
    """
    in_block = False
    for line in stderr.splitlines():
        if "discovery refused" in line:
            in_block = True
            yield line
        elif in_block and _is_refusal_continuation(line):
            yield line
        else:
            in_block = False


def _run_notes(lines: List[str]) -> List[str]:
    """The signal-only view of kernel stderr for a run record.

    `stderr_tail` already keeps the raw tail; `notes` earns its place by keeping what
    a human needs *after* a build that printed hundreds of progress lines. Since
    `notes` is itself tail-capped, letting the progress stream in did not merely add
    noise — it evicted the refusals the record exists to preserve.
    """
    notes: List[str] = []
    in_refusal = False
    for line in lines:
        if "discovery refused" in line:
            in_refusal = True
            notes.append(line)
            continue
        if in_refusal and _is_refusal_continuation(line):
            notes.append(line)
            continue
        in_refusal = False
        if "reclaim:" in line or "Reclaim:" in line or line.strip().startswith("["):
            notes.append(line)
    return notes


def _run(
    argv: List[str], *, cwd: Path, timeout: float, stage: str = ""
) -> subprocess.CompletedProcess:
    """Run one kernel command, record it, and translate a failure into a diagnosis.

    While a ``build`` runs, ``LIVE_BUILD_RELPATH`` carries its pid, run id and
    the kernel's latest progress line so another process (`dev map status`,
    an agent deciding whether to wait) can see it. The kernel's stderr is
    streamed for that, not captured after the fact; stdout is collected whole.
    """
    stage = stage or _stage_of(argv)
    run_id = uuid.uuid4().hex[:12]
    root = Path(cwd)
    live_path = root / LIVE_BUILD_RELPATH if stage == "build" else None
    started_at = time.time()
    started_mono = time.monotonic()
    stderr_lines: List[str] = []
    stdout_chunks: List[str] = []
    last_progress = ""

    def _write_live(pid: int) -> None:
        # Every progress line is written, none is coalesced away. A rate
        # limiter here dropped the line that opened a long stage whenever it
        # arrived within 200 ms of the previous write, and the marker then
        # showed the *previous* stage for the whole of the long one. The kernel
        # prints a line per stage and per extraction slice — tens per build —
        # and each write is one small atomic file.
        if live_path is None:
            return
        try:
            live_path.parent.mkdir(parents=True, exist_ok=True)
            write_json_atomically(
                live_path,
                {
                    "run_id": run_id,
                    "pid": pid,
                    "argv": argv[1:],
                    "binary": argv[0],
                    "started_at": started_at,
                    "updated_at": time.time(),
                    "stage": last_progress,
                },
            )
        except OSError:
            logger.debug("could not write the live build marker", exc_info=True)

    def _drain_stderr(stream) -> None:  # noqa: ANN001
        nonlocal last_progress
        for line in stream:
            line = line.rstrip("\n")
            stderr_lines.append(line)
            if line.strip():
                last_progress = line.strip()
                _write_live(proc.pid)

    def _drain_stdout(stream) -> None:  # noqa: ANN001
        stdout_chunks.append(stream.read())

    def _payload(exit_code: Optional[int], code: str, ok: bool) -> Dict[str, Any]:
        return {
            "stage": stage,
            "binary": argv[0],
            "argv": argv[1:],
            "cwd": str(root),
            "pid": getattr(proc, "pid", None) if "proc" in locals() else None,
            "started_at": started_at,
            "duration_s": round(time.monotonic() - started_mono, 3),
            "exit_code": exit_code,
            "ok": ok,
            "code": code,
            "notes": _run_notes(stderr_lines)[-RUN_TAIL_LINES:],
            "stderr_tail": stderr_lines[-RUN_TAIL_LINES:],
            "stdout_tail": "".join(stdout_chunks).splitlines()[-10:],
            "invoked_by": " ".join(os.path.basename(a) if i == 0 else a for i, a in enumerate(sys.argv[:3])),
        }

    try:
        proc = subprocess.Popen(
            argv, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
        )
    except FileNotFoundError as exc:
        _record_run(
            root,
            _payload(None, "binary_missing", False),
            run_id=run_id,
            summary=f"devmap {stage}: binary missing",
        )
        raise DevMapEngineError(
            f"devmap binary not found: {argv[0]}",
            code="binary_missing",
            fix="cargo build --release -p devmap-cli (in rust-port/), or set DEVMAP_BINARY",
            run_id=run_id,
            stage=stage,
        ) from exc

    _write_live(proc.pid)
    readers = [
        threading.Thread(target=_drain_stderr, args=(proc.stderr,), daemon=True),
        threading.Thread(target=_drain_stdout, args=(proc.stdout,), daemon=True),
    ]
    for reader in readers:
        reader.start()
    timed_out = False
    try:
        try:
            proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            proc.kill()
            proc.wait()
        for reader in readers:
            reader.join(timeout=5.0)
    finally:
        if live_path is not None:
            try:
                live_path.unlink()
            except FileNotFoundError:
                pass
            except OSError:
                logger.debug("could not remove the live build marker", exc_info=True)

    stdout_text = "".join(stdout_chunks)
    stderr_text = "\n".join(stderr_lines)
    if timed_out:
        _record_run(
            root,
            _payload(None, "kernel_timeout", False),
            run_id=run_id,
            summary=f"devmap {stage}: timed out after {timeout:.0f}s",
        )
        raise DevMapEngineError(
            f"devmap timed out after {timeout:.0f}s: {' '.join(argv[1:])}"
            + (f"\nlast progress: {last_progress}" if last_progress else ""),
            code="kernel_timeout",
            fix="dev map runs --last 1 --json (see the last progress line); rerun, or raise the timeout",
            run_id=run_id,
            stage=stage,
            evidence=stderr_lines[-8:],
        )
    if proc.returncode != 0:
        output = (stderr_text or stdout_text).strip()
        code, fix = classify_kernel_failure(output)
        explained = _explain_kernel_failure(argv, output)
        tail = output.splitlines()
        _record_run(
            root,
            _payload(proc.returncode, code, False),
            run_id=run_id,
            summary=f"devmap {stage}: {code} (exit {proc.returncode})",
        )
        raise DevMapEngineError(
            explained
            or (f"devmap exited {proc.returncode}: {' '.join(argv[1:])}\n" + "\n".join(tail[-8:])),
            code=code,
            fix=fix,
            run_id=run_id,
            stage=stage,
            evidence=tail[-8:],
        )
    _record_run(
        root,
        _payload(proc.returncode, "ok", True),
        run_id=run_id,
        summary=f"devmap {stage}: ok in {time.monotonic() - started_mono:.1f}s",
    )
    return subprocess.CompletedProcess(argv, proc.returncode, stdout_text, stderr_text)


def repair_pending(root: Path, *, timeout: float = 300.0) -> str:
    """``devmap repair --pending``: drop queue rows the kernel can never index."""
    root = Path(root).expanduser().resolve()
    binary = find_engine_binary(root)
    completed = _run(
        [binary, "--db", str(root / DEFAULT_DB_RELPATH), "--progress", "never", "repair", "--pending"],
        cwd=root,
        timeout=timeout,
        stage="repair",
    )
    return (completed.stdout or "").strip()


def _explain_kernel_failure(argv: List[str], output: str) -> Optional[str]:
    """Turn a kernel refusal the operator cannot act on into one they can.

    The kernel's "unsupported future schema version N" is correct and useless:
    it names neither the binary that is too old, nor the store that is newer,
    nor what to run. Measured cost of that gap on this machine: every `dev map`
    failing for a day while a fresh build sat in `rust-port/target`.
    """
    if _FUTURE_SCHEMA_MARKER not in output:
        return None
    version = ""
    for token in output.split():
        if token.isdigit():
            version = token
    binary = argv[0]
    built = "unknown build time"
    try:
        import datetime as _dt

        built = _dt.datetime.fromtimestamp(Path(binary).stat().st_mtime).isoformat(
            timespec="seconds"
        )
    except OSError:
        pass
    db = ""
    if "--db" in argv:
        db = argv[argv.index("--db") + 1]
    return (
        f"the devmap binary {binary} (built {built}) is older than the store "
        f"{db or '(unknown path)'}, which is at schema version {version or '?'}. "
        "Rebuild the kernel with `cargo build --release -p devmap-cli` in rust-port/, "
        f"or set {BINARY_ENV_VAR} to a newer build."
    )


def compute_freshness(root: Path) -> dict[str, str]:
    """The three freshness digests, computed once from one snapshot of the tree.

    Split out from the old `stamp_freshness` so the values can be handed to the
    kernel *before* it writes, rather than patched into the artifacts after. See
    `build_map` for why that matters; the reasoning about which values these are
    and why they come from `RepoMapper` is unchanged and reproduced below.

    The kernel cannot write two of the three: they are SHA-1 digests over the
    git file set, and no hashing crate is linked in that workspace. Left empty
    they are not merely absent — `RepoMapper.map_is_stale` skips its check only
    when `generated_head` is *also* empty, and the Rust map does carry a head.
    So `"" != <real digest>` on every call and the map reads permanently stale,
    which makes `--if-stale` never short-circuit and the watcher rebuild
    forever.

    `generated_head` is computed here too, and for the same reason. The kernel
    writes it from the newest *persisted generation* — honest for the store,
    but a different question from the one `map_is_stale` asks, which is whether
    this artifact describes the tree at the current `git rev-parse HEAD`. An
    incremental build that finds no changed file persists no new generation, so
    after a commit that touches nothing indexed the stamp keeps pointing at the
    previous commit and the map reads stale the moment `dev map` finishes.
    Measured on this repository: HEAD `e109d16`, stored `30daf62`, stale on a
    map one second old.

    All three come from `RepoMapper`, the same object `map_is_stale` uses, read
    once from one snapshot of the tree — a field written by one rule and read
    by another is worse than none at all, because it can read fresh when it is
    not. An unavailable HEAD is the empty string rather than a raised error,
    matching the Python writer: a repository with no commits still gets a
    usable map, and staleness then rests on the two fingerprints.
    """
    # The digests are the checker's own methods: `RepoMapper.map_is_stale`
    # compares a map against `_files_fingerprint` / `_content_fingerprint`, so
    # the writer computes them with the same code and the two cannot drift.
    # (They used to be imported from the Python graph builder, which is gone.)
    from devcouncil.indexing.repo_mapper import RepoMapper

    mapper = RepoMapper(project_root=root)
    try:
        files = mapper.get_git_files()
    except Exception as exc:  # noqa: BLE001 - any failure here must fail the stage
        raise DevMapEngineError(f"cannot enumerate git files to fingerprint: {exc}") from exc

    return {
        "generated_head": mapper._git_head(),
        "indexed_hash": mapper._files_fingerprint(files),
        "content_fingerprint": mapper._content_fingerprint(files),
    }


def stamp_freshness(root: Path, *artifacts: Path) -> None:
    """Stamp `generated_head` / `indexed_hash` / `content_fingerprint` into
    every Rust-written artifact.

    **Kept as the fallback path only.** `build_map` now passes these values to
    `devmap manifest`, which writes them itself; this read-modify-write remains
    for a kernel too old to accept the flags. It is retained rather than deleted
    because the alternative on such a binary is an unstamped map, which reads
    permanently stale — the exact failure the docstring below describes.

    The kernel cannot write two of the three: they are SHA-1 digests over the
    git file set, and no hashing crate is linked in that workspace. Left empty
    they are not merely absent — `RepoMapper.map_is_stale` skips its check only
    when `generated_head` is *also* empty, and the Rust map does carry a head.
    So `"" != <real digest>` on every call and the map reads permanently stale,
    which makes `--if-stale` never short-circuit and the watcher rebuild
    forever.

    `generated_head` is stamped here too, and for the same reason. The kernel
    writes it from the newest *persisted generation* — honest for the store,
    but a different question from the one `map_is_stale` asks, which is whether
    this artifact describes the tree at the current `git rev-parse HEAD`. An
    incremental build that finds no changed file persists no new generation, so
    after a commit that touches nothing indexed the stamp keeps pointing at the
    previous commit and the map reads stale the moment `dev map` finishes.
    Measured on this repository: HEAD `e109d16`, stored `30daf62`, stale on a
    map one second old.

    All three come from `RepoMapper`, the same object `map_is_stale` uses, read
    once from one snapshot of the tree — a field written by one rule and read
    by another is worse than none at all, because it can read fresh when it is
    not. An unavailable HEAD is stamped as the empty string rather than raised
    on, matching the Python writer: a repository with no commits still gets a
    usable map, and staleness then rests on the two fingerprints.

    Every artifact gets the *same* values. The kernel writes the map and the
    graph from one freshness identity on purpose; stamping only one of them
    would reintroduce exactly the drift that single invocation prevents.
    """
    freshness = compute_freshness(root)

    for artifact in artifacts:
        try:
            payload = json.loads(artifact.read_text(encoding="utf-8"))
        except (OSError, ValueError) as exc:
            raise DevMapEngineError(
                f"cannot read the artifact devmap just wrote ({artifact.name}): {exc}"
            ) from exc
        payload.update(freshness)
        write_json_atomically(artifact, payload)


def write_json_atomically(path: Path, payload: object) -> None:
    """Replace `path` with `payload`, never leaving a partial file behind.

    A *unique* temp name, not `<name>.tmp`. Two `dev map` runs against one
    repository share a fixed temp name, so one renames it away and the other's
    rename raises FileNotFoundError — measured, 1 of 8 concurrent workers died
    this way. The Rust store survived the same race intact (SC28); this was the
    Python stamp being the only unguarded writer left.
    """
    handle, tmp_name = tempfile.mkstemp(dir=str(path.parent), prefix=path.name + ".", suffix=".tmp")
    try:
        with os.fdopen(handle, "w", encoding="utf-8") as stream:
            json.dump(payload, stream, indent=2)
        os.replace(tmp_name, path)
    except BaseException:
        # Never leave a partial temp behind for the next run to trip over.
        try:
            os.unlink(tmp_name)
        except OSError:
            pass
        raise


# Back-compat alias for importers of the previous private name.
_write_json_atomically = write_json_atomically


def build_map(
    root: Path,
    *,
    output: Optional[Path] = None,
    graph_output: Optional[Path] = None,
    timeout: float = 900.0,
    full: bool = False,
) -> Path:
    """Build the store and write both artifacts. Returns the map path.

    ``full`` forces a cold rebuild through the kernel's ``build --full``. It is
    passed through, never emulated: a kernel that lacks the flag rejects it and
    the error says so, which beats a `--full` that quietly ran incrementally.
    """
    root = root.expanduser().resolve()
    if not root.is_dir():
        raise DevMapEngineError(f"project root does not exist: {root}")

    binary = find_engine_binary(root)
    db_path = root / DEFAULT_DB_RELPATH
    # A relative output path is resolved against `root`, never against the
    # process's cwd. `dev map --project-root /other/repo` passes the *default*
    # `.devcouncil/repo_map.json`, and resolving that against cwd made the
    # engine read — and nearly rewrite — the map belonging to whichever
    # repository the shell happened to be sitting in.
    def _under_root(candidate: Optional[Path], fallback: str) -> Path:
        if candidate is None:
            return root / fallback
        candidate = Path(candidate).expanduser()
        return candidate if candidate.is_absolute() else (root / candidate)

    map_path = _under_root(output, DEFAULT_MAP_RELPATH)
    graph_path = _under_root(graph_output, DEFAULT_GRAPH_RELPATH)
    db_path.parent.mkdir(parents=True, exist_ok=True)
    graph_path.parent.mkdir(parents=True, exist_ok=True)

    base = [binary, "--db", str(db_path), "--progress", "never"]
    # The build streams its progress: that is what the live marker and the run
    # record are made of. The manifest is a fast write with nothing to report.
    build_argv = [binary, "--db", str(db_path), "--progress", "always", "build", str(root)]
    if full:
        build_argv.append("--full")
    built = _run(build_argv, cwd=root, timeout=timeout, stage="build")
    # Discovery refusals reach stderr on a *successful* build, and capturing the
    # stream would swallow them. A file dropped for being oversized or unreadable
    # is absent from the graph, so a caller who never sees this line cannot tell
    # "not in this repository" from "refused by the indexer".
    for line in iter_refusal_lines(built.stderr or ""):
        print(line, file=sys.stderr)
    # Compute the freshness digests *before* the manifest runs so the kernel can
    # write them itself.
    #
    # The alternative — the read-modify-write `stamp_freshness` still does — cost
    # 1.68 s of a 2.72 s `dev map` on this repository, essentially all of it
    # Python parsing and re-encoding a 26 MB `code_graph.json` that the kernel
    # had just encoded, in order to set three scalars. Handing the values to the
    # writer removes the second serialization entirely.
    #
    # A failure to compute them is still fatal, exactly as before: an unstamped
    # map reads permanently stale to `map_is_stale`, so silently continuing
    # would leave the watcher rebuilding forever with nothing to show why.
    freshness = compute_freshness(root)
    stamp_flags = [
        argument
        for key, value in freshness.items()
        # An empty value is not passed at all. The kernel treats a blank flag as
        # absent anyway, and omitting it keeps the artifact's "unavailable"
        # marker for that field — which is the honest state when, say, a
        # repository has no commits and `generated_head` is genuinely unknown.
        if value
        for argument in (f"--{key.replace('_', '-')}", value)
    ]

    # `--force` is required because the artifacts on disk may have been written
    # by the Python engine, which devmap refuses to clobber unprompted. Passing
    # it here is the cutover being explicit, not a guard being bypassed.
    manifest_argv = [
        *base,
        "manifest",
        "--output",
        str(map_path),
        "--graph-output",
        str(graph_path),
        "--force",
    ]
    stamped_by_kernel = _manifest_accepts_stamp_flags(binary)
    _run(
        [*manifest_argv, *stamp_flags] if stamped_by_kernel else manifest_argv,
        cwd=root,
        timeout=timeout,
    )

    for produced in (map_path, graph_path):
        if not produced.is_file():
            raise DevMapEngineError(f"devmap reported success but did not write {produced}")

    # Only a kernel that could not be told the values gets them patched in
    # afterwards. Doing both would re-serialize the graph for no reason.
    if not stamped_by_kernel:
        stamp_freshness(root, map_path, graph_path)
    return map_path
