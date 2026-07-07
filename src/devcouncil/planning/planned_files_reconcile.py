"""Reconcile planner-emitted ``planned_files`` against the real repository.

The planner names ``planned_files`` as free-form LLM output. A
``modify``/``delete``/``read_only`` entry that points at a path the repo does not
actually contain is not a harmless typo: it becomes the file whitelist that
downstream scope enforcement trusts. A plausible-but-wrong path (a typo, or a file
that was renamed) then silently reverts the legitimate write the agent makes to the
*real* path, because the real path was never whitelisted.

This pass grounds those paths in the repo map's actual file set before the plan is
persisted. Its guiding invariant is **it only ever relaxes or corrects scope, never
tightens it** — so it can remove false reverts but never introduce a new one:

  - ``create`` entries are left as-is (a new file legitimately isn't in the map yet).
  - a path that exists in the map is kept (normalized).
  - a non-existent ``modify``/``delete``/``read_only`` path is *repaired* to a real
    path when exactly one file in the repo shares its basename (the typo/rename case).
  - anything else (no basename match, or an ambiguous multi-match) is kept as-is and
    reported as an advisory warning, never dropped.

Import-light (domain + stdlib only) so it stays unit-testable without booting the
planner's LLM/router stack.
"""

from __future__ import annotations

from typing import Iterable

from devcouncil.domain.task import PlannedFile, Task


def _normalize(path: str) -> str:
    """Strip ``./`` prefixes, normalize separators, and drop trailing slashes."""
    return path.strip().replace("\\", "/").removeprefix("./").rstrip("/")


def _basename(path: str) -> str:
    return path.rsplit("/", 1)[-1]


def repo_files_from_map(repo_map: object) -> list[str]:
    """Extract tracked file paths from a ``RepoMap`` (or its ``dict`` form)."""
    files = getattr(repo_map, "files", None)
    if files is None and isinstance(repo_map, dict):
        files = repo_map.get("files", [])
    result: list[str] = []
    for entry in files or []:
        path = getattr(entry, "path", None)
        if path is None and isinstance(entry, dict):
            path = entry.get("path")
        if path:
            result.append(path)
    return result


def reconcile_planned_files(
    tasks: list[Task],
    repo_files: Iterable[str],
) -> tuple[list[Task], list[str]]:
    """Ground each task's ``planned_files`` against the real repo file set.

    Returns the (possibly rewritten) tasks and a list of human-readable warnings
    describing every repair or unresolved mismatch. When ``repo_files`` is empty
    (no map available), tasks are returned untouched — we can't distinguish a
    hallucinated path from a real one, so we degrade gracefully rather than guess.
    """
    known = {_normalize(f) for f in repo_files if f and f.strip()}

    if not known:
        return list(tasks), []

    by_basename: dict[str, set[str]] = {}
    for f in known:
        by_basename.setdefault(_basename(f), set()).add(f)

    warnings: list[str] = []
    new_tasks: list[Task] = []

    for task in tasks:
        if not task.planned_files:
            new_tasks.append(task)
            continue

        changed = False
        kept: list[PlannedFile] = []
        for pf in task.planned_files:
            norm = _normalize(pf.path)

            # A file this task creates legitimately isn't in the map yet.
            if pf.allowed_change == "create" or norm in known:
                if norm != pf.path:
                    kept.append(pf.model_copy(update={"path": norm}))
                    changed = True
                else:
                    kept.append(pf)
                continue

            # Non-existent modify/delete/read_only target: repair on a unique
            # basename match (the typo / rename case), otherwise keep + warn.
            candidates = by_basename.get(_basename(norm), set())
            if len(candidates) == 1:
                repaired = next(iter(candidates))
                kept.append(pf.model_copy(update={"path": repaired}))
                changed = True
                warnings.append(
                    f"{task.id}: planned file '{pf.path}' not found in repo; "
                    f"repaired to '{repaired}' (unique basename match)."
                )
            else:
                kept.append(pf.model_copy(update={"path": norm}) if norm != pf.path else pf)
                if norm != pf.path:
                    changed = True
                detail = (
                    f"{len(candidates)} basename matches — ambiguous"
                    if candidates
                    else "no matching file"
                )
                warnings.append(
                    f"{task.id}: planned file '{pf.path}' ({pf.allowed_change}) "
                    f"not found in repo ({detail}); left as-is for review."
                )

        new_tasks.append(task.model_copy(update={"planned_files": kept}) if changed else task)

    return new_tasks, warnings


def expand_scope_with_dependents(
    tasks: list[Task],
    dependents: dict[str, list[str]],
    repo_files: Iterable[str],
    max_per_file: int = 8,
) -> tuple[list[Task], list[str]]:
    """Widen each task's ``planned_files`` with the real callers of its writable files.

    The other half of the scope bug ``reconcile_planned_files`` doesn't cover: the
    planner names ``foo.py`` to modify but not the files that *import* ``foo.py`` and
    must change with it. Those omitted callers are outside the whitelist, so the
    agent's necessary edit to them is reverted. Using the repo map's ``dependents``
    (reverse import edges), add each writable file's callers as ``modify``-scoped
    entries — capped per file and drawn only from files that actually exist, so this
    only ever *relaxes* scope from a grounded signal, never invents paths.
    """
    if not dependents:
        return list(tasks), []

    known = {_normalize(f) for f in repo_files if f and f.strip()}
    norm_dependents = {
        _normalize(k): [_normalize(v) for v in (vs or [])] for k, vs in dependents.items()
    }

    warnings: list[str] = []
    new_tasks: list[Task] = []

    for task in tasks:
        existing = {_normalize(pf.path) for pf in task.planned_files}
        additions: list[PlannedFile] = []
        for pf in task.planned_files:
            if pf.allowed_change not in ("modify", "delete"):
                continue
            norm = _normalize(pf.path)
            added = 0
            for dep in norm_dependents.get(norm, []):
                if added >= max_per_file:
                    break
                if dep in existing:
                    continue
                if known and dep not in known:
                    continue
                existing.add(dep)
                additions.append(
                    PlannedFile(
                        path=dep,
                        reason=f"imports {norm}; may need updating when it changes (repo map dependents)",
                        allowed_change="modify",
                    )
                )
                added += 1

        if additions:
            new_tasks.append(
                task.model_copy(update={"planned_files": [*task.planned_files, *additions]})
            )
            warnings.append(
                f"{task.id}: widened scope with {len(additions)} dependent file(s) so a "
                "required caller edit isn't reverted."
            )
        else:
            new_tasks.append(task)

    return new_tasks, warnings
