"""Map/verify dead-symbol semantic parity tests."""

from __future__ import annotations

import subprocess

from devcouncil.domain.task import PlannedFile, Task
from devcouncil.indexing.repo_mapper import RepoMapper
from devcouncil.verification.checks.dead_symbols import detect_dead_symbol_gaps
from devcouncil.verification.checks.wiring import detect_unwired_file_gaps


def _git(root, *args):
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True)


def _commit(root):
    _git(root, "init")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init")


def _write(tmp_path, files):
    for rel, content in files.items():
        p = tmp_path / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content, encoding="utf-8")


def _diff_for(path: str, body: str) -> str:
    lines = body.splitlines()
    hunk = "\n".join(f"+{ln}" for ln in lines)
    return (
        f"diff --git a/{path} b/{path}\n"
        f"--- a/{path}\n"
        f"+++ b/{path}\n"
        f"@@ -0,0 +1,{len(lines)} @@\n"
        f"{hunk}\n"
    )


def test_map_and_verify_agree_test_reference_clears(tmp_path):
    body = "def helper():\n    return 1\n"
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/mod.py": body,
        "tests/test_mod.py": (
            "from pkg.mod import helper\ndef test_it():\n    assert helper() == 1\n"
        ),
    })
    _commit(tmp_path)

    repo_map = RepoMapper(tmp_path).map_repo()
    map_has = any("helper" in c for c in repo_map.dead_symbol_candidates)

    gaps = detect_dead_symbol_gaps(
        task=Task(id="TASK-1", title="t", description="d", planned_files=[]),
        project_root=tmp_path,
        diff_content=_diff_for("pkg/mod.py", body),
        next_gap_id=lambda t, k: f"{t}-{k}-1",
        dead_symbol_blocking=True,
    )
    verify_has = any(
        g.gap_type == "dead_symbol" and "helper" in g.description and g.blocking
        for g in gaps
    )
    assert map_has is False
    assert verify_has is False


def test_map_and_verify_agree_wiring_decorator_exempts(tmp_path):
    body = (
        "import typer\napp = typer.Typer()\n"
        "@app.command()\ndef handle():\n    return 1\n"
    )
    _write(tmp_path, {"pkg/__init__.py": "", "pkg/cli.py": body})
    _commit(tmp_path)

    repo_map = RepoMapper(tmp_path).map_repo()
    map_has = any("handle" in c for c in repo_map.dead_symbol_candidates)

    gaps = detect_dead_symbol_gaps(
        task=Task(id="TASK-1", title="t", description="d", planned_files=[]),
        project_root=tmp_path,
        diff_content=_diff_for("pkg/cli.py", body),
        next_gap_id=lambda t, k: f"{t}-{k}-1",
        dead_symbol_blocking=True,
    )
    verify_has = any("handle" in (g.description or "") for g in gaps if g.gap_type == "dead_symbol")
    assert map_has is False
    assert verify_has is False


def test_map_and_verify_agree_same_file_use_clears(tmp_path):
    body = (
        "class ModelsConfig:\n    name: str = 'x'\n\n"
        "class AppConfig:\n    models: ModelsConfig = ModelsConfig()\n"
    )
    _write(tmp_path, {"pkg/__init__.py": "", "pkg/config.py": body})
    _commit(tmp_path)

    repo_map = RepoMapper(tmp_path).map_repo()
    map_has = any("ModelsConfig" in c for c in repo_map.dead_symbol_candidates)

    gaps = detect_dead_symbol_gaps(
        task=Task(id="TASK-1", title="t", description="d", planned_files=[]),
        project_root=tmp_path,
        diff_content=_diff_for("pkg/config.py", body),
        next_gap_id=lambda t, k: f"{t}-{k}-1",
        dead_symbol_blocking=True,
    )
    verify_has = any(
        "ModelsConfig" in (g.description or "") and g.blocking
        for g in gaps if g.gap_type == "dead_symbol"
    )
    assert map_has is False
    assert verify_has is False


def test_map_and_verify_agree_dynamic_import_clears(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/loader.py": (
            "import importlib\n"
            "def load():\n"
            "    return importlib.import_module('pkg.plugin')\n"
        ),
        "pkg/plugin.py": "def hook():\n    return 1\n",
    })
    _commit(tmp_path)

    repo_map = RepoMapper(tmp_path).map_repo()
    map_unwired = "pkg/plugin.py" in repo_map.unwired_candidates

    gaps = detect_unwired_file_gaps(
        task=Task(
            id="TASK-1", title="t", description="d",
            planned_files=[PlannedFile(path="pkg/plugin.py", reason="new", allowed_change="create")],
        ),
        project_root=tmp_path,
        changed_files=["pkg/plugin.py"],
        diff_content="",
        get_untracked_files=lambda: ["pkg/plugin.py"],
        next_gap_id=lambda t, k: f"{t}-{k}-1",
        unwired_enabled=True,
        unwired_blocking=True,
        classify_fn=lambda files: (["pkg/plugin.py"], []),
    )
    verify_unwired = any(
        g.file == "pkg/plugin.py" and g.blocking for g in gaps if g.gap_type == "unwired_file"
    )
    assert map_unwired is False
    assert verify_unwired is False


def test_map_and_verify_agree_allow_unwired_suppresses_blocking(tmp_path):
    body = "# devcouncil: allow-unwired\ndef lonely():\n    return 1\n"
    _write(tmp_path, {"pkg/__init__.py": "", "pkg/orphan.py": body})
    _commit(tmp_path)

    repo_map = RepoMapper(tmp_path).map_repo()
    assert "pkg/orphan.py" not in repo_map.unwired_candidates

    gaps = detect_unwired_file_gaps(
        task=Task(
            id="TASK-1", title="t", description="scaffolding for later",
            planned_files=[PlannedFile(path="pkg/orphan.py", reason="new", allowed_change="create")],
        ),
        project_root=tmp_path,
        changed_files=["pkg/orphan.py"],
        diff_content="",
        get_untracked_files=lambda: ["pkg/orphan.py"],
        next_gap_id=lambda t, k: f"{t}-{k}-1",
        unwired_enabled=True,
        unwired_blocking=True,
        classify_fn=lambda files: (["pkg/orphan.py"], []),
    )
    assert all(not g.blocking for g in gaps if g.file == "pkg/orphan.py")


def test_map_and_verify_agree_test_dynamic_import_does_not_clear(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/plugin.py": "def hook():\n    return 1\n",
        "tests/test_plugin.py": (
            "import importlib\n"
            "def test_it():\n"
            "    importlib.import_module('pkg.plugin')\n"
        ),
    })
    _commit(tmp_path)

    repo_map = RepoMapper(tmp_path).map_repo()
    map_unwired = "pkg/plugin.py" in repo_map.unwired_candidates

    gaps = detect_unwired_file_gaps(
        task=Task(
            id="TASK-1", title="t", description="d",
            planned_files=[PlannedFile(path="pkg/plugin.py", reason="new", allowed_change="create")],
        ),
        project_root=tmp_path,
        changed_files=["pkg/plugin.py"],
        diff_content="",
        get_untracked_files=lambda: ["pkg/plugin.py"],
        next_gap_id=lambda t, k: f"{t}-{k}-1",
        unwired_enabled=True,
        unwired_blocking=True,
        classify_fn=lambda files: (["pkg/plugin.py"], []),
    )
    verify_unwired = any(
        g.file == "pkg/plugin.py" and g.blocking for g in gaps if g.gap_type == "unwired_file"
    )
    assert map_unwired is True
    assert verify_unwired is True
