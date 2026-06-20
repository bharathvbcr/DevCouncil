import json
import fnmatch
import copy

from typer.testing import CliRunner

from devcouncil.cli.main import app


runner = CliRunner()


def test_repo_map_artifact_is_stable_and_valid(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    (tmp_path / "pyproject.toml").write_text("[project]\nname='sample'\n", encoding="utf-8")
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "sample.py").write_text("print('sample')\n", encoding="utf-8")
    (tmp_path / "README.md").write_text("# sample\n", encoding="utf-8")

    assert runner.invoke(app, ["init"]).exit_code == 0

    first = runner.invoke(app, ["map", "sample", "--output", ".devcouncil/repo_map.json"])
    assert first.exit_code == 0
    map_path = tmp_path / ".devcouncil" / "repo_map.json"
    assert map_path.exists()

    first_raw = map_path.read_text(encoding="utf-8")
    first_payload = json.loads(first_raw)
    # `dev init` now generates AGENTS.md and CLAUDE.md, so the agent guides are a stable part of the
    # initialized repository state and are present (and regenerated identically) on both map runs.

    second = runner.invoke(app, ["map", "sample", "--output", ".devcouncil/repo_map.json"])
    assert second.exit_code == 0
    second_raw = map_path.read_text(encoding="utf-8")
    second_payload = json.loads(second_raw)

    def normalize_repo_map(payload: dict) -> dict:
        normalized = copy.deepcopy(payload)

        normalized["files"] = [
            {**file, "path": file["path"].replace("\\", "/"), "area": file["area"].replace("\\", "/")}
            for file in normalized["files"]
        ]

        normalized["subsystems"] = [
            {
                **subsystem,
                "area": subsystem["area"].replace("\\", "/"),
                "entry_points": [entry.replace("\\", "/") for entry in subsystem["entry_points"]],
                "critical_files": [path.replace("\\", "/") for path in subsystem["critical_files"]],
                "neighbors": [path.replace("\\", "/") for path in subsystem["neighbors"]],
                "handoff_paths": [handoff.replace("\\", "/") for handoff in subsystem["handoff_paths"]],
                "role_files": {
                    role: [path.replace("\\", "/") for path in paths]
                    for role, paths in subsystem["role_files"].items()
                },
            }
            for subsystem in normalized["subsystems"]
        ]

        return normalized

    first_payload = normalize_repo_map(first_payload)
    second_payload = normalize_repo_map(second_payload)

    assert first_payload == second_payload, "repo_map.json payload is not deterministic"
    assert "files" in first_payload
    assert "subsystems" in first_payload

    files = first_payload["files"]
    file_paths = [item["path"] for item in files]
    assert file_paths == sorted(file_paths), "repo_map files section is not sorted"
    assert len(file_paths) == len(set(file_paths)), "repo_map files section contains duplicates"

    file_set = set(file_paths)
    subsystems = first_payload["subsystems"]
    seen_areas = set()

    def path_exists(candidate: str) -> bool:
        candidate = candidate.replace("\\", "/")
        if candidate.startswith("src/"):
            full = candidate
        elif candidate:
            full = f"src/devcouncil/{candidate}"
        else:
            return False
        if "*" in full:
            return any(fnmatch.fnmatch(item, full) for item in file_set)
        return full in file_set

    for subsystem in subsystems:
        area = subsystem["area"]
        # Subsystems are inferred generically from the directory tree for non-DevCouncil
        # repos (this sample repo), so the area just needs to be a real path prefix with
        # files under it (asserted below) rather than a hardcoded DevCouncil path.
        assert area, "subsystem area must be non-empty"
        assert area not in seen_areas, f"duplicate subsystem area: {area}"
        seen_areas.add(area)
        assert any(path.startswith(f"{area}/") for path in file_set), f"area prefix missing files: {area}"
        assert subsystem["entry_points"], f"missing entry_points for {area}"
        assert subsystem["critical_files"], f"missing critical_files for {area}"
        assert isinstance(subsystem["neighbors"], list), f"neighbors must be a list for {area}"
        assert isinstance(subsystem["handoff_paths"], list), f"handoff paths must be a list for {area}"
        assert isinstance(subsystem["role_files"], dict), f"role_files must be a dict for {area}"
        for handoff in subsystem["handoff_paths"]:
            parts = [x.strip() for x in handoff.split("->")]
            assert len(parts) == 2, f"invalid handoff format: {handoff}"
            assert all(path_exists(p) for p in parts), f"handoff path missing in map artifact for {area}: {handoff}"
        for role, paths in subsystem["role_files"].items():
            assert paths, f"empty role bucket '{role}' in {area}"
            for path in paths:
                assert path_exists(path), f"role file path missing: {area}:{path}"
