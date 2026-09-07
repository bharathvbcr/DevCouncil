"""The whole-graph consumers read `code_graph.json`, not the Python store.

`load_code_graph` is the retired engine's read path. It opens the Python
`index.sqlite` cache, imports `code_graph.json` into it on first read — a ~94 MB
write from a *read* path, under a writer lease — and then re-materialises every
node and edge as pydantic models out of SQLite on every call after that. The
data it returns is the kernel's; only the route is Python's.

Six consumers still went that way at the start of this lane: `dev map`'s
`check`, `process`, `dead`, `viz` and `export` commands (through
`_require_graph`), the OKF bundler, the graph HTML renderer, the dead-symbol
gate's `symbol_has_non_test_inbound`, and the opt-in PDG layer. Each of them
wants the whole graph, and the whole graph is on disk in the artifact the kernel
itself writes.

Every test here ran with `load_code_graph` patched to raise, so a consumer that
regressed onto it failed loudly rather than passing because the store happened
to be warm. That function is deleted now, and with it the only thing that
created `index.sqlite` from a read; the tripwire is therefore the file itself —
`no_python_store` fails any test that leaves one behind.
"""

from __future__ import annotations

import json
import subprocess

import pytest
from typer.testing import CliRunner

import devcouncil.indexing.graph.build as graph_build
from devcouncil.cli.commands.graph_cmd import app as graph_app
from tests.unit.graph_fixtures import kernel_graph

runner = CliRunner()


