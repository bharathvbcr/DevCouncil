"""Load, export and PDG-annotate the code knowledge graph (schema v2).

The graph itself is built by the Rust kernel; this module reads it back from
the canonical SQLite store (:func:`load_code_graph`), writes the compatibility
``code_graph.json`` export (:func:`write_code_graph`), and computes the opt-in
Python PDG layer over it. The Python extractor and resolver that used to build
the graph here (``build_code_graph``, ``assemble_graph``, ``extract_all``) were
retired once the kernel became the only writer of both map artifacts.
"""

from __future__ import annotations

import ast
import hashlib
import json
import logging
import os
import tempfile
from pathlib import Path
from typing import Callable, Dict, Iterable, List, Optional, Set

from devcouncil.indexing.graph.schema import CodeGraph
from devcouncil.utils.fsio import atomic_write_text
from devcouncil.utils.json_persist import dump_json, read_json

logger = logging.getLogger(__name__)

GRAPH_REL = Path(".devcouncil") / "graph" / "code_graph.json"


class CompatibilityGraphTooLarge(ValueError):
    """Compatibility JSON exceeded the configured import/export boundary."""


def graph_path(root: Path) -> Path:
    return root / GRAPH_REL


# Bumped whenever the digest algorithm changes: a fingerprint stamped under an
# older scheme must never compare equal to one computed under a newer one, so
# an upgrade reads stale exactly once and then rebuilds.
_CONTENT_SCHEME = "c2"
_HASH_CHUNK = 1 << 20
_CONTENT_CACHE_REL = Path(".devcouncil") / "cache" / "content_hashes.json"


def _content_cache_path(root: Path) -> Path:
    return root / _CONTENT_CACHE_REL


def _stat_key(st: os.stat_result) -> str:
    """Cheap identity for a file's bytes, used only to reuse a cached digest.

    ``ctime_ns`` is what makes this safe. It is the inode-change time, and
    unlike ``mtime_ns`` it cannot be back-dated by ``os.utime``, ``cp -p``,
    ``rsync --times`` or tar extraction, so a rewrite that restores the old
    mtime still moves ctime and can never present the key of the content it
    replaced.
    """
    return f"{st.st_size}:{st.st_mtime_ns}:{st.st_ctime_ns}"


def _file_digest(path: Path) -> str:
    digest = hashlib.blake2b(digest_size=16)
    with open(path, "rb") as handle:
        while True:
            chunk = handle.read(_HASH_CHUNK)
            if not chunk:
                break
            digest.update(chunk)
    return digest.hexdigest()


def _load_content_cache(root: Path) -> Dict[str, List[str]]:
    """Advisory only: an absent, unreadable or foreign-scheme cache is a rehash."""
    try:
        raw = read_json(_content_cache_path(root))
    except (OSError, ValueError):
        return {}
    if not isinstance(raw, dict) or raw.get("scheme") != _CONTENT_SCHEME:
        return {}
    entries = raw.get("entries")
    return entries if isinstance(entries, dict) else {}


def _save_content_cache(root: Path, entries: Dict[str, List[str]]) -> None:
    try:
        path = _content_cache_path(root)
        path.parent.mkdir(parents=True, exist_ok=True)
        atomic_write_text(
            path, dump_json({"scheme": _CONTENT_SCHEME, "entries": entries}) + "\n"
        )
    except OSError:
        # A cache we cannot persist costs time on the next call, never accuracy.
        logger.debug("content-hash cache not written under %s", root, exc_info=True)


