"""Performance and memory guards for deploy-safe repository-map caps."""

from __future__ import annotations


import pytest
from pydantic import ValidationError

from devcouncil.app.config import IndexingConfig
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
    # The dependents cap belonged to the Python map writer; the kernel now
    # writes ``dependents`` and ``RepoMapper`` no longer holds ``_DEPENDENTS_MAX``.
    assert not hasattr(mapper, "_DEPENDENTS_MAX")
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


