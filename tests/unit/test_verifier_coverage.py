import json
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

from devcouncil.domain.evidence import CommandResult
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.verification import diff_coverage as dc
from devcouncil.verification.verifier import (
    MAX_UNTRACKED_DIFF_BYTES,
    VerificationOutcome,
    Verifier,
)


def _task(**updates):
    data = {
        "id": "TASK-1",
        "title": "Remove old API",
        "description": "remove old_name and keep checks",
        "requirement_ids": ["REQ-1"],
        "acceptance_criterion_ids": ["AC-1"],
        "planned_files": [PlannedFile(path="src/app.py", reason="scope", allowed_change="modify")],
    }
    data.update(updates)
    return Task(**data)


def _requirement():
    return Requirement(
        id="REQ-1",
        title="req",
        description="desc",
        priority="high",
        source="user",
        acceptance_criteria=[
            AcceptanceCriterion(
                id="AC-1",
                description="old_name is intentionally removed",
                verification_method="unit_test",
            )
        ],
    )


def _cmd_result(**updates):
    data = {
        "command": "pytest",
        "exit_code": 1,
        "stdout_path": "",
        "stderr_path": "",
        "summary": "",
    }
    data.update(updates)
    return CommandResult(**data)


def test_outcome_and_git_public_methods_handle_failures(tmp_path, monkeypatch):
    verifier = Verifier(tmp_path)
    assert VerificationOutcome(mode="compiled", compiler_active=True).as_dict()["mode"] == "compiled"

    verifier._has_head = lambda: True

    def boom(*args, **kwargs):
        raise subprocess.CalledProcessError(1, args[0])

    monkeypatch.setattr(subprocess, "check_output", boom)

    assert verifier.get_diff() == ""
    assert verifier.get_changed_files() == []


def test_initial_status_untracked_and_snapshot_helpers(tmp_path, monkeypatch):
    verifier = Verifier(tmp_path)
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "app.py").write_text("print('x')\n", encoding="utf-8")
    (tmp_path / "__pycache__").mkdir()
    (tmp_path / "__pycache__" / "ignored.pyc").write_text("x", encoding="utf-8")

    calls = []

    def fake_run(cmd, **kwargs):
        calls.append(cmd)
        return SimpleNamespace(returncode=0, stdout="cached.py\n" if "--cached" in cmd else "work.py\n")

    monkeypatch.setattr(subprocess, "run", fake_run)
    verifier._get_untracked_files_diff = lambda: "diff --git a/new.py b/new.py\n+new"

    assert "cached.py" in verifier._get_initial_repo_diff()
    assert "diff --git" in verifier._get_initial_repo_diff()

    def fail_check_output(*args, **kwargs):
        raise subprocess.CalledProcessError(1, args[0])

    monkeypatch.setattr(subprocess, "check_output", fail_check_output)
    status_files = verifier._get_status_files()

    assert "src/app.py" in status_files
    assert all("__pycache__" not in path for path in status_files)

    snapshot = tmp_path / ".devcouncil" / "baseline.json"
    snapshot.parent.mkdir()
    snapshot.write_text(json.dumps({"changed_files": ["src\\app.py", 7, ""]}), encoding="utf-8")
    assert verifier._load_baseline_files() == {"src/app.py", ""}
    snapshot.write_text("{not json", encoding="utf-8")
    assert verifier._load_baseline_files() == set()


def test_task_committed_and_after_patch_footprints(tmp_path, monkeypatch):
    verifier = Verifier(tmp_path)

    def fake_run(cmd, **kwargs):
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(subprocess, "run", fake_run)
    monkeypatch.setattr(subprocess, "check_output", lambda *a, **k: b"diff --git a/x b/x\n+x")
    assert verifier._committed_task_diff("TASK-1").startswith("diff --git")
    assert verifier._task_produced_changes("TASK-1") is True

    monkeypatch.setattr(subprocess, "run", lambda *a, **k: SimpleNamespace(returncode=1))
    patch_path = tmp_path / ".devcouncil" / "checkpoints" / "TASK-2-after.patch"
    patch_path.parent.mkdir(parents=True)
    patch_path.write_text("\n+work\n", encoding="utf-8")
    assert verifier._task_produced_changes("TASK-2") is True
    assert verifier._task_produced_changes("TASK-3") is False


