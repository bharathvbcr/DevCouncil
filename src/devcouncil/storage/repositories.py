from sqlmodel import Session, col, delete, select
from typing import List, Optional, Any
import json
from devcouncil.storage.models import (
    RequirementModel,
    AssumptionModel,
    TaskModel,
    GapModel,
    EvidenceModel,
    CritiqueFindingModel,
    ProjectStateModel,
)
from devcouncil.domain.requirement import Requirement, AcceptanceCriterion
from devcouncil.domain.assumption import Assumption
from devcouncil.domain.task import Task, PlannedFile
from devcouncil.domain.gap import Gap
from devcouncil.domain.evidence import CommandResult, DiffEvidence, TestEvidence
from devcouncil.domain.critique import CritiqueFinding
from devcouncil.artifacts.graph import ArtifactGraph

class RequirementRepository:
    def __init__(self, session: Session):
        self.session = session

    def get_all(self) -> List[Requirement]:
        statement = select(RequirementModel)
        models = self.session.exec(statement).all()
        results = []
        for m in models:
            ac_list = [AcceptanceCriterion.model_validate(ac) for ac in json.loads(m.acceptance_criteria_json)]
            results.append(Requirement.model_validate({
                "id": m.id,
                "title": m.title,
                "description": m.description,
                "priority": m.priority,
                "source": m.source,
                "acceptance_criteria": ac_list,
            }))
        return results

    def save(self, req: Requirement):
        model = RequirementModel(
            id=req.id,
            title=req.title,
            description=req.description,
            priority=req.priority,
            source=req.source,
            acceptance_criteria_json=json.dumps([ac.model_dump() for ac in req.acceptance_criteria])
        )
        self.session.merge(model)
        self.session.commit()


class AssumptionRepository:
    def __init__(self, session: Session):
        self.session = session

    def get_all(self) -> List[Assumption]:
        statement = select(AssumptionModel)
        models = self.session.exec(statement).all()
        return [
            Assumption.model_validate({
                "id": m.id,
                "statement": m.statement,
                "confidence": m.confidence,
                "impact": m.impact,
                "reversible": m.reversible,
                "requires_user_confirmation": m.requires_user_confirmation,
                "linked_requirement_ids": json.loads(m.linked_requirement_ids_json),
                "status": m.status,
            })
            for m in models
        ]

    def save(self, assumption: Assumption):
        model = AssumptionModel(
            id=assumption.id,
            statement=assumption.statement,
            confidence=assumption.confidence,
            impact=assumption.impact,
            reversible=assumption.reversible,
            requires_user_confirmation=assumption.requires_user_confirmation,
            linked_requirement_ids_json=json.dumps(assumption.linked_requirement_ids),
            status=assumption.status,
        )
        self.session.merge(model)
        self.session.commit()

class TaskRepository:
    def __init__(self, session: Session):
        self.session = session

    def get_by_id(self, task_id: str) -> Optional[Task]:
        statement = select(TaskModel).where(TaskModel.id == task_id)
        m = self.session.exec(statement).first()
        if not m:
            return None
        
        pf_list = [PlannedFile.model_validate(pf) for pf in json.loads(m.planned_files_json)]
        return Task.model_validate({
            "id": m.id,
            "title": m.title,
            "description": m.description,
            "requirement_ids": json.loads(m.requirement_ids_json),
            "acceptance_criterion_ids": json.loads(m.acceptance_criterion_ids_json),
            "planned_files": pf_list,
            "expected_tests": json.loads(m.expected_tests_json),
            "allowed_commands": json.loads(m.allowed_commands_json),
            "forbidden_changes": json.loads(m.forbidden_changes_json),
            "status": m.status,
        })

    def get_all(self) -> List[Task]:
        statement = select(TaskModel)
        models = self.session.exec(statement).all()
        results = []
        for m in models:
            pf_list = [PlannedFile.model_validate(pf) for pf in json.loads(m.planned_files_json)]
            results.append(Task.model_validate({
                "id": m.id,
                "title": m.title,
                "description": m.description,
                "requirement_ids": json.loads(m.requirement_ids_json),
                "acceptance_criterion_ids": json.loads(m.acceptance_criterion_ids_json),
                "planned_files": pf_list,
                "expected_tests": json.loads(m.expected_tests_json),
                "allowed_commands": json.loads(m.allowed_commands_json),
                "forbidden_changes": json.loads(m.forbidden_changes_json),
                "status": m.status,
            }))
        return results

    def save(self, task: Task):
        model = TaskModel(
            id=task.id,
            title=task.title,
            description=task.description,
            requirement_ids_json=json.dumps(task.requirement_ids),
            acceptance_criterion_ids_json=json.dumps(task.acceptance_criterion_ids),
            planned_files_json=json.dumps([pf.model_dump() for pf in task.planned_files]),
            expected_tests_json=json.dumps(task.expected_tests),
            allowed_commands_json=json.dumps(task.allowed_commands),
            forbidden_changes_json=json.dumps(task.forbidden_changes),
            status=task.status
        )
        self.session.merge(model)
        self.session.commit()

