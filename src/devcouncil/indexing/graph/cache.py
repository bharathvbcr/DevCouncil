"""Extraction cache v5 — symbols, calls, import_details, modules/specs keyed by sha256."""

from __future__ import annotations

import hashlib
import logging
from pathlib import Path
from typing import Any, Dict, List, Optional, Set, cast

from devcouncil.indexing.graph.extract_python import (
    ExtractedCall,
    ExtractedImport,
    ExtractedSymbol,
    FileExtraction,
)
from devcouncil.indexing.graph.extract_ts import extract_file
from devcouncil.utils.fsio import atomic_write_json
from devcouncil.utils.json_persist import read_json

logger = logging.getLogger(__name__)

# v1 = Python modules; v2 = + JS/TS specs; v3 = + symbols + calls;
# v4 = + import_details (named imports / alias maps) for warm/cold edge parity;
# v5 = + references (non-call name/attribute loads) for callback-aware liveness.
PARSE_CACHE_VERSION = 5

# Keys preserved across partial updates (RepoMapper modules/specs vs graph symbols).
_PRESERVE_KEYS = (
    "symbols",
    "calls",
    "all_exports",
    "reexports",
    "references",
    "language",
    "import_details",
    "modules",
    "specs",
)


def cache_path(root: Path) -> Path:
    return root / ".devcouncil" / "cache" / "repo_map_parse.json"


def load_parse_cache(root: Path) -> Dict[str, Dict[str, Any]]:
    """Load cache. Version mismatch → empty dict (caller rebuilds in place)."""
    try:
        data = read_json(cache_path(root))
        if data.get("version") == PARSE_CACHE_VERSION and isinstance(data.get("files"), dict):
            return cast(Dict[str, Dict[str, Any]], data["files"])
    except Exception:
        pass
    return {}


def save_parse_cache(root: Path, files: Dict[str, Dict[str, Any]]) -> None:
    try:
        path = cache_path(root)
        path.parent.mkdir(parents=True, exist_ok=True)
        atomic_write_json(path, {"version": PARSE_CACHE_VERSION, "files": files})
    except Exception:
        logger.debug("Failed to write graph parse cache", exc_info=True)


def merge_parse_cache(
    root: Path,
    updates: Dict[str, Dict[str, Any]],
    managed: Set[str],
) -> None:
    """Merge updates for ``managed`` paths; preserve sibling-language / sibling-field keys.

    Paths in ``managed`` but absent from ``updates`` are pruned. When an update
    omits graph or import fields, existing values from the prior entry are kept
    so RepoMapper modules/specs passes and graph extract passes share one store.
    """
    cache = load_parse_cache(root)
    merged = {k: v for k, v in cache.items() if k not in managed}
    for path, entry in updates.items():
        prev = cache.get(path)
        if isinstance(prev, dict) and isinstance(entry, dict):
            for key in _PRESERVE_KEYS:
                if key not in entry and key in prev:
                    entry = {**entry, key: prev[key]}
        merged[path] = entry
    if merged != cache:
        save_parse_cache(root, merged)


def _symbol_to_dict(s: ExtractedSymbol) -> Dict[str, Any]:
    return {
        "kind": s.kind,
        "name": s.name,
        "qualname": s.qualname,
        "line": s.line,
        "end_line": s.end_line,
        "bases": list(s.bases),
        "implements": list(getattr(s, "implements", []) or []),
        "decorators": list(s.decorators),
        "exported": s.exported,
    }


def _call_to_dict(c: ExtractedCall) -> Dict[str, Any]:
    return {
        "name": c.name,
        "line": c.line,
        "receiver": c.receiver,
        "qualname_hint": c.qualname_hint,
    }


def _import_to_dict(imp: ExtractedImport) -> Dict[str, Any]:
    return {
        "module": imp.module,
        "names": list(imp.names),
        "alias_map": dict(imp.alias_map),
    }


def _symbol_from_dict(d: Dict[str, Any]) -> ExtractedSymbol:
    return ExtractedSymbol(
        kind=str(d.get("kind") or "function"),
        name=str(d.get("name") or ""),
        qualname=str(d.get("qualname") or d.get("name") or ""),
        line=int(d.get("line") or 0),
        end_line=int(d.get("end_line") or d.get("line") or 0),
        bases=list(d.get("bases") or []),
        implements=list(d.get("implements") or []),
        decorators=list(d.get("decorators") or []),
        exported=bool(d.get("exported", False)),
    )


