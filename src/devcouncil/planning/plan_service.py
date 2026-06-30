import logging
from typing import List, Tuple
from pydantic import BaseModel
from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.llm.router import ModelRouter

logger = logging.getLogger(__name__)

class PlanOutput(BaseModel):
    id: str
    rationale: str
    tasks: List[Task]


def backfill_acceptance_criteria(
    tasks: List[Task], requirements: List[Requirement]
) -> Tuple[List[Task], List[Tuple[str, str]]]:
    """Guarantee every acceptance criterion is owned by a task.

    The spec elaborates edge-case/error criteria, but a planner (especially a weak one)
    may link only some — or none — of them to tasks via ``acceptance_criterion_ids``,
    silently dropping the rest from per-criterion verification. That is a core reason a
    planned+gated run can be no better than the raw prompt: the elaborated edges never
    become something a task is accountable for building and proving.

    For each criterion not covered by any task, attach it to a WRITABLE task that
    implements its requirement (falling back to any task on that requirement). A criterion
    whose requirement no task owns is left alone — the requirement-coverage gate already
    flags that. Returns the (possibly rewritten) tasks and the ``(task_id, ac_id)`` links
    that were added.
    """
    covered = {ac_id for task in tasks for ac_id in task.acceptance_criterion_ids}
    assignments: dict[str, List[str]] = {}
    for req in requirements:
        uncovered = [ac.id for ac in req.acceptance_criteria if ac.id not in covered]
        if not uncovered:
            continue
        req_ac_ids = {ac.id for ac in req.acceptance_criteria}
        candidates = [t for t in tasks if req.id in t.requirement_ids]
        writable = [
            t for t in candidates
            if any(pf.allowed_change in ("create", "modify", "delete") for pf in t.planned_files)
        ]
        # Prefer the writable task already implementing some of this requirement's criteria
        # (the primary implementer — most likely where the missing behavior also belongs),
        # so a backfilled criterion lands on the task that actually builds it rather than an
        # unrelated sibling. Fall back to any writable task, then any task on the requirement.
        primary = [t for t in writable if req_ac_ids.intersection(t.acceptance_criterion_ids)]
        target = primary or writable or candidates
        if not target:
            continue  # no task owns this requirement; requirement-coverage gap handles it
        assignments.setdefault(target[0].id, []).extend(uncovered)

    if not assignments:
        return tasks, []

    backfilled: List[Tuple[str, str]] = []
    new_tasks: List[Task] = []
    for task in tasks:
        add = assignments.get(task.id)
        if add:
            merged = list(dict.fromkeys([*task.acceptance_criterion_ids, *add]))
            new_tasks.append(task.model_copy(update={"acceptance_criterion_ids": merged}))
            backfilled.extend((task.id, ac_id) for ac_id in add)
        else:
            new_tasks.append(task)
    return new_tasks, backfilled

class PlanService:
    def __init__(self, router: ModelRouter):
        self.router = router

    async def generate_plan(self, role: str, goal: str, requirements_json: str, repo_map_json: str) -> PlanOutput:
        prompt = f"""
Goal: {goal}

Requirements:
{requirements_json}

Repository Map:
{repo_map_json}

Your task is to create a detailed implementation plan.
- Break down the requirements into atomic implementation tasks, but use the FEWEST
  tasks that cover them — do NOT over-decompose. A small goal (e.g. add one function
  plus its test) is typically one or two tasks, not four.
- Each file's changes must be OWNED BY A SINGLE TASK. Never create two tasks that both
  create/modify the same file — that causes duplicate or conflicting edits. If work on
  a file spans concerns, keep it in one task or split by FILE, not by sub-edit.
- For each task, specify which files will be created or modified. Every implementation
  task must declare at least one writable (create/modify) planned file — a task that
  only reads files cannot implement anything.
- Fill expected_tests with RUNNABLE shell commands (not prose) that exit 0 iff the
  task's acceptance criteria hold and can run immediately after THIS task with no
  missing tools or files. Prove BEHAVIOR with self-contained inline assertions, e.g.
  python -c "import calc; assert calc.add(2,3)==5". Use pytest only on a whole test
  file this or an earlier task creates (python -m pytest tests/test_x.py -q), never a
  ::node id. Do NOT assert repository/git state (git status, changed-file sets,
  append-only contents) and do NOT invoke flake8/mypy/ruff/eslint/tsc/npm unless the
  repo is already configured for them.
- Ensure each task maps back to at least one requirement.
- Populate each task's acceptance_criterion_ids with the IDs of the specific acceptance
  criteria that task implements. EVERY acceptance criterion in the requirements above must
  be owned by exactly one task — do NOT drop edge-case, boundary, or error-handling
  criteria. An unowned criterion is a behavior nobody is accountable for building, which is
  how subtle requirements get silently missed.

Role-specific instructions:
"""
        if role == "planner_a":
            prompt += "You are the pragmatic tech lead. Optimize for simplicity and minimal dependencies."
        else:
            prompt += "You are the production-readiness architect. Optimize for security, performance, and edge cases."

        messages = [
            {"role": "user", "content": prompt}
        ]

        result = await self.router.complete_structured(
            role=role,
            messages=messages,
            schema=PlanOutput
        )
        logger.info("Plan generated by %s: %d task(s)", role, len(result.tasks))
        return result
