"""Rank 13 (targeted) — the model-call cost ledger is written under the provider's
project root, not the process cwd."""

from devcouncil.llm.provider import OpenRouterProvider, _log_model_call


def test_log_model_call_writes_under_project_root(tmp_path, monkeypatch):
    monkeypatch.delenv("DEVCOUNCIL_LOG_DIR", raising=False)
    other = tmp_path / "elsewhere"
    other.mkdir()
    monkeypatch.chdir(other)  # cwd deliberately different from the project root

    project = tmp_path / "project"
    project.mkdir()
    _log_model_call({"model": "m"}, {"choices": []}, {"total_tokens": 10}, project)

    ledger = project / ".devcouncil" / "logs" / "model_calls.jsonl"
    assert ledger.exists(), "ledger must live under the project root"
    assert not (other / ".devcouncil").exists(), "must not log to the cwd"


def test_provider_carries_project_root(tmp_path):
    provider = OpenRouterProvider("key", project_root=tmp_path)
    assert provider.project_root == tmp_path
