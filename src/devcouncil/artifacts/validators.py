from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.app.errors import GatingError

class ArtifactValidator:
    """Validates DevCouncil artifacts (tasks, requirements, etc.)."""
    
    @staticmethod
    def validate_requirement(req: Requirement) -> None:
        if not req.title:
            raise GatingError(f"Requirement {req.id} missing title.")
        if not req.acceptance_criteria:
            raise GatingError(f"Requirement {req.id} must have at least one acceptance criterion.")
        for ac in req.acceptance_criteria:
            if not ac.verification_method:
                raise GatingError(f"Acceptance criterion {ac.id} in {req.id} missing verification method.")

    @staticmethod
    def validate_task(task: Task) -> None:
        if not task.requirement_ids:
            raise GatingError(f"Task {task.id} must map to at least one requirement.")
        if not task.planned_files:
            raise GatingError(f"Task {task.id} must have at least one planned file.")
        if not task.acceptance_criterion_ids:
            raise GatingError(f"Task {task.id} must map to at least one acceptance criterion.")
        if not task.allowed_commands and not task.expected_tests:
            raise GatingError(f"Task {task.id} must define allowed commands or expected tests.")
