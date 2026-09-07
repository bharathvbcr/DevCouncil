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


def _kernel_map(tmp_path: Path, files: dict[str, str]) -> dict:
    """The map the kernel builds over ``files``, written under a fresh root.

    Every test here used to read the checkout's own ``.devcouncil/repo_map.json``.
    That file is whatever kernel last ran ``dev map`` in this checkout — the
    harness's post-tool hook writes it uninvited, and a worktree carried one
    from a kernel that predated the inventory pass, so the assertions failed
    against machine state rather than against the tree. A fixture the kernel
    builds here and now measures the kernel in the tree, skips only when no
    kernel is built, and reads the same artifact through the same seam.
    """
    from tests.unit.graph_fixtures import _have_kernel, git_init_commit, write_sources

    if not _have_kernel():
        pytest.skip("devmap kernel not built (cargo build --release -p devmap-cli)")
    from devcouncil.indexing.map_artifacts import refresh_map_artifacts

    root = tmp_path / "repo"
    write_sources(root, files)
    git_init_commit(root)
    (root / ".devcouncil").mkdir(exist_ok=True)
    refresh_map_artifacts(root, root / ".devcouncil" / "repo_map.json", quiet=True)
    return json.loads((root / ".devcouncil" / "repo_map.json").read_text(encoding="utf-8"))


_POLYGLOT_FIXTURE = {
    "pyproject.toml": (
        '[project]\nname = "fixture"\nversion = "0.1.0"\n\n[tool.pytest.ini_options]\ntestpaths = ["tests"]\n'
    ),
    "uv.lock": "version = 1\n",
    "package.json": '{"name": "fixture", "scripts": {"test": "jest"}}\n',
    "package-lock.json": '{"name": "fixture", "lockfileVersion": 3}\n',
    "rust/Cargo.toml": '[package]\nname = "fixture"\nversion = "0.1.0"\n',
    "rust/src/lib.rs": "pub fn one() -> u32 {\n    1\n}\n",
    "backend/go.mod": "module example.com/fixture\n\ngo 1.22\n",
    "backend/main.go": "package main\n\nfunc main() {}\n",
    "README.md": "# fixture\n",
    "src/pkg/__init__.py": "",
    "src/pkg/core.py": "def helper():\n    return 1\n",
    "tests/test_core.py": "from pkg.core import helper\n\n\ndef test_helper():\n    assert helper() == 1\n",
}


def test_repo_map_package_managers_and_test_commands_are_detected(tmp_path):
    """The kernel reads the repository's own manifests, not a constant.

    Split out of the xfail below when `inventory::scan` landed: this half is
    computed now, from `uv.lock` / `package-lock.json` / `Cargo.toml` /
    `go.mod` under a bounded walk of the repository root, and the marker says
    so. The other half — `lsp` and `candidate_files` — still has no producer
    in the kernel and keeps its xfail. The fixture declares all four managers
    and every test runner the rules know, at the root and nested, so the
    assertion is about the rules rather than about this checkout.
    """
    raw = _kernel_map(tmp_path, _POLYGLOT_FIXTURE)
    marker = (raw.get("meta") or {}).get("devmap_rust") or {}
    assert marker.get("package_managers_computed") is True, (
        f"an empty package_managers from a pass that ran must be distinguishable from one nothing looked at: {marker}"
    )
    assert marker.get("test_commands_computed") is True, marker
    for manager in ("uv", "npm", "cargo", "go mod"):
        assert manager in raw["package_managers"], (manager, raw["package_managers"])
    for command in ("pytest", "npm test", "cargo test", "go test ./..."):
        assert command in raw["test_commands"], (command, raw["test_commands"])


@pytest.mark.xfail(
    strict=True,
    reason=(
        "the kernel manifest writes candidate_files=[] and lsp={} as constants "
        "(rust-port/crates/devmap-query/src/manifest.rs): `candidate_files` is "
        "goal-ranked and this producer is given no goal, and no language server "
        "is consulted by the kernel at all. Both are marked "
        "`*_computed: false` so a reader can tell the constant from an answer; "
        "package_managers/test_commands were split out above once "
        "`inventory::scan` made them real."
    ),
)
def test_repo_map_lsp_and_candidate_files_are_detected(tmp_path):
    raw = _kernel_map(tmp_path, _POLYGLOT_FIXTURE)
    assert "python" in raw["lsp"]["languages"]
    assert len(raw["candidate_files"]) > 0  # manifest.rs writes `candidate_files: []`


