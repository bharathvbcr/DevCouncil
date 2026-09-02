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
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import List, Optional

DEFAULT_DB_RELPATH = ".devcouncil/codeintel/devmap.sqlite"
DEFAULT_MAP_RELPATH = ".devcouncil/repo_map.json"
DEFAULT_GRAPH_RELPATH = ".devcouncil/graph/code_graph.json"


class DevMapEngineError(RuntimeError):
    """The Rust kernel could not produce a map. Never downgraded to a warning."""


def find_engine_binary() -> str:
    """Locate a devmap binary that can actually do what this module asks of it.

    Two failures are guarded here, both observed on this machine.

    **Location.** `DevMapClient._find_devmap_binary` searches
    `<root_dir>/rust-port/target`, where `root_dir` is the repository being
    mapped. That is right for DevCouncil and wrong for every other repository,
    which has no `rust-port/` and silently falls through to `PATH`. The kernel
    ships inside this package, so it is located relative to *this file*.

    **Capability, not version.** `~/.cargo/bin/devmap` reports `devmap 0.1.0`,
    exactly what the freshly built binary reports, and does not support
    `--graph-output`. A version string is not evidence of a capability when both
    builds carry the same one, so the probe asks the binary what it supports and
    refuses anything that cannot write the graph companion — rather than
    discovering it mid-build and leaving a map with no `code_graph.json`.
    """
    import shutil

    package_root = Path(__file__).resolve().parent.parent.parent
    candidates = [
        package_root / "rust-port" / "target" / "release" / "devmap",
        package_root / "rust-port" / "target" / "debug" / "devmap",
    ]
    found = shutil.which("devmap")
    if found:
        candidates.append(Path(found))

    rejected: List[str] = []
    for candidate in candidates:
        if not (candidate.is_file() and os.access(candidate, os.X_OK)):
            continue
        # Through the memoised probe, so the later stamp-capability check reuses
        # this process launch instead of spending its own.
        help_text = _manifest_help(str(candidate))
        if not help_text:
            rejected.append(f"{candidate} (did not respond to --help)")
            continue
        if "--graph-output" in help_text:
            return str(candidate)
        rejected.append(f"{candidate} (too old: no --graph-output)")

    detail = "; ".join(rejected) if rejected else "none found"
    raise DevMapEngineError(
        "no devmap binary supports this map engine — "
        f"checked: {detail}. Build it with "
        "`cargo build --release -p devmap-cli` in rust-port/."
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


def _run(argv: List[str], *, cwd: Path, timeout: float) -> subprocess.CompletedProcess:
    try:
        completed = subprocess.run(
            argv, cwd=cwd, capture_output=True, text=True, timeout=timeout
        )
    except FileNotFoundError as exc:
        raise DevMapEngineError(f"devmap binary not found: {argv[0]}") from exc
    except subprocess.TimeoutExpired as exc:
        raise DevMapEngineError(
            f"devmap timed out after {timeout:.0f}s: {' '.join(argv[1:])}"
        ) from exc
    if completed.returncode != 0:
        tail = (completed.stderr or completed.stdout or "").strip().splitlines()
        raise DevMapEngineError(
            f"devmap exited {completed.returncode}: {' '.join(argv[1:])}\n"
            + "\n".join(tail[-8:])
        )
    return completed


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
    # Imported from the Python indexer deliberately: one owner for the digest,
    # so writer and checker cannot drift. Relocating these two pure helpers is
    # part of removing `devcouncil.indexing`, not of this change.
    from devcouncil.indexing.graph.build import _files_fingerprint, content_fingerprint
    from devcouncil.indexing.repo_mapper import RepoMapper

    mapper = RepoMapper(project_root=root)
    try:
        files = mapper.get_git_files()
    except Exception as exc:  # noqa: BLE001 - any failure here must fail the stage
        raise DevMapEngineError(f"cannot enumerate git files to fingerprint: {exc}") from exc

    return {
        "generated_head": mapper._git_head(),
        "indexed_hash": _files_fingerprint(files),
        "content_fingerprint": content_fingerprint(root, files),
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
        _write_json_atomically(artifact, payload)


def _write_json_atomically(path: Path, payload: object) -> None:
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


def build_map(
    root: Path,
    *,
    output: Optional[Path] = None,
    graph_output: Optional[Path] = None,
    timeout: float = 900.0,
) -> Path:
    """Build the store and write both artifacts. Returns the map path."""
    root = root.expanduser().resolve()
    if not root.is_dir():
        raise DevMapEngineError(f"project root does not exist: {root}")

    binary = find_engine_binary()
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
    built = _run([*base, "build", str(root)], cwd=root, timeout=timeout)
    # Discovery refusals reach stderr on a *successful* build, and capturing the
    # stream would swallow them. A file dropped for being oversized or unreadable
    # is absent from the graph, so a caller who never sees this line cannot tell
    # "not in this repository" from "refused by the indexer".
    for line in (built.stderr or "").splitlines():
        if "discovery refused" in line or line.startswith("    "):
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
