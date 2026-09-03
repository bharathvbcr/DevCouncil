import fnmatch
import subprocess
from collections import Counter
import json
from pathlib import Path

import pytest

from devcouncil.indexing.repo_mapper import RepoMapper


def _path_exists(file_set: set[str], path: str) -> bool:
    path = path.strip()
    if "*" in path:
        return any(fnmatch.fnmatch(item, path) for item in file_set)
    return path in file_set


def test_repo_mapper_basic():
    """The kernel's map of this repository has the shape every consumer reads.

    Used to build the map with the Python engine; the kernel writes it now, so
    the test reads the artifact `dev map` left (skipped when there is none).
    The kernel maps the whole repository (benchmarks, rust-port, ...) into a
    handful of subsystems; the Python writer's per-package split under
    ``src/devcouncil`` was its own granularity choice and is not asserted.
    """
    from devcouncil.indexing.repo_mapper import RepoMap

    map_path = Path(".") / ".devcouncil" / "repo_map.json"
    if not map_path.is_file():
        pytest.skip("no kernel-built map on disk: run `dev map`")
    raw = json.loads(map_path.read_text(encoding="utf-8"))
    # The engine stamp is on the artifact; the RepoMap model does not declare it.
    assert raw["map_engine"] == "devmap-rust"
    repo_map = RepoMap.model_validate(raw)

    assert "python" in repo_map.languages
    assert "pyproject.toml" in repo_map.important_files
    assert all("__pycache__" not in item.path for item in repo_map.files)
    assert any(
        entry.path == "src/devcouncil/cli/main.py" and entry.area == "src/devcouncil/cli"
        for entry in repo_map.files
    )
    file_set = {item.path for item in repo_map.files}
    file_paths = [item.path for item in repo_map.files]
    assert file_paths == sorted(file_paths), "files paths are not sorted deterministically"
    assert len(file_paths) == len(set(file_paths)), "files list contains duplicate paths"

    # The kernel guarantees a file's area is its parent directory; which
    # directories become subsystems depends on the tree and is not asserted.
    for item in repo_map.files:
        parent = item.path.rsplit("/", 1)[0] if "/" in item.path else "."
        assert item.area == parent, f"area of {item.path} is {item.area}, not {parent}"
    areas = [s.area for s in repo_map.subsystems]
    duplicates = [area for area, count in Counter(areas).items() if count > 1]
    assert not duplicates, f"duplicate subsystem areas: {sorted(duplicates)}"
    for subsystem in repo_map.subsystems:
        area = subsystem.area
        assert area and not ("\\" in area or area.endswith("/"))
        assert any(path.startswith(f"{area}/") for path in file_set), (
            f"area points to no indexed file: {area}"
        )
        assert subsystem.entry_points, f"missing entry_points for {area}"
        assert subsystem.critical_files, f"missing critical_files for {area}"
        for path in subsystem.entry_points:
            assert _path_exists(file_set, path), f"entry_point missing for {area}: {path}"
        for path in subsystem.critical_files:
            assert _path_exists(file_set, path), f"critical_file missing for {area}: {path}"
        assert isinstance(subsystem.role_files, dict)
        assert isinstance(subsystem.handoff_paths, list)
        assert isinstance(subsystem.neighbors, list)
        for handoff in subsystem.handoff_paths:
            bits = handoff.split("->")
            assert len(bits) == 2, f"bad handoff format in {area}: {handoff}"
            for target in bits:
                assert _path_exists(file_set, target), f"handoff target missing for {area}: {target}"
        for neighbor in subsystem.neighbors:
            assert any(path.startswith(f"{neighbor}/") for path in file_set), (
                f"neighbor missing files for {area}: {neighbor}"
            )


@pytest.mark.xfail(
    strict=True,
    reason=(
        "the kernel manifest leaves every subsystem's role_files empty and writes no "
        "handoff_paths, and classifies every file (README.md included) as kind=code. "
        "CLAUDE.md steps 5-6 tell agents to navigate by role_files / handoff_paths, "
        "and the Python writer filled them."
    ),
)
def test_repo_map_subsystem_roles_handoffs_and_file_kinds():
    map_path = Path(".") / ".devcouncil" / "repo_map.json"
    if not map_path.is_file():
        pytest.skip("no kernel-built map on disk: run `dev map`")
    raw = json.loads(map_path.read_text(encoding="utf-8"))
    for subsystem in raw["subsystems"]:
        assert subsystem.get("role_files"), f"role_files empty for {subsystem['area']}"
    assert any(subsystem.get("handoff_paths") for subsystem in raw["subsystems"])
    assert any(f["path"] == "README.md" and f["kind"] == "doc" for f in raw["files"])


