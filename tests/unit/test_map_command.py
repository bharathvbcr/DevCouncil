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
    """A payload with no `liveness_meta` renders its counts as floors.

    This fixture predates the disclosure and carries none, so no total is
    knowable for the three capped lists; `unreachable_files` is not capped and
    still reads as a plain count.
    """
    repo_map = SimpleNamespace(
        entry_roots=["main"],
        unwired_candidates=["u.py"],
        unreachable_files=["r.py"],
        dead_symbol_candidates=["d.f"],
    )
    summary = map_cmd._liveness_summary(repo_map)
    assert "liveness:" in summary
    assert "1+ entry roots" in summary
    assert "1 unreachable" in summary
    assert "disclosed no total" in summary


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
    monkeypatch.setattr(engine, "build_map_result", _boom)
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


def test_map_if_stale_refuses_a_cold_build(tmp_path, monkeypatch):
    """`--if-stale` must never trigger a full index.

    `.devcouncil/` is gitignored, so every fresh git worktree starts with no
    map. This branch used to fall straight through to a full cold build with no
    message, which is how an editor hook wired to `dev map --if-stale` came to
    kick off a multi-minute index on the first file edit — and, when the hook
    was killed on timeout, leave the build orphaned at PPID 1 holding a core.
    """
    monkeypatch.chdir(tmp_path)
    _git_repo(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    # The condition under test is "no map on disk", which is the state of every
    # fresh git worktree. `dev init` may leave one behind, so remove it rather
    # than assuming.
    map_path = tmp_path / ".devcouncil" / "repo_map.json"
    map_path.unlink(missing_ok=True)
    assert not map_path.exists()

    built = []
    monkeypatch.setattr(
        map_cmd, "build_map",
        lambda *a, **k: built.append(1), raising=False,
    )

    result = runner.invoke(app, ["map", "--if-stale"])

    assert result.exit_code == 0
    assert built == [], "--if-stale must not start a build when there is no map"

    # Rich hard-wraps console output, so match on whitespace-normalized text —
    # otherwise this assertion depends on terminal width, not behaviour.
    said = " ".join(result.output.split())
    assert "nothing to refresh" in said
    assert "will not start a cold build" in said
    # The message has to name the way out, or the refusal just relocates the
    # confusion.
    assert "Run dev map" in said


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
    # No observer: the loop must poll and still rebuild on stale evidence.
    monkeypatch.setattr(map_cmd, "_start_change_observer", lambda root, changed: None)

    def _stop_after_one(_changed, _timeout):
        if calls:
            raise KeyboardInterrupt
        return False

    monkeypatch.setattr(map_cmd, "_wait_for_change", _stop_after_one)
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
    monkeypatch.setattr(map_cmd, "_start_change_observer", lambda root, changed: None)

    def _stop_after_two(_changed, _timeout):
        if len(attempts) >= 2:
            raise KeyboardInterrupt
        return False

    monkeypatch.setattr(map_cmd, "_wait_for_change", _stop_after_two)
    map_cmd._watch_map(root)
    assert len(attempts) >= 2, "a failed rebuild must not end the watch"


def test_watch_map_wakes_on_a_filesystem_event_not_a_timer(tmp_path, monkeypatch):
    """The old loop polled `map_is_stale` every two seconds — three git
    subprocesses and two stats per file, ~90 ms/tick here, seconds/tick at
    70k files. A write must wake the loop long before the slow poll fires."""
    import threading

    import devcouncil.devmap_engine as engine

    root = tmp_path.resolve()
    (root / ".devcouncil").mkdir(parents=True, exist_ok=True)
    (root / ".devcouncil" / "repo_map.json").write_text('{"files": []}', encoding="utf-8")

    built = threading.Event()

    def _build(_root, **_kwargs):
        built.set()
        raise KeyboardInterrupt  # end the watch from inside the rebuild

    monkeypatch.setattr(engine, "build_map", _build)
    monkeypatch.setattr(map_cmd.RepoMapper, "map_is_stale", lambda self, data: True)
    # A poll interval far longer than the test: only an event can wake the loop.
    monkeypatch.setattr(map_cmd, "WATCH_POLL_INTERVAL_SECONDS", 600.0)
    monkeypatch.setattr(map_cmd, "WATCH_DEBOUNCE_SECONDS", 0.05)

    def _write_later() -> None:
        time.sleep(0.6)
        (root / "k.py").write_text("x = 1\n", encoding="utf-8")

    threading.Thread(target=_write_later, daemon=True).start()
    started = time.monotonic()
    map_cmd._watch_map(root)
    assert built.is_set()
    assert time.monotonic() - started < 30.0, "the rebuild waited for the poll, not the event"


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


def test_map_warns_when_the_project_root_is_nested_inside_another(tmp_path, monkeypatch):
    """A nested `.devcouncil/config.yaml` silently indexes a subtree twice.

    Root resolution is just `--project-root` (default: cwd), so an agent that
    cd's into `backend/service/` to run its build — in a repo that has a nested
    config there — indexes that subtree as its own project. Observed: three
    configs committed by accident into a polyglot repo produced a 713MB
    duplicate index of one subtree, on its own rebuild schedule, next to the
    real root index. From inside the subdirectory nothing looks wrong.
    """
    parent = tmp_path / "repo"
    child = parent / "backend" / "service"
    (parent / ".devcouncil").mkdir(parents=True)
    (parent / ".devcouncil" / "config.yaml").write_text("project: {}\n")
    child.mkdir(parents=True)

    assert map_cmd._enclosing_project_root(child) == parent.resolve()
    # The real root is not nested in anything.
    assert map_cmd._enclosing_project_root(parent) is None


def test_enclosing_project_root_ignores_a_directory_without_a_config(tmp_path):
    parent = tmp_path / "repo"
    child = parent / "sub"
    child.mkdir(parents=True)
    (parent / ".devcouncil").mkdir()          # dir exists, no config.yaml
    assert map_cmd._enclosing_project_root(child) is None


# --- _liveness_summary: the capped lists carry their denominators -----------------
#
# `dev map` printed `liveness: 20 entry roots, 200 unwired, ... 200 dead symbols`
# on a repository whose own `repo_map.json` recorded 136 entry roots, 540 unwired
# and 208 dead symbols. Every one of those lists is capped by the kernel before it
# reaches the artifact, and the cap is invisible in the list itself, so the status
# line an agent reads before deciding "that is all of them" was a capped sample
# presented as complete coverage. The totals were already in `liveness_meta`; only
# this renderer dropped them.


def _meta_map(**buckets):
    """A RepoMap-shaped stand-in carrying a `liveness_meta` disclosure."""
    return SimpleNamespace(
        entry_roots=["e%d" % i for i in range(buckets.pop("entry_shown", 0))],
        unwired_candidates=["u%d.py" % i for i in range(buckets.pop("unwired_shown", 0))],
        unreachable_files=[],
        dead_symbol_candidates=["d%d.f" % i for i in range(buckets.pop("dead_shown", 0))],
        liveness_unreachable_unreliable=True,
        liveness_meta=buckets.pop("liveness_meta", {}),
        **buckets,
    )


def test_liveness_summary_prints_the_totals_the_map_disclosed():
    repo_map = _meta_map(
        entry_shown=20,
        unwired_shown=200,
        dead_shown=200,
        liveness_meta={
            "entry_roots": {"shown": 20, "total": 136, "truncated": True},
            "unwired": {"shown": 200, "total": 540, "truncated": True},
            "dead_symbol": {"shown": 200, "total": 208, "truncated": True},
        },
    )
    summary = map_cmd._liveness_summary(repo_map)
    assert "20 of 136 entry roots" in summary
    assert "200 of 540 unwired" in summary
    assert "200 of 208 dead symbols" in summary
    # No floor marker: every bucket answered.
    assert "+" not in summary.split("(prefer")[0]


def test_liveness_summary_states_a_complete_list_plainly():
    """A list that was not cut reads as a count, not as `N of N`."""
    repo_map = _meta_map(
        entry_shown=3,
        unwired_shown=2,
        dead_shown=1,
        liveness_meta={
            "entry_roots": {"shown": 3, "total": 3, "truncated": False},
            "unwired": {"shown": 2, "total": 2, "truncated": False},
            "dead_symbol": {"shown": 1, "total": 1, "truncated": False},
        },
    )
    summary = map_cmd._liveness_summary(repo_map)
    assert "3 entry roots" in summary
    assert " of " not in summary
    assert "disclosed no total" not in summary


def test_liveness_summary_marks_an_undisclosed_total_as_a_floor():
    """No disclosure is not the same answer as "the list is complete"."""
    repo_map = _meta_map(entry_shown=1, unwired_shown=200, dead_shown=4, liveness_meta={})
    summary = map_cmd._liveness_summary(repo_map)
    assert "200+ unwired" in summary
    assert "1+ entry roots" in summary
    assert "4+ dead symbols" in summary
    assert "disclosed no total" in summary


def test_liveness_summary_distrusts_a_total_its_own_list_disproves():
    """`total` below the number of entries present is incoherent, not authoritative."""
    repo_map = _meta_map(
        unwired_shown=200,
        liveness_meta={"unwired": {"shown": 200, "total": 7, "truncated": False}},
    )
    summary = map_cmd._liveness_summary(repo_map)
    assert "200+ unwired" in summary
    assert "of 7" not in summary


def test_liveness_summary_reports_files_unwired_could_not_examine():
    """Files whose imports never extracted are excluded from the unwired population.

    `liveness_meta.unwired.total` counts only the files the question could be
    asked of; `excluded_coverage_loss` counts the ones it could not.
    """
    repo_map = _meta_map(
        unwired_shown=2,
        liveness_meta={
            "unwired": {"shown": 2, "total": 2, "truncated": False, "excluded_coverage_loss": 3},
        },
    )
    summary = map_cmd._liveness_summary(repo_map)
    assert "3 files excluded from unwired" in summary
