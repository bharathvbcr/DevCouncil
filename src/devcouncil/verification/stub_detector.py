"""Stub/TODO detection over the lines a task's diff *added*.

Catches the classic lazy-agent moves before they reach "verified": placeholder
bodies (``pass`` / ``...`` / ``raise NotImplementedError``), TODO/FIXME markers,
neutered or skipped tests, assert-free test functions, and per-language "not
implemented" idioms. Scanning is restricted to lines the diff added, so
pre-existing debt in the repo never blocks a task that did not touch it.

Escape hatch: a line containing ``devcouncil: allow-stub`` suppresses stub
findings on that line **only when the task description mentions scaffolding**
(see :func:`task_allows_scaffolding`). Every newly added allow-stub marker is
still surfaced separately via :func:`detect_stub_declarations` so a human can
audit intentional placeholders — agents cannot silently blanket-suppress stubs.
"""

from __future__ import annotations

import ast
import logging
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, List, Tuple

from devcouncil.domain.task import Task

logger = logging.getLogger(__name__)

ALLOW_MARKER = "devcouncil: allow-stub"

# Documentation files: TODO lists in docs are normal writing, not lazy code.
_DOC_SUFFIXES = {".md", ".rst", ".txt"}

_SCAFFOLDING_RE = re.compile(r"\bscaffold(?:ing)?\b", re.IGNORECASE)

# Shouty markers are matched case-sensitively so prose like "hack" in a comment
# does not fire; ``stub`` and the explicit "not implemented"/"implement later"
# phrases are matched case-insensitively.
_SHOUTY_MARKER_RE = re.compile(r"\b(TODO|FIXME|XXX|HACK)\b")
_STUB_MARKER_RE = re.compile(r"\bstub\b", re.IGNORECASE)
_PHRASE_MARKER_RE = re.compile(r"\b(not implemented|implement (?:this|later|me))\b", re.IGNORECASE)

# Comment delimiters used to isolate the COMMENT portion of a line. The bare
# ``stub`` word is only a laziness signal when a human wrote it as an annotation
# ("# stub implementation for now") — in CODE it is a legitimate identifier
# (``class StubProvider``, ``def stub_response()``, test doubles) and flagging it
# produced blocking false positives on hard tasks. Shouty TODO/FIXME markers and
# the explicit "not implemented" phrases still match anywhere on the line.
_COMMENT_DELIMS = ("#", "//", "/*", "--", ";;", "<!--")


def _comment_segment(text: str) -> str:
    """The portion of ``text`` from the first comment delimiter onward ('' if none)."""
    first = -1
    for delim in _COMMENT_DELIMS:
        idx = text.find(delim)
        if idx != -1 and (first == -1 or idx < first):
            first = idx
    return text[first:] if first != -1 else ""

# Commented-out assert lines in test files — neutered verification.
_COMMENTED_ASSERT_RE = re.compile(r"^\s*#+\s*assert\b", re.IGNORECASE)

# Empty exported function bodies added in JS/TS/Go (added lines only).
_EMPTY_EXPORTED_FN_RE = re.compile(
    r"^\s*(?:export\s+)?(?:async\s+)?function\s+\w+\s*\([^)]*\)\s*\{\s*\}\s*;?\s*$"
)

_SKIPPED_TEST_RES: Tuple[Tuple[re.Pattern, str], ...] = (
    (re.compile(r"@pytest\.mark\.skip\b"), "pytest test skipped"),
    (re.compile(r"@pytest\.mark\.skipif\(\s*True"), "pytest test skipped unconditionally"),
    (re.compile(r"@unittest\.skip\b"), "unittest test skipped"),
    (re.compile(r"\b(?:it|test|describe)\.skip\s*\("), "JS test skipped"),
    (re.compile(r"\bx(?:it|describe|test)\s*\("), "JS test disabled (xit/xdescribe)"),
    (re.compile(r"assert\s+True\s*(#.*(placeholder|stub|todo).*)?$", re.IGNORECASE), "placeholder assertion (assert True)"),
)

_LANG_STUB_RES: Tuple[Tuple[re.Pattern, str], ...] = (
    (re.compile(r"throw\s+new\s+Error\(\s*['\"](?:not implemented|todo|unimplemented)", re.IGNORECASE), "JS/TS not-implemented throw"),
    (re.compile(r"\btodo!\s*\("), "Rust todo!()"),
    (re.compile(r"\bunimplemented!\s*\("), "Rust unimplemented!()"),
    (re.compile(r"panic\(\s*[\"'](?:TODO|not implemented|unimplemented)", re.IGNORECASE), "Go not-implemented panic"),
    (re.compile(r"raise\s+NotImplementedError\b"), "NotImplementedError raised"),
)