@pytest.mark.xfail(
    strict=True,
    reason=(
        "the kernel manifest writes package_managers=[], candidate_files=[] and lsp={} as constants "
        "(rust-port/crates/devmap-query/src/manifest.rs); the Python writer detected "
        "uv/npm from lockfiles and the LSP languages. wiki.py and the MCP map handler "
        "render these fields, so they are silently empty until the kernel fills them."
    ),
)
def test_repo_map_package_managers_and_lsp_are_detected():
    map_path = Path(".") / ".devcouncil" / "repo_map.json"
    if not map_path.is_file():
        pytest.skip("no kernel-built map on disk: run `dev map`")
    raw = json.loads(map_path.read_text(encoding="utf-8"))
    assert "uv" in raw["package_managers"]
    assert "npm" in raw["package_managers"]
    assert "python" in raw["lsp"]["languages"]
    assert len(raw["candidate_files"]) > 0  # manifest.rs writes `candidate_files: []`

def test_kernel_map_shape_on_a_fixture_repo(tmp_path):
    """The same shape checks against a map the kernel builds here and now.

    The live-map test above skips on a fresh clone; this one only skips when
    no kernel binary is built, so the contract is exercised wherever the
    kernel is.
    """
    from devcouncil.indexing.repo_mapper import RepoMap

    from tests.unit.graph_fixtures import _have_kernel, git_init_commit, write_sources

    if not _have_kernel():
        pytest.skip("devmap kernel not built (cargo build --release -p devmap-cli)")
    from devcouncil.indexing.map_artifacts import refresh_map_artifacts

    root = tmp_path / "repo"
    write_sources(
        root,
        {
            "pyproject.toml": '[project]\nname = "fixture"\nversion = "0.1.0"\n',
            "README.md": "# fixture\n",
            "src/pkg/__init__.py": "",
            "src/pkg/cli/__init__.py": "",
            "src/pkg/cli/main.py": "from pkg.cli.commands.init import run\n\n\ndef main():\n    return run()\n",
            "src/pkg/cli/commands/__init__.py": "",
            "src/pkg/cli/commands/init.py": "def run():\n    return 1\n",
            "src/pkg/__pycache__/junk.cpython-312.pyc": "",
        },
    )
    git_init_commit(root)
    (root / ".devcouncil").mkdir(exist_ok=True)
    refresh_map_artifacts(root, root / ".devcouncil" / "repo_map.json", quiet=True)
    raw = json.loads((root / ".devcouncil" / "repo_map.json").read_text(encoding="utf-8"))
    assert raw["map_engine"] == "devmap-rust"
    repo_map = RepoMap.model_validate(raw)

    assert "python" in repo_map.languages
    assert "pyproject.toml" in repo_map.important_files
    paths = [item.path for item in repo_map.files]
    assert "src/pkg/cli/main.py" in paths
    assert all("__pycache__" not in path for path in paths)
    assert paths == sorted(paths) and len(paths) == len(set(paths))
    for item in repo_map.files:
        parent = item.path.rsplit("/", 1)[0] if "/" in item.path else "."
        assert item.area == parent
    assert repo_map.subsystems, "a repository with sources maps to at least one subsystem"
    file_set = set(paths)
    for subsystem in repo_map.subsystems:
        assert any(path.startswith(f"{subsystem.area}/") for path in file_set)
        assert subsystem.entry_points and subsystem.critical_files
        for path in [*subsystem.entry_points, *subsystem.critical_files]:
            assert _path_exists(file_set, path), f"{subsystem.area}: {path}"


def test_repo_mapper_filters_temp_files(monkeypatch):
    mapper = RepoMapper(Path("."))

    def fake_check_output(*args, **kwargs):
        return b"src/devcouncil/cli/main.py\ntmp_dbg_repo_map.py\n"

    monkeypatch.setattr(subprocess, "check_output", fake_check_output)

    files = mapper.get_git_files()

    assert "src/devcouncil/cli/main.py" in files
    assert "tmp_dbg_repo_map.py" not in files