def _call_from_dict(d: Dict[str, Any]) -> ExtractedCall:
    return ExtractedCall(
        name=str(d.get("name") or ""),
        line=int(d.get("line") or 0),
        receiver=str(d.get("receiver") or ""),
        qualname_hint=str(d.get("qualname_hint") or ""),
    )


def _import_from_dict(d: Dict[str, Any]) -> ExtractedImport:
    alias_raw = d.get("alias_map") or {}
    alias_map = {
        str(k): str(v) for k, v in alias_raw.items()
    } if isinstance(alias_raw, dict) else {}
    return ExtractedImport(
        module=str(d.get("module") or ""),
        names=[n for n in (d.get("names") or []) if isinstance(n, str)],
        alias_map=alias_map,
    )


def extraction_to_cache_entry(ext: FileExtraction, digest: str) -> Dict[str, Any]:
    entry: Dict[str, Any] = {
        "sha256": digest,
        "symbols": [_symbol_to_dict(s) for s in ext.symbols],
        "calls": [_call_to_dict(c) for c in ext.calls],
        "import_details": [_import_to_dict(i) for i in ext.import_details],
        "all_exports": list(ext.all_exports),
        "reexports": list(ext.reexports),
        "references": list(ext.references),
        "language": ext.language,
    }
    if ext.path.endswith(".py"):
        entry["modules"] = list(ext.imports)
    elif ext.path.endswith(".go"):
        entry["specs"] = list(ext.imports)
    else:
        entry["specs"] = list(ext.imports)
    return entry


def extraction_from_cache_entry(path: str, entry: Dict[str, Any]) -> FileExtraction:
    imports: List[str] = []
    if isinstance(entry.get("modules"), list):
        imports = [m for m in entry["modules"] if isinstance(m, str)]
    elif isinstance(entry.get("specs"), list):
        imports = [m for m in entry["specs"] if isinstance(m, str)]
    symbols = [
        _symbol_from_dict(s)
        for s in (entry.get("symbols") or [])
        if isinstance(s, dict)
    ]
    calls = [
        _call_from_dict(c)
        for c in (entry.get("calls") or [])
        if isinstance(c, dict)
    ]
    import_details = [
        _import_from_dict(i)
        for i in (entry.get("import_details") or [])
        if isinstance(i, dict)
    ]
    return FileExtraction(
        path=path,
        language=str(entry.get("language") or ""),
        imports=imports,
        import_details=import_details,
        symbols=symbols,
        calls=calls,
        all_exports=[x for x in (entry.get("all_exports") or []) if isinstance(x, str)],
        reexports=[x for x in (entry.get("reexports") or []) if isinstance(x, str)],
        references=[x for x in (entry.get("references") or []) if isinstance(x, str)],
    )


def extract_cached(
    root: Path,
    path: str,
    *,
    cache: Optional[Dict[str, Dict[str, Any]]] = None,
    force: bool = False,
) -> tuple[FileExtraction, Dict[str, Any]]:
    """Extract one file, using cache when sha256 matches.

    Returns ``(extraction, cache_entry)``.
    """
    cache = cache if cache is not None else load_parse_cache(root)
    try:
        raw = (root / path).read_bytes()
    except OSError:
        empty = FileExtraction(path=path, language="")
        return empty, {
            "sha256": "",
            "modules": [],
            "symbols": [],
            "calls": [],
            "import_details": [],
        }
    digest = hashlib.sha256(raw).hexdigest()
    entry = cache.get(path)
    if (
        not force
        and isinstance(entry, dict)
        and entry.get("sha256") == digest
        and isinstance(entry.get("symbols"), list)
        and isinstance(entry.get("calls"), list)
        and isinstance(entry.get("import_details"), list)
    ):
        return extraction_from_cache_entry(path, entry), entry
    source = raw.decode("utf-8", errors="replace")
    ext = extract_file(path, source)
    fresh = extraction_to_cache_entry(ext, digest)
    return ext, fresh
