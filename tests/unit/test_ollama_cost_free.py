"""Ollama calls are always free in the cost ledger.

Regression guard for the review finding: an Ollama model echoes back an open-ended
bare tag (e.g. ``mistral:latest``) with NO ``ollama/`` prefix, so model-id matching
alone would bill it at DEFAULT_PRICING. The ledger trusts the recorded ``provider``
instead.
"""

import json

import pytest

from devcouncil.telemetry.cost import _model_calls_file, group_cost


@pytest.fixture(autouse=True)
def _isolated_cost_ledger(tmp_path, monkeypatch):
    log_dir = tmp_path / ".devcouncil" / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("DEVCOUNCIL_LOG_DIR", str(log_dir))


def _write_ledger(root, records):
    log_file = _model_calls_file(root)
    log_file.parent.mkdir(parents=True, exist_ok=True)
    log_file.write_text(
        "\n".join(json.dumps(r) for r in records) + "\n", encoding="utf-8"
    )


def test_ollama_record_is_free_even_with_bare_tag(tmp_path):
    _write_ledger(tmp_path, [{
        "provider": "ollama",
        "response": {"model": "mistral:latest"},  # bare tag, not on any curated list
        "usage": {"prompt_tokens": 5000, "completion_tokens": 5000},
        "task_id": "T1",
    }])
    summary = group_cost(tmp_path)
    assert summary["total_cost"] == 0.0
    assert summary["by_task"]["T1"]["cost"] == 0.0


def test_non_ollama_record_still_costs(tmp_path):
    _write_ledger(tmp_path, [{
        "provider": "openrouter",
        "response": {"model": "some/unknown-model"},
        "usage": {"prompt_tokens": 1000, "completion_tokens": 1000},
        "task_id": "T1",
    }])
    summary = group_cost(tmp_path)
    assert summary["total_cost"] > 0.0
