"""Every Python reader finds the artifacts where the kernel actually put them.

The kernel resolves its state directory per repository
(``devmap_extract::paths``): ``$DEVMAP_HOME``, else an existing ``.devmap/``,
else an existing ``.devcouncil/``, else ``.devmap/``. Six Python readers spelled
it ``.devcouncil`` as a literal instead, so on a repository using the standalone
layout each of them looked in a directory that does not exist and reported "no
map" for a map that does -- silently, which is how a hard-coded ``.devcouncil/``
broke ``verify.sh`` gate 8 once already.

The corpus here is a repository that already has ``.devmap/``: exactly the shape
those literals get wrong, and the shape the kernel's rule 2 selects. The
expected location is taken from ``devmap --json status``, not from the resolver
under test, so these assertions are about the kernel's answer rather than about
agreement with ourselves.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest

from devcouncil import devmap_engine
from tests.unit.graph_fixtures import _have_kernel, git_init_commit


@pytest.fixture(autouse=True)
def _clear_state_dir_cache():
    """The resolver caches per root; tmp roots are fresh but the cache is not."""
    cache = getattr(devmap_engine, "_STATE_DIR_CACHE", {})
    cache.clear()
    yield
    cache.clear()


@pytest.fixture()
def standalone_root(tmp_path: Path) -> Path:
    """A committed repository on the kernel's standalone (``.devmap/``) layout."""
    if not _have_kernel():
        pytest.skip("devmap kernel not built (cargo build --release -p devmap-cli)")
    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg" / "__init__.py").write_text("", encoding="utf-8")
    (tmp_path / "pkg" / "core.py").write_text(
        "def helper():\n    return 1\n\n\ndef caller():\n    return helper()\n",
        encoding="utf-8",
    )
    git_init_commit(tmp_path)
    # The repository declares the standalone layout, as a migrated one does.
    (tmp_path / ".devmap").mkdir()
    return tmp_path


def _kernel_state_dir(root: Path) -> Path:
    """Where the *kernel* says this root's state lives, asked directly."""
    binary = devmap_engine.find_engine_binary(root)
    reported = json.loads(
        subprocess.run(
            [binary, "--json", "status"],
            cwd=root,
            capture_output=True,
            text=True,
            check=True,
        ).stdout
    )["db_path"]
    return (root / Path(reported)).parent.parent.resolve()


def _git_head(root: Path) -> str:
    return subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=root,
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()


def _build(root: Path) -> None:
    """A `dev map` run, writing to the map path the kernel would resolve."""
    from devcouncil.indexing.map_artifacts import refresh_map_artifacts

    refresh_map_artifacts(root, _kernel_state_dir(root) / "repo_map.json", quiet=True)


def test_the_resolver_agrees_with_the_kernel(standalone_root: Path) -> None:
    kernel_dir = _kernel_state_dir(standalone_root)
    assert kernel_dir == (standalone_root / ".devmap").resolve()
    assert devmap_engine.state_dir(standalone_root) == kernel_dir


def test_a_root_with_no_state_dir_keeps_devcouncils_layout(tmp_path: Path) -> None:
    """Rule 2: nothing on disk means what DevCouncil's own writer will create.

    Following the kernel's *default* here would point every reader at
    ``.devmap/`` while `dev map` wrote ``.devcouncil/``.
    """
    assert devmap_engine.state_dir(tmp_path) == (tmp_path / ".devcouncil").resolve()
    assert devmap_engine.map_path(tmp_path).name == "repo_map.json"


def test_an_existing_devcouncil_root_is_unchanged(tmp_path: Path) -> None:
    (tmp_path / ".devcouncil").mkdir()
    assert devmap_engine.state_dir(tmp_path) == (tmp_path / ".devcouncil").resolve()


def test_the_writer_puts_the_artifacts_in_the_resolved_directory(
    standalone_root: Path,
) -> None:
    """`dev map` must not create a second state directory beside the kernel's."""
    _build(standalone_root)
    resolved = _kernel_state_dir(standalone_root)

    assert (resolved / "repo_map.json").is_file()
    assert (resolved / "graph" / "code_graph.json").is_file()
    assert (resolved / "codeintel" / "devmap.sqlite").is_file()

    # `.devcouncil/` still holds the *orchestrator's* things -- `logs/traces.jsonl`
    # is written by every DevCouncil stage, not just this one, and is not map
    # state. What must not appear there is a second copy of the kernel's state.
    for rel in ("repo_map.json", "graph", "codeintel/devmap.sqlite"):
        assert not (standalone_root / ".devcouncil" / rel).exists(), (
            f"the Python seam wrote map state to .devcouncil/{rel}, which the "
            "kernel's daemon, hooks and Go clients do not read"
        )


def test_every_reader_finds_the_map_in_the_resolved_directory(
    standalone_root: Path,
) -> None:
    """The readers the brief names, on one corpus, in one assertion each."""
    from devcouncil.devmap_client import DevMapClient
    from devcouncil.indexing.graph.build import pdg_sidecar_path, python_paths_for_pdg
    from devcouncil.indexing.semantic_index import SemanticIndex
    from devcouncil.knowledge.wiki import _load_repo_map

    _build(standalone_root)
    resolved = _kernel_state_dir(standalone_root)

    # 1. devmap_engine's own artifact paths.
    assert devmap_engine.map_path(standalone_root) == resolved / "repo_map.json"
    assert devmap_engine.graph_path(standalone_root) == resolved / "graph" / "code_graph.json"
    assert (
        devmap_engine.store_path(standalone_root)
        == resolved / "codeintel" / "devmap.sqlite"
    )

    # 2. DevMapClient.read_repo_map -- must read, not re-run `manifest`.
    payload = DevMapClient(standalone_root).read_repo_map()
    assert payload.get("files"), payload.keys()

    # 3. knowledge.wiki -- must load the map, not regenerate it.
    repo_map = _load_repo_map(standalone_root, remap=False)
    assert repo_map.files, "the wiki regenerated a map that was already on disk"

    # 4. indexing.semantic_index -- the recorded path must be the real one.
    snapshot = SemanticIndex(standalone_root).create_snapshot("t", "before")
    recorded = Path(json.loads(snapshot.read_text(encoding="utf-8"))["repo_map_path"])
    assert recorded == resolved / "repo_map.json"
    assert recorded.is_file()

    # 5. the PDG file list, which reads the map's `files` inventory.
    assert "pkg/core.py" in python_paths_for_pdg(standalone_root)

    # 6. the PDG sidecar, derived from the graph path.
    assert pdg_sidecar_path(standalone_root) == resolved / "graph" / "pdg.json"


def test_devmap_health_reports_the_resolved_artifacts(standalone_root: Path) -> None:
    """`dev map status` / `doctor` must not report the map as missing."""
    from devcouncil import devmap_health

    _build(standalone_root)
    head = _git_head(standalone_root)
    status = devmap_health.collect_map_status(standalone_root)

    assert status["artifacts"]["repo_map"]["exists"] is True, status["artifacts"]
    assert status["artifacts"]["code_graph"]["exists"] is True, status["artifacts"]
    assert status["store"]["exists"] is True, status["store"]
    # The freshness probe read the resolved map: it found the head the kernel
    # stamped into it. (It is not `fresh: True` here because the same build
    # writes AGENTS.md / CLAUDE.md after the map, which moves the tracked-file
    # fingerprint -- that is the map's own business, not the resolver's.)
    assert status["index_freshness"]["map_head"] == head, status["index_freshness"]
    assert status["index_freshness"]["current_head"] == head, status["index_freshness"]
