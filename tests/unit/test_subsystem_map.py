"""Unit tests for the shared subsystem/area repo-map helpers."""

from devcouncil.indexing.subsystem_map import (
    are_neighbors,
    area_for_path,
    areas_touched,
    cross_boundary_pairs,
    dependents_of,
    impact_targets,
    neighbors_for_area,
)

_MAP = {
    "files": [{"path": "scripts/tool.py", "area": "scripts"}],
    "subsystems": [
        {"area": "src/ui", "neighbors": ["src/api"]},
        {"area": "src/api", "neighbors": ["src/ui", "src/storage"]},
        {"area": "src/storage", "neighbors": ["src/api"]},
    ],
    "dependents": {"src/api/handler.py": ["src/ui/view.py"]},
}


def test_area_for_path_prefers_longest_prefix():
    assert area_for_path("src/api/handler.py", _MAP) == "src/api"
    assert area_for_path("src/ui/x.py", _MAP) == "src/ui"


def test_area_for_path_falls_back_to_file_entry():
    assert area_for_path("scripts/tool.py", _MAP) == "scripts"


def test_area_for_path_unknown_is_none():
    assert area_for_path("random/unknown.py", _MAP) is None


def test_neighbors_and_adjacency():
    assert neighbors_for_area("src/api", _MAP) == ["src/ui", "src/storage"]
    assert are_neighbors("src/ui", "src/api", _MAP) is True
    assert are_neighbors("src/ui", "src/storage", _MAP) is False
    # Same area is trivially adjacent; unknown side never flags.
    assert are_neighbors("src/ui", "src/ui", _MAP) is True
    assert are_neighbors("src/ui", None, _MAP) is True


def test_dependents_and_impact_targets():
    assert dependents_of("src/api/handler.py", _MAP) == ["src/ui/view.py"]
    deps, neighbors = impact_targets("src/api/handler.py", _MAP)
    assert deps == ["src/ui/view.py"]
    assert neighbors == ["src/ui", "src/storage"]


def test_areas_touched_and_cross_boundary_pairs():
    paths = ["src/ui/a.py", "src/storage/b.py", "src/api/c.py"]
    assert areas_touched(paths, _MAP) == ["src/api", "src/storage", "src/ui"]
    # ui<->storage is the only non-neighbor pair among the three.
    assert cross_boundary_pairs(paths, _MAP) == [("src/storage", "src/ui")]


def test_cross_boundary_pairs_empty_when_all_neighbors():
    assert cross_boundary_pairs(["src/ui/a.py", "src/api/b.py"], _MAP) == []
