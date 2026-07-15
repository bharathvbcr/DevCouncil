"""Coverage for `dev map`/`dev graph-context` helpers and branches not exercised by
the end-to-end map test: db guard, if-stale, wiki refresh, watch loop, graph-context."""

from __future__ import annotations

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
    import devcouncil.cli.commands.wiki as wiki_cmd
    monkeypatch.setattr(wiki_cmd, "wiki_dir_for", lambda root: tmp_path / "wiki")
    assert map_cmd._wiki_index_rel(tmp_path) is None


def test_wiki_index_rel_returns_relative(tmp_path, monkeypatch):
    import devcouncil.cli.commands.wiki as wiki_cmd
    wiki = tmp_path / "wiki"
    wiki.mkdir()
    (wiki / "index.md").write_text("# wiki", encoding="utf-8")
    monkeypatch.setattr(wiki_cmd, "wiki_dir_for", lambda root: wiki)
    assert map_cmd._wiki_index_rel(tmp_path) == "wiki/index.md"


def test_wiki_index_rel_absolute_when_outside_root(tmp_path, monkeypatch):
    import devcouncil.cli.commands.wiki as wiki_cmd
    outside = tmp_path.parent / f"{tmp_path.name}_wiki_outside"
    outside.mkdir()
    (outside / "index.md").write_text("# wiki", encoding="utf-8")
    monkeypatch.setattr(wiki_cmd, "wiki_dir_for", lambda root: outside)
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


# --- map command: db unavailable --------------------------------------------------


def test_map_db_unavailable_exits(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    _git_repo(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    monkeypatch.setattr(map_cmd, "get_db", lambda root: None)
    result = runner.invoke(app, ["map"])
    assert result.exit_code == 1


# --- map command: --if-stale skips a fresh map ------------------------------------


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
    import devcouncil.cli.commands.wiki as wiki_cmd
    monkeypatch.setattr(wiki_cmd, "wiki_dir_for", lambda root: tmp_path / "wiki")
    # No index.md → returns quietly.
    map_cmd._refresh_wiki_skeletons(tmp_path, SimpleNamespace())


def test_refresh_wiki_skeletons_refreshes_stale(tmp_path, monkeypatch):
    import devcouncil.cli.commands.wiki as wiki_cmd
    import devcouncil.knowledge.wiki as wiki_mod

    wiki = tmp_path / "wiki"
    wiki.mkdir()
    (wiki / "index.md").write_text("# wiki", encoding="utf-8")
    monkeypatch.setattr(wiki_cmd, "wiki_dir_for", lambda root: wiki)
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
    import devcouncil.cli.commands.wiki as wiki_cmd
    def boom(root):
        raise RuntimeError("wiki dir failed")
    monkeypatch.setattr(wiki_cmd, "wiki_dir_for", boom)
    # Must never raise — wiki refresh is a convenience layer.
    map_cmd._refresh_wiki_skeletons(tmp_path, SimpleNamespace())


# --- _watch_map -------------------------------------------------------------------


def test_watch_map_processes_batch_then_stops(tmp_path, monkeypatch):
    import devcouncil.codeintel.sync as sync_mod
    import devcouncil.indexing.graph.build as graph_build

    root = tmp_path.resolve()
    (root / "a.py").write_text("x = 1\n", encoding="utf-8")

    refreshed = {}
    monkeypatch.setattr(
        graph_build, "refresh_map_for_paths",
        lambda root, batch, liveness=True: refreshed.setdefault("batch", batch),
    )

    class _FakeCoordinator:
        pending = ["a.py"]

        def __init__(self, callback):
            self.callback = callback

        def start(self):
            return SimpleNamespace(backend="FakeObserver", state="healthy")

        def status(self):
            return SimpleNamespace(pending=list(self.pending), last_error="", degraded_reason="")

        def sync_now(self):
            self.callback(list(self.pending))
            self.pending = []
            return True

        def stop(self, timeout=2):
            return None

    monkeypatch.setattr(
        sync_mod,
        "get_sync_coordinator",
        lambda root, **kwargs: _FakeCoordinator(kwargs["sync_callback"]),
    )

    counter = {"n": 0}

    def fake_sleep(_seconds):
        counter["n"] += 1
        if counter["n"] >= 2:
            raise KeyboardInterrupt

    # _watch_map imports `time` locally; patch the stdlib module it resolves to.
    import time as _time
    monkeypatch.setattr(_time, "sleep", fake_sleep)

    map_cmd._watch_map(root, liveness=True)
    assert refreshed["batch"] == ["a.py"]


def test_watch_map_refresh_error_is_ignored(tmp_path, monkeypatch):
    import devcouncil.codeintel.sync as sync_mod

    root = tmp_path.resolve()
    (root / "a.py").write_text("x = 1\n", encoding="utf-8")

    class _FakeCoordinator:
        def start(self):
            return SimpleNamespace(backend="FakeObserver", state="healthy")

        def status(self):
            return SimpleNamespace(
                pending=["a.py"],
                last_error="RuntimeError: refresh exploded",
                degraded_reason="",
            )

        def sync_now(self):
            return False

        def stop(self, timeout=2):
            return None

    monkeypatch.setattr(sync_mod, "get_sync_coordinator", lambda root, **kwargs: _FakeCoordinator())

    counter = {"n": 0}

    def fake_sleep(_seconds):
        counter["n"] += 1
        if counter["n"] >= 2:
            raise KeyboardInterrupt

    import time as _time
    monkeypatch.setattr(_time, "sleep", fake_sleep)

    messages = []
    monkeypatch.setattr(map_cmd.status_console, "print", lambda msg: messages.append(str(msg)))
    map_cmd._watch_map(root, liveness=True)
    assert any("Watch refresh failed" in m for m in messages)


# --- map command: --watch flag dispatch -------------------------------------------


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