class GapRepository:
    def __init__(self, session: Session):
        self.session = session

    def get_all(self) -> List[Gap]:
        statement = select(GapModel)
        models = self.session.exec(statement).all()
        results = []
        for m in models:
            results.append(Gap.model_validate({
                "id": m.id,
                "severity": m.severity,
                "gap_type": m.gap_type,
                "requirement_id": m.requirement_id,
                "task_id": m.task_id,
                "description": m.description,
                "evidence": json.loads(m.evidence_json),
                "recommended_fix": m.recommended_fix,
                "blocking": m.blocking,
            }))
        return results

    def save(self, gap: Gap):
        model = GapModel(
            id=gap.id,
            severity=gap.severity,
            gap_type=gap.gap_type,
            requirement_id=gap.requirement_id,
            task_id=gap.task_id,
            description=gap.description,
            evidence_json=json.dumps(gap.evidence),
            recommended_fix=gap.recommended_fix,
            blocking=gap.blocking
        )
        self.session.merge(model)
        self.session.commit()

    def delete_for_task(self, task_id: str):
        self.session.exec(delete(GapModel).where(col(GapModel.task_id) == task_id))
        self.session.commit()

    def delete_plan_gaps(self):
        self.session.exec(delete(GapModel).where(col(GapModel.id).like("GAP-PLAN-%")))
        self.session.commit()

class EvidenceRepository:
    def __init__(self, session: Session):
        self.session = session

    def save_command_result(self, task_id: str, result: CommandResult):
        model = EvidenceModel(
            type="command",
            task_id=task_id,
            data_json=result.model_dump_json()
        )
        self.session.add(model)
        self.session.commit()

    def save_diff_evidence(self, ev: DiffEvidence):
        model = EvidenceModel(
            type="diff",
            task_id=ev.task_id,
            data_json=ev.model_dump_json()
        )
        self.session.add(model)
        self.session.commit()

    def save_test_evidence(self, ev: TestEvidence, task_id: Optional[str] = None):
        model = EvidenceModel(
            type="test",
            task_id=task_id,
            requirement_id=ev.requirement_id,
            acceptance_criterion_id=ev.acceptance_criterion_id,
            data_json=ev.model_dump_json()
        )
        self.session.add(model)
        self.session.commit()

    def get_all(self) -> List[Any]:
        statement = select(EvidenceModel)
        models = self.session.exec(statement).all()
        results: List[Any] = []
        for m in models:
            data = json.loads(m.data_json)
            if m.type == "command":
                results.append(CommandResult.model_validate(data))
            elif m.type == "diff":
                results.append(DiffEvidence.model_validate(data))
            elif m.type == "test":
                results.append(TestEvidence.model_validate(data))
        return results

    def delete_for_task(self, task_id: str):
        self.session.exec(delete(EvidenceModel).where(col(EvidenceModel.task_id) == task_id))
        self.session.commit()


class CritiqueFindingRepository:
    def __init__(self, session: Session):
        self.session = session

    def get_all(self) -> List[CritiqueFinding]:
        statement = select(CritiqueFindingModel)
        models = self.session.exec(statement).all()
        return [
            CritiqueFinding.model_validate({
                "id": m.id,
                "source_agent": m.source_agent,
                "target_plan_id": m.target_plan_id,
                "severity": m.severity,
                "finding_type": m.finding_type,
                "claim": m.claim,
                "linked_requirement_id": m.linked_requirement_id,
                "suggested_requirement": m.suggested_requirement,
                "suggested_task": m.suggested_task,
                "falsifiable_check": m.falsifiable_check,
                "status": m.status,
            })
            for m in models
        ]

    def save(self, finding: CritiqueFinding):
        model = CritiqueFindingModel(
            id=finding.id,
            source_agent=finding.source_agent,
            target_plan_id=finding.target_plan_id,
            severity=finding.severity,
            finding_type=finding.finding_type,
            claim=finding.claim,
            linked_requirement_id=finding.linked_requirement_id,
            suggested_requirement=finding.suggested_requirement,
            suggested_task=finding.suggested_task,
            falsifiable_check=finding.falsifiable_check,
            status=finding.status,
        )
        self.session.merge(model)
        self.session.commit()


