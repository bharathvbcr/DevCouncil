"""Regression coverage for the Python ⇄ Rust map seam, from the 2026-09-02 audit.

Every test here failed against the code as it stood when it was written. Each
one names the defect it pins, in the order the audit found them:

1. `dev map` was dead on this machine. The installed `dev` (a `uv tool`
   install, not the repository checkout) located the kernel relative to its own
   site-packages, found no `rust-port/`, fell through to a `~/.cargo/bin/devmap`
   built the day before the schema moved 11→12, and every build ended with
   "unsupported future schema version 12". A freshly built kernel sat in the
   repository the whole time. Two rules were wrong at once: the *repository
   being mapped* was never searched, and among candidates that pass the
   capability probe the *first* won rather than the *newest*.
2. The IPC client had its own, different rule for the same question.
3. The client module defined one deadline constant twice with two values.
4. The Rust manifest writes file entries without `summary`, so every Python
   consumer that validates the map as a `RepoMap` — the wiki, the incremental
   refresh, the degraded-stamp — raised on the artifact `dev map` itself writes.
5. `map.py` carried ~140 lines of the retired Python engine after an
   unconditional `raise typer.Exit`, and seven flags were accepted and ignored.
6. Verify, checkout, plan, MCP ingest and `dev map init/ingest/sync` still
   rewrote `repo_map.json` through the Python engine — a second writer for the
   artifact the Rust kernel owns, on exactly the paths agents hit most.
"""

from __future__ import annotations

import ast
import json
import os
import subprocess
import time
from pathlib import Path

import pytest

from devcouncil import devmap_engine
from devcouncil.devmap_engine import DevMapEngineError, build_map, find_engine_binary


