import sys
from pathlib import Path

# Ensure directory is on python path
sys.path.insert(0, str(Path(__file__).resolve().parent))

import pytest
from topology_a_hierarchy import TopologyAHierarchy
from topology_b_namesakes import TopologyBNamesakes
from topology_c_fanout import TopologyCFanout, DISPATCH_FANOUT_CAP
from topology_d_deadcode import TopologyDDeadcode
from topology_e_clones import TopologyEClones
from stress_harness import DcStressHarness


def test_topology_a_deep_hierarchy_ladder():
    topo = TopologyAHierarchy(depth=10)
    result = topo.verify()
    assert result["status"] == "PASS"
    assert result["hierarchy_depth"] == 10
    assert result["no_cycles"] is True
    assert result["leaf"] == "ClassLevel_9"
    assert result["root"] == "ClassLevel_0"


def test_topology_b_dense_namesake_matrix():
    topo = TopologyBNamesakes(num_packages=10, structs_per_pkg=50)
    result = topo.verify()
    assert result["status"] == "PASS"
    assert result["total_types"] == 500
    assert result["total_methods"] == 2000
    assert result["false_unique_globals"] == 0
    assert result["zero_false_unique_globals"] is True


def test_topology_c_interface_fanout_cap():
    topo = TopologyCFanout(num_interfaces=5, impls_per_interface=20)
    result = topo.verify()
    assert result["status"] == "PASS"
    assert result["interfaces_tested"] == 5
    assert result["fanout_cap_enforced"] == DISPATCH_FANOUT_CAP
    assert result["edge_count_per_call"] == 16


def test_topology_d_dead_code_islands_and_clusters():
    topo = TopologyDDeadcode()
    result = topo.verify()
    assert result["status"] == "PASS"
    assert result["dead_symbols_count"] == 100
    assert result["precision"] == 1.0
    assert result["recall"] == 1.0
    assert result["mutual_recursion_handled"] is True
    assert result["clusters_handled"] is True


def test_topology_e_clone_matrix_and_noise_suppression():
    topo = TopologyEClones()
    result = topo.verify()
    assert result["status"] == "PASS"
    assert result["t1_clones_recall"] == 1.0
    assert result["t2_clones_recall"] == 1.0
    assert result["trivial_accessors_suppressed"] == 50
    assert result["trivial_noise_floor"] == 0.0


def test_dc_stress_wal_concurrency():
    harness = DcStressHarness()
    try:
        # Run 8 concurrent readers with 1 continuous writer
        metrics = harness.run_wal_concurrency_stress(num_readers=8, writer_iterations=100)
        assert metrics["zero_sqlite_busy"] is True, f"Encountered {metrics['sqlite_busy_errors']} SQLITE_BUSY errors"
        assert metrics["sqlite_busy_errors"] == 0
        assert metrics["total_reads_completed"] > 0
    finally:
        harness.cleanup()


def test_dc_stress_rss_memory_bounds():
    harness = DcStressHarness()
    try:
        mem = harness.verify_memory_bounds()
        assert mem["current_rss_mb"] < mem["ceiling_large_mb"]
    finally:
        harness.cleanup()
