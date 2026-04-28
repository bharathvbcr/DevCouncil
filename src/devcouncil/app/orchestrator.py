import logging
import json
from pathlib import Path
from typing import Optional, Any

from devcouncil.app.state_machine import StateMachine, ProjectPhase
from devcouncil.app.run_context import RunContext
from devcouncil.app.events import bus, EventTypes
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import StateRepository

logger = logging.getLogger(__name__)

class Orchestrator:
    """Central orchestrator managing the workflow lifecycle and state transitions."""
    def __init__(self, project_root: Path, persist_state: bool = True):
        self.project_root = project_root
        self.persist_state = persist_state
        
        db = get_db(self.project_root)
        if db:
            with db.get_session() as session:
                repo = StateRepository(session)
                state = repo.get_state()
                if state:
                    self.state_machine = StateMachine(ProjectPhase(state.current_phase))
                    self.state_machine._history = [ProjectPhase(p) for p in json.loads(state.history_json)]
                else:
                    self.state_machine = StateMachine(ProjectPhase.NEW)
        else:
            self.state_machine = StateMachine(ProjectPhase.NEW)
            
        self.current_run: Optional[RunContext] = None

    def reset_state_machine(self, initial: ProjectPhase = ProjectPhase.NEW):
        """Start a new lifecycle sequence without depending on prior persisted phase."""
        self.state_machine = StateMachine(initial)

    async def start_run(self, run_id: str, goal: str) -> RunContext:
        """Start a new orchestration run."""
        self.current_run = RunContext(
            run_id=run_id,
            project_root=str(self.project_root),
            goal=goal
        )
        self.current_run.initialize()
        
        await bus.emit(EventTypes.PLANNING_STARTED, {"run_id": run_id, "goal": goal})
        return self.current_run

    async def transition_to(self, target_phase: ProjectPhase):
        """Transition the project phase and emit events."""
        old_phase = self.state_machine.phase
        self.state_machine.transition(target_phase)
        
        db = get_db(self.project_root)
        if db and self.persist_state:
            with db.get_session() as session:
                repo = StateRepository(session)
                repo.save_state(
                    self.state_machine.phase.value,
                    [p.value for p in self.state_machine.history]
                )
                
        logger.info(f"Transitioned from {old_phase.value} to {target_phase.value}")
        
        if target_phase == ProjectPhase.PLAN_APPROVED:
            await bus.emit(EventTypes.PLANNING_COMPLETED, {"run_id": self.current_run.run_id if self.current_run else None})

    def save_run_artifact(self, name: str, data: Any, is_json: bool = True):
        """Save a local artifact specific to this run (in .devcouncil/runs/)."""
        if not self.current_run:
            logger.warning("Attempted to save artifact without an active run context.")
            return
            
        if is_json:
            self.current_run.save_json_artifact(name, data)
        else:
            self.current_run.save_artifact(name, data)
