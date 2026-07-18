"""Crash-isolated native Tree-sitter worker pool."""

from __future__ import annotations

import atexit
import multiprocessing
import os
import re
import threading
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path
from typing import Any

_ACTIVATION_LOCK = threading.Lock()
_ACTIVATION_ATTEMPTED = False
_ACTIVATION_STATUS: dict[str, Any] = {"installed": False, "activated": False}


def _activate_companion_once() -> dict[str, Any]:
    """Activate packaged assets once per process without fetching grammars."""

    global _ACTIVATION_ATTEMPTED, _ACTIVATION_STATUS
    with _ACTIVATION_LOCK:
        if _ACTIVATION_ATTEMPTED:
            return _ACTIVATION_STATUS
        _ACTIVATION_ATTEMPTED = True
        try:
            import devcouncil_codeintel_grammars as grammar_assets
        except ImportError:
            return _ACTIVATION_STATUS
        try:
            _ACTIVATION_STATUS = {"installed": True, **grammar_assets.activate()}
        except Exception as exc:
            _ACTIVATION_STATUS = {
                "installed": True,
                "activated": False,
                "ok": False,
                "error": f"{type(exc).__name__}: {exc}",
            }
        return _ACTIVATION_STATUS


def _structure_row(item: Any) -> dict[str, Any]:
    span = getattr(item, "span", None)
    return {
        "name": str(getattr(item, "name", "") or ""),
        "kind": str(getattr(item, "kind", "") or ""),
        "start_line": int(getattr(span, "start_line", 0)) if span is not None else 0,
        "end_line": int(getattr(span, "end_line", 0)) if span is not None else 0,
        "decorators": [str(value) for value in getattr(item, "decorators", []) or []],
        "children": [_structure_row(child) for child in getattr(item, "children", []) or []],
    }


def _declared_name(node: Any, raw: bytes) -> str:
    """Resolve a definition's identifier through C-family declarator nesting."""
    current = node
    for _ in range(8):
        candidate = None
        for field in ("name", "declarator"):
            try:
                candidate = current.child_by_field_name(field)
            except (AttributeError, TypeError):
                candidate = None
            if candidate is not None:
                break
        if candidate is None:
            return ""
        if str(getattr(candidate, "type", "")).endswith("identifier"):
            return raw[candidate.start_byte:candidate.end_byte].decode("utf-8", errors="replace")
        current = candidate
    return ""


def _fill_missing_structure_names(tree: Any, raw: bytes, rows: list[dict[str, Any]]) -> None:
    """The language pack's structure pass yields ``name=None`` for C-family
    definitions — the identifier hides inside the declarator chain — and
    downstream extraction drops nameless rows, losing every C/C++/ObjC symbol.
    Recover names from the parse tree, matched by start line."""
    pending: dict[int, list[dict[str, Any]]] = {}

    def collect(items: list[dict[str, Any]]) -> None:
        for row in items:
            if not row["name"]:
                pending.setdefault(int(row["start_line"]), []).append(row)
            collect(row["children"])

    collect(rows)
    if not pending:
        return
    stack = [tree.root_node]
    while stack:
        node = stack.pop()
        stack.extend(reversed(list(getattr(node, "children", ()) or ())))
        point = getattr(node, "start_point", None)
        waiting = pending.get(int(point[0])) if point is not None else None
        if not waiting:
            continue
        name = _declared_name(node, raw)
        if not name:
            continue
        row = waiting.pop(0)
        row["name"] = name


