"""Unwired-file verification gate tests."""

from __future__ import annotations

import subprocess

from devcouncil.domain.task import PlannedFile, Task
from devcouncil.verification.checks.wiring import (
    added_files_from_diff,
    detect_unwired_file_gaps,
)
from devcouncil.verification.difficulty import resolve_rigor_policy


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


def _task(
    *,
    title: str = "t",
    description: str = "d",
    planned_files: list | None = None,
    difficulty: str = "hard",
) -> Task:
    return Task(
        id="TASK-1",
        title=title,
        description=description,
        planned_files=list(planned_files or []),
        difficulty=difficulty,  # type: ignore[arg-type]
    )


def _gap_id(task_id: str, kind: str) -> str:
    return f"{task_id}-{kind}-1"


def _run(tmp_path, *, added_paths, task=None, changed=None, diff=None):
    task = task or _task(
        planned_files=[
            PlannedFile(path=p, reason="new", allowed_change="create") for p in added_paths
        ]
    )
    changed = changed or list(added_paths)
    diff = diff or ""
    return detect_unwired_file_gaps(
        task=task,
        project_root=tmp_path,
        changed_files=changed,
        diff_content=diff,
        get_untracked_files=lambda: list(added_paths),
        next_gap_id=_gap_id,
        unwired_enabled=True,
        unwired_blocking=True,
        classify_fn=lambda files: (list(added_paths), []),
    )


def test_never_imported_module_flagged(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/existing.py": "x = 1\n",
        "pkg/orphan.py": "def lonely():\n    return 1\n",
    })
    _commit(tmp_path)
    gaps = _run(tmp_path, added_paths=["pkg/orphan.py"])
    assert any(g.gap_type == "unwired_file" and g.file == "pkg/orphan.py" for g in gaps)


def test_clears_via_preexisting_importer(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/existing.py": "from pkg.orphan import lonely\n",
        "pkg/orphan.py": "def lonely():\n    return 1\n",
    })
    _commit(tmp_path)
    gaps = _run(tmp_path, added_paths=["pkg/orphan.py"])
    assert not any(g.file == "pkg/orphan.py" and g.blocking for g in gaps)


def test_clears_via_init_reexport(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "from .orphan import lonely\n",
        "pkg/orphan.py": "def lonely():\n    return 1\n",
    })
    _commit(tmp_path)
    gaps = _run(tmp_path, added_paths=["pkg/orphan.py"])
    assert not any(g.file == "pkg/orphan.py" and g.blocking for g in gaps)


def test_new_file_cycle_still_flagged(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/a.py": "from pkg.b import b\ndef a():\n    return b()\n",
        "pkg/b.py": "from pkg.a import a\ndef b():\n    return a()\n",
    })
    _commit(tmp_path)
    gaps = _run(tmp_path, added_paths=["pkg/a.py", "pkg/b.py"])
    flagged = {g.file for g in gaps if g.gap_type == "unwired_file" and g.blocking}
    assert "pkg/a.py" in flagged
    assert "pkg/b.py" in flagged


def test_test_only_importer_still_flagged(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/orphan.py": "def lonely():\n    return 1\n",
        "tests/test_orphan.py": "from pkg.orphan import lonely\n",
    })
    _commit(tmp_path)
    gaps = _run(tmp_path, added_paths=["pkg/orphan.py"])
    assert any(
        g.file == "pkg/orphan.py" and "test" in g.description.lower()
        for g in gaps
    )


def test_readme_mention_does_not_clear(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/orphan.py": "def lonely():\n    return 1\n",
        "README.md": "See pkg/orphan.py for details.\n",
    })
    _commit(tmp_path)
    gaps = _run(tmp_path, added_paths=["pkg/orphan.py"])
    assert any(g.file == "pkg/orphan.py" and g.blocking for g in gaps)


def test_main_basename_does_not_exempt(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/main.py": "def run():\n    return 1\n",
    })
    _commit(tmp_path)
    gaps = _run(tmp_path, added_paths=["pkg/main.py"])
    assert any(g.file == "pkg/main.py" for g in gaps)


