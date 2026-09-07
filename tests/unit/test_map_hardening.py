"""Hardening regressions for the dev map build pipeline.

``dev map dead`` reported dead-code results from an index frozen at a commit
five days behind HEAD with no staleness signal at all (observed in production on
2026-08-11). That gate is what is left here.

The FTS5 ``_prune`` regressions -- a single DELETE by the UNINDEXED
``generation_id`` column running 2+ hours at 100% CPU -- went with
``CodeIntelStore``: they were regressions in a Python store that nothing writes
any more. So did ``interrupt_writes`` and the FK-index check. The
supervised-worker regressions (self-enforced deadline, group SIGKILL for
SIGTERM-ignoring stragglers, ``changed-*.txt`` handoff GC) went earlier, with
``build_worker`` / ``run_isolated_full_build``: the Rust kernel builds in its
own process and supervises itself.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

from typer.testing import CliRunner

from devcouncil.indexing.graph.schema import CodeGraph, GraphEdge, GraphNode, NodeKind


def _graph(name: str = "main", *, path: str = "src/app.py", head: str = "abc") -> CodeGraph:
    return CodeGraph(
        nodes=[
            GraphNode(id=path, kind=NodeKind.FILE, path=path, name=Path(path).name, language="python"),
            GraphNode(
                id=f"{path}::{name}",
                kind=NodeKind.FUNCTION,
                path=path,
                name=name,
                line=1,
                end_line=2,
                language="python",
            ),
        ],
        edges=[GraphEdge(source=path, target=f"{path}::{name}", kind="contains")],
        entry_roots=[path],
        generated_head=head,
        indexed_hash="files",
        content_fingerprint="content",
        meta={"fixture": True},
    )


def _git(root: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(root), *args],
        capture_output=True,
        text=True,
        check=True,
    )
    return result.stdout.strip()


def _init_repo_with_commit(root: Path) -> str:
    _git(root, "init", "-q")
    _git(root, "config", "user.email", "test@test")
    _git(root, "config", "user.name", "test")
    (root / "file.txt").write_text("hello\n", encoding="utf-8")
    _git(root, "add", ".")
    _git(root, "commit", "-qm", "initial")
    return _git(root, "rev-parse", "HEAD")


def _seed_graph(root: Path, graph: CodeGraph) -> None:
    """Put a graph where both the freshness probe and the readers look.

    Both artifacts, because they answer different halves of these tests: the
    dead-code list comes from `code_graph.json`, and the staleness verdict from
    `repo_map.json`'s `generated_head`.

    This wrote them with `write_code_graph`, which also persisted a generation
    into the Python `index.sqlite` store -- which is where the freshness probe
    read `generated_head` from. Production had had no caller of that writer
    since Lane M3, so on a real repository the probe found no generation,
    returned `fresh: None`, and `dev map dead` exited 0 on an index built at
    another commit. The gate below passed only because its fixture used a writer
    production no longer had. The probe reads the map artifact now, so the
    fixture writes the map artifact.
    """
    from tests.unit.graph_fixtures import write_graph_artifact

    write_graph_artifact(root, graph)
    map_path = root / ".devcouncil" / "repo_map.json"
    map_path.parent.mkdir(parents=True, exist_ok=True)
    map_path.write_text(
        json.dumps({"generated_head": graph.generated_head, "files": []}),
        encoding="utf-8",
    )


# ---------------------------------------------------------------------------
# 1. FTS prune must never delete by the UNINDEXED generation_id column.
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# 2. A long write must be abortable.
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# 3. Index freshness must be computed and surfaced, and `dev map dead`
#    must fail loud on a stale index.
# ---------------------------------------------------------------------------


def test_index_freshness_reports_stale_and_fresh_heads(tmp_path: Path) -> None:
    """The verdict comes from the artifact the kernel writes.

    `codeintel.service.index_freshness` answered this from the Python store's
    committed generation. Nothing had written one since the kernel took over, so
    it returned "no committed index generation" on every repository and the
    banner below could not fire. `devmap_health.map_freshness` is the surviving
    owner of the question and reads `repo_map.json`.
    """
    from devcouncil.cli.commands.graph_cmd import _index_freshness_fields

    head = _init_repo_with_commit(tmp_path)
    _seed_graph(tmp_path, _graph("one", head="0" * 40))

    stale = _index_freshness_fields(tmp_path)
    assert stale["fresh"] is False
    assert stale["current_head"] == head
    assert stale["map_head"] == "0" * 40
    assert isinstance(stale["age_seconds"], float)

    _seed_graph(tmp_path, _graph("two", head=head))
    fresh = _index_freshness_fields(tmp_path)
    assert fresh["fresh"] is True
    assert fresh["map_head"] == head


def test_index_freshness_unknown_without_a_map(tmp_path: Path) -> None:
    """No map is "cannot judge", never "fresh"."""
    from devcouncil.cli.commands.graph_cmd import _index_freshness_fields

    result = _index_freshness_fields(tmp_path)
    assert result["fresh"] is None
    assert result["reason"]


def test_graph_dead_fails_loud_on_stale_index(tmp_path: Path) -> None:
    from devcouncil.cli.commands.graph_cmd import app

    _init_repo_with_commit(tmp_path)
    _seed_graph(tmp_path, _graph("one", head="0" * 40))

    runner = CliRunner()
    result = runner.invoke(app, ["dead", "--project-root", str(tmp_path)])
    assert result.exit_code == 3, (
        "a dead-code report from an index built at another commit must not "
        f"exit 0 (got {result.exit_code}); output: {result.output}"
    )
    assert "stale" in result.output.lower()

    allowed = runner.invoke(
        app, ["dead", "--project-root", str(tmp_path), "--allow-stale"]
    )
    assert allowed.exit_code == 0, allowed.output

    as_json = runner.invoke(
        app, ["dead", "--project-root", str(tmp_path), "--json", "--allow-stale"]
    )
    assert as_json.exit_code == 0, as_json.output
    import json as json_module

    payload = json_module.loads(as_json.stdout)
    assert payload["index_freshness"]["fresh"] is False


def test_graph_dead_exits_zero_when_index_matches_head(tmp_path: Path) -> None:
    from devcouncil.cli.commands.graph_cmd import app

    head = _init_repo_with_commit(tmp_path)
    _seed_graph(tmp_path, _graph("one", head=head))

    runner = CliRunner()
    result = runner.invoke(app, ["dead", "--project-root", str(tmp_path)])
    assert result.exit_code == 0, result.output