def test_kernel_map_shape_on_a_fixture_repo(tmp_path):
    """The navigation fields the generated agent guide points at are answers.

    `role_files`, `handoff_paths` and `files[].kind` were once constants — `{}`,
    `[]` and the literal `"code"` on every entry, 1,417 of 1,417 on this
    repository, `README.md` included — while steps 5 and 6 of the guide `dev
    map` writes told agents to navigate by exactly those fields. The assertions
    were always the right ones; they hold against a map the kernel builds here,
    and the provenance markers are what let a consumer tell an empty bucket
    from an unimplemented field.
    """
    from devcouncil.indexing.repo_mapper import RepoMap

    raw = _kernel_map(
        tmp_path,
        {
            "pyproject.toml": '[project]\nname = "fixture"\nversion = "0.1.0"\n',
            "README.md": "# fixture\n",
            "src/pkg/__init__.py": "",
            "src/pkg/cli/__init__.py": "",
            "src/pkg/cli/main.py": "from pkg.cli.commands.init import run\n\n\ndef main():\n    return run()\n",
            "src/pkg/cli/commands/__init__.py": "",
            "src/pkg/cli/commands/init.py": "def run():\n    return 1\n",
            "src/pkg/__pycache__/junk.cpython-312.pyc": "",
            "tests/test_main.py": "from pkg.cli.main import main\n\n\ndef test_main():\n    assert main() == 1\n",
        },
    )
    assert raw["map_engine"] == "devmap-rust"
    repo_map = RepoMap.model_validate(raw)

    assert "python" in repo_map.languages
    assert "pyproject.toml" in repo_map.important_files

    # The inventory pass ran over this fixture's root. It declares a bare
    # `pyproject.toml` and no lock file, so the honest answer is an empty
    # `package_managers` — and the marker is what makes that empty list an
    # answer rather than the constant it used to be.
    marker = (raw.get("meta") or {}).get("devmap_rust") or {}
    assert marker.get("package_managers_computed") is True, f"the inventory did not run over the fixture: {marker}"
    assert marker.get("test_commands_computed") is True, marker
    assert "uv" not in repo_map.package_managers, (
        f"a bare pyproject.toml is not evidence of uv (tests/unit/test_cli_commands.py:91): {repo_map.package_managers}"
    )
    assert repo_map.test_commands == ["pytest"], (
        "this fixture declares no runner but carries a test file, which is "
        f"what the pytest rule reads: {repo_map.test_commands}"
    )
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

    # The three navigation fields, on the raw record: every subsystem has role
    # buckets, some subsystem hands off to another (cli -> cli/commands), the
    # README is a doc rather than "code", and `kind` partitions the corpus.
    for subsystem in raw["subsystems"]:
        assert subsystem.get("role_files"), f"role_files empty for {subsystem['area']}"
    assert any(subsystem.get("handoff_paths") for subsystem in raw["subsystems"]), (
        "main.py imports commands/init.py across subsystems, so at least one "
        f"handoff_paths must be non-empty: {[s['area'] for s in raw['subsystems']]}"
    )
    assert any(f["path"] == "README.md" and f["kind"] == "doc" for f in raw["files"])
    assert len({f["kind"] for f in raw["files"]}) >= 4, (
        f"files[].kind is close to a constant again: {sorted({f['kind'] for f in raw['files']})}"
    )
    # The real per-role totals travel with the capped samples, so a reader can
    # tell a small subsystem from a truncated one.
    for subsystem in raw["subsystems"]:
        counts = subsystem.get("role_file_counts") or {}
        assert set(subsystem["role_files"]) <= set(counts), (
            f"role bucket without a count in {subsystem['area']}: {sorted(set(subsystem['role_files']) - set(counts))}"
        )
    for flag in ("role_files_computed", "handoff_paths_computed", "file_kinds_computed"):
        assert marker.get(flag) is True, f"missing provenance marker {flag}: {marker}"


def test_repo_mapper_filters_temp_files(monkeypatch):
    mapper = RepoMapper(Path("."))

    def fake_check_output(*args, **kwargs):
        return b"src/devcouncil/cli/main.py\ntmp_dbg_repo_map.py\n"

    monkeypatch.setattr(subprocess, "check_output", fake_check_output)

    files = mapper.get_git_files()

    assert "src/devcouncil/cli/main.py" in files
    assert "tmp_dbg_repo_map.py" not in files
