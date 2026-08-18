"""Coverage for `dev map`/`dev graph-context` helpers and branches not exercised by
the end-to-end map test: db guard, if-stale, wiki refresh, watch loop, graph-context."""

from __future__ import annotations

import time

import subprocess
from pathlib import Path
from types import SimpleNamespace

from typer.testing import CliRunner

import devcouncil.cli.commands.map as map_cmd
from devcouncil.cli.main import app

runner = CliRunner()


def _git_repo(tmp_path: Path):
    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    subprocess.run(["git", "config", "user.email", "t@t.com"], cwd=tmp_path, capture_output=True)
    subprocess.run(["git", "config", "user.name", "t"], cwd=tmp_path, capture_output=True)
    (tmp_path / "a.py").write_text("def f():\n    return 1\n", encoding="utf-8")
    subprocess.run(["git", "add", "-A"], cwd=tmp_path, capture_output=True)
    subprocess.run(["git", "commit", "-m", "init"], cwd=tmp_path, capture_output=True)


# --- _wiki_index_rel --------------------------------------------------------------


def test_wiki_index_rel_none_when_absent(tmp_path, monkeypatch):
    import devcouncil.knowledge.wiki as wiki_mod
    monkeypatch.setattr(wiki_mod, "wiki_dir_for", lambda root: tmp_path / "wiki")
    assert map_cmd._wiki_index_rel(tmp_path) is None


def test_wiki_index_rel_returns_relative(tmp_path, monkeypatch):
    import devcouncil.knowledge.wiki as wiki_mod
    wiki = tmp_path / "wiki"
    wiki.mkdir()
    (wiki / "index.md").write_text("# wiki", encoding="utf-8")
    monkeypatch.setattr(wiki_mod, "wiki_dir_for", lambda root: wiki)
    assert map_cmd._wiki_index_rel(tmp_path) == "wiki/index.md"


def test_wiki_index_rel_absolute_when_outside_root(tmp_path, monkeypatch):
    import devcouncil.knowledge.wiki as wiki_mod
    outside = tmp_path.parent / f"{tmp_path.name}_wiki_outside"
    outside.mkdir()
    (outside / "index.md").write_text("# wiki", encoding="utf-8")
    monkeypatch.setattr(wiki_mod, "wiki_dir_for", lambda root: outside)
    # index.md is not under `root` → ValueError → returns the absolute string.
    result = map_cmd._wiki_index_rel(tmp_path)
    assert result == str(outside / "index.md")


# --- _important_surfaces ----------------------------------------------------------


def test_important_surfaces_from_subsystems():
    repo_map = SimpleNamespace(
        subsystems=[SimpleNamespace(area="core", summary="core stuff")],
        important_files=[],
    )
    lines = map_cmd._important_surfaces(repo_map)
    assert lines[0].startswith("1. `core/`")


def test_important_surfaces_falls_back_to_files():
    repo_map = SimpleNamespace(subsystems=[], important_files=["a.py", "b.py"])
    lines = map_cmd._important_surfaces(repo_map)
    assert "a.py" in lines[0]


def test_important_surfaces_default_when_empty():
    repo_map = SimpleNamespace(subsystems=[], important_files=[])
    lines = map_cmd._important_surfaces(repo_map)
    assert lines == ["1. See `.devcouncil/repo_map.json` for the file index."]


# --- _liveness_summary ------------------------------------------------------------


def test_liveness_summary_none_when_all_empty():
    repo_map = SimpleNamespace(
        entry_roots=[], unwired_candidates=[], unreachable_files=[], dead_symbol_candidates=[],
    )
    assert map_cmd._liveness_summary(repo_map) is None


def test_liveness_summary_reports_counts():
    repo_map = SimpleNamespace(
        entry_roots=["main"],
        unwired_candidates=["u.py"],
        unreachable_files=["r.py"],
        dead_symbol_candidates=["d.f"],
    )
    summary = map_cmd._liveness_summary(repo_map)
    assert "liveness:" in summary
    assert "1 entry roots" in summary


# --- map command: an unusable engine fails the stage -------------------------------