def content_fingerprint(root: Path, files: List[str], *, persist_cache: bool = True) -> str:
    """sha1 over sorted ``(path, digest-of-bytes)`` — identical bytes fingerprint
    identically, however many times they were rewritten.

    This hashed ``(path, size, mtime_ns)`` until 2026-09-04, which answered a
    different question than the one every caller asks, and was wrong in both
    directions. A formatter, a branch checkout, or any byte-identical rewrite
    moved mtime and marked a current map stale; an edit that happened to
    preserve size and mtime marked a changed tree fresh. In a repository whose
    files are rewritten by hooks and build workers the first case fired
    constantly, so ``repo map stale`` was on permanently and stopped carrying
    information — the reason it was ignored.

    Hashing a 4.3k-file tree costs ~3s, too slow for a per-prompt hook, so
    digests are memoised in ``.devcouncil/cache/content_hashes.json`` behind a
    ``(size, mtime_ns, ctime_ns)`` key and the unchanged path stays a bare
    stat. The cache is advisory: losing it costs time, never correctness. The
    cache lives under ``.devcouncil``, which ``_is_runtime_or_generated_file``
    excludes from the inventory, so it can never fingerprint itself.

    ``persist_cache=False`` reads the memo but never writes it, for callers that
    must not modify the project. The MCP freshness probe is one: it runs on ~20
    tools annotated ``readOnlyHint: true``, and that annotation is what a host
    uses to decide whether to ask the user before the call — so a tool carrying
    it must not create or rewrite a file in the repository. Because the cache is
    advisory, declining to write it costs those callers a rehash and nothing
    else.
    """
    cache = _load_content_cache(root)
    entries: Dict[str, List[str]] = {}
    lines: List[str] = []
    recomputed = False
    for rel in sorted(files):
        path = root / rel
        try:
            st = path.stat()
        except OSError:
            # Distinct from every real digest, and stable while it stays gone.
            lines.append(f"{rel}\0-")
            continue
        key = _stat_key(st)
        cached = cache.get(rel)
        if isinstance(cached, list) and len(cached) == 2 and cached[0] == key:
            digest = str(cached[1])
        else:
            try:
                digest = _file_digest(path)
            except OSError:
                lines.append(f"{rel}\0-")
                continue
            recomputed = True
        entries[rel] = [key, digest]
        lines.append(f"{rel}\0{digest}")
    if persist_cache and (recomputed or entries.keys() != cache.keys()):
        _save_content_cache(root, entries)
    body = hashlib.sha1("\n".join(lines).encode("utf-8")).hexdigest()
    return f"{_CONTENT_SCHEME}:{body}"


def _slim_graph_export(graph: CodeGraph) -> CodeGraph:
    """Copy for JSON export: drop bulky meta already recoverable elsewhere.

    ``node_communities`` duplicates per-node ``community``; legacy dead strings
    remain uncapped on the in-memory/SQLite graph for repo_map consumers.
    Volatile PageRank floats are stripped from ``god_nodes`` (degree/fan metrics
    remain); incremental bookkeeping keys stay but no longer re-inflate via
    pretty-indent or duplicated community maps.
    """
    meta = dict(graph.meta or {})
    meta.pop("node_communities", None)
    meta.pop("legacy_dead_symbol_candidates", None)
    gods = meta.get("god_nodes")
    if isinstance(gods, list):
        cleaned: list[dict[str, object]] = []
        for row in gods:
            if not isinstance(row, dict):
                continue
            item = dict(row)
            item.pop("pagerank", None)
            cleaned.append(item)
        meta["god_nodes"] = cleaned
    meta["compatibility_export_tier"] = "slim"
    return graph.model_copy(update={"meta": meta})


def _compact_graph_export(graph: CodeGraph) -> CodeGraph:
    """Aggressive slim: strip node/edge extras and cap noisy liveness lists."""
    slim = _slim_graph_export(graph)
    nodes = [
        n.model_copy(update={"extras": {}})
        for n in slim.nodes
    ]
    edges = [
        e.model_copy(update={"extras": {}, "reason": ""})
        for e in slim.edges
    ]
    meta = dict(slim.meta or {})
    meta["compatibility_export_tier"] = "compact"
    meta["unreachable_omitted"] = len(slim.unreachable_files or [])
    meta["unwired_omitted"] = max(0, len(slim.unwired_candidates or []) - 200)
    return slim.model_copy(
        update={
            "nodes": nodes,
            "edges": edges,
            "unreachable_files": [],
            "unwired_candidates": list(slim.unwired_candidates or [])[:200],
            "dead_code": list(slim.dead_code or [])[:500],
            "meta": meta,
        }
    )


def _stub_graph_export(graph: CodeGraph) -> CodeGraph:
    """Pointer-only compatibility JSON when even compact export exceeds the limit."""
    return CodeGraph(
        schema_version=graph.schema_version,
        nodes=[],
        edges=[],
        dead_code=[],
        entry_roots=list(graph.entry_roots or [])[:200],
        unwired_candidates=list(graph.unwired_candidates or [])[:100],
        unreachable_files=[],
        generated_head=graph.generated_head,
        indexed_hash=graph.indexed_hash,
        content_fingerprint=graph.content_fingerprint,
        meta={
            "compatibility_export_tier": "stub",
            "sqlite_canonical": True,
            "node_count": len(graph.nodes),
            "edge_count": len(graph.edges),
            "dead_code_count": len(graph.dead_code or []),
            "unwired_count": len(graph.unwired_candidates or []),
            "unreachable_count": len(graph.unreachable_files or []),
            "liveness_unreachable_unreliable": bool(
                (graph.meta or {}).get("liveness_unreachable_unreliable")
            ),
            "compatibility_export_reason": (
                "exceeded indexing.graph_json_max_bytes; prefer SQLite-backed "
                "`dev map` commands"
            ),
        },
    )


