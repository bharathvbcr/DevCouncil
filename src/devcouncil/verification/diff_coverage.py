"""Diff↔coverage intersection — proof that the *changed lines* were exercised.

DevCouncil's headline promise is that a passing test must prove the **new logic
was exercised**, not merely that *some* suite exited 0. An agent can make a green
suite pass while the changed code is never imported, never called, or shadowed by
an unrelated passing test. This module closes that gap: it runs a task's test
command under coverage instrumentation, then intersects the lines the tests
actually executed with the lines the diff changed.

Two failure shapes are caught:

1. **Touched-but-not-exercised** — the changed file *is* in the coverage report,
   but the changed executable lines were never executed (e.g. a passing test that
   exercises a different branch).
2. **Never-imported** — the changed source file is *absent* from the coverage
   report entirely, meaning the tests never loaded it.

False-positive discipline (mirrors :class:`~devcouncil.verification.verifier.Verifier`):
this analysis only ever produces a *signal* when it has reliable data — a
parseable diff with real hunks, a detected coverage tool, and changed *executable*
lines to measure. When any of those is missing it returns
``DiffCoverageResult(measured=False, ...)`` and the verifier degrades to its prior
behaviour rather than blocking correct work. Coverage is read from the **target
repository's** own tooling (coverage.py for Python); DevCouncil never forces its
own coverage dependency into the project under verification.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Optional, Set

# A unified-diff hunk header: ``@@ -<old> +<newStart>[,<newLen>] @@``. We only need
# the new-file starting line to walk added/context lines into new-file numbers.
_HUNK_RE = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@")

# Path fragments that mark a file as test code. Test files are excluded from the
# "must be exercised" denominator: a test exercising itself is not the new logic
# whose behaviour an acceptance criterion is about.
_TEST_MARKERS = (
    "/tests/",
    "tests/",
    "/test_",
    "test_",
    "_test.",
    ".test.",
    ".spec.",
    "_spec.",
    "/spec/",
)


def _strip_diff_prefix(path: str) -> str:
    """Strip a leading ``a/`` or ``b/`` and normalise to forward slashes."""
    path = path.strip().strip('"')
    if path.startswith(("a/", "b/")):
        path = path[2:]
    return path.replace("\\", "/")


def is_test_path(path: str) -> bool:
    lowered = path.replace("\\", "/").lower()
    return any(marker in lowered for marker in _TEST_MARKERS)


def is_code_like(line: str) -> bool:
    """Conservative heuristic: a non-blank line that is not a pure comment.

    Used only for changed files that are *absent* from the coverage report, where
    no authoritative executable-line set exists. Imports, ``def``/``class``,
    decorators and statements all count; blank lines and ``#`` comments do not.
    Kept deliberately conservative so it never inflates the denominator.
    """
    stripped = line.strip()
    return bool(stripped) and not stripped.startswith("#")


def parse_changed_lines(diff: str) -> Dict[str, Dict[int, str]]:
    """Parse a unified diff into ``{file_path: {new_line_number: added_text}}``.

    Only *added* lines (``+`` in the new file) are recorded, keyed by their line
    number in the post-change file so they can be intersected with coverage data.
    Deletions and context lines advance the counter but are not themselves
    "changed lines" we require coverage for. Paths are normalised relative to the
    repo (``a/``/``b/`` prefixes stripped, forward slashes).
    """
    changed: Dict[str, Dict[int, str]] = {}
    current_file: Optional[str] = None
    new_line = 0
    in_hunk = False

    for raw in diff.splitlines():
        if raw.startswith("diff --git") or raw.startswith("--- "):
            in_hunk = False
            continue
        if raw.startswith("+++ "):
            target = raw[4:].strip()
            if target == "/dev/null":
                current_file = None
            else:
                current_file = _strip_diff_prefix(target)
                changed.setdefault(current_file, {})
            in_hunk = False
            continue
        if raw.startswith("@@"):
            match = _HUNK_RE.match(raw)
            if match:
                new_line = int(match.group(1))
                in_hunk = True
            else:
                # A header we can't number (e.g. a combined-merge ``@@@``). Stay out
                # of hunk mode rather than mis-attribute added lines to line 0.
                in_hunk = False
            continue
        if not in_hunk or current_file is None:
            continue
        if raw.startswith("\\"):  # "\ No newline at end of file"
            continue
        if raw.startswith("+"):
            changed[current_file][new_line] = raw[1:]
            new_line += 1
        elif raw.startswith("-"):
            continue  # old-file only; does not advance the new-file counter
        else:
            new_line += 1  # context line

    return {path: lines for path, lines in changed.items() if lines}


def parse_coverage_json(data: dict, root: Path) -> "CoverageData":
    """Extract executed and executable lines per file from ``coverage json`` output.

    ``coverage.py`` reports ``executed_lines`` and ``missing_lines`` per file; their
    union is the set of statements coverage knows are executable. Paths are
    normalised to repo-relative POSIX so they intersect with diff paths.
    """
    executed: Dict[str, Set[int]] = {}
    executable: Dict[str, Set[int]] = {}
    files = data.get("files", {}) if isinstance(data, dict) else {}
    for raw_path, payload in files.items():
        if not isinstance(payload, dict):
            continue
        rel = _relativize(raw_path, root)
        if rel is None:
            continue
        run = {int(n) for n in payload.get("executed_lines", []) or []}
        miss = {int(n) for n in payload.get("missing_lines", []) or []}
        executed[rel] = run
        executable[rel] = run | miss
    return CoverageData(executed=executed, executable=executable)


def _relativize(raw_path: str, root: Path) -> Optional[str]:
    candidate = Path(raw_path)
    try:
        if candidate.is_absolute():
            rel = candidate.resolve().relative_to(root.resolve())
        else:
            rel = candidate
    except ValueError:
        # Outside the repo (site-packages, stdlib) — not a changed-file candidate.
        return None
    return rel.as_posix()


@dataclass
class CoverageData:
    executed: Dict[str, Set[int]] = field(default_factory=dict)
    executable: Dict[str, Set[int]] = field(default_factory=dict)


@dataclass
class DiffCoverageResult:
    """Outcome of intersecting changed lines with executed lines.

    ``measured`` is True only when there was a meaningful signal to compute — at
    least one changed *executable* line. When False, callers must NOT treat the
    result as evidence of a problem (false-positive discipline).
    """

    measured: bool
    tool: str = ""
    reason: str = ""
    changed_executable_lines: int = 0
    covered_changed_lines: int = 0
    uncovered_by_file: Dict[str, List[int]] = field(default_factory=dict)
    absent_files: List[str] = field(default_factory=list)

    @property
    def ratio(self) -> float:
        if self.changed_executable_lines == 0:
            return 1.0
        return self.covered_changed_lines / self.changed_executable_lines

    def summary(self) -> str:
        if not self.measured:
            return f"diff coverage not measured ({self.reason})" if self.reason else "diff coverage not measured"
        pct = round(self.ratio * 100)
        base = (
            f"{self.covered_changed_lines}/{self.changed_executable_lines} changed lines exercised "
            f"({pct}%) via {self.tool or 'coverage'}"
        )
        if self.absent_files:
            base += f"; not imported by tests: {', '.join(self.absent_files)}"
        return base


def intersect(
    changed: Dict[str, Dict[int, str]],
    coverage: CoverageData,
    *,
    tool: str = "coverage.py",
) -> DiffCoverageResult:
    """Intersect changed lines with executed lines to measure diff coverage.

    ``changed`` should already be filtered to measurable source files (e.g. ``.py``
    non-test files). For each file present in the coverage report we use coverage's
    authoritative executable-line set; for changed source files *absent* from the
    report (never imported) we fall back to the conservative ``is_code_like``
    heuristic and count those added lines as executable-but-uncovered.
    """
    total_executable = 0
    total_covered = 0
    uncovered_by_file: Dict[str, List[int]] = {}
    absent_files: List[str] = []

    for path, line_map in changed.items():
        changed_nums = set(line_map.keys())
        if path in coverage.executable:
            file_executable = changed_nums & coverage.executable[path]
            file_covered = changed_nums & coverage.executed.get(path, set())
            total_executable += len(file_executable)
            total_covered += len(file_covered)
            missing = sorted(file_executable - file_covered)
            if missing:
                uncovered_by_file[path] = missing
        else:
            # Absent from the coverage report -> the tests never loaded this file.
            code_like = sorted(num for num, text in line_map.items() if is_code_like(text))
            if code_like:
                total_executable += len(code_like)
                uncovered_by_file[path] = code_like
                absent_files.append(path)

    if total_executable == 0:
        return DiffCoverageResult(
            measured=False,
            tool=tool,
            reason="no changed executable lines to measure",
        )

    return DiffCoverageResult(
        measured=True,
        tool=tool,
        changed_executable_lines=total_executable,
        covered_changed_lines=total_covered,
        uncovered_by_file=uncovered_by_file,
        absent_files=absent_files,
    )


def measurable_python_changes(changed: Dict[str, Dict[int, str]]) -> Dict[str, Dict[int, str]]:
    """Filter parsed diff lines to Python source files (non-test) coverage.py can measure."""
    return {
        path: lines
        for path, lines in changed.items()
        if path.endswith(".py") and not is_test_path(path)
    }


_JS_SUFFIXES = (".js", ".ts", ".jsx", ".tsx", ".mjs", ".cjs")
_GO_SUFFIX = ".go"


def measurable_js_changes(changed: Dict[str, Dict[int, str]]) -> Dict[str, Dict[int, str]]:
    """Filter to JS/TS source files c8/nyc can instrument."""
    return {
        path: lines
        for path, lines in changed.items()
        if path.endswith(_JS_SUFFIXES) and not is_test_path(path)
    }


def measurable_go_changes(changed: Dict[str, Dict[int, str]]) -> Dict[str, Dict[int, str]]:
    """Filter to Go source files ``go test -coverprofile`` can measure."""
    return {
        path: lines
        for path, lines in changed.items()
        if path.endswith(_GO_SUFFIX) and not is_test_path(path)
    }


def parse_istanbul_json(data: dict, root: Path) -> CoverageData:
    """Parse c8/nyc ``coverage-final.json`` into :class:`CoverageData`."""
    executed: Dict[str, Set[int]] = {}
    executable: Dict[str, Set[int]] = {}
    if not isinstance(data, dict):
        return CoverageData()
    for raw_path, payload in data.items():
        if not isinstance(payload, dict):
            continue
        rel = _relativize(raw_path, root) or _relativize(payload.get("path", raw_path), root)
        if rel is None:
            continue
        stmt_map = payload.get("statementMap") or {}
        hits = payload.get("s") or {}
        run: Set[int] = set()
        all_lines: Set[int] = set()
        for sid, meta in stmt_map.items():
            if not isinstance(meta, dict):
                continue
            start = meta.get("start") or {}
            line = int(start.get("line", 0) or 0)
            if line <= 0:
                continue
            all_lines.add(line)
            if int(hits.get(sid, 0) or 0) > 0:
                run.add(line)
        if all_lines:
            executed[rel] = run
            executable[rel] = all_lines
    return CoverageData(executed=executed, executable=executable)


def parse_go_coverprofile(text: str, root: Path) -> CoverageData:
    """Parse ``go tool cover`` profile text into :class:`CoverageData`."""
    executed: Dict[str, Set[int]] = {}
    executable: Dict[str, Set[int]] = {}
    for raw in text.splitlines():
        if not raw or raw.startswith("mode:"):
            continue
        parts = raw.split()
        if len(parts) < 3:
            continue
        loc, count_str = parts[0], parts[2]
        try:
            hit = int(count_str)
        except ValueError:
            continue
        file_part = loc.split(":")[0]
        rel = _relativize(file_part, root)
        if rel is None:
            # go profiles often use module paths; try basename match later via intersect.
            rel = file_part.replace("\\", "/")
        span = loc.split(":")[-1] if ":" in loc else ""
        if "," not in span:
            continue
        start = span.split(",")[0]
        try:
            line = int(start.split(".")[0])
        except ValueError:
            continue
        executable.setdefault(rel, set()).add(line)
        if hit > 0:
            executed.setdefault(rel, set()).add(line)
    return CoverageData(executed=executed, executable=executable)


def merge_diff_coverage_results(results: List[DiffCoverageResult]) -> DiffCoverageResult:
    """Combine per-language measurements into one result."""
    measured = [r for r in results if r.measured]
    if not measured:
        reason = results[0].reason if results else "no measurable source changes in diff"
        return DiffCoverageResult(measured=False, reason=reason)
    total_exec = sum(r.changed_executable_lines for r in measured)
    total_cov = sum(r.covered_changed_lines for r in measured)
    uncovered: Dict[str, List[int]] = {}
    absent: List[str] = []
    tools: List[str] = []
    for r in measured:
        if r.tool:
            tools.append(r.tool)
        for path, lines in r.uncovered_by_file.items():
            uncovered.setdefault(path, []).extend(lines)
        absent.extend(r.absent_files)
    for path in uncovered:
        uncovered[path] = sorted(set(uncovered[path]))
    return DiffCoverageResult(
        measured=True,
        tool="+".join(dict.fromkeys(tools)) or "coverage",
        changed_executable_lines=total_exec,
        covered_changed_lines=total_cov,
        uncovered_by_file=uncovered,
        absent_files=sorted(set(absent)),
    )


def c8_run_argv(command_argv: List[str], *, reports_dir: str) -> Optional[List[str]]:
    """Wrap a Node/npm test command with c8 instrumentation."""
    if not command_argv:
        return None
    head = Path(command_argv[0]).name.lower()
    if head.endswith(".exe"):
        head = head[:-4]
    # npx c8 ... OR node/node_modules/.bin/c8
    if head in {"npx", "pnpm", "yarn"}:
        return [command_argv[0], "c8", "--reporter=json", f"--reports-dir={reports_dir}", *command_argv[1:]]
    if head in {"npm", "node"}:
        return ["npx", "c8", "--reporter=json", f"--reports-dir={reports_dir}", *command_argv]
    return ["npx", "c8", "--reporter=json", f"--reports-dir={reports_dir}", *command_argv]


def go_cover_run_argv(command_argv: List[str], profile_path: str) -> Optional[List[str]]:
    """Inject ``-coverprofile`` into a ``go test`` invocation."""
    if not command_argv:
        return None
    head = Path(command_argv[0]).name.lower()
    if head.endswith(".exe"):
        head = head[:-4]
    if head != "go":
        return None
    args = list(command_argv)
    if "test" not in args:
        return None
    if "-coverprofile" in args:
        return args
    # Insert after ``go test``.
    idx = args.index("test") + 1
    return [*args[:idx], f"-coverprofile={profile_path}", *args[idx:]]


def coverage_run_argv(
    command_argv: List[str],
    python: str,
    *,
    append: bool,
    data_file: str,
    source: str = ".",
) -> Optional[List[str]]:
    """Transform a test command's argv into a ``coverage run`` invocation.

    Supports the common Python entry points. Returns ``None`` for commands that
    cannot be instrumented (the caller then leaves diff coverage unmeasured rather
    than guessing). ``python -c "<code>"`` is handled separately by the caller
    because it must materialise a temp script first.
    """
    if not command_argv:
        return None

    prefix = [python, "-m", "coverage", "run", f"--source={source}", f"--data-file={data_file}"]
    if append:
        prefix.append("-a")

    head = Path(command_argv[0]).name.lower()
    if head.endswith(".exe"):  # Windows: python.exe / pytest.exe
        head = head[:-4]
    rest = command_argv[1:]

    # python -m pytest / python -m unittest -> reuse the same module entry point.
    if head in {"python", "python3", "py"} and len(rest) >= 2 and rest[0] == "-m":
        module = rest[1]
        if module in {"pytest", "unittest"}:
            return [*prefix, "-m", module, *rest[2:]]
        return None
    # bare pytest -> run via the module entry point under coverage.
    if head in {"pytest", "py.test"}:
        return [*prefix, "-m", "pytest", *rest]
    return None


def inline_python_code(command_argv: List[str]) -> Optional[str]:
    """Return the ``CODE`` of a ``python -c "CODE"`` command, else None.

    DevCouncil's acceptance compiler and many planner ``expected_tests`` are inline
    assertions (``python -c "import m; assert m.f()==1"``). These are exactly the
    checks whose diff coverage matters, so they are instrumented via a temp script
    (see :func:`coverage_run_script_argv`) rather than left unmeasured.
    """
    if len(command_argv) < 3:
        return None
    head = Path(command_argv[0]).name.lower()
    if head.endswith(".exe"):
        head = head[:-4]
    if head in {"python", "python3", "py"} and command_argv[1] == "-c":
        return command_argv[2]
    return None


def inline_script_content(code: str, root: Path) -> str:
    """Wrap inline ``-c`` code as a script that imports like ``python -c`` would.

    ``python -c`` puts the current working directory on ``sys.path``; a plain script
    instead puts the script's own directory there. Since the temp script lives under
    ``.devcouncil/tmp`` we re-insert the repo root so ``import <module>`` resolves the
    same way the original inline check did.
    """
    return f"import sys\nsys.path.insert(0, {str(Path(root))!r})\n{code}\n"


def coverage_run_script_argv(
    script_path: str,
    python: str,
    *,
    append: bool,
    data_file: str,
    source: str = ".",
) -> List[str]:
    """A ``coverage run`` invocation for a materialised script (used for inline checks)."""
    prefix = [python, "-m", "coverage", "run", f"--source={source}", f"--data-file={data_file}"]
    if append:
        prefix.append("-a")
    return [*prefix, script_path]