def _call_rows(parser: Any, source: str, tree: Any | None = None) -> list[dict[str, Any]]:
    raw = source.encode("utf-8")
    if tree is None:
        tree = parser.parse(raw)
    rows: list[dict[str, Any]] = []
    stack = [tree.root_node]
    while stack:
        node = stack.pop()
        stack.extend(reversed(list(getattr(node, "children", ()) or ())))
        node_type = str(getattr(node, "type", ""))
        if "call" not in node_type or any(
            value in node_type for value in ("callable", "calling_convention")
        ):
            continue
        callee = None
        for field in ("function", "name", "method"):
            try:
                callee = node.child_by_field_name(field)
            except (AttributeError, TypeError):
                callee = None
            if callee is not None:
                break
        if callee is None:
            named = list(getattr(node, "named_children", ()) or ())
            callee = named[0] if named else None
        if callee is None:
            continue
        text = raw[callee.start_byte:callee.end_byte].decode("utf-8", errors="replace")
        identifiers = re.findall(r"[A-Za-z_$][\w$]*", text)
        if not identifiers:
            continue
        name = identifiers[-1]
        receiver = ".".join(identifiers[:-1])
        rows.append({
            "name": name,
            "receiver": receiver,
            "line": int(node.start_point[0]) + 1,
        })
    return rows


def _native_process(language: str, source: str) -> dict[str, Any] | None:
    """Execute inside a child so a faulty native grammar cannot kill the server."""
    import tree_sitter_language_pack as pack

    if language not in set(pack.available_languages()):
        return None
    result = pack.process(
        source,
        pack.ProcessConfig(
            language=language,
            structure=True,
            imports=True,
            exports=True,
            symbols=True,
            diagnostics=True,
        ),
    )
    parser = pack.get_parser(language)
    raw = source.encode("utf-8")
    tree = parser.parse(raw)
    structure = [_structure_row(item) for item in getattr(result, "structure", []) or []]
    _fill_missing_structure_names(tree, raw, structure)
    return {
        "structure": structure,
        "imports": [
            {
                "source": str(getattr(item, "source", "") or ""),
                "items": [str(value) for value in getattr(item, "items", []) or []],
                "alias": str(getattr(item, "alias", "") or ""),
            }
            for item in getattr(result, "imports", []) or []
        ],
        "exports": [
            {"name": str(getattr(item, "name", "") or "")}
            for item in getattr(result, "exports", []) or []
        ],
        "calls": _call_rows(parser, source, tree),
    }


class ParserWorkerPool:
    """Small spawn-based pool that is discarded after a crash or timeout."""

    def __init__(self, *, max_workers: int | None = None, timeout: float = 30.0):
        cpu_bound = max(1, (os.cpu_count() or 2) // 2)
        self.max_workers = max(1, min(4, max_workers or cpu_bound))
        self.timeout = max(1.0, timeout)
        self._pool: ProcessPoolExecutor | None = None
        self._lock = threading.Lock()

    def process(self, language: str, source: str) -> dict[str, Any] | None:
        try:
            pool = self._executor()
            return pool.submit(_native_process, language, source).result(timeout=self.timeout)
        except Exception:
            self.restart()
            return None

    def _executor(self) -> ProcessPoolExecutor:
        with self._lock:
            if self._pool is None:
                _activate_companion_once()
                self._pool = ProcessPoolExecutor(
                    max_workers=self.max_workers,
                    mp_context=multiprocessing.get_context("spawn"),
                    initializer=_activate_companion_once,
                )
            return self._pool

    def restart(self) -> None:
        with self._lock:
            pool, self._pool = self._pool, None
        if pool is not None:
            pool.shutdown(wait=False, cancel_futures=True)

    def close(self) -> None:
        with self._lock:
            pool, self._pool = self._pool, None
        if pool is not None:
            pool.shutdown(wait=True, cancel_futures=True)


_POOL = ParserWorkerPool()
atexit.register(_POOL.close)


def process_tree_sitter(language: str, source: str) -> dict[str, Any] | None:
    return _POOL.process(language, source)


def parser_worker_status() -> dict[str, object]:
    return {
        "start_method": "spawn",
        "max_workers": _POOL.max_workers,
        "timeout_seconds": _POOL.timeout,
        "pid": os.getpid(),
        "module": str(Path(__file__).resolve()),
        "grammar_companion": dict(_ACTIVATION_STATUS),
    }
