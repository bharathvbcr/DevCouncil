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
        "  repo_map_liveness_cap: 50000\n",
        encoding="utf-8",
    )

    mapper = RepoMapper(tmp_path)

    assert mapper._LIVENESS_CAP == 50_000
    assert mapper.max_map_size == 100_000
    with pytest.raises(ValidationError):
        IndexingConfig(repo_map_liveness_cap=100_001)
    # The dependents and unwired caps went with the Python map writer: the kernel
    # writes ``dependents`` and ``unwired_candidates`` now and reads neither knob,
    # and a setting that is validated and then ignored is worse than one that is
    # rejected. ``repo_map_liveness_cap`` above is the one that is still read.
    for retired in ("repo_map_unwired_cap", "repo_map_dependents_cap"):
        assert retired not in IndexingConfig.model_fields


def test_repository_mapper_compatibility_alias_defaults_to_current_directory(
    tmp_path, monkeypatch
):
    from devcouncil.indexing.repo_mapper import RepositoryMapper

    monkeypatch.chdir(tmp_path)
    mapper = RepositoryMapper()

    assert mapper.project_root == tmp_path
    assert mapper.max_map_size > 10_000



def test_a_config_that_still_sets_a_retired_cap_is_told_so(tmp_path, caplog):
    """A removed key must not go quiet.

    Pydantic's default ``extra="ignore"`` drops an unknown key without a word, so a
    config carrying ``repo_map_dependents_cap`` from before the Python map writer was
    retired would keep looking effective while doing nothing. Fails against the
    pre-removal code, where the key was real and no warning existed.
    """
    import logging

    from devcouncil.app.config import RETIRED_CONFIG_KEYS, load_config

    state = tmp_path / ".devcouncil"
    state.mkdir()
    (state / "config.yaml").write_text(
        "indexing:\n"
        "  repo_map_liveness_cap: 50000\n"
        "  repo_map_unwired_cap: 5000\n"
        "  repo_map_dependents_cap: 2048\n",
        encoding="utf-8",
    )

    with caplog.at_level(logging.WARNING, logger="devcouncil.app.config"):
        config = load_config(tmp_path)

    # The surviving knob still takes effect; only the retired ones are inert.
    assert config.indexing.repo_map_liveness_cap == 50_000
    warned = " ".join(record.getMessage() for record in caplog.records)
    for retired in ("indexing.repo_map_unwired_cap", "indexing.repo_map_dependents_cap"):
        assert retired in RETIRED_CONFIG_KEYS
        assert retired in warned, f"{retired} was dropped silently"
