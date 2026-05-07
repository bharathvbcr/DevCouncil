import fnmatch
import subprocess
from collections import Counter
from pathlib import Path

from devcouncil.indexing.repo_mapper import RepoMapper


def _path_exists(file_set: set[str], path: str) -> bool:
    path = path.strip()
    if not path.startswith("src/devcouncil/"):
        path = f"src/devcouncil/{path}"
    if "*" in path:
        return any(fnmatch.fnmatch(item, path) for item in file_set)
    return path in file_set


def test_repo_mapper_basic():
    mapper = RepoMapper(Path("."))
    repo_map = mapper.map_repo("init")
    
    assert "python" in repo_map.languages
    assert "uv" in repo_map.package_managers
    assert "npm" in repo_map.package_managers
    assert "pyproject.toml" in repo_map.important_files
    assert "python" in repo_map.lsp["languages"]
    assert all("__pycache__" not in item["path"] for item in repo_map.candidate_files)
    assert any(entry.path == "src/devcouncil/cli/main.py" and entry.area == "src/devcouncil/cli" for entry in repo_map.files)
    assert any(entry.path == "README.md" and entry.kind == "doc" for entry in repo_map.files)
    file_set = {item.path for item in repo_map.files}
    file_paths = [item.path for item in repo_map.files]
    assert file_paths == sorted(file_paths), "files paths are not sorted deterministically"
    assert len(file_paths) == len(set(file_paths)), "files list contains duplicate paths"
    area_targets = {
        "src/devcouncil/council",
        "src/devcouncil/domain",
        "src/devcouncil/indexing",
        "src/devcouncil/integrations",
        "src/devcouncil/live",
        "src/devcouncil/llm",
        "src/devcouncil/planning",
        "src/devcouncil/repo",
        "src/devcouncil/reporting",
        "src/devcouncil/telemetry",
        "src/devcouncil/ui",
        "src/devcouncil/utils",
    }
    subsystem_areas = {s.area for s in repo_map.subsystems}
    assert area_targets.issubset(subsystem_areas)
    areas = [s.area for s in repo_map.subsystems]
    duplicates = [area for area, count in Counter(areas).items() if count > 1]
    assert not duplicates, f"duplicate subsystem areas: {sorted(duplicates)}"

    for area in areas:
        subsystem = next(s for s in repo_map.subsystems if s.area == area)
        assert isinstance(subsystem.area, str)
        assert subsystem.area and not ("\\" in subsystem.area or subsystem.area.endswith("/"))
        assert subsystem.area.startswith("src/devcouncil/")
        assert any(path.startswith(f"{subsystem.area}/") for path in file_set), (
            f"area points to missing src/devcouncil prefix: {subsystem.area}"
        )
        assert subsystem.entry_points, f"missing entry_points for {area}"
        assert subsystem.critical_files, f"missing critical_files for {area}"
        for path in subsystem.entry_points:
            assert _path_exists(file_set, path), f"entry_point missing for {area}: {path}"
        for path in subsystem.critical_files:
            assert _path_exists(file_set, path), f"critical_file missing for {area}: {path}"
        assert isinstance(subsystem.role_files, dict)
        assert subsystem.role_files, f"role_files empty for {area}"
        assert isinstance(subsystem.handoff_paths, list)
        assert isinstance(subsystem.neighbors, list)
        for handoff in subsystem.handoff_paths:
            bits = handoff.split("->")
            assert len(bits) == 2, f"bad handoff format in {area}: {handoff}"
            for target in bits:
                assert _path_exists(file_set, target), f"handoff target missing for {area}: {target}"
        for neighbor in subsystem.neighbors:
            assert any(path.startswith(f"{neighbor}/") for path in file_set), f"neighbor missing files for {area}: {neighbor}"
    # "init" should match some files if any contain it
    # Since we have src/devcouncil/cli/commands/init.py, it should match
    assert len(repo_map.candidate_files) > 0


def test_repo_mapper_filters_temp_files(monkeypatch):
    mapper = RepoMapper(Path("."))

    def fake_check_output(*args, **kwargs):
        return b"src/devcouncil/cli/main.py\ntmp_dbg_repo_map.py\n"

    monkeypatch.setattr(subprocess, "check_output", fake_check_output)

    files = mapper.get_git_files()

    assert "src/devcouncil/cli/main.py" in files
    assert "tmp_dbg_repo_map.py" not in files
