"""The suite is the same size whichever pytest entry point you type.

`pytest tests` collected 4,178 and `python -m pytest tests` collected 4,462,
from the same tree. Eighteen modules under ``tests/unit/`` import their helpers
as ``tests.unit.support_maps`` / ``tests.unit.graph_fixtures``, and ``tests/``
has no ``__init__.py``, so ``tests`` is importable only when the repository root
is on ``sys.path``. ``python -m pytest`` puts the working directory there
itself; bare ``pytest`` does not.

pytest reports the difference as a *collection error*, not a failure. A run with
``--continue-on-collection-errors`` would have reported the other 4,178 green
with 284 tests silently absent — and three of the four CI workflows invoke
pytest bare (``npm-publish.yml``, ``codeintel-platform.yml``,
``devcouncil-evidence.yml``), while ``ci.yml`` uses ``coverage run -m pytest``
and so collected a different set from all three.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]

# One module from each helper family. Both were among the eighteen.
PROBES = (
    "tests/unit/test_parse_cache.py",  # imports tests.unit.support_maps
    "tests/unit/test_graph_html.py",  # imports tests.unit.graph_fixtures
)


def _collect(argv: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        argv,
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        timeout=300,
        check=False,
    )


def _count(output: str) -> int:
    """Tests collected, read from pytest's own summary line."""
    for line in reversed(output.splitlines()):
        if "test" in line and "collected" in line:
            for token in line.split():
                if token.isdigit():
                    return int(token)
    raise AssertionError(f"no collection count in pytest output:\n{output}")


def test_both_pytest_entry_points_collect_the_same_tests() -> None:
    """Bare ``pytest`` and ``python -m pytest`` must agree, module for module.

    Asserted on a couple of modules rather than the whole tree so the test costs
    two short subprocesses instead of two full collections; the failure mode is
    per-module (an unresolved import fails that module's collection), so a
    module that imports each helper family is the whole population that matters.
    """
    bare = _collect([sys.executable.replace("python", "pytest"), *PROBES, "--collect-only", "-q"])
    dashm = _collect([sys.executable, "-m", "pytest", *PROBES, "--collect-only", "-q"])

    assert bare.returncode == 0, (
        "bare `pytest` failed to collect — the repository root is not on sys.path, "
        "so `from tests.unit... import` raises ModuleNotFoundError. Add `.` to "
        f"`pythonpath` in pyproject.toml.\n{bare.stdout}\n{bare.stderr}"
    )
    assert dashm.returncode == 0, f"{dashm.stdout}\n{dashm.stderr}"
    assert _count(bare.stdout) == _count(dashm.stdout), (
        "the two entry points collected different numbers of tests from the same "
        f"files:\nbare:\n{bare.stdout}\n-m:\n{dashm.stdout}"
    )
    assert _count(bare.stdout) > 0, "the probe collected nothing, so it proves nothing"


def test_the_repository_root_is_importable_under_pytest() -> None:
    """The mechanism, asserted directly.

    The test above spends two subprocesses; this one is the invariant they
    exist to protect, and it costs nothing.
    """
    import tests.unit.support_maps  # noqa: F401
    import tests.unit.graph_fixtures  # noqa: F401
