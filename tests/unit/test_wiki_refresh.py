"""Unit tests for the post-verify wiki-refresh trigger."""

import json
from importlib import resources

from devcouncil.app.config import WikiRefreshConfig
from devcouncil.reporting.report_builder import ReportBuilder
from devcouncil.verification.wiki_refresh import (
    WikiRefreshOutcome,
    evaluate_wiki_refresh,
    is_large_change,
    wiki_refresh_advisory_gap,
)


def _write_map(tmp_path, areas):
    (tmp_path / ".devcouncil").mkdir(exist_ok=True)
    data = {
        "files": [],
        "subsystems": [{"area": a, "neighbors": []} for a in areas],
    }
    (tmp_path / ".devcouncil" / "repo_map.json").write_text(json.dumps(data), encoding="utf-8")


def test_is_large_change_thresholds():
    assert is_large_change(files_touched=8, subsystems_touched=1, min_files=8, min_subsystems=3)
    assert is_large_change(files_touched=1, subsystems_touched=3, min_files=8, min_subsystems=3)
    assert not is_large_change(files_touched=2, subsystems_touched=1, min_files=8, min_subsystems=3)


def test_small_change_is_not_considered(tmp_path):
    _write_map(tmp_path, ["src/a", "src/b"])
    outcome = evaluate_wiki_refresh(
        tmp_path, ["src/a/one.py"], config=WikiRefreshConfig(),
    )
    assert outcome.considered is False
    assert "below threshold" in outcome.reason


def test_disabled_config_short_circuits(tmp_path):
    _write_map(tmp_path, ["src/a"])
    outcome = evaluate_wiki_refresh(
        tmp_path, ["src/a/f%d.py" % i for i in range(20)],
        config=WikiRefreshConfig(enabled=False),
    )
    assert outcome.considered is False
    assert "disabled" in outcome.reason


def test_many_files_triggers_consideration_and_flags_stale(tmp_path):
    _write_map(tmp_path, ["src/a"])
    files = ["src/a/f%d.py" % i for i in range(10)]
    outcome = evaluate_wiki_refresh(
        tmp_path, files, config=WikiRefreshConfig(min_files=8, auto_update=False),
    )
    assert outcome.considered is True
    assert outcome.triggered is False
    assert outcome.files_touched == 10
    # No wiki exists in the fixture, so stale-page detection is empty but non-crashing.
    assert isinstance(outcome.stale_pages, list)


def test_many_subsystems_triggers_consideration(tmp_path):
    _write_map(tmp_path, ["src/a", "src/b", "src/c"])
    outcome = evaluate_wiki_refresh(
        tmp_path,
        ["src/a/x.py", "src/b/y.py", "src/c/z.py"],
        config=WikiRefreshConfig(min_files=99, min_subsystems=3),
    )
    assert outcome.considered is True
    assert outcome.subsystems_touched == 3


def test_auto_update_invokes_library_refresh(tmp_path, monkeypatch):
    _write_map(tmp_path, ["src/a"])
    calls = {}

    def fake_refresh(root, *, llm, force, remap):
        calls["root"] = root
        calls["llm"] = llm
        calls["force"] = force
        calls["remap"] = remap

    import devcouncil.knowledge.wiki as wiki_lib

    monkeypatch.setattr(wiki_lib, "refresh_wiki", fake_refresh)

    outcome = evaluate_wiki_refresh(
        tmp_path, ["src/a/f%d.py" % i for i in range(10)],
        config=WikiRefreshConfig(min_files=8, auto_update=True),
    )
    assert outcome.considered is True
    assert outcome.triggered is True
    assert calls["llm"] is False
    assert calls["force"] is False
    assert calls["remap"] is False


def test_auto_update_does_not_call_console_print(tmp_path, monkeypatch):
    _write_map(tmp_path, ["src/a"])
    import devcouncil.knowledge.wiki as wiki_lib

    monkeypatch.setattr(wiki_lib, "refresh_wiki", lambda *a, **k: wiki_lib.WikiResult())

    console_calls = []

    import devcouncil.cli.commands.wiki as wiki_cmd

    monkeypatch.setattr(wiki_cmd.console, "print", lambda *a, **k: console_calls.append(a))

    outcome = evaluate_wiki_refresh(
        tmp_path, ["src/a/f%d.py" % i for i in range(10)],
        config=WikiRefreshConfig(min_files=8, auto_update=True),
    )
    assert outcome.triggered is True
    assert console_calls == []


def test_wiki_refresh_advisory_gap_for_stale_pages():
    outcome = WikiRefreshOutcome(
        considered=True,
        triggered=False,
        stale_pages=["subsystems/foo.md", "overview/index.md"],
        reason="large change",
    )
    gap = wiki_refresh_advisory_gap(outcome, task_id="T-1", gap_id="T-1-WIKI-1")
    assert gap is not None
    assert gap.blocking is False
    assert gap.severity == "low"
    assert "stale" in gap.description.lower()
    assert gap.evidence == ["subsystems/foo.md", "overview/index.md"]


def test_wiki_refresh_advisory_gap_skipped_when_auto_updated():
    outcome = WikiRefreshOutcome(
        considered=True,
        triggered=True,
        stale_pages=["subsystems/foo.md"],
    )
    assert wiki_refresh_advisory_gap(outcome, task_id="T-1", gap_id="T-1-WIKI-1") is None


def test_report_builder_includes_wiki_refresh_metadata():
    from devcouncil.artifacts.graph import ArtifactGraph

    wiki_refresh = {
        "considered": True,
        "triggered": False,
        "stale_pages": ["subsystems/foo.md"],
        "reason": "large change — 1 stale wiki page(s)",
    }
    payload = json.loads(
        ReportBuilder.build_json(ArtifactGraph(), wiki_refresh=wiki_refresh)
    )
    assert payload["wiki_refresh"] == wiki_refresh


CAMPAIGN_PROMPT_FILES = (
    "coordinator.md",
    "director.md",
    "protocol.md",
    "reviewer.md",
    "worker.md",
)


def test_campaign_prompts_packaged_for_wheel():
    base = resources.files("devcouncil.campaign")
    for name in CAMPAIGN_PROMPT_FILES:
        path = base.joinpath("prompts", name)
        assert path.is_file(), f"missing packaged prompt: {name}"
        text = path.read_text(encoding="utf-8")
        assert text.startswith("#")
        assert len(text) > 50
