import json
import logging
from typing import List
from pydantic import BaseModel
from devcouncil.domain.task import Task
from devcouncil.domain.requirement import Requirement
from devcouncil.domain.gap import Gap
from devcouncil.llm.router import ModelRouter
from devcouncil.utils.redaction import redact_string

logger = logging.getLogger(__name__)

class ReviewOutput(BaseModel):
    is_satisfactory: bool
    findings: List[Gap]

class ImplementationReviewer:
    """Uses LLM to review code changes against task requirements."""
    
    def __init__(self, router: ModelRouter):
        self.router = router

    async def review_changes(
        self, 
        task: Task, 
        requirements: List[Requirement], 
        diff: str
    ) -> ReviewOutput:
        linked_reqs = [r for r in requirements if r.id in task.requirement_ids]
        if not linked_reqs:
            linked_reqs = requirements
        requirements_json = json.dumps([r.model_dump() for r in linked_reqs], indent=2)
        redacted_diff = redact_string(diff)
        prompt = f"""
You are an expert software reviewer. Review the following code changes against the task requirements.
Task: {task.title}
Description: {task.description}

Requirements:
{requirements_json}

Code Diff:
{redacted_diff}

Your task is to identify if the implementation is complete, correct, and follows best practices.
- Identify missing edge cases.
- Identify architectural drift.
- Identify security risks not caught by static scans.

Return a JSON object with 'is_satisfactory' and a list of 'findings' (as Gap objects).
"""
        messages = [{"role": "user", "content": prompt}]

        logger.info("Implementation review: task=%s diff_bytes=%d", task.id, len(diff))
        result = await self.router.complete_structured(
            role="implementation_reviewer",
            messages=messages,
            schema=ReviewOutput
        )
        logger.info(
            "Implementation review for %s: satisfactory=%s findings=%d",
            task.id, result.is_satisfactory, len(result.findings),
        )
        return result
