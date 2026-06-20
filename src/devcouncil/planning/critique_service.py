from typing import List
from pydantic import BaseModel
from devcouncil.domain.critique import CritiqueFinding
from devcouncil.llm.router import ModelRouter

class CritiqueOutput(BaseModel):
    findings: List[CritiqueFinding]

class RebuttalItem(BaseModel):
    finding_id: str
    decision: str # "accepted", "rejected"
    reason: str
    suggested_change: str | None = None

class RebuttalOutput(BaseModel):
    rebuttals: List[RebuttalItem]

class CritiqueService:
    def __init__(self, router: ModelRouter):
        self.router = router

    async def generate_critique(self, role: str, target_plan_json: str, requirements_json: str) -> CritiqueOutput:
        prompt = f"""
Requirements:
{requirements_json}

Target Plan:
{target_plan_json}

You are a hostile staff engineer reviewing another team's implementation plan.
Find missing requirements, bad assumptions, missing tests, security risks, migration risks, and unverifiable claims.
Do not praise. Do not rewrite the plan.
Every finding must include a falsifiable_check.
"""
        messages = [
            {"role": "user", "content": prompt}
        ]
        
        return await self.router.complete_structured(
            role=role,
            messages=messages,
            schema=CritiqueOutput,
            # Degrade gracefully on weaker models: an un-critiqued plan is still a
            # usable plan, far better than crashing the whole planning run.
            fallback=CritiqueOutput(findings=[]),
        )

    async def generate_rebuttal(self, role: str, original_plan_json: str, findings_json: str) -> RebuttalOutput:
        prompt = f"""
Original Plan:
{original_plan_json}

Critique Findings:
{findings_json}

You are the planner who created the original plan. Review the critique findings.
- A finding can be rejected only with artifact evidence or strong justification.
- A finding can be accepted and converted into a requirement/task/test.
- No hand-wavy rebuttals.
"""
        messages = [
            {"role": "user", "content": prompt}
        ]
        
        return await self.router.complete_structured(
            role=role,
            messages=messages,
            schema=RebuttalOutput,
            # No rebuttals means findings stand as-is — a safe, conservative default.
            fallback=RebuttalOutput(rebuttals=[]),
        )
