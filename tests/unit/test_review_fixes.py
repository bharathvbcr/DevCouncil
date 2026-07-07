"""Regression tests for bugs found in the adversarial diff review."""

from devcouncil.domain.evidence import CommandResult
from devcouncil.execution.policy_engine import TaskPolicyEngine
from devcouncil.indexing.repo_mapper import RepoMapper
from devcouncil.integrations.mcp.util import diff_target_paths as _diff_target_paths
from devcouncil.verification.verifier import Verifier


# --- verifier: malformed-command classification (pre-existing design preserved) ---

def test_pytest_collection_error_still_malformed(tmp_path):
    v = Verifier(tmp_path)
    result = CommandResult(
        command="python -m pytest tests/test_x.py",
        exit_code=5,  # no tests collected
        stdout_path="", stderr_path="", summary="no tests ran",
    )
    assert v._command_is_malformed(result) is True


def test_missing_launcher_still_malformed(tmp_path):
    v = Verifier(tmp_path)
    result = CommandResult(
        command="pytst tests/", exit_code=127, stdout_path="", stderr_path="",
        summary="command not found: pytst",
    )
    assert v._command_is_malformed(result) is True


# --- mcp: diff target extraction covers rename sources and spaced paths ---

def test_diff_target_paths_includes_rename_source():
    diff = (
        "diff --git a/src/old.py b/src/new.py\n"
        "similarity index 100%\n"
        "rename from src/old.py\n"
        "rename to src/new.py\n"
    )
    targets = _diff_target_paths(diff)
    assert "src/old.py" in targets  # the moved-out source must be policy-checked
    assert "src/new.py" in targets


def test_diff_target_paths_handles_spaces():
    diff = (
        "diff --git a/src/my file.py b/src/my file.py\n"
        "--- a/src/my file.py\n"
        "+++ b/src/my file.py\n"
        "@@ -1 +1 @@\n-x\n+y\n"
    )
    assert _diff_target_paths(diff) == ["src/my file.py"]


# --- policy: leading-plus refspec is a force push ---

def test_plus_refspec_is_force_push(tmp_path):
    engine = TaskPolicyEngine(tmp_path)
    assert engine.evaluate_hook_command("git push origin +HEAD:master").action == "deny"
    assert engine.evaluate_hook_command("git push origin feature").action != "deny"


# --- repo-mapper: import resolution edges ---

def _make_pkg(tmp_path):
    import subprocess
    files = {
        "src/pkg/__init__.py": "",
        "src/pkg/feature.py": "from . import helpers\nimport json\n",  # submodule + stdlib
        "src/pkg/helpers.py": "def h():\n    return 1\n",
        "src/pkg/json.py": "X = 1\n",  # decoy whose stem collides with stdlib json
    }
    for rel, content in files.items():
        p = tmp_path / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content, encoding="utf-8")
    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    subprocess.run(["git", "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"], cwd=tmp_path, capture_output=True)
    subprocess.run(["git", "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "i"], cwd=tmp_path, capture_output=True)


def test_submodule_import_edge_and_no_stdlib_false_edge(tmp_path):
    _make_pkg(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    deps = repo_map.dependents
    # `from . import helpers` must produce feature -> helpers (previously dropped).
    assert "src/pkg/feature.py" in deps.get("src/pkg/helpers.py", [])
    # `import json` (stdlib) must NOT create a false edge to the decoy src/pkg/json.py.
    assert "src/pkg/feature.py" not in deps.get("src/pkg/json.py", [])
