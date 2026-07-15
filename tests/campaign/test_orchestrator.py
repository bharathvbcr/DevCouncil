"""Campaign orchestration: parallel dispatch, dependency waves, QC gating."""

from __future__ import annotations

import threading
from typing import List, Set, Tuple

from devcouncil.domain.task import PlannedFile, Task
from devcouncil.campaign.mailbox import Mailbox
from devcouncil.campaign.orchestrator import CampaignResult, Campaign, build_verifier_fn


class _Result:
    def __init__(self, success: bool, message: str = ""):
        self.success = success
        self.message = message


class FakeExecutorFactory:
    """Records which owner ran which task; success configurable per task id."""

    def __init__(self, fail: Set[str] | None = None):
        self.fail = fail or set()
        self.calls: List[Tuple[str, str]] = []  # (owner, task_id)
        self._lock = threading.Lock()

    def __call__(self, owner: str):
        factory = self

        class _Exec:
            def run_task(self, task: Task, requirements):
                with factory._lock:
                    factory.calls.append((owner, task.id))
                return _Result(task.id not in factory.fail, f"ran {task.id}")

        return _Exec()


def _verify_all_pass(task, reqs):
    return True, []


def _verify_blocking(blocked: Set[str]):
    def _fn(task, reqs):
        if task.id in blocked:
            return False, [f"{task.id} missing acceptance evidence"]
        return True, []

    return _fn


def _task(tid, title="Implement thing", desc="apply", deps=None, status="planned") -> Task:
    return Task(id=tid, title=title, description=desc, depends_on=deps or [], status=status)


def test_all_tasks_verified_and_dashboard_written(tmp_path):
    tasks = [_task("T1"), _task("T2"), _task("T3")]
    fac = FakeExecutorFactory()
    camp = Campaign(
        tmp_path, goal="Ship", tasks=tasks, executor_factory=fac, verify_fn=_verify_all_pass, num_workers=3
    )
    result = camp.run()
    assert isinstance(result, CampaignResult)
    assert set(result.verified) == {"T1", "T2", "T3"}
    assert result.success is True
    assert result.dashboard_path is not None and result.dashboard_path.exists()
    board = result.dashboard_path.read_text()
    assert "Achievements" in board and "T1" in board
    # Each task actually ran through an executor.
    assert {c[1] for c in fac.calls} == {"T1", "T2", "T3"}


def test_dependencies_run_in_order(tmp_path):
    # T2 depends on T1, T3 depends on T2 -> strictly sequential.
    tasks = [_task("T3", deps=["T2"]), _task("T1"), _task("T2", deps=["T1"])]
    fac = FakeExecutorFactory()
    camp = Campaign(tmp_path, goal="Chain", tasks=tasks, executor_factory=fac, verify_fn=_verify_all_pass)
    result = camp.run()
    order = [c[1] for c in fac.calls]
    assert order.index("T1") < order.index("T2") < order.index("T3")
    assert set(result.verified) == {"T1", "T2", "T3"}


def test_blocked_prerequisite_skips_dependents(tmp_path):
    tasks = [_task("T1"), _task("T2", deps=["T1"])]
    fac = FakeExecutorFactory()
    camp = Campaign(
        tmp_path,
        goal="Gate",
        tasks=tasks,
        executor_factory=fac,
        verify_fn=_verify_blocking({"T1"}),
    )
    result = camp.run()
    assert result.blocked == ["T1"]
    assert result.skipped == ["T2"]
    # T2 must never have been dispatched, since its prerequisite failed QC.
    assert all(call[1] != "T2" for call in fac.calls)
    assert result.success is False


def test_execution_failure_marks_failed(tmp_path):
    tasks = [_task("T1")]
    fac = FakeExecutorFactory(fail={"T1"})
    camp = Campaign(tmp_path, goal="x", tasks=tasks, executor_factory=fac, verify_fn=_verify_all_pass)
    result = camp.run()
    assert result.outcomes[0].status == "failed"
    assert result.outcomes[0].executed is False


def test_unmet_external_dependency_is_skipped(tmp_path):
    tasks = [_task("T1", deps=["NOPE"])]
    camp = Campaign(tmp_path, goal="x", tasks=tasks, verify_fn=_verify_all_pass)
    result = camp.run()
    assert result.skipped == ["T1"]