class PlanningStateRepository:
    """Manage the persisted active plan graph."""

    def __init__(self, session: Session):
        self.session = session

    def clear_active_plan(self):
        for model in (
            EvidenceModel,
            GapModel,
            CritiqueFindingModel,
            TaskModel,
            AssumptionModel,
            RequirementModel,
        ):
            self.session.exec(delete(model))

    def replace_active_plan(
        self,
        requirements: list[Requirement],
        assumptions: list[Assumption],
        tasks: list[Task],
        findings: list[CritiqueFinding],
    ):
        self.clear_active_plan()
        for req in requirements:
            self.session.merge(RequirementModel(
                id=req.id,
                title=req.title,
                description=req.description,
                priority=req.priority,
                source=req.source,
                acceptance_criteria_json=json.dumps([ac.model_dump() for ac in req.acceptance_criteria]),
            ))

        for assumption in assumptions:
            self.session.merge(AssumptionModel(
                id=assumption.id,
                statement=assumption.statement,
                confidence=assumption.confidence,
                impact=assumption.impact,
                reversible=assumption.reversible,
                requires_user_confirmation=assumption.requires_user_confirmation,
                linked_requirement_ids_json=json.dumps(assumption.linked_requirement_ids),
                status=assumption.status,
            ))

        for task in tasks:
            self.session.merge(TaskModel(
                id=task.id,
                title=task.title,
                description=task.description,
                requirement_ids_json=json.dumps(task.requirement_ids),
                acceptance_criterion_ids_json=json.dumps(task.acceptance_criterion_ids),
                planned_files_json=json.dumps([pf.model_dump() for pf in task.planned_files]),
                expected_tests_json=json.dumps(task.expected_tests),
                allowed_commands_json=json.dumps(task.allowed_commands),
                forbidden_changes_json=json.dumps(task.forbidden_changes),
                status=task.status,
            ))

        for finding in findings:
            self.session.merge(CritiqueFindingModel(
                id=finding.id,
                source_agent=finding.source_agent,
                target_plan_id=finding.target_plan_id,
                severity=finding.severity,
                finding_type=finding.finding_type,
                claim=finding.claim,
                linked_requirement_id=finding.linked_requirement_id,
                suggested_requirement=finding.suggested_requirement,
                suggested_task=finding.suggested_task,
                falsifiable_check=finding.falsifiable_check,
                status=finding.status,
            ))

class ArtifactGraphRepository:
    def __init__(self, session: Session):
        self.session = session
        self.req_repo = RequirementRepository(session)
        self.assumption_repo = AssumptionRepository(session)
        self.task_repo = TaskRepository(session)
        self.gap_repo = GapRepository(session)
        self.evidence_repo = EvidenceRepository(session)
        self.finding_repo = CritiqueFindingRepository(session)

    def load_graph(self) -> ArtifactGraph:
        graph = ArtifactGraph()
        for req in self.req_repo.get_all():
            graph.add_requirement(req)
        for assumption in self.assumption_repo.get_all():
            graph.add_assumption(assumption)
        for task in self.task_repo.get_all():
            graph.add_task(task)
        for finding in self.finding_repo.get_all():
            graph.add_finding(finding)
        for gap in self.gap_repo.get_all():
            graph.add_gap(gap)
        for ev in self.evidence_repo.get_all():
            if isinstance(ev, CommandResult):
                graph.add_command_result(ev)
            elif isinstance(ev, DiffEvidence):
                graph.add_diff_evidence(ev)
            elif isinstance(ev, TestEvidence):
                graph.add_test_evidence(ev)
        return graph


class StateRepository:
    def __init__(self, session: Session):
        self.session = session

    def get_state(self) -> Optional[ProjectStateModel]:
        statement = select(ProjectStateModel).where(ProjectStateModel.id == "singleton")
        return self.session.exec(statement).first()

    def save_state(self, state: ProjectStateModel | str, history: Optional[List[str]] = None):
        if isinstance(state, ProjectStateModel):
            state.id = "singleton"
        else:
            state = ProjectStateModel(
                id="singleton",
                current_phase=state,
                history_json=json.dumps(history or []),
            )
        self.session.merge(state)
        self.session.commit()

    def record_phase(self, phase: str):
        current = self.get_state()
        history = []
        if current:
            history = json.loads(current.history_json)
        if not history or history[-1] != phase:
            history.append(phase)
        self.save_state(phase, history)
