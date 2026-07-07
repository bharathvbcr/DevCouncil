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

Review rules — follow them exactly:
- The diff above IS the complete, authoritative record of the change. Review the CODE in it.
  Do not ask for proof that the change was made, and do not report "no evidence of
  implementation" — you are looking at the implementation.
- Every finding must cite the specific file/hunk in the diff that demonstrates the problem
  (in its 'evidence'). A concern you cannot tie to a concrete line in the diff is not a finding.
- Do not review prose, commit-message style, response formatting, or your estimate of effort.
- Severity rubric: 'critical' is reserved for a demonstrable correctness or security defect
  visible in the diff (wrong behavior, data loss, injection, broken requirement). Missing
  edge cases are 'high' at most; style and structure are 'low'.
- If the diff implements the requirements correctly, return is_satisfactory=true and an
  empty findings list. An empty review of correct code is a correct review.

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