def test_map_engine_unavailable_exits(tmp_path, monkeypatch):
    """`dev map` must end red when the map engine cannot run.

    Replaces `test_map_db_unavailable_exits`, which patched `map_cmd.get_db` —
    the Python indexer's store handle. The Rust kernel owns its own store, so
    that seam no longer exists, but the contract it protected does and is the
    reason there is no Python fallback: a map command that cannot build must
    not exit 0 having quietly produced nothing.
    """
    import devcouncil.devmap_engine as engine

    monkeypatch.chdir(tmp_path)
    _git_repo(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    def _boom(*_args, **_kwargs):
        raise engine.DevMapEngineError("kernel unavailable")

    monkeypatch.setattr(map_cmd, "build_map", _boom, raising=False)
    monkeypatch.setattr(engine, "build_map", _boom)
    result = runner.invoke(app, ["map"])
    assert result.exit_code == 1


def test_map_if_stale_skips_when_fresh(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    _git_repo(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    # Build the map once.
    assert runner.invoke(app, ["map"]).exit_code == 0
    # A subsequent --if-stale run detects a fresh map and skips the rebuild.
    monkeypatch.setattr(map_cmd.RepoMapper, "map_is_stale", lambda self, data: False)
    result = runner.invoke(app, ["map", "--if-stale"])
    assert result.exit_code == 0
    assert "Map is fresh" in result.output


# --- graph_context_cmd ------------------------------------------------------------


def test_graph_context_json(tmp_path, monkeypatch):
    monkeypatch.setattr(
        map_cmd, "CodeReviewGraphAdapter",
        lambda root: SimpleNamespace(
            get_context=lambda files: SimpleNamespace(
                available=True,
                impacted_files=["b.py"],
                related_tests=["test_b.py"],
                model_dump_json=lambda indent=2: '{"available": true}',
            )
        ),
    )
    result = runner.invoke(app, ["graph-context", "--file", "a.py", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "available" in result.output


def test_graph_context_human_available(tmp_path, monkeypatch):
    monkeypatch.setattr(
        map_cmd, "CodeReviewGraphAdapter",
        lambda root: SimpleNamespace(
            get_context=lambda files: SimpleNamespace(
                available=True,
                impacted_files=["b.py"],
                related_tests=["test_b.py"],
            )
        ),
    )
    result = runner.invoke(app, ["graph-context", "--file", "a.py", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Impacted files" in result.output
    assert "Related tests" in result.output


def test_graph_context_available_but_empty(tmp_path, monkeypatch):
    monkeypatch.setattr(
        map_cmd, "CodeReviewGraphAdapter",
        lambda root: SimpleNamespace(
            get_context=lambda files: SimpleNamespace(available=True, impacted_files=[], related_tests=[])
        ),
    )
    result = runner.invoke(app, ["graph-context", "--project-root", str(tmp_path)])
    assert result.exit_code == 0


def test_graph_context_unavailable(tmp_path, monkeypatch):
    monkeypatch.setattr(
        map_cmd, "CodeReviewGraphAdapter",
        lambda root: SimpleNamespace(
            get_context=lambda files: SimpleNamespace(available=False, impacted_files=[], related_tests=[])
        ),
    )
    result = runner.invoke(app, ["graph-context", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "not available" in result.output


# --- _refresh_wiki_skeletons ------------------------------------------------------


def test_refresh_wiki_skeletons_no_wiki_is_noop(tmp_path, monkeypatch):
    import devcouncil.knowledge.wiki as wiki_mod
    monkeypatch.setattr(wiki_mod, "wiki_dir_for", lambda root: tmp_path / "wiki")
    # No index.md → returns quietly.
    map_cmd._refresh_wiki_skeletons(tmp_path, SimpleNamespace())


def test_refresh_wiki_skeletons_refreshes_stale(tmp_path, monkeypatch):
    import devcouncil.knowledge.wiki as wiki_mod

    wiki = tmp_path / "wiki"
    wiki.mkdir()
    (wiki / "index.md").write_text("# wiki", encoding="utf-8")
    monkeypatch.setattr(wiki_mod, "wiki_dir_for", lambda root: wiki)
    monkeypatch.setattr(wiki_mod, "wiki_stale_pages", lambda root, repo_map, wiki_dir: ["page1"])
    monkeypatch.setattr(wiki_mod, "_project_name", lambda root: "Proj")
    monkeypatch.setattr(
        wiki_mod, "generate_wiki",
        lambda root, repo_map, wiki_dir, project_name: SimpleNamespace(changed=["page1"]),
    )
    result_console = []
    monkeypatch.setattr(map_cmd.status_console, "print", lambda msg: result_console.append(msg))
    map_cmd._refresh_wiki_skeletons(tmp_path, SimpleNamespace())
    assert any("Refreshed" in m for m in result_console)


def test_refresh_wiki_skeletons_swallows_errors(tmp_path, monkeypatch):
    import devcouncil.knowledge.wiki as wiki_mod
    def boom(root):
        raise RuntimeError("wiki dir failed")
    monkeypatch.setattr(wiki_mod, "wiki_dir_for", boom)
    # Must never raise — wiki refresh is a convenience layer.
    map_cmd._refresh_wiki_skeletons(tmp_path, SimpleNamespace())
def test_watch_map_rebuilds_when_the_fingerprint_moves(tmp_path, monkeypatch):
    """Watch must fire on exactly the evidence `--if-stale` reads.

    Replaces `test_watch_map_processes_batch_then_stops`, which drove the Python
    coordinator's changed-path batching (`refresh_map_for_paths` plus a fake
    observer). The Rust kernel rebuilds the whole map, so there is no batch to
    assert; what survives is the contract that matters — a stale fingerprint
    causes a rebuild, and a fresh one does not.
    """
    import devcouncil.devmap_engine as engine

    root = tmp_path.resolve()
    (root / ".devcouncil").mkdir(parents=True, exist_ok=True)
    (root / ".devcouncil" / "repo_map.json").write_text('{"files": []}', encoding="utf-8")

    calls: list[str] = []
    monkeypatch.setattr(engine, "build_map", lambda r, **_k: calls.append("built"))
    monkeypatch.setattr(map_cmd.RepoMapper, "map_is_stale", lambda self, data: True)

    real_sleep = time.sleep

    def _stop_after_one(_seconds):
        real_sleep(0)
        if calls:
            raise KeyboardInterrupt

    monkeypatch.setattr(time, "sleep", _stop_after_one)
    map_cmd._watch_map(root)
    assert calls == ["built"], "a stale fingerprint must trigger exactly one rebuild"


def test_watch_map_reports_a_failed_rebuild_and_keeps_watching(tmp_path, monkeypatch):
    """A failed rebuild must be visible, and must not end the watch.

    Replaces `test_watch_map_refresh_error_is_ignored`. "Ignored" is the wrong
    contract to keep: a watcher that swallows failures looks alive while serving
    an increasingly stale map. It stays alive *and* says so.
    """
    import devcouncil.devmap_engine as engine

    root = tmp_path.resolve()
    (root / ".devcouncil").mkdir(parents=True, exist_ok=True)
    (root / ".devcouncil" / "repo_map.json").write_text('{"files": []}', encoding="utf-8")

    attempts: list[int] = []

    def _always_fails(_root, **_kwargs):
        attempts.append(1)
        raise engine.DevMapEngineError("boom")

    monkeypatch.setattr(engine, "build_map", _always_fails)
    monkeypatch.setattr(map_cmd.RepoMapper, "map_is_stale", lambda self, data: True)

    real_sleep = time.sleep

    def _stop_after_two(_seconds):
        real_sleep(0)
        if len(attempts) >= 2:
            raise KeyboardInterrupt

    monkeypatch.setattr(time, "sleep", _stop_after_two)
    map_cmd._watch_map(root)
    assert len(attempts) >= 2, "a failed rebuild must not end the watch"


def test_map_watch_flag_invokes_watch_map(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    _git_repo(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    called = {}
    monkeypatch.setattr(map_cmd, "_watch_map", lambda root, liveness=True: called.setdefault("hit", True))
    result = runner.invoke(app, ["map", "--watch"])
    assert result.exit_code == 0
    assert called["hit"] is True


# --- map command: --if-stale rebuilds when stale ----------------------------------


def test_map_if_stale_rebuilds_when_stale(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    _git_repo(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    assert runner.invoke(app, ["map"]).exit_code == 0
    monkeypatch.setattr(map_cmd.RepoMapper, "map_is_stale", lambda self, data: True)
    result = runner.invoke(app, ["map", "--if-stale"])
    assert result.exit_code == 0
    assert "Wrote repository map" in result.output


# --- agent guides: existing file without the marker is left untouched -------------


def test_map_leaves_unmarked_agent_guides_alone(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    _git_repo(tmp_path)
    # Pre-create AGENTS.md WITHOUT the managed marker → must be preserved.
    (tmp_path / "AGENTS.md").write_text("# my own agents doc\n", encoding="utf-8")
    assert runner.invoke(app, ["init"]).exit_code == 0
    result = runner.invoke(app, ["map"])
    assert result.exit_code == 0
    assert (tmp_path / "AGENTS.md").read_text(encoding="utf-8") == "# my own agents doc\n"


# --- graph HTML written after map when configured ---------------------------------


def test_map_writes_graph_html_when_configured(tmp_path, monkeypatch):
    import yaml

    monkeypatch.chdir(tmp_path)
    _git_repo(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    cfg_path = tmp_path / ".devcouncil" / "config.yaml"
    cfg = yaml.safe_load(cfg_path.read_text(encoding="utf-8"))
    cfg.setdefault("indexing", {})["write_graph_html"] = True
    cfg_path.write_text(yaml.safe_dump(cfg), encoding="utf-8")

    import devcouncil.indexing.viz as viz
    monkeypatch.setattr(viz, "write_graph_html", lambda root, open_browser=False: tmp_path / "graph.html")

    result = runner.invoke(app, ["map"])
    assert result.exit_code == 0