def test_pyproject_scripts_entry_clears(tmp_path):
    _write(tmp_path, {
        "pyproject.toml": (
            "[project]\nname = \"x\"\nversion = \"0\"\n"
            "[project.scripts]\nmycli = \"pkg.cli:main\"\n"
        ),
        "pkg/__init__.py": "",
        "pkg/cli.py": "def main():\n    print('hi')\n",
    })
    _commit(tmp_path)
    gaps = _run(tmp_path, added_paths=["pkg/cli.py"])
    assert not any(g.file == "pkg/cli.py" and g.blocking for g in gaps)


def test_tsconfig_alias_import_clears(tmp_path):
    _write(tmp_path, {
        "tsconfig.json": (
            '{"compilerOptions": {"baseUrl": ".", "paths": {"@/*": ["src/*"]}}}\n'
        ),
        "package.json": '{"name": "app"}\n',
        "src/orphan.ts": "export function lonely() { return 1; }\n",
        "src/app.ts": "import { lonely } from '@/orphan';\n",
    })
    _commit(tmp_path)
    gaps = _run(tmp_path, added_paths=["src/orphan.ts"])
    assert not any(g.file == "src/orphan.ts" and g.blocking for g in gaps)


def test_route_file_exempt(tmp_path):
    _write(tmp_path, {
        "package.json": '{"name": "app"}\n',
        "app/dashboard/page.tsx": "export default function Page() { return null; }\n",
    })
    _commit(tmp_path)
    gaps = _run(tmp_path, added_paths=["app/dashboard/page.tsx"])
    assert not any(g.file == "app/dashboard/page.tsx" for g in gaps)


def test_go_file_in_existing_package_not_flagged(tmp_path):
    _write(tmp_path, {
        "go.mod": "module example.com/app\n\ngo 1.21\n",
        "pkg/a.go": "package pkg\nfunc A() {}\n",
        "pkg/b.go": "package pkg\nfunc B() {}\n",
    })
    _commit(tmp_path)
    gaps = _run(tmp_path, added_paths=["pkg/b.go"])
    assert not any(g.file == "pkg/b.go" for g in gaps)


def test_allow_unwired_parity(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/orphan.py": "# devcouncil: allow-unwired\ndef lonely():\n    return 1\n",
    })
    _commit(tmp_path)
    # Marker always clears blocking; still emits an advisory declaration gap.
    gaps = _run(tmp_path, added_paths=["pkg/orphan.py"])
    assert any(g.gap_type == "unwired_file" for g in gaps)
    assert all(not g.blocking for g in gaps if g.file == "pkg/orphan.py")

    task = _task(
        description="scaffolding for later wiring",
        planned_files=[PlannedFile(path="pkg/orphan.py", reason="new", allowed_change="create")],
    )
    gaps2 = detect_unwired_file_gaps(
        task=task,
        project_root=tmp_path,
        changed_files=["pkg/orphan.py"],
        diff_content="",
        get_untracked_files=lambda: ["pkg/orphan.py"],
        next_gap_id=_gap_id,
        unwired_enabled=True,
        unwired_blocking=True,
        classify_fn=lambda files: (["pkg/orphan.py"], []),
    )
    assert all(not g.blocking for g in gaps2)
    assert any(g.gap_type == "unwired_file" for g in gaps2)


def test_rigor_blocking_modes():
    task = _task(difficulty="easy")
    policy = resolve_rigor_policy(task)
    assert policy.unwired_enabled is True
    assert policy.unwired_blocking is False
    hard = resolve_rigor_policy(_task(difficulty="hard"))
    assert hard.unwired_blocking is True


def test_committed_diff_fallback_parses_added():
    diff = (
        "diff --git a/pkg/new.py b/pkg/new.py\n"
        "new file mode 100644\n"
        "--- /dev/null\n"
        "+++ b/pkg/new.py\n"
        "@@ -0,0 +1,2 @@\n"
        "+def x():\n"
        "+    return 1\n"
    )
    assert "pkg/new.py" in added_files_from_diff(diff)
