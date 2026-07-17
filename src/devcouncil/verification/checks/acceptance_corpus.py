"""Acceptance-criteria ↔ corpus alignment gate (soft by default)."""

from __future__ import annotations

import logging
import re
from pathlib import Path
from typing import Callable, List, Optional, Set

from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task

logger = logging.getLogger(__name__)

_DOC_PATH_RE = re.compile(
    r"(?<![`\w])(?:docs/[\w./-]+\.(?:md|rst|txt)|README(?:\.md)?)(?![`\w])",
    re.IGNORECASE,
)
_DOC_CONCEPT_RE = re.compile(
    r"\b(?:documentation|corpus|wiki|architecture doc(?:umentation)?|user guide|dev guide)\b",
    re.IGNORECASE,
)
_EVIDENCE_PATH_RE = re.compile(
    r"(?:evidence|see|per|from)\s*:?\s*[`']?((?:docs/|README)[\w./-]*(?:\.md)?)[`']?",
    re.IGNORECASE,
)
_BACKTICK_TERM_RE = re.compile(r"`([^`]{3,80})`")


def _norm(path: str) -> str:
    return path.replace("\\", "/").lstrip("./")


def _ac_by_id(requirements: List[Requirement], ac_id: str):
    for req in requirements:
        for ac in req.acceptance_criteria:
            if ac.id == ac_id:
                return ac, req.id
    return None, None


def _doc_paths_in_text(text: str) -> List[str]:
    paths: List[str] = []
    for match in _DOC_PATH_RE.finditer(text):
        paths.append(_norm(match.group(0)))
    for match in _EVIDENCE_PATH_RE.finditer(text):
        paths.append(_norm(match.group(1)))
    return list(dict.fromkeys(paths))


def _cites_doc_concepts(description: str) -> bool:
    if _DOC_PATH_RE.search(description):
        return True
    if _DOC_CONCEPT_RE.search(description):
        return True
    for match in _BACKTICK_TERM_RE.finditer(description):
        term = match.group(1)
        if term.startswith("docs/") or term.endswith(".md") or "documentation" in term.lower():
            return True
    return False


def _corpus_terms(description: str, doc_paths: List[str]) -> List[str]:
    terms: List[str] = []
    for path in doc_paths:
        stem = Path(path).stem.replace("-", " ").replace("_", " ")
        if stem and stem.lower() != "readme":
            terms.append(stem)
        terms.append(path)
    for match in _BACKTICK_TERM_RE.finditer(description):
        term = match.group(1).strip()
        if len(term) >= 3 and not term.startswith("src/"):
            terms.append(term)
    if _DOC_CONCEPT_RE.search(description):
        for word in re.findall(r"[A-Za-z]{4,}", description):
            if word.lower() not in {"documentation", "acceptance", "criterion", "criteria", "verify", "behavior", "matches", "section"}:
                terms.append(word)
    # Residual phrase tokens from the description (e.g. "verification gates").
    for chunk in re.findall(r"[A-Za-z][A-Za-z0-9 -]{2,40}", description):
        stripped = chunk.strip()
        if " " in stripped and len(stripped) >= 8:
            terms.append(stripped)
    return list(dict.fromkeys(t for t in terms if t.strip()))


def _corpus_has_hit(project_root: Path, terms: List[str], doc_paths: List[str]) -> bool:
    if not terms and not doc_paths:
        return False
    try:
        from devcouncil.indexing.wiring import load_corpus_graph, query_corpus

        graph = load_corpus_graph(project_root)
        if graph is not None:
            doc_norm = {_norm(p) for p in doc_paths}
            for node in graph.nodes:
                node_path = _norm(node.path or "")
                if node_path in doc_norm or any(
                    node_path.endswith(p) for p in doc_norm if p
                ):
                    return True
        for term in terms[:8]:
            result = query_corpus(project_root, term, limit=3)
            if result.get("matches"):
                return True
    except Exception:
        logger.debug("corpus query failed during acceptance_corpus check", exc_info=True)
    return False


def _explicit_evidence_path(
    *,
    project_root: Path,
    doc_paths: List[str],
    changed_files: List[str],
    planned_paths: Set[str],
    expected_tests: List[str],
    diff_content: str,
) -> Optional[str]:
    changed_norm = {_norm(p) for p in changed_files}
    tests_blob = "\n".join(expected_tests)
    for path in doc_paths:
        rel = _norm(path)
        full = project_root / rel
        if not full.is_file():
            continue
        if rel in changed_norm or rel in planned_paths:
            return rel
        if rel in diff_content or rel in tests_blob:
            return rel
    return None


def detect_acceptance_corpus_gaps(
    *,
    task: Task,
    requirements: List[Requirement],
    project_root: Path,
    changed_files: List[str],
    diff_content: str,
    next_gap_id: Callable[[str, str], str],
    acceptance_corpus_enabled: bool = True,
    acceptance_corpus_blocking: bool = False,
) -> List[Gap]:
    if not acceptance_corpus_enabled or not task.acceptance_criterion_ids:
        return []
    gaps: List[Gap] = []
    planned_paths = {_norm(pf.path) for pf in task.planned_files}
    try:
        for ac_id in task.acceptance_criterion_ids:
            ac, req_id = _ac_by_id(requirements, ac_id)
            if ac is None:
                continue
            description = ac.description or ""
            if not _cites_doc_concepts(description):
                continue
            doc_paths = _doc_paths_in_text(description)
            terms = _corpus_terms(description, doc_paths)
            if _corpus_has_hit(project_root, terms, doc_paths):
                continue
            evidence_path = _explicit_evidence_path(
                project_root=project_root,
                doc_paths=doc_paths,
                changed_files=changed_files,
                planned_paths=planned_paths,
                expected_tests=list(task.expected_tests or []),
                diff_content=diff_content or "",
            )
            if evidence_path:
                continue
            gaps.append(Gap(
                id=next_gap_id(task.id, f"ACCORPUS-{ac_id}"),
                severity="high" if acceptance_corpus_blocking else "medium",
                gap_type="acceptance_corpus",
                task_id=task.id,
                requirement_id=req_id,
                acceptance_criterion_id=ac_id,
                description=(
                    f"Acceptance criterion `{ac_id}` cites documentation concepts but "
                    "has no corpus match and no explicit evidence path in the task diff, "
                    "planned files, or expected tests."
                ),
                evidence=[ac_id, description[:200], *(doc_paths[:3])],
                recommended_fix=(
                    "Add a corpus hit (run `dev corpus build` and align docs), cite an "
                    "existing doc path in planned/changed files, or add an explicit "
                    "`evidence: docs/...` path with supporting changes."
                ),
                blocking=acceptance_corpus_blocking,
                suggested_command="dev corpus build",
            ))
    except Exception:
        logger.debug("detect_acceptance_corpus_gaps failed", exc_info=True)
    return gaps
