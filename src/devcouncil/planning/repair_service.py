from typing import List
import json
from pydantic import BaseModel
from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task
from devcouncil.llm.router import ModelRouter

class RepairOutput(BaseModel):
    suggested_tasks: List[Task]

class RepairService:
    """Uses LLM to infer focused repair tasks from blocking gaps."""
    
    def __init__(self, router: ModelRouter):
        self.router = router

    async def generate_repair_plan(self, gaps: List[Gap], project_context: str) -> RepairOutput:
        prompt = f"""
The following blocking gaps were detected during verification.
Gaps:
{json.dumps([g.model_dump() for g in gaps], indent=2)}

Project Context:
{project_context}

Your task is to generate focused implementation tasks to fix these gaps.
- Each task must have a clear description and recommended fix.
- Specify 'planned_files' that need modification (infer from gap evidence).
- Link each task to the relevant 'requirement_id' mentioned in the gap.

Return a JSON object with 'suggested_tasks'.
"""
        messages = [{"role": "user", "content": prompt}]
        
        return await self.router.complete_structured(
            role="planner_a",  # Pragmatic tech lead is best suited for repair task generation
            messages=messages,
            schema=RepairOutput
        )