def test_cognition_task_routes_to_reviewer(tmp_path):
    tasks = [_task("T1", title="Design the auth architecture", desc="architecture")]
    fac = FakeExecutorFactory()
    camp = Campaign(tmp_path, goal="x", tasks=tasks, executor_factory=fac, verify_fn=_verify_all_pass)
    result = camp.run()
    assert result.outcomes[0].owner == "reviewer"
    assert result.outcomes[0].bloom == "Create"


def test_mailbox_traffic_follows_chain_of_command(tmp_path):
    tasks = [_task("T1")]
    mb = Mailbox(tmp_path)
    camp = Campaign(
        tmp_path, goal="Order66", tasks=tasks, verify_fn=_verify_all_pass, mailbox=mb
    )
    camp.run()
    coordinator_types = {m.type for m in mb.all("coordinator")}
    assert "cmd_new" in coordinator_types      # Director -> Coordinator
    assert "qc_result" in coordinator_types    # Reviewer -> Coordinator
    assert any(m.type == "report_received" for m in mb.all("reviewer"))  # Worker -> Reviewer
    assert any(m.type == "task_assigned" for m in mb.all("worker1"))  # Coordinator -> Worker


def test_on_task_update_receives_status_transitions(tmp_path):
    seen: List[str] = []
    tasks = [_task("T1")]
    camp = Campaign(
        tmp_path,
        goal="x",
        tasks=tasks,
        verify_fn=_verify_all_pass,
        on_task_update=lambda t: seen.append(t.status),
    )
    camp.run()
    assert "running" in seen and "verified" in seen


def test_executor_exception_is_contained(tmp_path):
    def boom(owner):
        class _Exec:
            def run_task(self, task, reqs):
                raise RuntimeError("cli exploded")

        return _Exec()

    tasks = [_task("T1"), _task("T2")]
    camp = Campaign(tmp_path, goal="x", tasks=tasks, executor_factory=boom, verify_fn=_verify_all_pass)
    result = camp.run()
    # A crashing executor blocks its own task but does not abort the campaign.
    assert {o.status for o in result.outcomes} == {"failed"}
    assert len(result.outcomes) == 2


def test_quality_control_is_serialized_under_parallel_execution(tmp_path):
    # Execution runs in parallel, but the Reviewer's verify gate must not overlap
    # (it touches the shared git tree). Track concurrent verify entries.
    import threading
    import time

    state = {"active": 0, "max": 0}
    lock = threading.Lock()

    def verify(task, reqs):
        with lock:
            state["active"] += 1
            state["max"] = max(state["max"], state["active"])
        time.sleep(0.02)  # widen the window so an unguarded overlap would show
        with lock:
            state["active"] -= 1
        return True, []

    tasks = [_task(f"T{i}") for i in range(4)]
    camp = Campaign(
        tmp_path,
        goal="x",
        tasks=tasks,
        executor_factory=FakeExecutorFactory(),
        verify_fn=verify,
        num_workers=4,
        max_parallel=4,
    )
    result = camp.run()
    assert state["max"] == 1  # QC never ran two at once
    assert len(result.verified) == 4


def test_parallel_verify_opt_out_allows_overlap(tmp_path):
    # With serialization disabled the lock is not taken; the campaign still works.
    tasks = [_task("T1"), _task("T2")]
    camp = Campaign(
        tmp_path,
        goal="x",
        tasks=tasks,
        executor_factory=FakeExecutorFactory(),
        verify_fn=_verify_all_pass,
        verify_serialized=False,
    )
    result = camp.run()
    assert set(result.verified) == {"T1", "T2"}


def test_already_done_tasks_satisfy_dependencies(tmp_path):
    tasks = [_task("T1", status="done"), _task("T2", deps=["T1"])]
    fac = FakeExecutorFactory()
    camp = Campaign(tmp_path, goal="x", tasks=tasks, executor_factory=fac, verify_fn=_verify_all_pass)
    result = camp.run()
    # T1 was already done; only T2 gets dispatched, and it is not skipped.
    assert [c[1] for c in fac.calls] == ["T2"]
    assert "T2" in result.verified


