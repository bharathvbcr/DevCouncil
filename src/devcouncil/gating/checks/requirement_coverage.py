from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.domain.gap import Gap
from typing import List

class RequirementCoverageCheck:
    """Detects requirements that are not mapped to any tasks."""
    
    def check(self, requirements: List[Requirement], tasks: List[Task]) -> List[Gap]:
        task_req_ids = set()
        for t in tasks:
            task_req_ids.update(t.requirement_ids)
            
        gaps = []
        for req in requirements:
            if req.id not in task_req_ids:
                gaps.append(Gap(
                    id=f"GAP-PLAN-{req.id}-UNMAPPED",
                    severity="high",
                    gap_type="requirement_not_planned",
                    requirement_id=req.id,
                    description=f"Requirement '{req.title}' is not covered by any task.",
                    recommended_fix="Decompose this requirement into one or more implementation tasks.",
                    blocking=True
                ))
        return gaps
