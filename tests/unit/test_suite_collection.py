"""The suite must be the same size however pytest was invoked.

Eighteen modules under ``tests/unit`` import their helpers by package path —
``from tests.unit.support_maps import ...`` — and ``tests/`` has no
``__init__.py``, so ``tests`` resolves only with the repository root on
``sys.path``.

``python -m pytest`` and ``uv run pytest`` put the working directory there
themselves, so they collected 4462 tests.  The bare ``pytest`` console script
does not, and aborted with ``ModuleNotFoundError: No module named 'tests'`` on
those eighteen — 4178 collected, 18 errors.  Same tree, same command-line
arguments, different suite, decided by which of the two forms you typed.  It is
a collection *error* rather than a failure, so a run with
``--continue-on-collection-errors`` reported the other 4178 green.

``pythonpath = ["src", "."]`` in ``pyproject.toml`` is what makes the three
agree.  These two tests are what stops it being deleted as redundant.
"""

from __future__ import annotations

import os
import subprocess
import sys
import tomllib
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]

# One of the eighteen. Named explicitly rather than collected dynamically: the
# point is that *this* import shape works, and a probe that found no such
# module would pass while proving nothing.
PACKAGE_IMPORTING_MODULE = Path("tests/unit/test_graph_query_tools.py")


def test_pythonpath_puts_the_repository_root_on_sys_path() -> None:
    """The setting itself, so its removal fails here and not in eighteen places."""
    config = tomllib.loads((REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    pythonpath = config["tool"]["pytest"]["ini_options"]["pythonpath"]
    assert "." in pythonpath, (
        "tests/unit imports helpers as `tests.unit.<name>`; without the "
        f"repository root on sys.path those modules cannot be collected: {pythonpath}"
    )
    assert (
        not (REPO_ROOT / "tests" / "__init__.py").exists()
    ), "tests/ became a package: drop the `.` above and this test, or keep both deliberately"


def test_the_bare_console_script_collects_the_package_importing_modules() -> None:
    """The property the setting exists for, proven by running it."""
    console_script = Path(sys.executable).parent / "pytest"
    if not console_script.exists():  # pragma: no cover - environment dependent
        pytest.skip(f"no pytest console script beside {sys.executable}")

    target = REPO_ROOT / PACKAGE_IMPORTING_MODULE
    assert target.exists(), f"fixture module moved: {PACKAGE_IMPORTING_MODULE}"

    # PYTHONPATH stripped on purpose. Inheriting it would let the parent's
    # sys.path answer the question the child is being asked, which is exactly
    # the confusion this test exists to remove.
    env = {k: v for k, v in os.environ.items() if k != "PYTHONPATH"}

    result = subprocess.run(  # noqa: S603 - fixed argv, no shell
        [
            str(console_script),
            "--collect-only",
            "-q",
            "-p",
            "no:cacheprovider",
            str(PACKAGE_IMPORTING_MODULE),
        ],
        cwd=REPO_ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=180,
    )
    assert result.returncode == 0, (
        "bare `pytest` could not collect a module that imports its helpers by "
        f"package path:\n{result.stdout[-3000:]}\n{result.stderr[-2000:]}"
    )
    assert "No module named 'tests'" not in result.stdout + result.stderr