def test_untracked_diff_formats_text_binary_empty_and_truncated_files(tmp_path):
    verifier = Verifier(tmp_path)
    text = tmp_path / "note.txt"
    text.write_text("one\ntwo", encoding="utf-8")
    binary = tmp_path / "blob.bin"
    binary.write_bytes(b"abc\0def")
    empty = tmp_path / "empty.txt"
    empty.write_text("", encoding="utf-8")
    huge = tmp_path / "huge.txt"
    huge.write_bytes(b"a" * (MAX_UNTRACKED_DIFF_BYTES + 10))

    text_diff = verifier._format_new_file_diff("note.txt", text)
    assert "@@ -0,0 +1,2 @@" in text_diff
    assert "+two" in text_diff
    assert "Binary files /dev/null and b/blob.bin differ" in verifier._format_new_file_diff("blob.bin", binary)
    assert verifier._format_new_file_diff("empty.txt", empty).endswith("+++ b/empty.txt\n")
    assert "diff truncated" in verifier._format_new_file_diff("huge.txt", huge)

    verifier._get_untracked_files = lambda: ["note.txt", "missing.txt", "blob.bin"]
    combined = verifier._get_untracked_files_diff()
    assert "note.txt" in combined
    assert "blob.bin" in combined
    assert "missing.txt" not in combined


def test_command_environment_strips_current_virtualenv(tmp_path, monkeypatch):
    verifier = Verifier(tmp_path)
    venv = tmp_path / "venv"
    base = tmp_path / "base"
    venv_bin = venv / "bin"
    venv_bin.mkdir(parents=True)
    other = tmp_path / "bin"
    other.mkdir()

    monkeypatch.setattr(sys, "prefix", str(venv))
    monkeypatch.setattr(sys, "base_prefix", str(base), raising=False)
    monkeypatch.setattr(
        "devcouncil.verification.verifier.os.environ",
        {
            "PATH": f"{venv_bin}{__import__('os').pathsep}{other}",
            "VIRTUAL_ENV": str(venv),
            "PYTHONHOME": str(base),
            "UV_INTERNAL__PYTHONHOME": str(base),
        },
    )

    env = verifier._verification_env()

    assert str(venv_bin) not in env["PATH"]
    assert str(other) in env["PATH"]
    assert "VIRTUAL_ENV" not in env
    assert "PYTHONHOME" not in env
    assert "UV_INTERNAL__PYTHONHOME" not in env


def test_run_command_success_and_exception_paths(tmp_path, monkeypatch):
    verifier = Verifier(tmp_path)
    verifier._command_timeout_cache = 7
    verifier._verification_env = lambda: {"PATH": "/bin"}
    verifier._split_command = lambda command: ["tool", "--flag"]
    saved = []
    verifier._save_log = lambda label, command, stream, content: saved.append((label, stream, content)) or str(tmp_path / f"{stream}.log")
    monkeypatch.setattr("devcouncil.verification.verifier.shutil.which", lambda name, path=None: "/usr/bin/tool")

    def fake_run(argv, **kwargs):
        assert argv[0] == "/usr/bin/tool"
        assert kwargs["timeout"] == 7
        return SimpleNamespace(returncode=2, stdout="ok\n", stderr="Traceback\nError: bad\n")

    monkeypatch.setattr(subprocess, "run", fake_run)
    result = verifier._run_command("tool --flag", task_id="T")

    assert result.exit_code == 2
    assert "stderr: Error: bad" in result.summary
    assert ("T", "stdout", "ok\n") in saved

    monkeypatch.setattr(subprocess, "run", lambda *a, **k: (_ for _ in ()).throw(OSError("nope")))
    failed = verifier._run_command("tool --flag")
    assert failed.exit_code == -1
    assert "Failed to run command: nope" in failed.summary