def _graph_json_indent(root: Path) -> int | None:
    """Compact JSON by default; honor ``indexing.compact_graph_json``."""
    try:
        from devcouncil.app.config import load_config

        if bool(load_config(root).indexing.compact_graph_json):
            return None
        return 2
    except Exception:
        return None


def _graph_json_max_bytes(root: Path) -> int:
    try:
        from devcouncil.app.config import load_config

        return int(load_config(root).indexing.graph_json_max_bytes)
    except Exception:
        return 128 * 1024 * 1024


def _write_graph_json_bounded(
    path: Path,
    graph: CodeGraph,
    *,
    indent: int | None,
    max_bytes: int,
) -> None:
    """Stream JSON to a sibling temporary file and atomically publish it."""
    path.parent.mkdir(parents=True, exist_ok=True)
    encoder = json.JSONEncoder(
        indent=indent,
        ensure_ascii=False,
        separators=(",", ":") if indent is None else None,
    )
    payload = graph.model_dump(mode="json")
    fd, raw_temp = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=path.parent)
    temp = Path(raw_temp)
    written = 0
    try:
        os.chmod(temp, (path.stat().st_mode & 0o777) if path.exists() else 0o644)
        with os.fdopen(fd, "wb") as handle:
            for chunk in encoder.iterencode(payload):
                encoded = chunk.encode("utf-8")
                written += len(encoded)
                if written + 1 > max_bytes:
                    raise CompatibilityGraphTooLarge(
                        f"compatibility graph export exceeds {max_bytes} bytes"
                    )
                handle.write(encoded)
            handle.write(b"\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temp, path)
    finally:
        temp.unlink(missing_ok=True)


def _write_compatibility_export_tiers(root: Path, path: Path, graph: CodeGraph) -> str:
    """Try slim → compact → stub until the size cap fits. Returns the tier used.

    Raises ``CompatibilityGraphTooLarge`` only when even the stub cannot be written
    (misconfigured tiny limit). When the stub tier is used, still raises after a
    successful write so callers mark ``compatibility_export=degraded`` while
    leaving a usable pointer JSON on disk.
    """
    from devcouncil.codeintel import get_codeintel_service

    indent = _graph_json_indent(root)
    max_bytes = _graph_json_max_bytes(root)
    # The stub is a bounded pointer JSON (~1 KB floor plus capped entry lists);
    # always leave it on disk even under a tiny configured cap, backstopped at
    # 1 MiB against pathological entry-root lists.
    stub_cap = max(max_bytes, 1024 * 1024)
    tiers: list[tuple[str, CodeGraph, int]] = [
        ("slim", _slim_graph_export(graph), max_bytes),
        ("compact", _compact_graph_export(graph), max_bytes),
        ("stub", _stub_graph_export(graph), stub_cap),
    ]
    last_err: CompatibilityGraphTooLarge | None = None
    for tier_name, export_graph, tier_cap in tiers:
        try:
            _write_graph_json_bounded(
                path, export_graph, indent=indent, max_bytes=tier_cap
            )
        except CompatibilityGraphTooLarge as exc:
            last_err = exc
            logger.warning(
                "compatibility export tier %s exceeded %s bytes; trying next",
                tier_name,
                max_bytes,
            )
            continue
        get_codeintel_service(root).store.record_compatibility_export(path, export_graph)
        if tier_name == "stub":
            raise CompatibilityGraphTooLarge(
                f"compatibility graph export exceeded {max_bytes} bytes; "
                "wrote stub JSON (SQLite remains canonical)"
            )
        return tier_name
    raise last_err or CompatibilityGraphTooLarge(
        f"compatibility graph export exceeds {max_bytes} bytes"
    )


def write_code_graph(
    root: Path,
    graph: CodeGraph,
    *,
    changed_paths: Set[str] | None = None,
    analysis_shards: dict[str, dict[str, object]] | None = None,
    _lease_held: bool = False,
    progress: Callable[[str, int, int], None] | None = None,
) -> Path:
    if not _lease_held:
        from devcouncil.codeintel.build_control import graph_build_session

        with graph_build_session(root):
            return write_code_graph(
                root,
                graph,
                changed_paths=changed_paths,
                analysis_shards=analysis_shards,
                _lease_held=True,
                progress=progress,
            )
    # SQLite is the canonical store. Keep the deterministic JSON artifact as a
    # compatibility/export boundary for existing consumers and older clients.
    from devcouncil.codeintel import get_codeintel_service

    root = root.expanduser().resolve()
    get_codeintel_service(root).persist(
        graph,
        changed_paths=changed_paths,
        analysis_shards=analysis_shards,
        progress=progress,
    )
    path = graph_path(root)
    path.parent.mkdir(parents=True, exist_ok=True)
    if progress is not None:
        progress("export:json", 0, 1)
    _write_compatibility_export_tiers(root, path, graph)
    if progress is not None:
        progress("export:json", 1, 1)
    return path


#: Stamped on a graph whose on-disk form is a size-capped export.
#:
#: The Python store used to hide this: `load_code_graph` preferred SQLite, which
#: held the uncapped graph, so a consumer reading a `compact` or `stub`
#: `code_graph.json` still got complete lists. Reading the artifact directly
#: removes that cover, and a capped export must therefore say it is capped
#: rather than answer in the shape of a complete one — `compact` truncates
#: `unwired_candidates` at 200 and `dead_code` at 500, and `stub` empties the
#: nodes and edges entirely.
GRAPH_INCOMPLETE_META = "graph_export_incomplete_reason"

#: Export tiers whose lists are the whole graph. `None` is the kernel's own
#: artifact, which is not tiered at all.
_COMPLETE_EXPORT_TIERS = (None, "slim")


def read_code_graph(root: Path) -> Optional[CodeGraph]:
    """The kernel's ``code_graph.json``, parsed once and validated.

    This is a read of the kernel's own artifact, not a second engine: the same
    `devmap manifest` run that writes `repo_map.json` writes this file, and the
    Python side has not built a graph since the kernel became the only writer.

    It replaces :func:`load_code_graph` for every consumer that wants the whole
    graph. That function reached the graph through the Python `index.sqlite`
    cache — importing this same JSON into it on first read, then re-materialising
    every node and edge as pydantic models out of SQLite on every call after.

    Measured on this repository as a tmp corpus (1,637 files, 36 MB artifact),
    interleaved A/B, n=11, on a contended machine, both routes returning the
    same 18,316 nodes / 104,951 edges / 192 dead entries and the same node-id
    set:

    ==========================  ========  ========  ==========
    route                       p50       min       peak RSS
    ==========================  ========  ========  ==========
    load_code_graph (warm)      1193.9ms  1154.3ms  250 MB
    read_code_graph             322.5ms   310.4ms   409 MB
    ==========================  ========  ========  ==========

    The first `load_code_graph` on a fresh checkout costs 4787.0 ms and writes a
    104.2 MB `index.sqlite` — from a read path, under a writer lease.

    The trade is not free and is stated rather than buried: `json.load` builds
    the whole object tree at once where the store re-materialised it row by row,
    so peak RSS goes *up* by ~159 MB on this corpus. It buys 3.7x on the warm
    read and removes the 104 MB write and the second store it lands in.

    Returns ``None`` when there is no artifact, when it is unreadable, or when
    it is larger than ``indexing.graph_json_max_bytes`` — the same bound
    :func:`write_code_graph` enforces on the way out, so a file this refuses is
    one this process would refuse to write.

    A size-capped export is returned *with* :data:`GRAPH_INCOMPLETE_META` set in
    ``meta``, never silently. See that constant for why.
    """
    root = root.expanduser().resolve()
    path = graph_path(root)
    if not path.is_file():
        return None
    limit = _graph_json_max_bytes(root)
    try:
        size = path.stat().st_size
        if size > limit:
            logger.warning(
                "code graph export is %d bytes, over the %d-byte bound; "
                "not read (raise indexing.graph_json_max_bytes to read it)",
                size,
                limit,
            )
            return None
        graph = CodeGraph.model_validate(read_json(path))
    except Exception:
        logger.debug("Failed to read code graph export", exc_info=True)
        return None
    tier = (graph.meta or {}).get("compatibility_export_tier")
    if tier not in _COMPLETE_EXPORT_TIERS:
        graph.meta[GRAPH_INCOMPLETE_META] = (
            f"code_graph.json is a size-capped {tier!r} export: its node, edge "
            "and liveness lists are truncated or empty. Re-run `dev map` after "
            "raising indexing.graph_json_max_bytes for a complete answer."
        )
    return _annotate_graph_degraded(root, graph)


def load_code_graph(root: Path) -> Optional[CodeGraph]:
    from devcouncil.codeintel import get_codeintel_service

    root = root.expanduser().resolve()
    path = graph_path(root)
    service = get_codeintel_service(root)
    # SQLite is canonical. Import the JSON compatibility artifact only when the
    # store is empty/missing — never let an external JSON rewrite clobber a
    # committed generation (accidental or malicious mtime/digest churn).
    try:
        if not service.store.exists() and path.is_file():
            if path.stat().st_size > _graph_json_max_bytes(root):
                raise CompatibilityGraphTooLarge(
                    f"compatibility graph import exceeds {_graph_json_max_bytes(root)} bytes"
                )
            data = read_json(path)
            exported = CodeGraph.model_validate(data)
            # Import is a store write: hold the writer lease like every other
            # persist path. GraphBuildBusy lands in the except below and the
            # read falls through to whatever generation the builder commits.
            from devcouncil.codeintel.build_control import graph_build_session

            with graph_build_session(root):
                service.persist(exported)
                service.store.record_compatibility_export(path, exported)
            return _annotate_graph_degraded(root, exported)
        recorded_digest, recorded_mtime = service.store.compatibility_export_state()
        if (
            path.is_file()
            and recorded_mtime is not None
            and path.stat().st_mtime_ns != recorded_mtime
        ):
            # Refresh the handshake when the on-disk export matches the store;
            # otherwise leave SQLite authoritative and let ``dev map doctor``
            # / export self-heal report drift.
            from devcouncil.codeintel.store.sqlite import compatibility_graph_digest

            if path.stat().st_size <= _graph_json_max_bytes(root):
                data = read_json(path)
                exported = CodeGraph.model_validate(data)
                if compatibility_graph_digest(exported) == recorded_digest:
                    service.store.record_compatibility_export(path, exported)
                elif _written_by_kernel(exported):
                    # The Rust kernel is the engine and this JSON is its
                    # export. "SQLite wins" was written when SQLite was the
                    # engine's own store; now it is a read cache for the
                    # Python-only query commands, and a cache that refuses
                    # the source it caches serves generation 76 against a
                    # kernel at 511 — every `_require_graph` command did,
                    # measured on this repository. Import once per build,
                    # then serve from the cache as before.
                    from devcouncil.codeintel.build_control import graph_build_session

                    with graph_build_session(root):
                        service.persist(exported)
                        service.store.record_compatibility_export(path, exported)
                    return _annotate_graph_degraded(root, exported)
                else:
                    logger.info(
                        "ignoring external compatibility graph that diverges from "
                        "canonical store (sqlite wins); re-export with "
                        "`dev map export` / map refresh if JSON must catch up"
                    )
    except Exception:
        logger.debug("Failed to reconcile compatibility graph export", exc_info=True)
    try:
        graph = service.load()
        if graph is not None:
            return _annotate_graph_degraded(root, graph)
    except Exception:
        # A corrupt/newer database must not strand users who still have the
        # versioned JSON export. ``dev map doctor`` reports the store failure;
        # compatibility reads remain available until it is repaired/rebuilt.
        logger.debug("Failed to load canonical code-intelligence store", exc_info=True)
    if not path.is_file():
        return None
    try:
        if path.stat().st_size > _graph_json_max_bytes(root):
            raise CompatibilityGraphTooLarge(
                f"compatibility graph import exceeds {_graph_json_max_bytes(root)} bytes"
            )
        data = read_json(path)
        return _annotate_graph_degraded(root, CodeGraph.model_validate(data))
    except Exception:
        logger.debug("Failed to load code graph", exc_info=True)
        return None


def _written_by_kernel(graph: CodeGraph) -> bool:
    """Whether a graph export carries the Rust kernel's engine stamp."""
    meta = graph.meta if isinstance(graph.meta, dict) else {}
    return meta.get("map_engine") == "devmap-rust"


def _annotate_graph_degraded(root: Path, graph: CodeGraph) -> CodeGraph:
    """Surface repo_map lean/degraded handshake on graph payloads for consumers."""
    map_path = root / ".devcouncil" / "repo_map.json"
    if not map_path.is_file():
        return graph
    try:
        data = read_json(map_path)
        if not isinstance(data, dict) or not data.get("graph_degraded"):
            return graph
        graph.meta["graph_degraded"] = True
        graph.meta["graph_degraded_reason"] = str(data.get("graph_degraded_reason") or "")
    except Exception:
        logger.debug("graph_degraded annotation failed", exc_info=True)
    return graph


# --- Opt-in PDG layer (CFG / reaching-def / CDG / taint) ---


def build_pdg_for_paths(
    root: Path,
    graph: CodeGraph,
    *,
    paths: Optional[Iterable[str]] = None,
):
    """Analyze Python files and return a PDG layer."""
    from devcouncil.indexing.graph.pdg.cdg import build_cdg
    from devcouncil.indexing.graph.pdg.cfg import build_cfg_for_function
    from devcouncil.indexing.graph.pdg.reaching_def import compute_reaching_defs
    from devcouncil.indexing.graph.pdg.schema import FilePDG, FunctionPDG, PDGLayer, PDG_VERSION
    from devcouncil.indexing.graph.pdg.taint import analyze_taint

    root = root.expanduser().resolve()
    if paths is None:
        paths = sorted({n.path for n in graph.nodes if n.path.endswith(".py")})

    def _python_functions(tree: ast.AST) -> List[tuple[str, ast.AST]]:
        out: List[tuple[str, ast.AST]] = []
        if isinstance(tree, ast.Module):
            for node in tree.body:
                if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    out.append((node.name, node))
                elif isinstance(node, ast.ClassDef):
                    for item in node.body:
                        if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)):
                            out.append((f"{node.name}.{item.name}", item))
        seen: Set[str] = set()
        unique: List[tuple[str, ast.AST]] = []
        for qual, fn in out:
            if qual in seen:
                continue
            seen.add(qual)
            unique.append((qual, fn))
        return unique

    layer = PDGLayer(version=PDG_VERSION)
    for raw in paths:
        rel = raw.replace("\\", "/")
        path = root / rel
        if path.suffix.lower() != ".py":
            continue
        try:
            source = path.read_text(encoding="utf-8")
            tree = ast.parse(source, filename=rel)
        except (OSError, SyntaxError, UnicodeDecodeError):
            logger.debug("PDG skip %s", rel, exc_info=True)
            continue
        lines = source.splitlines()
        functions: List[FunctionPDG] = []
        for qualname, fn_node in _python_functions(tree):
            start_line = int(getattr(fn_node, "lineno", 0) or 0)
            end_line = int(getattr(fn_node, "end_lineno", start_line) or start_line)
            cfg = build_cfg_for_function(rel, qualname, fn_node, lines)
            reaching = compute_reaching_defs(cfg, fn_node)
            cdg = build_cdg(cfg, fn_node)
            taint = analyze_taint(rel, qualname, fn_node, reaching)
            functions.append(
                FunctionPDG(
                    path=rel,
                    qualname=qualname,
                    start_line=start_line,
                    end_line=end_line,
                    blocks=cfg.blocks,
                    cfg_edges=cfg.edges,
                    reaching_def=reaching,
                    cdg=cdg,
                    taint=taint,
                )
            )
        if functions:
            file_pdg = FilePDG(path=rel, language="python", functions=functions)
            layer.files[rel] = file_pdg
            for fn in functions:
                layer.taint_findings.extend(fn.taint)
    return layer

def merge_pdg_into_graph(graph: CodeGraph, layer) -> Dict[str, dict[str, object]]:
    """Persist PDG summary in graph.meta and return analysis shards."""
    graph.meta["pdg"] = layer.to_meta()
    shards: Dict[str, dict[str, object]] = {}
    for path in layer.files:
        payload = layer.shard_payload(path)
        if payload is not None:
            shards[path] = payload
    return shards


def load_pdg_layer(graph: CodeGraph):
    from devcouncil.indexing.graph.pdg.schema import PDGLayer, PDG_VERSION, TaintFinding

    raw = graph.meta.get("pdg")
    if not isinstance(raw, dict):
        return None
    layer = PDGLayer(version=int(raw.get("version") or PDG_VERSION))
    for item in raw.get("taint_findings") or []:
        if isinstance(item, dict):
            layer.taint_findings.append(TaintFinding.from_dict(item))
    return layer
