"""Generic (non-DevCouncil) repo mapping: real subsystems from any directory tree."""

import subprocess

from devcouncil.indexing.repo_mapper import RepoMapper


def _git(root, *args):
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True)


def _make_repo(tmp_path):
    files = {
        "src/myapp/__init__.py": "",
        "src/myapp/main.py": "from myapp.api.handlers import handle\nfrom myapp.core.models import Model\n",
        "src/myapp/api/__init__.py": "",
        "src/myapp/api/handlers.py": "from myapp.core.models import Model\n\ndef handle():\n    return Model()\n",
        "src/myapp/api/routes.py": "from myapp.api.handlers import handle\n",
        "src/myapp/core/__init__.py": "",
        "src/myapp/core/models.py": "from myapp.core.db import connect\n\nclass Model:\n    pass\n",
        "src/myapp/core/db.py": "def connect():\n    return None\n",
        "tests/test_api.py": "from myapp.api.handlers import handle\n",
        "README.md": "# myapp\n",
        "pyproject.toml": "[project]\nname='myapp'\n",
    }
    for rel, content in files.items():
        p = tmp_path / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content, encoding="utf-8")
    _git(tmp_path, "init")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init")


def test_detects_source_root(tmp_path):
    _make_repo(tmp_path)
    mapper = RepoMapper(tmp_path)
    assert mapper.detect_source_root(mapper.get_git_files()) == "src/myapp"


def test_generic_subsystems_from_directory_tree(tmp_path):
    _make_repo(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()

    areas = {s.area for s in repo_map.subsystems}
    assert "src/myapp/api" in areas
    assert "src/myapp/core" in areas

    by_area = {s.area: s for s in repo_map.subsystems}
    # Cross-area import (api.handlers -> core.models) becomes a neighbor edge.
    assert "src/myapp/core" in by_area["src/myapp/api"].neighbors
    # The most-imported file in core (models.py, imported by main + handlers) ranks
    # ahead of db.py in critical_files.
    core_critical = by_area["src/myapp/core"].critical_files
    assert core_critical.index("src/myapp/core/models.py") < core_critical.index("src/myapp/core/db.py")
    # Summary is derived from the area's files, not a bare stem.
    assert "models" in by_area["src/myapp/core"].summary


def test_generic_important_files_seeded_from_import_degree(tmp_path):
    _make_repo(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    # handlers.py and models.py are the most depended-on modules.
    assert "src/myapp/core/models.py" in repo_map.important_files
    assert "src/myapp/api/handlers.py" in repo_map.important_files


def test_generic_file_areas_are_directory_based(tmp_path):
    _make_repo(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    area_of = {f.path: f.area for f in repo_map.files}
    assert area_of["src/myapp/api/handlers.py"] == "src/myapp/api"
    assert area_of["src/myapp/core/db.py"] == "src/myapp/core"
    assert area_of["tests/test_api.py"] == "tests"


def test_dependents_index_reverses_imports(tmp_path):
    _make_repo(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    deps = repo_map.dependents
    # models.py is imported by main.py and handlers.py.
    assert set(deps.get("src/myapp/core/models.py", [])) >= {
        "src/myapp/main.py",
        "src/myapp/api/handlers.py",
    }
    # db.py is imported only by models.py.
    assert deps.get("src/myapp/core/db.py") == ["src/myapp/core/models.py"]
    # A leaf with no importers is absent from the index.
    assert "src/myapp/main.py" not in deps


def test_map_records_head_and_hash_and_detects_staleness(tmp_path):
    _make_repo(tmp_path)
    mapper = RepoMapper(tmp_path)
    repo_map = mapper.map_repo()
    assert repo_map.generated_head  # current HEAD captured
    assert repo_map.indexed_hash

    data = repo_map.model_dump()
    assert mapper.map_is_stale(data) is False  # fresh right after generation

    # A new committed file changes both HEAD and the tracked-file set -> stale.
    (tmp_path / "src" / "myapp" / "extra.py").write_text("x = 1\n", encoding="utf-8")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(tmp_path, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "add extra")
    assert RepoMapper(tmp_path).map_is_stale(data) is True


def test_map_without_fingerprints_is_not_flagged_stale(tmp_path):
    _make_repo(tmp_path)
    # A legacy map (pre-fingerprinting) must not raise false staleness alarms.
    assert RepoMapper(tmp_path).map_is_stale({"subsystems": []}) is False