def test_command_and_diff_coverage_helpers(tmp_path, monkeypatch):
    verifier = Verifier(tmp_path)
    verifier._untracked_cache = ["new.py"]
    monkeypatch.setattr(
        subprocess,
        "check_output",
        lambda *a, **k: b"A\tadded.py\nD\tgone.py\nM\tkept.py\nbadline\n",
    )
    assert verifier._classify_change_paths(["new.py", "added.py", "gone.py", "kept.py"]) == (
        ["added.py", "new.py"],
        ["gone.py"],
    )

    verifier._diff_coverage_override = (False, True, 0.75)
    assert verifier._diff_coverage_settings() == (False, True, 0.75)
    verifier._diff_coverage_override = None
    monkeypatch.setattr("devcouncil.verification.verifier.load_config", lambda root: (_ for _ in ()).throw(RuntimeError("bad")))
    assert verifier._diff_coverage_settings() == (True, False, 0.0)

    monkeypatch.setattr("devcouncil.verification.verifier.shutil.which", lambda name, path=None: f"/bin/{name}" if name == "python3" else None)
    assert verifier._resolve_coverage_python({"PATH": "/bin"}) == "/bin/python3"
    verifier._coverage_python = "/custom/python"
    assert verifier._resolve_coverage_python({}) == "/custom/python"

    monkeypatch.setattr(subprocess, "run", lambda *a, **k: SimpleNamespace(returncode=0))
    assert verifier._coverage_available("/bin/python", {}) is True
    monkeypatch.setattr(subprocess, "run", lambda *a, **k: (_ for _ in ()).throw(OSError("missing")))
    assert verifier._coverage_available("/bin/python", {}) is False

    assert verifier._coverage_target_commands(_task(expected_tests=["pytest tests/a.py"])) == ["pytest tests/a.py"]
    assert verifier._coverage_target_commands(_task(allowed_commands=["ruff check", "echo no"])) == ["ruff check"]
    verifier._load_commands = lambda: {"test": ["pytest"]}
    assert verifier._coverage_target_commands(_task()) == ["pytest"]

    changed = {"src/app.py": {1, 2}}
    monkeypatch.setattr(dc, "parse_changed_lines", lambda diff: changed)
    monkeypatch.setattr(dc, "measurable_python_changes", lambda parsed: {})
    skipped = verifier.measure_diff_coverage(_task(), "diff")
    assert skipped.measured is False
    assert "no measurable Python changes" in skipped.reason


def test_semantic_diff_classifications_cover_blocking_and_advisory_paths(tmp_path, monkeypatch):
    semantic_dir = tmp_path / ".devcouncil" / "semantic" / "TASK-1"
    semantic_dir.mkdir(parents=True)
    (semantic_dir / "after.json").write_text("{}", encoding="utf-8")
    verifier = Verifier(tmp_path)

    classifications = [
        {"type": "exported_symbol_added", "path": "src/renamed.py", "name": "moved_name"},
        {"type": "exported_symbol_removed", "path": "src/app.py", "name": "moved_name"},
        {"type": "exported_symbol_removed", "path": "src/app.py", "name": "old_name"},
        {"type": "exported_symbol_removed", "path": "src/app.py", "name": "surprise"},
        {"type": "public_api_change", "path": "src/other.py", "name": "api"},
        {"type": "public_api_change", "path": "src/app.py", "name": "planned_api"},
        {"type": "import_dependency_change", "path": "src/app.py", "statement": "import newpkg"},
        {"type": "import_dependency_change", "path": "src/other.py", "statement": "import os"},
        {"type": "config_schema_dependency_change", "path": "schema.json"},
    ]

    class FakeSemanticIndex:
        def __init__(self, root):
            self.root = root

        def diff(self, task_id):
            return {"classifications": classifications}

    monkeypatch.setattr("devcouncil.indexing.semantic_index.SemanticIndex", FakeSemanticIndex)
    monkeypatch.setattr(verifier, "_is_new_third_party_import", lambda top: top == "newpkg")

    gaps = verifier._check_semantic_diff(_task(), [_requirement()])
    by_text = "\n".join(g.description for g in gaps)

    assert "surprise" in by_text
    assert any(g.gap_type == "dependency_risk" and g.blocking and g.file == "src/app.py" for g in gaps)
    assert any(g.evidence == ["src/other.py"] and not g.blocking for g in gaps)
    assert any(g.description.startswith("Config/schema") and g.blocking for g in gaps)
    assert any("planned file src/app.py" in g.description and not g.blocking for g in gaps)
    assert any("moved_name" in g.description and not g.blocking for g in gaps)
    assert any("old_name" in g.description and not g.blocking for g in gaps)


