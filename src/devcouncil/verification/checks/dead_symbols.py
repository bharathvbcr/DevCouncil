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
from devcouncil.devmap_client import (
    REACHED,
    REACH_UNKNOWN,
    UNREACHED,
    DevMapClient,
    DevMapClientError,
    SymbolReach,
)
from devcouncil.verification.checks.semantic_diff import task_intent_text
from devcouncil.verification.stub_detector import added_lines_by_file, task_allows_scaffolding

logger = logging.getLogger(__name__)

_IDENT_RE = re.compile(r"\b([A-Za-z_][A-Za-z0-9_]*)\b")
# Intentionally narrow: export/__all__ heuristics are only implemented for py/js.
# Do not widen to Swift/Kotlin until real analyzers exist (map entry seeds still
# populate entry_roots for navigation).
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


def _symbol_reach(project_root: Path, path: str, name: str) -> SymbolReach:
    """Ask the kernel whether anything non-test reaches ``name``.

    Two routes to one answer, in order of authority:

    * ``DevMapClient.symbol_is_reached`` — the live store.
    * ``symbol_has_non_test_inbound`` — ``code_graph.json``, the artifact the
      same kernel writes. Not a second engine; the same answer by another route
      when the store cannot be reached.

    The fallback can only *clear* a symbol. It reads a file that may predate the
    diff being checked, so its silence is not evidence — a stale graph that
    happens not to mention a symbol added five minutes ago would otherwise
    confirm the very finding it cannot speak to. Reached is safe from a stale
    graph (a caller that existed still exists in the tree, or the symbol is
    older than the graph); unreached is not.
    """
    try:
        client = DevMapClient(project_root)
        reach = client.symbol_is_reached(path, name)
    except DevMapClientError as exc:
        reach = SymbolReach(REACH_UNKNOWN, f"kernel unavailable: {exc}")
    except (OSError, ValueError) as exc:
        # Narrow deliberately. A missing binary, an unreadable store or a
        # malformed argument are conditions this function is expected to meet;
        # anything else is a defect here and must surface as one rather than be
        # rendered as "the graph could not confirm it".
        reach = SymbolReach(REACH_UNKNOWN, f"{type(exc).__name__}: {exc}")

    if reach.verdict != REACH_UNKNOWN:
        return reach

    try:
        from devcouncil.indexing.graph.query import symbol_has_non_test_inbound

        if symbol_has_non_test_inbound(project_root, path, name):
            return SymbolReach(REACHED, "code_graph.json inbound edge")
    except (ImportError, OSError, ValueError, KeyError, TypeError) as exc:
        return SymbolReach(REACH_UNKNOWN, f"{reach.detail}; fallback also failed: {exc}")
    return reach


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

        candidates.sort(key=lambda item: (item[0], item[1], item[3]))

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
                        id=next_gap_id(task.id, f"DEADDECL-{path}:{start}:{name}"),
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

                # One query, three answers. `reached` clears the symbol,
                # `unreached` confirms the token scan, and `unknown` means the
                # strongest check did not run — which is neither of the other
                # two and must not be spelled like them.
                reach = _symbol_reach(project_root, path, name)
                if reach.verdict == REACHED:
                    continue
                graph_confirmed = reach.verdict == UNREACHED
                if not graph_confirmed:
                    logger.warning(
                        "dead-symbol graph confirmation unavailable for %s at %s:%s (%s); "
                        "the finding rests on the token scan alone and is reported "
                        "non-blocking",
                        name,
                        path,
                        start,
                        reach.detail,
                    )

                if lsp_pool is not None:
                    try:
                        confirmed = lsp_pool.confirm_unreferenced(path, start, name)
                        if confirmed is False:
                            continue
                    except Exception:
                        logger.debug("LSP dead-symbol confirm failed for %s", name, exc_info=True)

                # An unconfirmed finding is downgraded on every axis at once:
                # severity, blocking, and the sentence itself. Previously only
                # the evidence list differed, so a finding whose strongest
                # check never ran blocked a task with the same weight and the
                # same flat assertion — "is never referenced" — as one the
                # graph confirmed. A gate that cannot tell the caller it failed
                # open is not a gate.
                gaps.append(Gap(
                    id=next_gap_id(task.id, f"DEAD-{path}:{start}:{name}"),
                    severity=("high" if dead_symbol_blocking else "medium") if graph_confirmed else "low",
                    gap_type="dead_symbol",
                    task_id=task.id,
                    description=(
                        f"New public symbol `{name}` at {path}:{start} is never referenced "
                        "outside its own definition."
                        if graph_confirmed
                        else (
                            f"New public symbol `{name}` at {path}:{start} has no reference in a "
                            "token scan, but the call graph could not confirm it — treat this as "
                            "unverified, not as dead code."
                        )
                    ),
                    evidence=(
                        [f"{path}:{start}", f"symbol:{name}"]
                        if graph_confirmed
                        else [
                            f"{path}:{start}",
                            f"symbol:{name}",
                            "graph-confirmation:unavailable",
                        ]
                    ),
                    recommended_fix=(
                        f"Call or register `{name}` from the code that needs it "
                        f"(use `dev scope update {task.id} --lease-token <token> "
                        f"--planned-file <caller>` if the caller is out of scope), or remove it."
                    ),
                    blocking=dead_symbol_blocking and graph_confirmed,
                    file=path,
                    line=start,
                ))
        finally:
            if lsp_pool is not None:
                lsp_pool.close()
    except Exception:
        # Loud, and at the level a reader will actually see. Returning `[]` is
        # indistinguishable from "the gate ran and found nothing", so a gate
        # that degrades silently is a gate that reports `approved` for
        # `unexamined`. It still degrades — a verification run must not be
        # brought down by this check — but it says so, and it says so once per
        # failure rather than at debug level where nothing is listening.
        logger.error(
            "detect_dead_symbol_gaps failed for task %s; the dead-symbol gate did NOT "
            "run and its silence is not a pass",
            getattr(task, "id", "<unknown>"),
            exc_info=True,
        )
        # And say it where a reader will see it. `verify_orchestration` has a
        # `quality_gate_failed` gap for exactly this, guarded by `except
        # Exception` around the call — but this function never raises (its
        # docstring promises as much), so that branch has never fired and the
        # outage reached the log alone. The report said nothing, which reads as
        # "the gate ran and found nothing".
        #
        # Non-blocking: an outage is not evidence of a defect in the diff. But
        # it is evidence the run is incomplete, and that belongs in the result.
        try:
            return [Gap(
                id=next_gap_id(task.id, "QGFAIL-dead_symbol"),
                severity="medium",
                gap_type="quality_gate_failed",
                task_id=task.id,
                description=(
                    "Dead-symbol verification gate crashed; it did not run and its "
                    "silence is not a pass."
                ),
                evidence=["gate:dead_symbol"],
                recommended_fix=(
                    "Re-run verify after fixing the gate error; do not treat this pass "
                    "as proven."
                ),
                blocking=False,
            )]
        except Exception:
            # `next_gap_id` or the task itself may be what failed. Losing the
            # marker is worse than losing nothing, but it is all that is left.
            logger.error("could not even record the dead-symbol gate outage", exc_info=True)
            return []
    return gaps
