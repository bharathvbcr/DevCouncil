"""Heuristic doc→code reference gate for changed documentation."""

from __future__ import annotations

import logging
import re
from pathlib import Path
from typing import Callable, List

from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task

logger = logging.getLogger(__name__)

_CODE_REF_RE = re.compile(
    r"(?<![`\w])(?:src|tests|docs)/[\w./-]+\.(?:py|md|ts|tsx|js|jsx|json|yaml|yml|toml)"
)


def _norm(path: str) -> str:
    return path.replace("\\", "/").lstrip("./")


def _broken_refs_in_text(project_root: Path, text: str) -> List[str]:
    broken: List[str] = []
    for match in _CODE_REF_RE.finditer(text):
        ref = match.group(0)
        if not (project_root / ref).is_file():
            broken.append(ref)
    return broken


def detect_doc_code_ref_gaps(
    *,
    task: Task,
    project_root: Path,
    changed_files: List[str],
    diff_content: str,
    next_gap_id: Callable[[str, str], str],
    doc_code_ref_enabled: bool = True,
    doc_code_ref_blocking: bool = False,
) -> List[Gap]:
    if not doc_code_ref_enabled:
        return []
    gaps: List[Gap] = []
    try:
        doc_paths = sorted(
            p for p in changed_files
            if _norm(p).endswith(".md") or _norm(p).startswith("docs/")
        )
        for rel in doc_paths:
            path = project_root / rel
            if not path.is_file():
                continue
            try:
                text = path.read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            broken = _broken_refs_in_text(project_root, text)
            if not broken:
                continue
            gaps.append(Gap(
                id=next_gap_id(task.id, f"DOCREF-{rel}"),
                severity="high" if doc_code_ref_blocking else "medium",
                gap_type="doc_code_ref",
                task_id=task.id,
                description=(
                    f"Changed doc `{rel}` references missing code paths: "
                    f"{', '.join(broken[:5])}"
                    + (" …" if len(broken) > 5 else "")
                ),
                evidence=[rel, *broken[:8]],
                recommended_fix="Fix or remove broken code references in the changed documentation.",
                blocking=doc_code_ref_blocking,
                file=rel,
            ))
    except Exception:
        logger.debug("detect_doc_code_ref_gaps failed", exc_info=True)
    return gaps
