from typing import List
from pydantic import BaseModel
from devcouncil.domain.requirement import Requirement
from devcouncil.domain.assumption import Assumption
from devcouncil.llm.router import ModelRouter

class BlockingQuestion(BaseModel):
    id: str
    question: str
    reason: str

class SpecOutput(BaseModel):
    requirements: List[Requirement]
    assumptions: List[Assumption]
    blocking_questions: List[BlockingQuestion]

class SpecService:
    def __init__(self, router: ModelRouter):
        self.router = router

    async def generate_spec(self, goal: str, repo_map_json: str) -> SpecOutput:
        prompt = f"""
Goal: {goal}

Repository Map:
{repo_map_json}

Your task is to draft the initial software specification for this goal.
1. Identify functional and non-functional requirements.
2. Extract any assumptions you are making about the codebase or architecture.
3. List any blocking questions that the user must answer before implementation can proceed.

Each requirement MUST have clear acceptance criteria with verification methods.
Each assumption MUST have a confidence and impact level.
"""
        messages = [
            {"role": "user", "content": prompt}
        ]
        
        return await self.router.complete_structured(
            role="spec_writer",
            messages=messages,
            schema=SpecOutput
        )
