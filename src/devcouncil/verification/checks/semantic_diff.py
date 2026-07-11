"""Semantic-index drift checks extracted from Verifier."""

from __future__ import annotations

import logging
import re
import sys
from pathlib import Path
from typing import Callable, List, Optional

from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.utils.json_persist import read_json

logger = logging.getLogger(__name__)


def task_intent_text(task: Task, requirements: Optional[List[Requirement]]) -> str:
    """Lowercased text describing what the task is meant to do."""
    parts = [task.title or "", task.description or ""]
    if requirements:
        ac_ids = set(task.acceptance_criterion_ids)
        for req in requirements:
            for ac in req.acceptance_criteria:
                if ac.id in ac_ids:
                    parts.append(ac.description or "")
    return " ".join(parts).lower()


def import_top_level(statement: str) -> Optional[str]:
    """Top-level package of an import statement, or None for relative/local/unparseable."""
    s = (statement or "").strip()
    if s.startswith("import "):
        first = s[len("import "):].split(",")[0].strip()
        top = first.split(" as ")[0].strip().split(".")[0].strip()
        return top or None
    if s.startswith("from "):
        rest = s[len("from "):].lstrip()
        if rest.startswith("."):
            return None
        mod = rest.split(" import ")[0].strip()
        return (mod.split(".")[0].strip() or None) if mod else None
    return None


def stdlib_modules() -> frozenset[str]:
    names = getattr(sys, "stdlib_module_names", None)
    return frozenset(names) if names else frozenset()


def load_project_dependencies(project_root: Path) -> set[str]:
    """Lower-cased distribution names declared by the project."""
    deps: set[str] = set()
    split_re = r"[><=!~;\[\] ]"
    pyproject = project_root / "pyproject.toml"
    if pyproject.exists():
        try:
            import tomllib

            data = tomllib.loads(pyproject.read_text(encoding="utf-8"))
            project = data.get("project", {}) or {}
            for dep in project.get("dependencies", []) or []:
                pkg = re.split(split_re, dep.strip())[0].strip().lower()
                if pkg:
                    deps.add(pkg)
            for group in (project.get("optional-dependencies", {}) or {}).values():
                for dep in group or []:
                    pkg = re.split(split_re, dep.strip())[0].strip().lower()
                    if pkg:
                        deps.add(pkg)
        except Exception:
            pass
    requirements = project_root / "requirements.txt"
    if requirements.exists():
        try:
            for line in requirements.read_text(encoding="utf-8").splitlines():
                line = line.strip()
                if line and not line.startswith("#"):
                    pkg = re.split(split_re, line)[0].strip().lower()
                    if pkg:
                        deps.add(pkg)
        except Exception:
            pass
    package_json = project_root / "package.json"
    if package_json.exists():
        try:
            data = read_json(package_json)
            for key in ("dependencies", "devDependencies", "optionalDependencies"):
                deps.update(k.lower() for k in (data.get(key) or {}).keys())
        except Exception:
            pass
    return deps


def is_new_third_party_import(top: Optional[str], *, project_deps: set[str]) -> bool:
    """True only when ``top`` is a genuinely new, undeclared third-party package."""
    if not top:
        return False
    if top in stdlib_modules():
        return False
    if top.lower() in project_deps:
        return False
    try:
        import importlib.util

        if importlib.util.find_spec(top) is not None:
            return False
    except Exception:
        return False
    return True


