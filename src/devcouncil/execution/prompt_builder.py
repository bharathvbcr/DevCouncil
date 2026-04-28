from pathlib import Path
from typing import List

from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.integrations.code_review_graph import CodeReviewGraphAdapter

class PromptBuilder:
    def __init__(self, project_root: Path = Path(".")):
        self.project_root = project_root

    def build_task_prompt(self, task: Task, requirements: List[Requirement]) -> str:
        req_map = {r.id: r for r in requirements}
        task_reqs = [req_map[rid] for rid in task.requirement_ids if rid in req_map]
        
        prompt = f"""# Implement {task.id}: {task.title}

## Goal
{task.description}

## Requirements
"""
        for req in task_reqs:
            prompt += f"- {req.id}: {req.title}\n"
            for ac in req.acceptance_criteria:
                prompt += f"  - [ ] {ac.description} ({ac.verification_method})\n"

        prompt += "\n## Allowed files\n"
        for pf in task.planned_files:
            prompt += f"- `{pf.path}` ({pf.allowed_change}): {pf.reason}\n"

        if task.forbidden_changes:
            prompt += "\n## Forbidden changes\n"
            for fc in task.forbidden_changes:
                prompt += f"- `{fc}`\n"

        prompt += "\n## Expected tests\n"
        for et in task.expected_tests:
            prompt += f"- `{et}`\n"

        prompt += "\n## Allowed commands\n"
        for cmd in task.allowed_commands:
            prompt += f"- `{cmd}`\n"

        graph_context = CodeReviewGraphAdapter(self.project_root).prompt_section(
            [planned.path for planned in task.planned_files]
        )
        if graph_context:
            prompt += f"\n{graph_context}"

        prompt += """
## Instructions
1. Implement the goal described above.
2. Ensure all acceptance criteria are met.
3. Only modify the allowed files.
4. Run the allowed commands to verify your work.
5. Provide evidence of passing tests.
"""
        return prompt
