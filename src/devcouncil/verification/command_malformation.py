"""Detect when a verification command failed to run vs. a real test failure."""

from __future__ import annotations

import re
from pathlib import Path
from typing import Optional, Tuple

from devcouncil.domain.evidence import CommandResult

# Signatures that mean the verification command itself could not run (or had
# nothing to run), so its non-zero exit says nothing about whether the
# implementation is correct — a tooling/plan defect, not a code defect.
MALFORMED_COMMAND_SIGNATURES = (
    "syntaxerror",
    "invalid syntax",
    "indentationerror",
    "no module named",
    "can't open file",
    "no such file or directory",
    "file or directory not found",
    "no tests ran",
    "no tests collected",
    "error: not found",
    "is not recognized as an internal or external command",
    "command not found",
    "executable file not found",
    "failed to run command",
    "importerror",
    "modulenotfounderror",
)

UNCONDITIONAL_UNRUNNABLE_SIGNATURES = (
    "syntaxerror",
    "invalid syntax",
    "indentationerror",
    "can't open file",
    "is not recognized as an internal or external command",
    "command not found",
    "executable file not found",
    "failed to run command",
    "no tests ran",
    "no tests collected",
    "error: not found",
)

PYTEST_NONRUN_EXIT_CODES = {4, 5}

TRACEBACK_FRAME_RE = re.compile(r'File "(?P<file>[^"]+)", line (?P<line>\d+)')


def is_traceback_frame(line: str) -> bool:
    """True for a Python traceback frame line: ``  File "...", line N``."""
    stripped = line.strip()
    return stripped.startswith('File "') and ", line " in stripped


def malformed_signature_precedes_traceback(text: str) -> bool:
    """Decide whether an unrunnable-launcher signature is authoritative."""
    if not text:
        return False
    low_all = text.lower()
    if any(sig in low_all for sig in UNCONDITIONAL_UNRUNNABLE_SIGNATURES):
        return True
    lines = text.splitlines()
    lowered_lines = [ln.lower() for ln in lines]
    first_frame_idx: Optional[int] = None
    for idx, line in enumerate(lines):
        if is_traceback_frame(line):
            first_frame_idx = idx
            break
    for idx, low in enumerate(lowered_lines):
        if any(sig in low for sig in MALFORMED_COMMAND_SIGNATURES):
            if first_frame_idx is None or idx < first_frame_idx:
                return True
            return False
    return False


def launcher_text(result: CommandResult) -> str:
    """Captured output for launcher-vs-test analysis, ordered stderr then stdout."""
    parts: list[str] = []
    for path in (result.stderr_path, result.stdout_path):
        if not path:
            continue
        try:
            content = Path(path).read_text(encoding="utf-8", errors="replace")
            if content.strip():
                parts.append(content)
        except Exception:
            pass
    if parts:
        return "\n".join(parts)
    return result.summary or ""


def relativize(project_root: Path, raw_path: str) -> str:
    """Normalize a traceback file path to a repo-relative posix path when possible."""
    normalized = raw_path.replace("\\", "/")
    try:
        candidate = Path(raw_path)
        if candidate.is_absolute():
            rel = candidate.resolve().relative_to(project_root.resolve())
            return rel.as_posix()
    except Exception:
        pass
    return normalized


def failure_location(project_root: Path, result: CommandResult) -> Tuple[Optional[str], Optional[int]]:
    """Best-effort (file, line) of a failing command's deepest traceback frame."""
    sources: list[str] = []
    for path in (result.stdout_path, result.stderr_path):
        if path:
            try:
                content = Path(path).read_text(encoding="utf-8", errors="replace")
                if content.strip():
                    sources.append(content)
            except Exception:
                pass
    sources.append(result.summary or "")
    best_file: Optional[str] = None
    best_line: Optional[int] = None
    for text in sources:
        for match in TRACEBACK_FRAME_RE.finditer(text):
            raw_file = match.group("file")
            if not raw_file or raw_file.startswith("<"):
                continue
            best_file = relativize(project_root, raw_file)
            try:
                best_line = int(match.group("line"))
            except ValueError:
                best_line = None
        if best_file is not None:
            return best_file, best_line
    return best_file, best_line


def command_is_malformed(result: CommandResult) -> bool:
    """True when a non-zero exit reflects a broken/unrunnable command."""
    if result.timed_out:
        return False
    is_pytest = "pytest" in (result.command or "")
    if is_pytest and result.exit_code in PYTEST_NONRUN_EXIT_CODES:
        return True
    text = launcher_text(result)
    return malformed_signature_precedes_traceback(text)