@pytest.fixture
def kernel_corpus(tmp_path):
    """A small tree the real kernel has indexed, with `code_graph.json` written."""
    files = {
        "pyproject.toml": (
            '[project]\nname="t"\nversion="0"\n'
            '[project.scripts]\ncli="pkg.main:main"\n'
        ),
        "pkg/__init__.py": "",
        "pkg/main.py": "from pkg import util\n\n\ndef main():\n    return util.run()\n",
        "pkg/util.py": "def run():\n    return 1\n\n\ndef unused():\n    return 2\n",
    }
    for rel, content in files.items():
        path = tmp_path / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
    for args in (
        ["init"],
        ["add", "-A"],
        ["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "init"],
    ):
        subprocess.run(["git", *args], cwd=tmp_path, check=True, capture_output=True)
    kernel_graph(tmp_path)  # builds the store and writes code_graph.json, or skips
    assert (tmp_path / ".devcouncil" / "graph" / "code_graph.json").is_file()
    return tmp_path


@pytest.fixture
def no_python_store(tmp_path, monkeypatch):
    """No consumer may create the Python store, and the module may not offer one.

    Two assertions in one, because they fail differently. The `getattr` is a
    structural check that the retired read path has not come back under its old
    name -- a consumer cannot regress onto a function that does not exist, and
    this fails the moment someone re-adds it. The file check is behavioural: it
    catches any *other* route into `index.sqlite`, which is what the monkeypatch
    this replaced could never see.
    """
    assert not hasattr(graph_build, "load_code_graph"), (
        "`load_code_graph` is back; it is the retired engine's whole-graph read "
        "through the Python index.sqlite cache"
    )
    yield monkeypatch
    store = tmp_path / ".devcouncil" / "codeintel" / "index.sqlite"
    assert not store.exists(), f"this consumer created the retired Python store at {store}"


def test_read_code_graph_returns_what_the_kernel_wrote(kernel_corpus):
    graph = graph_build.read_code_graph(kernel_corpus)
    assert graph is not None, "the kernel wrote an artifact this could not read"
    assert graph.nodes and graph.edges, "an empty graph is not a read"
    assert any(node.path == "pkg/util.py" for node in graph.nodes), (
        "the read must carry the corpus's own symbols"
    )
    assert graph.meta.get(graph_build.GRAPH_INCOMPLETE_META) is None, (
        "a full-size kernel artifact must not be marked capped"
    )



def test_read_code_graph_marks_a_size_capped_export(kernel_corpus, monkeypatch):
    """A `compact` or `stub` export has truncated lists and must say so.

    The store used to cover this: it held the uncapped graph, so a consumer
    reading a capped JSON still got complete lists. Reading the artifact
    directly removes the cover, so the artifact has to carry the disclosure.
    """
    path = graph_build.graph_path(kernel_corpus)
    payload = json.loads(path.read_text(encoding="utf-8"))
    payload["meta"]["compatibility_export_tier"] = "compact"
    path.write_text(json.dumps(payload), encoding="utf-8")

    graph = graph_build.read_code_graph(kernel_corpus)
    assert graph is not None
    reason = graph.meta.get(graph_build.GRAPH_INCOMPLETE_META)
    assert reason and "compact" in reason, (
        f"a capped export must name what it capped: {reason!r}"
    )



def test_read_code_graph_declines_an_oversized_artifact(kernel_corpus, monkeypatch):
    """Declined *before* the read, not after.

    A bound enforced after parsing has already paid the memory it exists to
    cap. `test_graph_export_size` pinned this on `load_code_graph`; that
    function and the tiered writer it shared the bound with are deleted, so the
    assertion moves to the reader that is left.
    """
    monkeypatch.setattr(graph_build, "_graph_json_max_bytes", lambda root: 16)
    monkeypatch.setattr(
        graph_build,
        "read_json",
        lambda _path: (_ for _ in ()).throw(AssertionError("oversized JSON was read")),
    )
    assert graph_build.read_code_graph(kernel_corpus) is None, (
        "an artifact over the configured bound must be declined, not read"
    )



def test_map_check_reads_the_artifact(kernel_corpus, no_python_store):
    result = runner.invoke(
        graph_app, ["check", "--project-root", str(kernel_corpus), "--json"]
    )
    assert result.exit_code == 0, result.output
    payload = json.loads(result.stdout)
    assert "god_nodes" in payload and "circular_imports" in payload



def test_map_process_reads_the_artifact(kernel_corpus, no_python_store):
    result = runner.invoke(
        graph_app, ["process", "--project-root", str(kernel_corpus), "--json"]
    )
    assert result.exit_code == 0, result.output
    assert isinstance(json.loads(result.stdout), list)


def test_map_export_okf_reads_the_artifact(kernel_corpus, no_python_store):
    out = kernel_corpus / "okf"
    result = runner.invoke(
        graph_app,
        [
            "export",
            "--format",
            "okf",
            "-o",
            str(out),
            "--project-root",
            str(kernel_corpus),
        ],
    )
    assert result.exit_code == 0, result.output
    assert out.is_dir() and any(out.iterdir()), "the bundle must actually be written"


def test_graph_html_reads_the_artifact(kernel_corpus, no_python_store):
    from devcouncil.indexing.viz import write_graph_html

    out = write_graph_html(kernel_corpus)
    assert out.is_file()
    assert out.stat().st_size > 0


def test_symbol_reach_gate_reads_the_artifact(kernel_corpus, no_python_store):
    from devcouncil.indexing.graph.query import symbol_has_non_test_inbound

    assert symbol_has_non_test_inbound(kernel_corpus, "pkg/util.py", "run") is True, (
        "pkg/main.py calls util.run; the gate must see it without the store"
    )
    assert symbol_has_non_test_inbound(kernel_corpus, "pkg/util.py", "unused") is False


def test_pdg_layer_reads_the_artifact_and_leaves_the_kernels_alone(
    kernel_corpus, no_python_store
):
    """The PDG layer is Python analysis; it must not rewrite the kernel's file.

    `dev map pdg build` merged its results into the `CodeGraph` and called
    `write_code_graph`, which rewrites `code_graph.json` — the artifact the
    kernel is the only writer of. The layer now lands in its own sidecar.
    """
    before = graph_build.graph_path(kernel_corpus).read_bytes()
    result = runner.invoke(
        graph_app,
        ["pdg", "build", "--project-root", str(kernel_corpus), "--json"],
    )
    assert result.exit_code == 0, result.output
    payload = json.loads(result.stdout)
    assert payload["ok"] is True
    assert graph_build.graph_path(kernel_corpus).read_bytes() == before, (
        "the PDG layer rewrote the kernel's code_graph.json"
    )
    sidecar = kernel_corpus / ".devcouncil" / "graph" / "pdg.json"
    assert sidecar.is_file(), "the PDG layer must write its own artifact"


def test_pdg_queries_read_the_sidecar(kernel_corpus, no_python_store):
    from devcouncil.indexing.graph.query import (
        explain_pdg_taint,
        query_pdg_controls,
        query_pdg_flows,
    )

    result = runner.invoke(
        graph_app, ["pdg", "build", "--project-root", str(kernel_corpus), "--json"]
    )
    assert result.exit_code == 0, result.output

    taint = explain_pdg_taint(kernel_corpus)
    assert taint["ok"] is True, taint
    controls = query_pdg_controls(kernel_corpus, "run")
    assert controls["ok"] is True, controls
    assert controls["functions"], controls
    flows = query_pdg_flows(kernel_corpus, "run")
    assert flows["ok"] is True, flows
