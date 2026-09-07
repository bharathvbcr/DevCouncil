"""Read and PDG-annotate the code knowledge graph (schema v2).

The graph is built *and written* by the Rust kernel; this module reads its
``code_graph.json`` back (:func:`read_code_graph`) and computes the opt-in
Python PDG layer beside it. The Python extractor and resolver that used to
build the graph here (``build_code_graph``, ``assemble_graph``,
``extract_all``) were retired once the kernel became the only writer of both
map artifacts, and the Python SQLite store they fed (``load_code_graph``,
``write_code_graph`` and the tiered ``code_graph.json`` export beneath them)
was retired after them, once every consumer read the kernel's artifact
directly.
"""

from __future__ import annotations

import ast
import hashlib
import json
import logging
import os
from pathlib import Path
from typing import Dict, Iterable, List, Optional, Set

from devcouncil.indexing.graph.schema import CodeGraph
from devcouncil.utils.fsio import atomic_write_text
from devcouncil.utils.json_persist import dump_json, read_json

logger = logging.getLogger(__name__)

def graph_path(root: Path) -> Path:
    """The kernel's ``code_graph.json``, wherever this root's state dir is.

    This was ``root / ".devcouncil" / "graph" / "code_graph.json"``, a literal.
    The kernel resolves the state directory per repository, so on one holding
    the standalone ``.devmap/`` layout that literal named a file that does not
    exist -- and every consumer of it reported "no graph" for a graph that does.
    :func:`devcouncil.devmap_engine.state_dir` is the one owner of the answer.

    Imported inside the function: this module is on the `dev map` import path
    and the facade above it is deferred for exactly that reason.
    """
    from devcouncil.devmap_engine import graph_path as resolved_graph_path

    return resolved_graph_path(root)


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


def _graph_json_max_bytes(root: Path) -> int:
    try:
        from devcouncil.app.config import load_config

        return int(load_config(root).indexing.graph_json_max_bytes)
    except Exception:
        return 128 * 1024 * 1024


#: Stamped on a graph whose on-disk form is a size-capped export.
#:
#: The Python store used to hide this: `load_code_graph` preferred SQLite, which
#: held the uncapped graph, so a consumer reading a `compact` or `stub`
#: `code_graph.json` still got complete lists. Reading the artifact directly
#: removes that cover, and a capped export must therefore say it is capped
#: rather than answer in the shape of a complete one — `compact` truncated
#: `unwired_candidates` at 200 and `dead_code` at 500, and `stub` emptied the
#: nodes and edges entirely. Both the store and the Python tiering are gone;
#: the tiers are still recognised because an artifact written by an older
#: DevCouncil is still on disk in existing checkouts, and one that says it is
#: capped must keep saying so.
GRAPH_INCOMPLETE_META = "graph_export_incomplete_reason"

#: Export tiers whose lists are the whole graph. `None` is the kernel's own
#: artifact, which is not tiered at all.
_COMPLETE_EXPORT_TIERS = (None, "slim")


