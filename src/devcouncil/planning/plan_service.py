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
- Break down the requirements into atomic implementation tasks.
- For each task, specify which files will be created or modified.
- Specify which tests are expected to verify the task.
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
