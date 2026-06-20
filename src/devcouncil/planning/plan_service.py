from typing import List
from pydantic import BaseModel
from devcouncil.domain.task import Task
from devcouncil.llm.router import ModelRouter

class PlanOutput(BaseModel):
    id: str
    rationale: str
    tasks: List[Task]

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

Role-specific instructions:
"""
        if role == "planner_a":
            prompt += "You are the pragmatic tech lead. Optimize for simplicity and minimal dependencies."
        else:
            prompt += "You are the production-readiness architect. Optimize for security, performance, and edge cases."

        messages = [
            {"role": "user", "content": prompt}
        ]
        
        return await self.router.complete_structured(
            role=role,
            messages=messages,
            schema=PlanOutput
        )
