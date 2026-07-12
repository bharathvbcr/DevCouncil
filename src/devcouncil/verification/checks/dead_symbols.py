"""Dead-symbol gate: flag newly added public top-level symbols nothing references.

Diff-scoped via ``added_lines_by_file`` + AST/span intersection. Test references
and intent-text naming clear a symbol; files already flagged ``unwired_file`` are
skipped. Never raises.
"""

from __future__ import annotations

import ast
import logging
import re
from pathlib import Path
from typing import Callable, List, Optional, Set, Tuple

from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.indexing.wiring import (
    ALLOW_UNWIRED,
    decorator_names,
    is_liveness_code_file,
    is_private_symbol,
    is_test_path,
    is_vendored_path,
    is_wiring_decorated,
    iter_js_export_symbols,
    parse_python_all_exports,
    parse_python_reexport_names,
    strip_js_comments,
    strip_py_comments,
    strip_string_literals,
)
from devcouncil.verification.checks.semantic_diff import task_intent_text
from devcouncil.verification.stub_detector import added_lines_by_file, task_allows_scaffolding

logger = logging.getLogger(__name__)

_IDENT_RE = re.compile(r"\b([A-Za-z_][A-Za-z0-9_]*)\b")
_CODE_SUFFIXES = {".py", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"}

# (path, start_line, end_line, name)
_SymbolCand = Tuple[str, int, int, str]


def _norm(path: str) -> str:
    s = path.replace("\\", "/")
    while s.startswith("./"):
        s = s[2:]
    return s


def _python_candidates(
    project_root: Path,
    path: str,
    added_lines: Set[int],
) -> List[_SymbolCand]:
    """Return (path, start, end, name) for public top-level defs intersecting added lines."""
    try:
        source = (project_root / path).read_text(encoding="utf-8", errors="replace")
        tree = ast.parse(source)
    except Exception:
        return []
    all_exports = parse_python_all_exports(source)
    reexports = parse_python_reexport_names(path, source)
    protected = all_exports | reexports
    out: List[_SymbolCand] = []
    for node in tree.body:
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            continue
        name = node.name
        if is_private_symbol(name):
            continue
        # ``__all__`` / barrel re-exports = public API surface (parity with graph).
        if name in protected:
            continue
        start = getattr(node, "lineno", None)
        end = getattr(node, "end_lineno", start) or start
        if start is None:
            continue
        span = set(range(start, (end or start) + 1))
        if not span & added_lines:
            continue
        if is_wiring_decorated(decorator_names(node)):
            continue
        out.append((path, start, end or start, name))
    return out


def _js_definition_span(source: str, name: str) -> Optional[Tuple[int, int]]:
    """Best-effort start/end lines for a JS/TS binding named ``name``."""
    patterns = (
        rf"(?m)^\s*(?:export\s+)?(?:async\s+)?function\s+{re.escape(name)}\b",
        rf"(?m)^\s*(?:export\s+)?class\s+{re.escape(name)}\b",
        rf"(?m)^\s*(?:export\s+)?(?:const|let|var)\s+{re.escape(name)}\b",
    )
    for pat in patterns:
        m = re.search(pat, source)
        if m:
            line = source[: m.start()].count("\n") + 1
            # Single-line approx is enough for token outside-span checks.
            return line, line
    return None


def _js_candidates(
    project_root: Path,
    path: str,
    added_lines: Set[int],
) -> List[_SymbolCand]:
    try:
        source = (project_root / path).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return []
    out: List[_SymbolCand] = []
    seen: Set[str] = set()
    export_hits = iter_js_export_symbols(source)
    for line, name in export_hits:
        if name in seen:
            continue
        span = _js_definition_span(source, name)
        if span is not None:
            start, end = span
        else:
            start = end = line
        # Fold export-list lines into the defining span so `export { name }` is
        # not treated as an external reference that clears the symbol.
        for eline, ename in export_hits:
            if ename == name:
                start = min(start, eline)
                end = max(end, eline)
        if line not in added_lines and not (set(range(start, end + 1)) & added_lines):
            continue
        lines = source.splitlines()
        if 0 < line <= len(lines):
            prev_lines = lines[: line - 1]
            if prev_lines:
                prev = prev_lines[-1].strip()
                if prev.startswith("@"):
                    continue
        seen.add(name)
        out.append((path, start, end, name))
    return out


def _build_token_index(
    project_root: Path,
    files: List[str],
    *,
    exclude: Set[str],
) -> tuple[dict[str, Set[str]], dict[str, dict[str, Set[int]]]]:
    """Map identifier token -> files, and token -> file -> line numbers."""
    index: dict[str, Set[str]] = {}
    lines_index: dict[str, dict[str, Set[int]]] = {}
    try:
        for rel in files:
            norm = _norm(rel)
            if norm in exclude:
                continue
            if is_vendored_path(norm):
                continue
            if Path(norm).suffix.lower() not in _CODE_SUFFIXES:
                continue
            path = project_root / norm
            if not path.is_file():
                continue
            try:
                text = path.read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            if path.suffix.lower() == ".py":
                cleaned = strip_string_literals(strip_py_comments(text))
            else:
                cleaned = strip_string_literals(strip_js_comments(text))
            for lineno, line in enumerate(cleaned.splitlines(), 1):
                for tok in _IDENT_RE.findall(line):
                    if len(tok) < 2:
                        continue
                    index.setdefault(tok, set()).add(norm)
                    lines_index.setdefault(tok, {}).setdefault(norm, set()).add(lineno)
    except Exception:
        logger.debug("token index build failed", exc_info=True)
    return index, lines_index


def _symbol_is_referenced(
    name: str,
    path: str,
    start: int,
    end: int,
    token_index: dict[str, Set[str]],
    lines_index: dict[str, dict[str, Set[int]]],
) -> bool:
    """True when ``name`` is used outside its defining span (other file or same-file)."""
    refs = token_index.get(name, set())
    if refs - {path}:
        return True
    same_lines = lines_index.get(name, {}).get(path, set())
    return any(ln < start or ln > end for ln in same_lines)


def detect_dead_symbol_gaps(
    *,
    task: Task,
    project_root: Path,
    diff_content: str,
    next_gap_id: Callable[[str, str], str],
    dead_symbol_enabled: bool = True,
    dead_symbol_blocking: bool = False,
    requirements: Optional[List[Requirement]] = None,
    unwired_files: Optional[Set[str]] = None,
    git_files: Optional[List[str]] = None,
    lsp_refs: Optional[bool] = None,
) -> List[Gap]:
    """Flag diff-added public top-level symbols with zero external references.

    When ``lsp_refs`` is True (or config ``indexing.lsp_refs`` when ``None``),
    token-scan dead candidates are confirmed via the optional live LSP client
    before becoming gaps — external references clear false positives.
    """
    gaps: List[Gap] = []
    if not dead_symbol_enabled or not diff_content:
        return gaps
    try:
        by_file = added_lines_by_file(diff_content)
        if not by_file:
            return gaps

        scaffolding_ok = task_allows_scaffolding(task)
        intent = task_intent_text(task, requirements) if requirements is not None else (
            f"{task.title} {task.description}"
        )
        unwired = {_norm(p) for p in (unwired_files or set())}

        candidates: List[_SymbolCand] = []
        for path, added in by_file.items():
            norm = _norm(path)
            if not is_liveness_code_file(norm) or is_test_path(norm):
                continue
            if norm in unwired:
                continue
            line_nums = {ln for ln, _ in added}
            if Path(norm).suffix.lower() == ".py":
                candidates.extend(_python_candidates(project_root, norm, line_nums))
            else:
                candidates.extend(_js_candidates(project_root, norm, line_nums))

        if not candidates:
            return gaps

        if git_files is None:
            try:
                from devcouncil.indexing.repo_mapper import RepoMapper

                tracked = RepoMapper(project_root).get_git_files()
            except Exception:
                tracked = []
        else:
            tracked = list(git_files)
        token_index, lines_index = _build_token_index(project_root, tracked, exclude=set())

        use_lsp = lsp_refs
        if use_lsp is None:
            try:
                from devcouncil.indexing.lsp_client import lsp_refs_enabled

                use_lsp = lsp_refs_enabled(project_root)
            except Exception:
                use_lsp = False
        lsp_pool = None
        if use_lsp:
            try:
                from devcouncil.indexing.lsp_client import LspSessionPool

                lsp_pool = LspSessionPool(project_root)
            except Exception:
                lsp_pool = None

        try:
            for path, start, end, name in candidates:
                # Intent-text naming = deliberate API addition.
                if re.search(rf"\b{re.escape(name)}\b", intent or ""):
                    continue

                # allow-unwired on the defining line/file with scaffolding parity.
                try:
                    source = (project_root / path).read_text(encoding="utf-8", errors="replace")
                    src_lines = source.splitlines()
                    line_text = src_lines[start - 1] if 0 < start <= len(src_lines) else ""
                except OSError:
                    line_text = ""
                    source = ""
                if ALLOW_UNWIRED in line_text or ALLOW_UNWIRED in source:
                    gaps.append(Gap(
                        id=next_gap_id(task.id, "DEADDECL"),
                        severity="medium",
                        gap_type="dead_symbol",
                        task_id=task.id,
                        description=(
                            f"Intentional unused symbol `{name}` declared at {path}:{start}."
                        ),
                        evidence=[f"{path}:{start}", f"symbol:{name}", ALLOW_UNWIRED],
                        recommended_fix=(
                            f"Wire `{name}` into its caller when scaffolding is complete, "
                            "or remove it."
                        ),
                        blocking=False,
                        file=path,
                        line=start,
                    ))
                    if scaffolding_ok:
                        continue

                if _symbol_is_referenced(name, path, start, end, token_index, lines_index):
                    continue

                if lsp_pool is not None:
                    try:
                        confirmed = lsp_pool.confirm_unreferenced(path, start, name)
                        if confirmed is False:
                            continue
                    except Exception:
                        logger.debug("LSP dead-symbol confirm failed for %s", name, exc_info=True)

                gaps.append(Gap(
                    id=next_gap_id(task.id, "DEAD"),
                    severity="high" if dead_symbol_blocking else "medium",
                    gap_type="dead_symbol",
                    task_id=task.id,
                    description=(
                        f"New public symbol `{name}` at {path}:{start} is never referenced "
                        "outside its own definition."
                    ),
                    evidence=[f"{path}:{start}", f"symbol:{name}"],
                    recommended_fix=(
                        f"Call or register `{name}` from the code that needs it "
                        f"(use `dev scope update {task.id} --lease-token <token> "
                        f"--planned-file <caller>` if the caller is out of scope), or remove it."
                    ),
                    blocking=dead_symbol_blocking,
                    file=path,
                    line=start,
                ))
        finally:
            if lsp_pool is not None:
                lsp_pool.close()
    except Exception:
        logger.debug("detect_dead_symbol_gaps failed; degrading to zero gaps", exc_info=True)
        return []
    return gaps
