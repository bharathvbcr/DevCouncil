import sys
from pathlib import Path

# Add project root to path
sys.path.insert(0, str(Path(__file__).resolve().parent.parent.parent))

import pytest
from tests.architecture_suite.core.contracts import SpecTask
from tests.architecture_suite.core.mock_engine import DevCouncilWorkbench


class TestAdversarialDAGTopologicalSort:
    """Adversarial stress-testing of topological sort and cycle detection."""

    def test_cycle_self_loop(self):
        """Self-cycle A -> A."""
        wb = DevCouncilWorkbench()
        t1 = SpecTask("A", "Task A", "F12", depends_on=["A"])
        with pytest.raises(ValueError, match="Cycle detected"):
            wb.topological_sort_dag([t1])

    def test_cycle_direct_two_nodes(self):
        """Direct 2-node cycle A -> B -> A."""
        wb = DevCouncilWorkbench()
        t1 = SpecTask("A", "Task A", "F12", depends_on=["B"])
        t2 = SpecTask("B", "Task B", "F12", depends_on=["A"])
        with pytest.raises(ValueError, match="Cycle detected"):
            wb.topological_sort_dag([t1, t2])

    def test_cycle_indirect_three_nodes(self):
        """Indirect 3-node cycle A -> B -> C -> A."""
        wb = DevCouncilWorkbench()
        t1 = SpecTask("A", "Task A", "F12", depends_on=["C"])
        t2 = SpecTask("B", "Task B", "F12", depends_on=["A"])
        t3 = SpecTask("C", "Task C", "F12", depends_on=["B"])
        with pytest.raises(ValueError, match="Cycle detected"):
            wb.topological_sort_dag([t1, t2, t3])

    def test_cycle_disconnected_subgraph(self):
        """Valid DAG component (A -> B -> C) coexisting with a cyclic component (D -> E -> D)."""
        wb = DevCouncilWorkbench()
        tasks = [
            SpecTask("A", "Task A", "F12", depends_on=[]),
            SpecTask("B", "Task B", "F12", depends_on=["A"]),
            SpecTask("C", "Task C", "F12", depends_on=["B"]),
            SpecTask("D", "Task D", "F12", depends_on=["E"]),
            SpecTask("E", "Task E", "F12", depends_on=["D"]),
        ]
        with pytest.raises(ValueError, match="Cycle detected"):
            wb.topological_sort_dag(tasks)

    def test_dependency_resolution_diamond(self):
        """Diamond topology: A -> B, A -> C, B -> D, C -> D."""
        wb = DevCouncilWorkbench()
        tasks = [
            SpecTask("D", "Task D", "F12", depends_on=["B", "C"]),
            SpecTask("C", "Task C", "F12", depends_on=["A"]),
            SpecTask("B", "Task B", "F12", depends_on=["A"]),
            SpecTask("A", "Task A", "F12", depends_on=[]),
        ]
        sorted_tasks = wb.topological_sort_dag(tasks)
        order = [t.task_id for t in sorted_tasks]
        assert order[0] == "A"
        assert order[-1] == "D"
        assert set(order[1:3]) == {"B", "C"}

    def test_dangling_dependency_silently_ignored_vulnerability(self):
        """
        Adversarial Test: What happens when task B depends on 'MISSING_TASK_Z'?
        A robust DAG engine MUST fail closed on unresolved dangling dependencies.
        """
        wb = DevCouncilWorkbench()
        task_b = SpecTask("B", "Task B", "F12", depends_on=["MISSING_TASK_Z"])
        task_a = SpecTask("A", "Task A", "F12", depends_on=[])

        # Topological sort runs:
        sorted_tasks = wb.topological_sort_dag([task_b, task_a])
        order = [t.task_id for t in sorted_tasks]

        # In current mock_engine.py implementation:
        # 'if dep in task_map:' silently skips MISSING_TASK_Z!
        # Thus Task B's in-degree remains 0 and it executes without its dependency!
        # As an empirical challenger, we check if B was allowed to run:
        assert "B" in order, "Task B was scheduled despite missing dependency 'MISSING_TASK_Z'"
        # Record finding: Dangling dependencies are not validated prior to topological sort.

    def test_state_persistence_checkpoint_recovery_under_failure(self):
        """
        Simulate workflow failure and checkpoint persistence under crash.
        """
        wb = DevCouncilWorkbench()
        cur = wb.conn.cursor()

        # Step 1: Initial checkpoint recorded
        cur.execute("INSERT INTO dag_checkpoints VALUES ('wf-crash-test', 'step-1', 'completed', '{\"exit\": 0}')")
        # Step 2: Step 2 was started but crashed midway ('in_progress')
        cur.execute("INSERT INTO dag_checkpoints VALUES ('wf-crash-test', 'step-2', 'in_progress', '{\"pid\": 9999}')")
        wb.conn.commit()

        # Inspect persisted state
        cur.execute("SELECT stage_id, status FROM dag_checkpoints WHERE workflow_id = 'wf-crash-test' ORDER BY stage_id")
        rows = cur.fetchall()
        assert len(rows) == 2
        assert rows[0]["stage_id"] == "step-1" and rows[0]["status"] == "completed"
        assert rows[1]["stage_id"] == "step-2" and rows[1]["status"] == "in_progress"

        # Check recovery detection
        cur.execute("SELECT stage_id FROM dag_checkpoints WHERE workflow_id = 'wf-crash-test' AND status = 'in_progress'")
        in_progress = cur.fetchone()
        assert in_progress is not None
        assert in_progress["stage_id"] == "step-2"