# JS/TS assert-free test: test/it blocks with no expect/assert.
_JS_ASSERT_FREE_RE = re.compile(
    r"^\s*(?:it|test)\s*\([^)]*\)\s*(?:=>)?\s*\{[^}]*\}\s*;?\s*$"
)

_HUNK_RE = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@")


@dataclass
class StubFinding:
    file: str
    line: int
    reason: str
    snippet: str


def task_allows_scaffolding(task: Task | None) -> bool:
    """True when the task explicitly declares scaffolding work."""
    if task is None:
        return False
    text = f"{task.title} {task.description}"
    return bool(_SCAFFOLDING_RE.search(text))


def added_lines_by_file(diff_content: str) -> Dict[str, List[Tuple[int, str]]]:
    """Parse a unified diff into ``{new_path: [(new_lineno, added_text), ...]}``.

    Only ``+`` lines are collected. Deleted files (``+++ /dev/null``) are skipped.
    Tolerant of malformed hunks — it never raises.
    """
    out: Dict[str, List[Tuple[int, str]]] = {}
    current: str | None = None
    lineno = 0
    in_hunk = False
    for raw in diff_content.splitlines():
        if raw.startswith("+++ "):
            target = raw[4:].strip()
            if target == "/dev/null":
                current = None
            else:
                current = target[2:] if target.startswith(("a/", "b/")) else target
            in_hunk = False
            continue
        if raw.startswith("@@"):
            m = _HUNK_RE.match(raw)
            if m and current is not None:
                lineno = int(m.group(1))
                in_hunk = True
            else:
                in_hunk = False
            continue
        if not in_hunk or current is None:
            continue
        if raw.startswith("+"):
            out.setdefault(current, []).append((lineno, raw[1:]))
            lineno += 1
        elif raw.startswith("-"):
            continue
        elif raw.startswith("\\"):  # "\ No newline at end of file"
            continue
        else:
            lineno += 1
    return out


def detect_stub_declarations(diff_content: str) -> List[StubFinding]:
    """Every newly added ``devcouncil: allow-stub`` marker (always advisory)."""
    findings: List[StubFinding] = []
    try:
        by_file = added_lines_by_file(diff_content)
    except Exception:  # pragma: no cover
        return []
    for path, added in by_file.items():
        if Path(path).suffix.lower() in _DOC_SUFFIXES:
            continue
        for lineno, text in added:
            if ALLOW_MARKER in text:
                findings.append(StubFinding(
                    path, lineno,
                    "intentional stub declared (allow-stub)",
                    text.strip()[:160],
                ))
    return findings


def _scan_added_line(
    path: str,
    lineno: int,
    text: str,
    *,
    honor_allow_stub: bool,
) -> List[StubFinding]:
    findings: List[StubFinding] = []
    if honor_allow_stub and ALLOW_MARKER in text:
        return findings
    snippet = text.strip()[:160]
    is_testish = "test" in Path(path).name.lower() or "/tests/" in f"/{path}"
    # ``stub`` only counts as a marker inside a comment, and never in test files —
    # test doubles ("stub provider") are standard practice, not laziness. TODO/FIXME
    # and "not implemented" phrases keep matching anywhere.
    stub_marker = not is_testish and _STUB_MARKER_RE.search(_comment_segment(text))
    if _SHOUTY_MARKER_RE.search(text) or stub_marker or _PHRASE_MARKER_RE.search(text):
        findings.append(StubFinding(path, lineno, "TODO/placeholder marker in added code", snippet))
    for pattern, reason in _LANG_STUB_RES:
        if pattern.search(text):
            findings.append(StubFinding(path, lineno, reason, snippet))
    if is_testish:
        for pattern, reason in _SKIPPED_TEST_RES:
            if pattern.search(text):
                findings.append(StubFinding(path, lineno, reason, snippet))
        if _COMMENTED_ASSERT_RE.search(text):
            findings.append(StubFinding(path, lineno, "commented-out assert in test", snippet))
        # Single-line JS test with empty body and no expect/assert.
        if path.endswith((".js", ".ts", ".jsx", ".tsx", ".mjs", ".cjs")):
            lowered = text.lower()
            if ("test(" in lowered or "it(" in lowered) and "expect(" not in lowered and "assert" not in lowered:
                if _JS_ASSERT_FREE_RE.search(text):
                    findings.append(StubFinding(path, lineno, "test with no assertions", snippet))
    if _EMPTY_EXPORTED_FN_RE.search(text):
        findings.append(StubFinding(path, lineno, "empty exported function body", snippet))
    return findings


def _function_has_assertions(node: ast.AST) -> bool:
    """Return True if the function body contains any verification statement."""
    for child in ast.walk(node):
        if isinstance(child, ast.Assert):
            return True
        if isinstance(child, ast.Call):
            func = child.func
            if isinstance(func, ast.Attribute):
                name = func.attr
                if name in {"assertEqual", "assertTrue", "assertFalse", "assertRaises", "assertIn", "assertNotIn"}:
                    return True
                if name == "raises" and isinstance(func.value, ast.Name) and func.value.id == "pytest":
                    return True
            if isinstance(func, ast.Name) and func.id == "raises":
                return True
    return False


