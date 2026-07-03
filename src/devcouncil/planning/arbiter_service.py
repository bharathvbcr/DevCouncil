import logging
from typing import List, Dict
from pydantic import BaseModel, Field
from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.llm.router import ModelRouter

logger = logging.getLogger(__name__)

class ArbiterDecision(BaseModel):
    # Empty finding lists are routinely OMITTED (not sent as "[]") by models on
    # providers without grammar-constrained decoding; an absent empty list is
    # not an arbitration failure, so default instead of crashing the run.
    # final_requirements/final_tasks stay required — a decision without them is
    # a real failure the healing/retry path must surface.
    accepted_finding_ids: List[str] = Field(default_factory=list)
    rejected_finding_ids: List[Dict[str, str]] = Field(default_factory=list)  # id, reason
    final_requirements: List[Requirement]
    final_tasks: List[Task]

class ArbiterService:
    def __init__(self, router: ModelRouter):
        self.router = router

    async def arbitrate(
        self, 
        goal: str, 
        requirements_json: str, 
        plan_a_json: str, 
        plan_b_json: str, 
        critique_a_json: str, 
        critique_b_json: str,
        rebuttal_a_json: str,
        rebuttal_b_json: str
    ) -> ArbiterDecision:
        prompt = f"""
Goal: {goal}

Initial Requirements:
{requirements_json}

Plan A: {plan_a_json}
Plan B: {plan_b_json}

Critique of Plan B by Critic A: {critique_a_json}
Critique of Plan A by Critic B: {critique_b_json}

Rebuttal of Critic B by Planner A: {rebuttal_a_json}
Rebuttal of Critic A by Planner B: {rebuttal_b_json}

You are the arbiter engineering manager. Your goal is to produce the final, definitive set of requirements and tasks.
- You do not decide by vibes.
- High-severity unrefuted findings from critics must be incorporated into the final requirements or tasks.
- If a planner successfully rebutted a finding, you may skip it.
- Produce a single, coherent task graph.
- Set each task's ``difficulty`` field to ``easy``, ``normal``, or ``hard`` based on scope
  (files touched, acceptance criteria count, cross-cutting concerns). Hard tasks get
  stricter verification — prefer splitting work that would score hard into smaller tasks.
- Mention ``scaffolding`` in a task description only when intentional placeholders are expected.
"""
        messages = [
            {"role": "user", "content": prompt}
        ]

        result = await self.router.complete_structured(
            role="arbiter",
            messages=messages,
            schema=ArbiterDecision
        )
        logger.info(
            "Arbiter decision: %d final requirement(s), %d final task(s), %d finding(s) accepted",
            len(result.final_requirements), len(result.final_tasks), len(result.accepted_finding_ids),
        )
        return result