def test_import_dependency_detection_and_project_dependency_parsing(tmp_path, monkeypatch):
    verifier = Verifier(tmp_path)
    (tmp_path / "pyproject.toml").write_text(
        '[project]\ndependencies=["Requests>=2", "rich[all]"]\n[project.optional-dependencies]\ndev=["PyTest"]\n',
        encoding="utf-8",
    )
    (tmp_path / "requirements.txt").write_text("# c\nFlask==3\n", encoding="utf-8")
    (tmp_path / "package.json").write_text(
        json.dumps({"dependencies": {"React": "latest"}, "devDependencies": {"Vite": "latest"}}),
        encoding="utf-8",
    )

    assert Verifier._import_top_level("import requests as r") == "requests"
    assert Verifier._import_top_level("import os.path, sys") == "os"
    assert Verifier._import_top_level("from package.sub import name") == "package"
    assert Verifier._import_top_level("from .local import x") is None
    assert Verifier._import_top_level("nonsense") is None

    deps = verifier._project_dependencies()
    assert {"requests", "rich", "pytest", "flask", "react", "vite"} <= deps
    (tmp_path / "requirements.txt").write_text("changed\n", encoding="utf-8")
    assert verifier._project_dependencies() is deps

    monkeypatch.setattr(Verifier, "_stdlib_modules", staticmethod(lambda: frozenset({"sys"})))
    monkeypatch.setattr(verifier, "_project_dependencies", lambda: {"requests"})
    assert verifier._is_new_third_party_import(None) is False
    assert verifier._is_new_third_party_import("sys") is False
    assert verifier._is_new_third_party_import("requests") is False
    monkeypatch.setattr("importlib.util.find_spec", lambda name: SimpleNamespace() if name == "installed" else None)
    assert verifier._is_new_third_party_import("installed") is False
    assert verifier._is_new_third_party_import("missingpkg") is True
    monkeypatch.setattr("importlib.util.find_spec", lambda name: (_ for _ in ()).throw(RuntimeError("ambiguous")))
    assert verifier._is_new_third_party_import("ambiguous") is False


def test_malformed_launcher_and_failure_location_helpers(tmp_path):
    verifier = Verifier(tmp_path)
    stdout = tmp_path / "stdout.log"
    stderr = tmp_path / "stderr.log"
    stdout.write_text(
        f'Traceback\n  File "{tmp_path / "tests" / "test_app.py"}", line 12, in test_x\n'
        f'  File "{tmp_path / "src" / "app.py"}", line 5, in f\nImportError: real failure\n',
        encoding="utf-8",
    )
    stderr.write_text("warning only\n", encoding="utf-8")
    result = _cmd_result(stdout_path=str(stdout), stderr_path=str(stderr), summary="ignored")

    assert verifier._launcher_text(result).startswith("warning only")
    assert verifier._malformed_signature_precedes_traceback("ModuleNotFoundError: No module named pytest") is True
    assert verifier._malformed_signature_precedes_traceback(stdout.read_text(encoding="utf-8")) is False
    assert verifier._command_is_malformed(result) is False
    assert verifier._command_is_malformed(_cmd_result(command="pytest", exit_code=5)) is True
    assert verifier._failure_location(result) == ("src/app.py", 5)
    assert verifier._relativize(str(tmp_path / "src" / "app.py")) == "src/app.py"
    assert verifier._relativize("C:\\repo\\file.py") == "C:/repo/file.py"


def test_command_selection_quality_stack_and_requirement_mapping(tmp_path, monkeypatch):
    verifier = Verifier(tmp_path)
    assert verifier._commands_for_task(_task(expected_tests=["pytest tests/x.py"])) == {"test": ["pytest tests/x.py"]}
    assert verifier._commands_for_task(_task(allowed_commands=["python -m unittest"])) == {"allowed": ["python -m unittest"]}
    verifier._load_commands = lambda: {"test": ["pytest"]}
    assert verifier._commands_for_task(_task()) == {"test": ["pytest"]}

    assert verifier._is_quality_only_command("python -m mypy src") is True
    assert verifier._is_quality_only_command("uv run ruff check") is True
    assert verifier._is_quality_only_command("npm run lint") is True
    assert verifier._is_quality_only_command("python -c 'assert True'") is False
    assert verifier._command_can_prove_acceptance("test", "echo nope") is True
    assert verifier._command_can_prove_acceptance("allowed", "python -m pytest") is True
    assert verifier._command_can_prove_acceptance("allowed", "echo nope") is False
    assert verifier._requirement_id_for_ac([_requirement()], "AC-1") == "REQ-1"
    assert verifier._requirement_id_for_ac([_requirement()], "AC-X") is None

    monkeypatch.setattr("devcouncil.repo.ci_scaffold.detect_stacks", lambda root: {"python"})
    monkeypatch.setattr("devcouncil.repo.ci_scaffold._command_stack", lambda cmd: "node")
    applicable, reason = verifier._command_applicable("npm test")
    assert applicable is False
    assert "node" in reason

    monkeypatch.setattr("devcouncil.repo.ci_scaffold.detect_stacks", lambda root: (_ for _ in ()).throw(RuntimeError("x")))
    assert verifier._command_applicable("npm test") == (True, "")