def detect_semantic_diff_gaps(
    *,
    project_root: Path,
    task: Task,
    requirements: Optional[List[Requirement]],
    next_gap_id: Callable[[str, str], str],
    project_deps: set[str],
) -> List[Gap]:
    """Run semantic-index drift checks when an after snapshot exists."""
    gaps: List[Gap] = []
    after_path = project_root / ".devcouncil" / "semantic" / task.id / "after.json"
    if not after_path.exists():
        return gaps
    try:
        from devcouncil.indexing.semantic_index import SemanticIndex

        result = SemanticIndex(project_root).diff(task.id)
    except Exception as exc:
        logger.warning("Semantic diff check failed for %s; skipping semantic gaps: %s", task.id, exc)
        return gaps

    planned_paths = {pf.path for pf in task.planned_files}
    classifications = result.get("classifications", [])
    readded_public = {
        item.get("name") for item in classifications
        if item.get("type") == "exported_symbol_added" and item.get("name")
    }
    intent_text = task_intent_text(task, requirements)
    for item in classifications:
        change_type = item.get("type", "")
        path = item.get("path", "")
        if change_type == "exported_symbol_removed":
            name = item.get("name", "")
            moved = name in readded_public
            intended = bool(name) and name.lower() in intent_text
            gaps.append(Gap(
                id=next_gap_id(task.id, "DRIFT"),
                severity="high",
                gap_type="architecture_drift",
                task_id=task.id,
                description=(
                    f"Public symbol '{name}' was removed from {path} — possible scope "
                    "drift: the executor changed a public API the task did not call for."
                ),
                evidence=[f"{path}:{name}"],
                recommended_fix=(
                    "Restore the removed public symbol. If its removal IS part of this "
                    "task, state that in the task description / acceptance criteria so the "
                    "change is an intended, reviewed decision rather than silent drift."
                ),
                blocking=(not moved and not intended),
                file=path,
            ))
        elif change_type == "public_api_change" and path not in planned_paths:
            gaps.append(Gap(
                id=next_gap_id(task.id, "SEM"),
                severity="high",
                gap_type="architecture_drift",
                task_id=task.id,
                description=f"Unplanned public API change detected in {path}.",
                evidence=[path],
                recommended_fix="Add file to planned_files and document acceptance criteria.",
                blocking=not bool(task.acceptance_criterion_ids),
            ))
        elif change_type == "public_api_change" and path in planned_paths:
            gaps.append(Gap(
                id=next_gap_id(task.id, "SIGDRIFT"),
                severity="medium",
                gap_type="architecture_drift",
                task_id=task.id,
                description=(
                    f"Public API signature change in planned file {path}"
                    + (f" ({item.get('name')})" if item.get("name") else "")
                    + ". Confirm callers are updated and the change is intended."
                ),
                evidence=[f"{path}:{item.get('name', '')}"],
                recommended_fix=(
                    "If the signature change is part of this task, note it in the task "
                    "description / acceptance criteria; otherwise revert it."
                ),
                blocking=False,
            ))
        elif change_type == "import_dependency_change":
            statement = item.get("statement", "")
            top = import_top_level(statement)
            new_third_party = is_new_third_party_import(top, project_deps=project_deps)
            if new_third_party:
                gaps.append(Gap(
                    id=next_gap_id(task.id, "DEPADD"),
                    severity="high",
                    gap_type="dependency_risk",
                    task_id=task.id,
                    description=(
                        f"New undeclared third-party dependency '{top}' imported in {path} "
                        f"({statement.strip()}). Adding a dependency the task did not plan is "
                        "supply-chain drift."
                    ),
                    evidence=[path, statement.strip()],
                    recommended_fix=(
                        f"Declare '{top}' in the project's dependencies and plan the change, "
                        "or use an existing/standard-library alternative."
                    ),
                    blocking=True,
                    file=path,
                ))
            elif path not in planned_paths:
                gaps.append(Gap(
                    id=next_gap_id(task.id, "IMP"),
                    severity="medium",
                    gap_type="dependency_risk",
                    task_id=task.id,
                    description=f"Import dependency change in {path}.",
                    evidence=[path],
                    recommended_fix="Confirm dependency change is intentional.",
                    blocking=False,
                ))
        elif change_type == "config_schema_dependency_change" and path not in planned_paths:
            gaps.append(Gap(
                id=next_gap_id(task.id, "CFG"),
                severity="high",
                gap_type="dependency_risk",
                task_id=task.id,
                description=f"Config/schema change detected in {path}.",
                evidence=[path],
                recommended_fix="Plan the config change or revert it.",
                blocking=True,
            ))
    return gaps
