import json
from unittest.mock import AsyncMock, MagicMock

import pytest

from devcouncil.app.orchestrator import Orchestrator
from devcouncil.app.state_machine import InvalidTransitionError, ProjectPhase
from devcouncil.app.events import EventTypes


@pytest.fixture
def anyio_backend():
    return "asyncio"


def _mock_db_with_phase(phase: str = "NEW", history: list[str] | None = None):
    db = MagicMock()
    session = MagicMock()
    db.get_session.return_value.__enter__.return_value = session
    db.get_session.return_value.__exit__.return_value = False

    state = MagicMock()
    state.current_phase = phase
    state.history_json = json.dumps(history or [phase])

    repo = MagicMock()
    repo.get_state.return_value = state
    return db, session, repo


@pytest.mark.anyio
async def test_orchestrator_transition_to_persists_phase_and_emits_event(tmp_path, monkeypatch):
    db, session, repo = _mock_db_with_phase("NEW", ["NEW"])
    monkeypatch.setattr("devcouncil.app.orchestrator.get_db", lambda root: db)
    monkeypatch.setattr("devcouncil.app.orchestrator.StateRepository", lambda s: repo)

    emitted = []
    monkeypatch.setattr(
        "devcouncil.app.orchestrator.bus.emit",
        AsyncMock(side_effect=lambda event, payload: emitted.append((event, payload))),
    )
    trace_calls = []
    monkeypatch.setattr(
        "devcouncil.app.orchestrator.TraceLogger",
        lambda root: MagicMock(log_event=lambda *a, **k: trace_calls.append((a, k))),
    )

    orch = Orchestrator(tmp_path, persist_state=True)
    orch.current_run = MagicMock(run_id="run-1")

    await orch.transition_to(ProjectPhase.REPO_MAPPED)

    assert orch.state_machine.phase == ProjectPhase.REPO_MAPPED
    repo.save_state.assert_called_once_with(
        ProjectPhase.REPO_MAPPED.value,
        [ProjectPhase.NEW.value, ProjectPhase.REPO_MAPPED.value],
    )
    assert trace_calls
    assert trace_calls[0][0][0] == "phase_transition"


@pytest.mark.anyio
async def test_orchestrator_start_run_initializes_context_and_logs_trace(tmp_path, monkeypatch):
    db, _, repo = _mock_db_with_phase()
    monkeypatch.setattr("devcouncil.app.orchestrator.get_db", lambda root: db)
    monkeypatch.setattr("devcouncil.app.orchestrator.StateRepository", lambda s: repo)

    emitted = []
    monkeypatch.setattr(
        "devcouncil.app.orchestrator.bus.emit",
        AsyncMock(side_effect=lambda event, payload: emitted.append((event, payload))),
    )
    trace_logger = MagicMock()
    monkeypatch.setattr("devcouncil.app.orchestrator.TraceLogger", lambda root: trace_logger)

    orch = Orchestrator(tmp_path, persist_state=False)
    run = await orch.start_run("run-abc", "ship feature")

    assert run.run_id == "run-abc"
    assert run.goal == "ship feature"
    assert run.run_dir.exists()
    trace_logger.log_event.assert_called_once()
    assert emitted == [(EventTypes.PLANNING_STARTED, {"run_id": "run-abc", "goal": "ship feature"})]


@pytest.mark.anyio
async def test_orchestrator_invalid_transition_raises_without_persisting(tmp_path, monkeypatch):
    db, _, repo = _mock_db_with_phase("NEW", ["NEW"])
    monkeypatch.setattr("devcouncil.app.orchestrator.get_db", lambda root: db)
    monkeypatch.setattr("devcouncil.app.orchestrator.StateRepository", lambda s: repo)

    orch = Orchestrator(tmp_path, persist_state=True)

    with pytest.raises(InvalidTransitionError):
        await orch.transition_to(ProjectPhase.TASK_VERIFIED)

    repo.save_state.assert_not_called()
    assert orch.state_machine.phase == ProjectPhase.NEW