def _fake_kernel(path: Path, *, capable: bool = True, build_stderr: str = "") -> Path:
    """A shell script that answers the capability probe like the real kernel.

    ``capable`` controls whether ``manifest --help`` advertises the graph
    companion and the stamp flags. ``build_stderr`` makes ``build`` fail with
    that text, so the engine's error translation can be exercised without a
    real store.
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    flags = (
        "--output --graph-output --force --generated-head --indexed-hash --content-fingerprint"
        if capable
        else "--output"
    )
    build_branch = (
        f"  build) echo '{build_stderr}' >&2; exit 1 ;;\n" if build_stderr else "  build) exit 0 ;;\n"
    )
    path.write_text(
        "#!/bin/sh\n"
        "# args: [--db X] [--progress never] [--json] <cmd> ...\n"
        "while [ $# -gt 0 ]; do\n"
        "  case \"$1\" in\n"
        "    --db|--progress) shift 2 ;;\n"
        "    --json) shift ;;\n"
        "    *) break ;;\n"
        "  esac\n"
        "done\n"
        "case \"$1\" in\n"
        f"  manifest) echo 'Usage: devmap manifest [OPTIONS] {flags}' ;;\n"
        "  serve) echo 'Usage: devmap serve --socket <SOCKET>' ;;\n"
        + build_branch
        + "  status) echo '{\"generation_id\":1,\"pending_count\":0,\"node_count\":1,\"edge_count\":0,\"is_fresh\":true}' ;;\n"
        "  *) echo 'Usage: devmap' ;;\n"
        "esac\n"
    )
    path.chmod(0o755)
    return path


def _set_mtime(path: Path, epoch: float) -> None:
    os.utime(path, (epoch, epoch))


def _isolate_binary_search(monkeypatch, tmp_path: Path, *, path_binary: Path | None = None) -> Path:
    """Point every search location at ``tmp_path`` so only what a test plants is found."""
    package_root = tmp_path / "pkg"
    monkeypatch.setattr(
        devmap_engine, "__file__", str(package_root / "src" / "devcouncil" / "devmap_engine.py")
    )
    monkeypatch.delenv("DEVMAP_BINARY", raising=False)
    import shutil as _shutil

    monkeypatch.setattr(_shutil, "which", lambda _name: str(path_binary) if path_binary else None)
    devmap_engine._clear_probe_caches()
    return package_root


# --- 1. binary selection ---------------------------------------------------------


def test_the_newest_capable_build_wins_over_a_stale_release_binary(tmp_path, monkeypatch):
    """Release was built before the schema bump, debug after. Debug must win.

    Both pass the capability probe — the probe asks about flags, and the flags
    did not change. Only the age separates them, and age is exactly what a
    schema bump moves.
    """
    package_root = _isolate_binary_search(monkeypatch, tmp_path)
    release = _fake_kernel(package_root / "rust-port" / "target" / "release" / "devmap")
    debug = _fake_kernel(package_root / "rust-port" / "target" / "debug" / "devmap")
    now = time.time()
    _set_mtime(release, now - 3600)
    _set_mtime(debug, now - 60)

    assert find_engine_binary() == str(debug)

    # And the other way round: a release rebuilt after the debug build wins.
    _set_mtime(release, now)
    devmap_engine._clear_probe_caches()
    assert find_engine_binary() == str(release)


def test_the_repository_being_mapped_can_supply_its_own_kernel(tmp_path, monkeypatch):
    """A `uv tool` install has no `rust-port/`; the repository under `--project-root` may.

    Before this, the package-relative search found nothing and PATH answered
    with a kernel a day older than the store — the exact failure measured on
    2026-09-02 (`unsupported future schema version 12`, every `dev map`).
    """
    on_path = _fake_kernel(tmp_path / "cargo-bin" / "devmap")
    _isolate_binary_search(monkeypatch, tmp_path, path_binary=on_path)
    repo = tmp_path / "repo"
    in_repo = _fake_kernel(repo / "rust-port" / "target" / "release" / "devmap")
    now = time.time()
    _set_mtime(on_path, now - 86400)
    _set_mtime(in_repo, now - 60)

    assert find_engine_binary(repo) == str(in_repo)


def test_an_explicit_binary_override_is_honoured_and_still_probed(tmp_path, monkeypatch):
    """`DEVMAP_BINARY` wins over every search location — but only if it is capable.

    An override that cannot write the graph companion is refused by name, not
    silently replaced by a search result the operator did not ask for.
    """
    package_root = _isolate_binary_search(monkeypatch, tmp_path)
    _fake_kernel(package_root / "rust-port" / "target" / "release" / "devmap")
    chosen = _fake_kernel(tmp_path / "elsewhere" / "devmap")
    monkeypatch.setenv("DEVMAP_BINARY", str(chosen))
    assert find_engine_binary() == str(chosen)

    stale = _fake_kernel(tmp_path / "stale" / "devmap", capable=False)
    monkeypatch.setenv("DEVMAP_BINARY", str(stale))
    devmap_engine._clear_probe_caches()
    with pytest.raises(DevMapEngineError) as caught:
        find_engine_binary()
    assert str(stale) in str(caught.value)
    assert "DEVMAP_BINARY" in str(caught.value)


def test_a_future_schema_refusal_names_the_binary_and_the_fix(tmp_path, monkeypatch):
    """The kernel's refusal is correct and useless: it does not say which binary
    is old, that the store is newer, or what to run. The seam must."""
    package_root = _isolate_binary_search(monkeypatch, tmp_path)
    kernel = _fake_kernel(
        package_root / "rust-port" / "target" / "release" / "devmap",
        build_stderr=(
            "Error: devmap store /r/.devmap/codeintel/devmap.sqlite: schema version 12 is not "
            "supported by this binary (schema 11); this devmap binary is older than the store; "
            "rebuild it with `cargo build --release -p devmap-cli`"
        ),
    )
    repo = tmp_path / "repo"
    repo.mkdir()
    subprocess.run(["git", "init", "-q", "."], cwd=repo, check=True)

    with pytest.raises(DevMapEngineError) as caught:
        build_map(repo)
    message = str(caught.value)
    assert str(kernel) in message
    assert "schema version 12" in message
    assert "older than the store" in message
    assert "cargo build --release -p devmap-cli" in message
    assert "DEVMAP_BINARY" in message


# --- 2. one rule for locating the kernel ------------------------------------------


def test_the_client_locates_the_kernel_by_the_same_rule_as_the_engine(tmp_path, monkeypatch):
    """Two finders with two rules pick two binaries; a query and a build then
    answer from different kernels with no signal that they did."""
    from devcouncil.devmap_client import DevMapClient

    on_path = _fake_kernel(tmp_path / "cargo-bin" / "devmap")
    package_root = _isolate_binary_search(monkeypatch, tmp_path, path_binary=on_path)
    debug = _fake_kernel(package_root / "rust-port" / "target" / "debug" / "devmap")
    now = time.time()
    _set_mtime(on_path, now - 86400)
    _set_mtime(debug, now - 60)
    other_repo = tmp_path / "other"
    other_repo.mkdir()

    client = DevMapClient(other_repo)
    assert client._find_devmap_binary() == find_engine_binary(other_repo) == str(debug)


def test_the_client_still_degrades_when_no_kernel_is_capable(tmp_path, monkeypatch):
    """`try_connect` must keep returning None, not raising, when there is no kernel."""
    from devcouncil.devmap_client import try_connect

    _isolate_binary_search(monkeypatch, tmp_path)
    repo = tmp_path / "repo"
    repo.mkdir()
    assert try_connect(repo) is None


# --- 3. the client module states each constant once --------------------------------


def test_devmap_client_constants_are_defined_once():
    source = Path(devmap_engine.__file__).with_name("devmap_client.py").read_text(encoding="utf-8")
    tree = ast.parse(source)
    seen: dict[str, int] = {}
    for node in tree.body:
        if isinstance(node, ast.Assign):
            for target in node.targets:
                if isinstance(target, ast.Name) and target.id.isupper():
                    seen[target.id] = seen.get(target.id, 0) + 1
    duplicated = sorted(name for name, count in seen.items() if count > 1)
    assert duplicated == [], f"defined more than once: {duplicated}"
    # A constant nothing reads is a constant whose value nobody checks.
    for name in seen:
        assert source.count(name) > 1, f"{name} is defined but never read"


# --- 4. the Rust map is a valid RepoMap --------------------------------------------


def test_the_rust_manifest_validates_as_a_repo_map():
    """File entries from `devmap manifest` carry no `summary`; the model must accept them."""
    from devcouncil.indexing.repo_mapper import RepoMap

    payload = {
        "languages": ["python"],
        "frameworks": [],
        "package_managers": [],
        "test_commands": [],
        "important_files": [],
        "candidate_files": [],
        "files": [{"area": "src", "kind": "code", "language": "python", "path": "src/a.py"}],
        "subsystems": [],
        "map_engine": "devmap-rust",
    }
    repo_map = RepoMap.model_validate(payload)
    assert repo_map.files[0].summary == ""


# --- 5. map.py: no dead engine, no ignored flags ----------------------------------


def test_map_py_carries_no_retired_python_engine_path():
    source = Path(devmap_engine.__file__).parent.joinpath("cli", "commands", "map.py").read_text(
        encoding="utf-8"
    )
    # Names that only the retired Python engine path used.
    for retired in (
        "GraphBuildBusy",
        "GraphBuildTimeout",
        "export_code_graph_json",
        "run_isolated_full_build",
        "graph_build_session",
    ):
        assert retired not in source, f"{retired} is still referenced by map.py"
    tree = ast.parse(source)
    for node in ast.walk(tree):
        if isinstance(node, ast.FunctionDef) and node.name == "map_repo":
            body = node.body
            for index, statement in enumerate(body):
                if (
                    isinstance(statement, ast.Raise)
                    and index < len(body) - 1
                ):
                    raise AssertionError(
                        "map_repo raises unconditionally before its last statement — "
                        "everything after it is unreachable"
                    )


@pytest.mark.parametrize("flag", ["--no-liveness", "--liveness", "--lsp-refs", "--no-lsp-refs"])
def test_flags_the_rust_engine_cannot_honour_are_gone(flag):
    """A flag that is accepted and ignored is worse than one that is rejected."""
    from typer.testing import CliRunner

    from devcouncil.cli.main import app

    result = CliRunner().invoke(app, ["map", flag])
    assert result.exit_code == 2, result.output
    assert "No such option" in result.output


def _engine_stub(monkeypatch, tmp_path: Path, *, record: list | None = None):
    """Replace the kernel with a writer of a minimal, valid Rust-shaped map."""
    from devcouncil.cli.commands import map as map_cmd

    def _fake_build_map(root: Path, *, output=None, graph_output=None, timeout=900.0, full=False):
        if record is not None:
            record.append({"root": root, "full": full})
        out = output if output is not None else root / ".devcouncil" / "repo_map.json"
        out = out if out.is_absolute() else root / out
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(
            json.dumps(
                {
                    "languages": ["python"],
                    "frameworks": [],
                    "package_managers": [],
                    "test_commands": [],
                    "important_files": [],
                    "candidate_files": [],
                    "files": [
                        {"area": ".", "kind": "code", "language": "python", "path": "k.py"}
                    ],
                    "subsystems": [],
                    "dependency_risks": [],
                    "generated_head": "abc",
                    "indexed_hash": "def",
                    "content_fingerprint": "ghi",
                    "map_engine": "devmap-rust",
                }
            ),
            encoding="utf-8",
        )
        graph = root / ".devcouncil" / "graph" / "code_graph.json"
        graph.parent.mkdir(parents=True, exist_ok=True)
        graph.write_text('{"nodes": [], "edges": []}', encoding="utf-8")
        return out

    def _fake_build_map_result(root: Path, **kwargs):  # noqa: ANN001
        return devmap_engine.BuildResult(
            map_path=_fake_build_map(root, **kwargs),
            graph_path=root / ".devcouncil" / "graph" / "code_graph.json",
        )

    # `build_map_result` is the one function that runs the kernel; `build_map`
    # is its path-returning adapter and delegates here.
    monkeypatch.setattr(devmap_engine, "build_map_result", _fake_build_map_result)
    monkeypatch.setattr(map_cmd, "build_map", _fake_build_map, raising=False)
    monkeypatch.setattr(devmap_engine, "build_map", _fake_build_map)
    # A repository without guides gets them written after the first build and
    # then rebuilt so the store knows the two new files; that second call is
    # its own contract (see test_graph_build_control) and would double every
    # count here. These tests are about routing, so the guides are held still.
    from devcouncil.indexing import map_artifacts

    monkeypatch.setattr(map_artifacts, "write_agent_guides", lambda *_a, **_k: False)
    return map_cmd


def _git_repo(root: Path) -> Path:
    root.mkdir(parents=True, exist_ok=True)
    (root / "k.py").write_text("def helper(a): return a\n", encoding="utf-8")
    subprocess.run(["git", "init", "-q", "."], cwd=root, check=True)
    subprocess.run(["git", "add", "-A"], cwd=root, check=True)
    subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "init"],
        cwd=root,
        check=True,
    )
    return root


def test_full_reaches_the_kernel(tmp_path, monkeypatch):
    from typer.testing import CliRunner

    from devcouncil.cli.main import app

    root = _git_repo(tmp_path / "repo")
    calls: list = []
    _engine_stub(monkeypatch, tmp_path, record=calls)
    result = CliRunner().invoke(app, ["map", "--full", "--no-wiki", "--project-root", str(root)])
    assert result.exit_code == 0, result.output
    assert calls and calls[-1]["full"] is True


def test_scan_deps_still_records_dependency_risks(tmp_path, monkeypatch):
    """The auditor is Python and engine-independent; the flag must keep working."""
    from typer.testing import CliRunner

    from devcouncil.cli.main import app

    root = _git_repo(tmp_path / "repo")
    map_cmd = _engine_stub(monkeypatch, tmp_path)
    monkeypatch.setattr(
        map_cmd.RepoMapper,
        "_scan_dependency_risks",
        lambda self: [{"package": "left-pad", "id": "GHSA-1", "severity": "high"}],
    )
    result = CliRunner().invoke(
        app, ["map", "--scan-deps", "--no-wiki", "--project-root", str(root)]
    )
    assert result.exit_code == 0, result.output
    written = json.loads((root / ".devcouncil" / "repo_map.json").read_text(encoding="utf-8"))
    assert written["dependency_risks"] == [
        {"package": "left-pad", "id": "GHSA-1", "severity": "high"}
    ]
    # Every other key the kernel wrote must survive the merge: the engine stamp
    # and the freshness fields (restamped from the tree when the guides were
    # created, so only their presence is asserted, not the stub's value).
    assert written["map_engine"] == "devmap-rust"
    assert written["generated_head"]
    assert written["indexed_hash"]


def test_a_status_probe_never_spawns_a_daemon(tmp_path, monkeypatch):
    """Asking the kernel for its status must not leave a 30-minute daemon behind.

    Measured: the probe after a build auto-spawned `devmap serve`, which
    reconciled the tree and committed a generation of its own seconds later —
    "one build, one generation" stopped holding, and every test run leaked a
    daemon per temporary repository.
    """
    from devcouncil import devmap_client
    from devcouncil.indexing.map_artifacts import _kernel_status

    spawned: list = []
    monkeypatch.setattr(
        devmap_client.subprocess,
        "Popen",
        lambda *a, **k: spawned.append(a) or (_ for _ in ()).throw(AssertionError("spawned")),
    )
    monkeypatch.setattr(
        devmap_client.DevMapClient,
        "_run_cli_command",
        lambda self, args, timeout=120.0, **_k: {
            "generation_id": 1,
            "pending_count": 0,
            "node_count": 1,
            "edge_count": 0,
            "is_fresh": True,
        },
    )
    monkeypatch.setattr(devmap_client.DevMapClient, "_supports_serve", lambda self, binary: True)
    store = tmp_path / ".devcouncil" / "codeintel" / "devmap.sqlite"
    store.parent.mkdir(parents=True)
    store.write_bytes(b"not empty")

    status = _kernel_status(tmp_path)
    assert status is not None and status.generation_id == 1
    assert spawned == []


def test_goal_still_ranks_candidate_files(tmp_path, monkeypatch):
    from typer.testing import CliRunner

    from devcouncil.cli.main import app

    root = _git_repo(tmp_path / "repo")
    map_cmd = _engine_stub(monkeypatch, tmp_path)
    monkeypatch.setattr(
        map_cmd.RepoMapper,
        "_ripgrep_search",
        lambda self, goal, files: [{"path": "k.py", "reason": f"ripgrep match for '{goal}'"}],
    )
    result = CliRunner().invoke(
        app, ["map", "--goal", "helper", "--no-wiki", "--project-root", str(root)]
    )
    assert result.exit_code == 0, result.output
    written = json.loads((root / ".devcouncil" / "repo_map.json").read_text(encoding="utf-8"))
    assert written["candidate_files"] == [{"path": "k.py", "reason": "ripgrep match for 'helper'"}]


def test_the_wiki_refresh_runs_on_the_rust_map_unless_disabled(tmp_path, monkeypatch):
    from typer.testing import CliRunner

    from devcouncil.cli.main import app

    root = _git_repo(tmp_path / "repo")
    map_cmd = _engine_stub(monkeypatch, tmp_path)
    seen: list = []
    monkeypatch.setattr(
        map_cmd, "_refresh_wiki_skeletons", lambda r, repo_map: seen.append(type(repo_map).__name__)
    )
    assert CliRunner().invoke(app, ["map", "--project-root", str(root)]).exit_code == 0
    assert seen == ["RepoMap"]
    seen.clear()
    assert CliRunner().invoke(app, ["map", "--no-wiki", "--project-root", str(root)]).exit_code == 0
    assert seen == []


def test_a_degraded_kernel_status_is_printed_after_a_build(tmp_path, monkeypatch, capsys):
    """`devmap status` said `is_fresh: false, 64 quarantined` while `dev map`
    printed a green line. A build that leaves the store degraded must say so."""
    from typer.testing import CliRunner

    from devcouncil.cli.main import app
    from devcouncil import devmap_client

    root = _git_repo(tmp_path / "repo")
    _engine_stub(monkeypatch, tmp_path)
    monkeypatch.setattr(
        devmap_client.DevMapClient,
        "status",
        lambda self: devmap_client.DevMapStatus(
            generation_id=7,
            pending_count=3,
            node_count=10,
            edge_count=4,
            is_fresh=False,
            degraded_reason="3 path(s) exceeded the retry threshold",
            quarantined_count=3,
        ),
    )
    result = CliRunner().invoke(app, ["map", "--no-wiki", "--project-root", str(root)])
    assert result.exit_code == 0, result.output
    said = " ".join(result.output.split())
    assert "retry threshold" in said
    assert "not fresh" in said or "degraded" in said


def _have_real_engine() -> bool:
    try:
        find_engine_binary()
    except DevMapEngineError:
        return False
    return True


@pytest.mark.skipif(
    not _have_real_engine(), reason="devmap kernel not built (cargo build --release -p devmap-cli)"
)
def test_creating_the_agent_guides_does_not_make_the_map_read_stale(tmp_path):
    """Measured: a three-file repository was stale on a map one second old.

    The kernel stamps the map from the tree, then the guides are written into
    that tree. In a repository that does not ignore them they join the
    inventory, and `map_is_stale` — the rule `--if-stale`, the watcher, verify
    and checkout all read — says the map is out of date before anyone touched
    a source file. Every one of those callers then rebuilds, forever.
    """
    from devcouncil.indexing.map_artifacts import refresh_map_artifacts
    from devcouncil.indexing.repo_mapper import RepoMapper

    root = _git_repo(tmp_path / "repo")
    (root / ".devcouncil").mkdir()
    assert not (root / "AGENTS.md").exists()
    result = refresh_map_artifacts(root, root / ".devcouncil" / "repo_map.json", quiet=True)
    assert (root / "AGENTS.md").is_file(), "the guide is created on first map"
    written = json.loads((root / ".devcouncil" / "repo_map.json").read_text(encoding="utf-8"))
    assert RepoMapper(root).map_is_stale(written) is False
    assert result.repo_map.generated_head == written["generated_head"]


# --- 7. dev map query does not fan out 41 kernel calls ----------------------------


def test_query_measures_edges_for_exact_matches_first_and_caps_the_fan_out(tmp_path, monkeypatch):
    """One `dev map query foo` used to issue 41 kernel round-trips: a search
    plus two edge calls for each of up to 20 partial matches. The symbol the
    caller asked for must come first with its edges measured; the tail must
    say it was not measured, not pretend to be empty."""
    from devcouncil.cli.commands import graph_cmd
    from devcouncil.devmap_client import BudgetedResponse

    calls: list[tuple[str, str]] = []

    class _Client:
        def search(self, query, limit=2000, semantic=False):
            items = [
                {"file_path": f"src/m{index}.py", "symbol_name": f"helper_{index}", "span": (1, 2)}
                for index in range(19)
            ]
            # The exact match arrives last in FTS order.
            items.append({"file_path": "src/core.py", "symbol_name": "helper", "span": (7, 9)})
            return BudgetedResponse(
                shown=20, hidden=0, total=20, truncated=False, tokens_used=100, items=items,
                resolution="Available",
            )

        def impact(self, target, depth=1):
            calls.append(("impact", target))
            return BudgetedResponse(
                shown=0, hidden=0, total=0, truncated=False, tokens_used=0, items=[],
                resolution="Available",
            )

        def deps(self, target, depth=1):
            calls.append(("deps", target))
            return BudgetedResponse(
                shown=0, hidden=0, total=0, truncated=False, tokens_used=0, items=[],
                resolution="Available",
            )

    monkeypatch.setattr("devcouncil.devmap_client.try_connect", lambda root: _Client())
    payload = graph_cmd._devmap_query_payload(tmp_path, "query", name_or_path="helper")
    definitions = payload["definitions"]
    assert definitions[0]["id"] == "src/core.py::helper"
    assert definitions[0]["callers"] == [] and definitions[0]["callees"] == []
    measured = [d for d in definitions if d["callers"] is not None]
    assert len(measured) == graph_cmd._QUERY_EDGE_DEFINITION_CAP
    assert len(calls) == 2 * graph_cmd._QUERY_EDGE_DEFINITION_CAP
    assert calls[0] == ("impact", "src/core.py::helper")
    tail = definitions[-1]
    assert tail["callers"] is None
    assert "not measured" in tail["callers_unavailable"]


# --- 6. one writer for repo_map.json ----------------------------------------------


def _stale_map(root: Path) -> Path:
    map_path = root / ".devcouncil" / "repo_map.json"
    map_path.parent.mkdir(parents=True, exist_ok=True)
    map_path.write_text(
        json.dumps({"generated_head": "deadbeef", "indexed_hash": "old", "files": []}),
        encoding="utf-8",
    )
    return map_path


def test_refresh_stale_map_if_needed_builds_through_the_kernel(tmp_path, monkeypatch):
    from devcouncil.indexing import map_artifacts, map_refresh

    root = _git_repo(tmp_path / "repo")
    _stale_map(root)
    calls: list = []
    _engine_stub(monkeypatch, tmp_path, record=calls)

    def _python_engine_must_not_run(*_a, **_k):
        raise AssertionError("the Python engine wrote repo_map.json")

    monkeypatch.setattr(map_artifacts, "generate_map_artifacts", _python_engine_must_not_run)
    assert map_refresh.refresh_stale_map_if_needed(root) is True
    assert len(calls) == 1
    written = json.loads((root / ".devcouncil" / "repo_map.json").read_text(encoding="utf-8"))
    assert written["map_engine"] == "devmap-rust"


def test_refresh_stale_map_if_needed_never_raises_when_the_kernel_is_unavailable(
    tmp_path, monkeypatch
):
    from devcouncil.indexing import map_refresh

    root = _git_repo(tmp_path / "repo")
    _stale_map(root)

    def _boom(*_a, **_k):
        raise DevMapEngineError("no kernel")

    monkeypatch.setattr(devmap_engine, "build_map_result", _boom)
    assert map_refresh.refresh_stale_map_if_needed(root) is False


def test_refresh_map_artifacts_builds_through_the_kernel(tmp_path, monkeypatch):
    from devcouncil.indexing import map_artifacts

    root = _git_repo(tmp_path / "repo")
    calls: list = []
    _engine_stub(monkeypatch, tmp_path, record=calls)
    result = map_artifacts.refresh_map_artifacts(root, root / ".devcouncil" / "repo_map.json")
    assert len(calls) == 1
    assert result.repo_map.files[0].path == "k.py"
    assert result.degraded is False
    assert result.mode == "devmap-rust"


@pytest.mark.parametrize("command", [["init"], ["ingest"], ["sync"]])
def test_graph_subcommands_build_through_the_kernel(tmp_path, monkeypatch, command):
    from typer.testing import CliRunner

    from devcouncil.cli.main import app

    root = _git_repo(tmp_path / "repo")
    calls: list = []
    _engine_stub(monkeypatch, tmp_path, record=calls)
    result = CliRunner().invoke(app, ["map", *command, "--project-root", str(root)])
    assert result.exit_code == 0, result.output
    assert len(calls) == 1


def test_mcp_graph_ingest_builds_through_the_kernel(tmp_path, monkeypatch):
    import asyncio

    from devcouncil.integrations.mcp.handlers.map import handle_graph_ingest

    root = _git_repo(tmp_path / "repo")
    calls: list = []
    _engine_stub(monkeypatch, tmp_path, record=calls)
    out = asyncio.run(handle_graph_ingest(root, {}))
    assert len(calls) == 1
    payload = json.loads(out[0].text)
    assert payload["ok"] is True
    assert payload["mode"] == "devmap-rust"


def test_the_future_schema_marker_is_a_phrase_the_store_actually_emits():
    """The seam recognises a store newer than the binary by a phrase in the
    kernel's refusal. The kernel rewrote that refusal (K3: it now names the
    store, both versions and the remedy) and the seam kept looking for the old
    one — so `classify_kernel_failure` filed the real failure under
    `kernel_failed` and `_explain_kernel_failure` never fired, while the tests
    fed the seam the old text from a fake kernel and stayed green. The phrase
    is read out of the store's source, the same way the wiring parity test
    reads `wiring.rs`."""
    from devcouncil import devmap_engine

    db_rs = (
        Path(__file__).resolve().parents[2] / "rust-port/crates/devmap-store/src/db.rs"
    ).read_text(encoding="utf-8")
    start = db_rs.index("fn unsupported_schema(")
    body = db_rs[start : db_rs.index("\n    }\n", start)]
    assert devmap_engine._FUTURE_SCHEMA_MARKER in body, (
        "the seam's marker must be a phrase the store's refusal spells: "
        f"{devmap_engine._FUTURE_SCHEMA_MARKER!r}"
    )