def test_parallel_state_counts_match_outcomes(tmp_path):
    """Shared achievements/blocked lists stay consistent under parallel workers."""
    tasks = [_task(f"T{i}") for i in range(8)]
    blocked_ids = {"T2", "T5"}
    fac = FakeExecutorFactory()
    camp = Campaign(
        tmp_path,
        goal="parallel state",
        tasks=tasks,
        executor_factory=fac,
        verify_fn=_verify_blocking(blocked_ids),
        num_workers=4,
        max_parallel=4,
    )
    result = camp.run()
    verified = [o for o in result.outcomes if o.status == "verified"]
    blocked = [o for o in result.outcomes if o.status == "blocked"]
    assert len(camp._state.achievements) == len(verified)
    assert len(camp._state.blocked) == len(blocked)
    assert {o.task_id for o in verified} == {f"T{i}" for i in range(8) if f"T{i}" not in blocked_ids}


def test_build_verifier_fn_concurrent_threads_no_runtime_error(tmp_path, monkeypatch):
    """async QC via build_verifier_fn must not leak asyncio.run errors from pool threads."""
    import asyncio

    class _FakeVerifier:
        def __init__(self, root, router=None):
            pass

        async def verify_task(self, task, requirements):
            await asyncio.sleep(0)
            return [], []

    monkeypatch.setattr("devcouncil.verification.verifier.Verifier", _FakeVerifier)
    verify = build_verifier_fn(tmp_path)
    task = _task("T1")
    errors: list[str] = []

    def _call():
        try:
            verify(task, [])
        except Exception as exc:
            errors.append(str(exc))

    threads = [threading.Thread(target=_call) for _ in range(2)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert not errors
    assert verify(task, []) == (True, [])


def _task_with_file(tid, path, deps=None) -> Task:
    return Task(
        id=tid,
        title=f"Edit {path}",
        description="apply",
        depends_on=deps or [],
        planned_files=[PlannedFile(path=path, reason="edit", allowed_change="modify")],
    )


def test_overlapping_planned_files_run_sequentially(tmp_path):
    """Two tasks touching the same file must not execute concurrently."""
    import time

    state = {"active": 0, "max": 0}
    exec_lock = threading.Lock()

    class _TrackingFactory:
        def __call__(self, owner: str):
            class _Exec:
                def run_task(self, task: Task, requirements):
                    with exec_lock:
                        state["active"] += 1
                        state["max"] = max(state["max"], state["active"])
                    time.sleep(0.03)
                    with exec_lock:
                        state["active"] -= 1
                    return _Result(True, f"ran {task.id}")

            return _Exec()

    tasks = [_task_with_file("T1", "src/shared.py"), _task_with_file("T2", "src/shared.py")]
    camp = Campaign(
        tmp_path,
        goal="overlap",
        tasks=tasks,
        executor_factory=_TrackingFactory(),
        verify_fn=_verify_all_pass,
        max_parallel=2,
    )
    result = camp.run()
    assert state["max"] == 1
    assert set(result.verified) == {"T1", "T2"}
    assert any("overlapping planned_files" in e for e in camp.events)


def test_budget_halt_marks_remaining_skipped(tmp_path):
    """3 tasks, budget triggers after wave 1 — remaining tasks skipped, success=False."""
    from unittest.mock import patch

    tasks = [_task("T1"), _task("T2"), _task("T3")]
    fac = FakeExecutorFactory()
    wave_checks = {"n": 0}

    def over_budget_after_first_wave(self):
        wave_checks["n"] += 1
        return wave_checks["n"] >= 1

    camp = Campaign(
        tmp_path,
        goal="Budget test",
        tasks=tasks,
        executor_factory=fac,
        verify_fn=_verify_all_pass,
        max_parallel=1,
        cost_budget_usd=1.0,
    )
    with patch.object(Campaign, "_over_budget", over_budget_after_first_wave):
        result = camp.run()

    assert result.halted is True
    assert result.halt_reason == "cost budget exceeded"
    assert result.success is False
    assert len(result.outcomes) == 3
    assert result.verified == ["T1"]
    skipped = [o for o in result.outcomes if o.status == "skipped"]
    assert len(skipped) == 2
    assert all(o.message == "campaign halted: cost budget exceeded" for o in skipped)
    assert {o.task_id for o in skipped} == {"T2", "T3"}