def read_code_graph(root: Path) -> Optional[CodeGraph]:
    """The kernel's ``code_graph.json``, parsed once and validated.

    This is a read of the kernel's own artifact, not a second engine: the same
    `devmap manifest` run that writes `repo_map.json` writes this file, and the
    Python side has not built a graph since the kernel became the only writer.

    It replaced `load_code_graph` for every consumer that wants the whole graph,
    and then outlived it. That function reached the graph through the Python
    `index.sqlite` cache — importing this same JSON into it on first read, then
    re-materialising every node and edge as pydantic models out of SQLite on
    every call after.

    Measured on this repository as a tmp corpus (1,637 files, 36 MB artifact),
    interleaved A/B, n=11, on a contended machine, both routes returning the
    same 18,316 nodes / 104,951 edges / 192 dead entries and the same node-id
    set:

    ==========================  ========  ========  ==========
    route                       p50       min       peak RSS
    ==========================  ========  ========  ==========
    load_code_graph (warm)      1193.9ms  1154.3ms  250 MB   (deleted)
    read_code_graph             322.5ms   310.4ms   409 MB
    ==========================  ========  ========  ==========

    The first `load_code_graph` on a fresh checkout costs 4787.0 ms and writes a
    104.2 MB `index.sqlite` — from a read path, under a writer lease.

    The trade is not free and is stated rather than buried: `json.load` builds
    the whole object tree at once where the store re-materialised it row by row,
    so peak RSS goes *up* by ~159 MB on this corpus. It buys 3.7x on the warm
    read and removes the 104 MB write and the second store it lands in.

    Returns ``None`` when there is no artifact, when it is unreadable, or when
    it is larger than ``indexing.graph_json_max_bytes``. That bound was shared
    with the Python export writer; the writer is gone and the bound is now
    purely this reader's ceiling on how much JSON it will materialise.

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


def _annotate_graph_degraded(root: Path, graph: CodeGraph) -> CodeGraph:
    """Surface repo_map lean/degraded handshake on graph payloads for consumers."""
    from devcouncil.devmap_engine import map_path

    map_file = map_path(root)
    if not map_file.is_file():
        return graph
    try:
        data = read_json(map_file)
        if not isinstance(data, dict) or not data.get("graph_degraded"):
            return graph
        graph.meta["graph_degraded"] = True
        graph.meta["graph_degraded_reason"] = str(data.get("graph_degraded_reason") or "")
    except Exception:
        logger.debug("graph_degraded annotation failed", exc_info=True)
    return graph


# --- Opt-in PDG layer (CFG / reaching-def / CDG / taint) ---

#: Where the opt-in PDG layer lives. Its own file, beside the kernel's.
#:
#: The layer used to be merged into the `CodeGraph` and written back with
#: `write_code_graph`, which rewrote `code_graph.json` — the artifact the kernel
#: is the only writer of. That write-back was also the last production caller of
#: the Python store's persist path, and it survived every `dev map` run only
#: until the next one, because the kernel rewrites the file from scratch. Both
#: the writer and the store it fed have since been deleted. Nothing outside the PDG commands ever read it back: `rg -uu pdg`
#: over the tree finds `graph.meta["pdg"]` read by `load_pdg_layer` and
#: `indexing/graph/query.py`, the per-file shards read by that same module, the
#: two `stats` reads in the commands that had just written them, and nothing in
#: `rust-port/` at all.
PDG_SIDECAR_NAME = "pdg.json"


def pdg_sidecar_path(root: Path) -> Path:
    """Beside `code_graph.json`, derived from it rather than re-spelled.

    Taking the directory from :func:`graph_path` means :data:`GRAPH_REL` stays
    the one place the graph state directory is named. A second literal
    ``.devcouncil/graph`` here would be a copy that drifts the first time the
    state directory moves — and it does move: the standalone kernel resolves a
    `.devmap` state dir. :func:`devcouncil.indexing.viz.write_graph_html`
    already places `graph.html` this way.
    """
    return graph_path(root).with_name(PDG_SIDECAR_NAME)


def python_paths_for_pdg(root: Path) -> List[str]:
    """Python files to analyse, from the kernel's own file inventory.

    `repo_map.json` is what the same `dev map` run writes beside
    `code_graph.json`, and it lists every file the kernel indexed with its
    language. Taking the list from here rather than from `graph.nodes` means the
    PDG layer needs no graph at all — it is a per-file analysis, and it was
    parsing a 34 MB graph to learn which files end in `.py`.

    Returns an empty list when there is no map, which the callers report as
    "run `dev map` first" rather than as "this repository has no Python".
    """
    from devcouncil.devmap_engine import map_path

    try:
        data = read_json(map_path(root))
    except (OSError, ValueError):
        return []
    files = data.get("files") if isinstance(data, dict) else None
    if not isinstance(files, list):
        return []
    paths: Set[str] = set()
    for entry in files:
        if not isinstance(entry, dict):
            continue
        path = str(entry.get("path") or "").replace("\\", "/")
        if path.endswith(".py"):
            paths.add(path)
    return sorted(paths)


def write_pdg_layer(root: Path, layer) -> Path:
    """Publish the PDG layer to its own artifact, atomically.

    Complete rather than capped: `PDGLayer.to_meta` trims `taint_findings` to
    500 because it was going into `graph.meta` beside everything else, and a
    reader of `dev map explain` was given that truncated list with nothing
    saying it was one. The per-function findings under `files` are the whole
    set, and :func:`read_pdg_layer` rebuilds the finding list from them.
    """
    payload = {
        "version": int(getattr(layer, "version", 1)),
        "stats": (layer.to_meta() or {}).get("stats") or {},
        "files": {
            path: file_pdg.to_dict() for path, file_pdg in sorted(layer.files.items())
        },
    }
    path = pdg_sidecar_path(root)
    path.parent.mkdir(parents=True, exist_ok=True)
    atomic_write_text(
        path,
        json.dumps(payload, ensure_ascii=False, separators=(",", ":"), sort_keys=True)
        + "\n",
    )
    return path


def read_pdg_layer_file(root: Path):
    """The PDG sidecar as a ``PDGLayer``, or ``None`` when it has not been built.

    ``None`` means "no PDG layer on disk" — the opt-in analysis has not run —
    which the query surfaces report as such rather than as "this code has no
    control flow".
    """
    from devcouncil.indexing.graph.pdg.schema import PDGLayer, PDG_VERSION, FilePDG

    path = pdg_sidecar_path(root)
    if not path.is_file():
        return None
    try:
        raw = read_json(path)
    except (OSError, ValueError):
        logger.debug("PDG sidecar unreadable", exc_info=True)
        return None
    if not isinstance(raw, dict):
        return None
    layer = PDGLayer(version=int(raw.get("version") or PDG_VERSION))
    files = raw.get("files")
    if isinstance(files, dict):
        for path_key, payload in files.items():
            if not isinstance(payload, dict):
                continue
            try:
                file_pdg = FilePDG.from_dict(payload)
            except (KeyError, TypeError, ValueError):
                logger.debug("PDG sidecar entry unreadable: %s", path_key, exc_info=True)
                continue
            layer.files[str(path_key)] = file_pdg
            for function in file_pdg.functions:
                layer.taint_findings.extend(function.taint)
    return layer


def build_pdg_for_paths(
    root: Path,
    graph: Optional[CodeGraph] = None,
    *,
    paths: Optional[Iterable[str]] = None,
):
    """Analyze Python files and return a PDG layer.

    ``graph`` is accepted for callers that already hold one and is used only to
    enumerate Python files; ``None`` takes that list from the kernel's own
    inventory instead (:func:`python_paths_for_pdg`). The analysis itself has
    never looked at the graph — it parses each file with ``ast``.
    """
    from devcouncil.indexing.graph.pdg.cdg import build_cdg
    from devcouncil.indexing.graph.pdg.cfg import build_cfg_for_function
    from devcouncil.indexing.graph.pdg.reaching_def import compute_reaching_defs
    from devcouncil.indexing.graph.pdg.schema import FilePDG, FunctionPDG, PDGLayer, PDG_VERSION
    from devcouncil.indexing.graph.pdg.taint import analyze_taint

    root = root.expanduser().resolve()
    if paths is None:
        paths = (
            sorted({n.path for n in graph.nodes if n.path.endswith(".py")})
            if graph is not None
            else python_paths_for_pdg(root)
        )

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