def _is_test_function(node: ast.FunctionDef | ast.AsyncFunctionDef) -> bool:
    if node.name.startswith("test_"):
        return True
    for dec in node.decorator_list:
        target = dec
        if isinstance(dec, ast.Call):
            target = dec.func
        if isinstance(target, ast.Attribute) and target.attr in {"test", "parametrize"}:
            return True
        if isinstance(target, ast.Name) and target.id in {"pytest_mark", "unittest"}:
            return True
    return False


def _python_assert_free_tests(
    project_root: Path,
    rel_path: str,
    added: List[Tuple[int, str]],
) -> List[StubFinding]:
    """Flag added/changed test functions whose body contains no assertions."""
    full = project_root / rel_path
    try:
        source = full.read_text(encoding="utf-8", errors="replace")
        tree = ast.parse(source)
    except Exception:
        return []
    added_linenos = {ln for ln, _ in added}
    source_lines = source.splitlines()
    findings: List[StubFinding] = []
    for node in ast.walk(tree):
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        if not _is_test_function(node):
            continue
        end = getattr(node, "end_lineno", node.lineno)
        span = set(range(node.lineno, end + 1))
        if not (span & added_linenos):
            continue
        if any(
            0 <= ln - 1 < len(source_lines) and ALLOW_MARKER in source_lines[ln - 1]
            for ln in span
        ):
            continue
        if _function_has_assertions(node):
            continue
        # Docstring-only bodies with no asserts still count as assert-free.
        findings.append(StubFinding(
            rel_path, node.lineno,
            f"test function '{node.name}' has no assertions",
            f"def {node.name}(...)",
        ))
    return findings


def _python_stub_bodies(
    project_root: Path,
    rel_path: str,
    added: List[Tuple[int, str]],
    *,
    honor_allow_stub: bool,
) -> List[StubFinding]:
    """AST pass: flag functions whose (added) body is only pass/.../docstring."""
    full = project_root / rel_path
    try:
        source = full.read_text(encoding="utf-8", errors="replace")
        tree = ast.parse(source)
    except Exception:
        return []
    added_linenos = {ln for ln, _ in added}
    source_lines = source.splitlines()
    findings: List[StubFinding] = []
    for node in ast.walk(tree):
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        body = node.body
        stmts = list(body)
        if stmts and isinstance(stmts[0], ast.Expr) and isinstance(getattr(stmts[0], "value", None), ast.Constant) \
                and isinstance(stmts[0].value.value, str):
            stmts = stmts[1:]  # drop docstring
        if not stmts:
            trivial = True
        else:
            trivial = all(
                isinstance(s, ast.Pass)
                or (isinstance(s, ast.Expr) and isinstance(s.value, ast.Constant) and s.value.value is Ellipsis)
                for s in stmts
            )
        if not trivial:
            continue
        end = getattr(node, "end_lineno", node.lineno)
        span = set(range(node.lineno, end + 1))
        if not (span & added_linenos):
            continue
        if honor_allow_stub and any(
            0 <= ln - 1 < len(source_lines) and ALLOW_MARKER in source_lines[ln - 1]
            for ln in span
        ):
            continue
        findings.append(StubFinding(
            rel_path, node.lineno,
            f"function '{node.name}' has a placeholder body (pass/...)",
            f"def {node.name}(...)",
        ))
    return findings


def detect_stubs(
    project_root: Path,
    diff_content: str,
    *,
    honor_allow_stub: bool = False,
) -> List[StubFinding]:
    """All stub/TODO findings in the diff's added lines. Never raises."""
    try:
        by_file = added_lines_by_file(diff_content)
    except Exception:  # pragma: no cover - defensive
        logger.debug("stub detection: diff parse failed", exc_info=True)
        return []
    findings: List[StubFinding] = []
    for path, added in by_file.items():
        if Path(path).suffix.lower() in _DOC_SUFFIXES:
            continue
        for lineno, text in added:
            findings.extend(_scan_added_line(path, lineno, text, honor_allow_stub=honor_allow_stub))
        if path.endswith(".py"):
            try:
                findings.extend(_python_stub_bodies(
                    project_root, path, added, honor_allow_stub=honor_allow_stub,
                ))
                findings.extend(_python_assert_free_tests(project_root, path, added))
            except Exception:  # pragma: no cover - defensive
                logger.debug("stub detection: AST pass failed for %s", path, exc_info=True)
    seen = set()
    unique: List[StubFinding] = []
    for f in findings:
        key = (f.file, f.line, f.reason)
        if key in seen:
            continue
        seen.add(key)
        unique.append(f)
    return unique
