"""Performance and memory guards for deploy-safe repository-map caps."""

from __future__ import annotations

import json
import time
import tracemalloc

import pytest
from pydantic import ValidationError

from devcouncil.app.config import IndexingConfig
from devcouncil.indexing.graph.liveness import apply_liveness_cap
from devcouncil.indexing.repo_mapper import RepoMapper


def test_repo_map_caps_are_configurable_and_bounded(tmp_path):
    state = tmp_path / ".devcouncil"
    state.mkdir()
    (state / "config.yaml").write_text(
        "indexing:\n"
        "  repo_map_liveness_cap: 50000\n"
        "  repo_map_dependents_cap: 2048\n",
        encoding="utf-8",
    )

    mapper = RepoMapper(tmp_path)

    assert mapper._LIVENESS_CAP == 50_000
    assert mapper._DEPENDENTS_MAX == 2_048
    assert mapper.max_map_size == 100_000
    with pytest.raises(ValidationError):
        IndexingConfig(repo_map_liveness_cap=100_001)
    with pytest.raises(ValidationError):
        IndexingConfig(repo_map_dependents_cap=4_097)


def test_repository_mapper_compatibility_alias_defaults_to_current_directory(
    tmp_path, monkeypatch
):
    from devcouncil.indexing.repo_mapper import RepositoryMapper

    monkeypatch.chdir(tmp_path)
    mapper = RepositoryMapper()

    assert mapper.project_root == tmp_path
    assert mapper.max_map_size > 10_000


def test_large_liveness_cap_stays_within_memory_and_runtime_budget():
    candidates = [f"src/package/module_{index}.py" for index in range(120_000)]

    tracemalloc.start()
    started = time.perf_counter()
    capped, meta = apply_liveness_cap(candidates, 20_000)
    encoded = json.dumps(capped)
    elapsed = time.perf_counter() - started
    _, peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()

    assert len(capped) == 20_000
    assert meta == {"total": 120_000, "shown": 20_000, "truncated": 100_000}
    assert encoded.startswith('["src/package/module_0.py"')
    assert elapsed < 5.0
    assert peak < 32 * 1024 * 1024
