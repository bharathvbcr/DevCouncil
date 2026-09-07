"""The Python `index.sqlite` graph store is retired; the kernel's artifact answers.

`load_code_graph` and `write_code_graph` were the store's only production reader
and writer, and both reached zero call sites by Lane M3. What was left was a
store nothing filled and surfaces that still asked it questions -- `dev map
cypher` among them, which answered "No committed graph generation." on every
repository because no code path had persisted a generation since the Python
engine was retired.

Every test here is a *consumer* test: it asserts the surface answers from the
kernel's `code_graph.json`, with no `index.sqlite` on disk at any point.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from tests.unit.graph_fixtures import git_init_commit, kernel_graph
from tests.unit.retired_modules import assert_module_is_gone


@pytest.fixture()
def kernel_corpus(tmp_path: Path) -> Path:
    """A small committed tree the kernel has indexed, with no Python store."""
    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg" / "__init__.py").write_text("", encoding="utf-8")
    (tmp_path / "pkg" / "core.py").write_text(
        "def helper():\n    return 1\n\n\ndef caller():\n    return helper()\n",
        encoding="utf-8",
    )
    (tmp_path / "main.py").write_text(
        "from pkg.core import caller\n\n\ndef main():\n    return caller()\n",
        encoding="utf-8",
    )
    git_init_commit(tmp_path)
    kernel_graph(tmp_path)
    return tmp_path


def _index_sqlite(root: Path) -> Path:
    return root / ".devcouncil" / "codeintel" / "index.sqlite"


#: Every module and public name the Python graph store consisted of.
#:
#: Two dozen tests used to guard this one consumer at a time, by monkeypatching
#: `load_code_graph` to raise. A patch cannot say anything about a consumer the
#: patch does not reach, and it is a per-test cost for a tree-wide property. The
#: property is asserted once, here, and it holds for every consumer including
#: the ones nobody wrote a test for.
_RETIRED_MODULES = (
    "devcouncil.codeintel.store.sqlite",
    "devcouncil.codeintel.build_control",
    "devcouncil.codeintel.sync",
    "devcouncil.codeintel.sync.lease",
)
_RETIRED_ATTRIBUTES = (
    ("devcouncil.indexing.graph.build", "load_code_graph"),
    ("devcouncil.indexing.graph.build", "write_code_graph"),
    ("devcouncil.indexing.graph.build", "merge_pdg_into_graph"),
    ("devcouncil.indexing.graph.build", "load_pdg_layer"),
    ("devcouncil.indexing.graph.intel", "diff_impact"),
    ("devcouncil.codeintel.service", "index_freshness"),
    ("devcouncil.codeintel.store", "CodeIntelStore"),
    ("devcouncil.codeintel", "CodeIntelStore"),
)
_RETIRED_METHODS = (
    ("devcouncil.codeintel.service", "CodeIntelService", "persist"),
    ("devcouncil.codeintel.service", "CodeIntelService", "load"),
    ("devcouncil.codeintel.service", "CodeIntelService", "cached_query"),
    ("devcouncil.codeintel.service", "CodeIntelService", "status"),
)


@pytest.mark.parametrize("name", _RETIRED_MODULES)
def test_retired_store_module_is_gone(name: str) -> None:
    assert_module_is_gone(name)


@pytest.mark.parametrize(("module", "attribute"), _RETIRED_ATTRIBUTES)
def test_retired_store_name_is_gone(module: str, attribute: str) -> None:
    import importlib

    assert not hasattr(importlib.import_module(module), attribute), (
        f"{module}.{attribute} is back; it belonged to the Python graph store"
    )


@pytest.mark.parametrize(("module", "cls", "method"), _RETIRED_METHODS)
def test_retired_service_method_is_gone(module: str, cls: str, method: str) -> None:
    import importlib

    owner = getattr(importlib.import_module(module), cls)
    assert not hasattr(owner, method), (
        f"{cls}.{method} is back; it answered about a generation nothing writes"
    )


def test_dead_code_gate_still_refuses_a_stale_index(kernel_corpus: Path) -> None:
    """`dev map dead` must exit 3 when the map was built at another commit.

    The gate exists because a frozen index misled a deletion decision on
    2026-08-11. It asked `codeintel.service.index_freshness`, which compared the
    Python store's committed generation against git HEAD -- and no code had
    committed a generation since `write_code_graph` lost its last caller, so the
    probe returned `fresh: None` ("no committed index generation") and the
    command exited 0 on any real repository. Its own test kept passing because
    the fixture called `write_code_graph` itself.

    Pre-change this exits 0 with no "stale" in the output.
    """
    import subprocess

    from typer.testing import CliRunner

    from devcouncil.cli.commands.graph_cmd import app as graph_app

    # Move HEAD on without rebuilding: the map is now a map of another commit.
    (kernel_corpus / "extra.py").write_text("def extra():\n    return 1\n", encoding="utf-8")
    for args in (
        ["add", "-A"],
        ["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "second"],
    ):
        subprocess.run(["git", *args], cwd=kernel_corpus, check=True, capture_output=True)

    result = CliRunner().invoke(
        graph_app, ["dead", "--project-root", str(kernel_corpus)]
    )
    assert result.exit_code == 3, (
        "a dead-code report from a map built at another commit must not exit 0 "
        f"(got {result.exit_code}); output: {result.output}"
    )
    assert "stale" in result.output.lower(), result.output


def test_dev_map_has_no_unlock_command() -> None:
    """`dev map unlock` freed the Python writer lease. Nothing takes one now.

    A recovery command for a lock no code acquires is a phantom: it can only
    ever report that there was nothing to unlock, which reads the same as a
    successful recovery.
    """
    from devcouncil.cli.commands.graph_cmd import app as graph_app

    names = {command.name for command in graph_app.registered_commands}
    assert "unlock" not in names, names


def test_cypher_answers_from_the_kernels_artifact(kernel_corpus: Path) -> None:
    """`dev map cypher` used to need a Python store generation nothing wrote.

    Pre-change this returns ``{"ok": False, "error": "No committed graph
    generation."}``: `run_cypher` asked `CodeIntelService`, whose store is empty
    because `write_code_graph` -- its last production writer -- has no callers.
    """
    from devcouncil.indexing.graph.cypher import run_cypher

    result = run_cypher(kernel_corpus, "MATCH (a)-[r:CALLS]->(b) RETURN a,b LIMIT 25")

    assert result["ok"] is True, result
    assert result["count"] >= 1, result
    assert not _index_sqlite(kernel_corpus).exists(), (
        "answering a cypher query created the retired Python store"
    )


def test_cypher_over_nodes_answers_from_the_kernels_artifact(kernel_corpus: Path) -> None:
    from devcouncil.indexing.graph.cypher import run_cypher

    result = run_cypher(
        kernel_corpus, "MATCH (a) WHERE contains(a.name, 'helper') RETURN a LIMIT 25"
    )

    assert result["ok"] is True, result
    assert result["count"] >= 1, result
    assert not _index_sqlite(kernel_corpus).exists()
