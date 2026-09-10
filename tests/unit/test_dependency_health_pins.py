"""Pins that keep this repository's own dependency scanners honest.

The 2026-09-10 health scan reported three independent defects, not one:

* GitPython 3.1.53 was a declared runtime dependency with 18 open Dependabot
  advisories, the newest of them RCE via git-config / clone hooks. No source
  file imported the ``git`` package. Git I/O goes through
  ``devcouncil.utils.proc`` (subprocess ``git`` with a timeout). Shipping an
  unused library that answers those advisories is how a scanner comes to call
  a tool we do not run a critical finding. Removing it is the floor; a version
  bump would have kept the surface.
* ``backend/go_orchestrator/go.mod`` named Go 1.26.4. govulncheck's stdlib
  findings (encoding/xml, net/http, crypto/tls, os, …) are fixed in 1.26.6,
  and CI installs whatever ``go-version-file`` reads. A comment in a STATUS
  file is not a pin.
* Root ``package.json`` is the npm wrapper for the Python CLI and has no npm
  dependencies, so ``npm audit`` failed with ENOLOCK. The testdata
  ``package.json`` under ``rust-port/testdata/capabilities/`` is a language
  probe, not a project: ``language_capabilities.rs`` extracts every file in
  that directory, and a lockfile there would be ingested as a probe.

Each assertion fails against the pre-fix tree (except the probe-corpus guard,
which is the reason we did not "fix" that warning by adding a lockfile).
"""

from __future__ import annotations

import json
import re
import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]

# The GitPython distribution imports as ``git``. ``from git import …`` /
# ``import git`` are the only forms that load it. ``import git_handlers`` and
# ``from … import git_output`` are this repository's own names.
_GITPYTHON_IMPORT = re.compile(
    r"^(?:from git(?:\.| import )|import git(?:$|\s|,))",
    re.MULTILINE,
)


def _direct_dependency_names() -> list[str]:
    data = tomllib.loads((REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    names: list[str] = []
    for spec in data["project"]["dependencies"]:
        name = re.split(r"[<>=!~;\[]", spec, maxsplit=1)[0].strip().lower()
        names.append(name)
    return names


def _locked_package_names() -> set[str]:
    lock = tomllib.loads((REPO_ROOT / "uv.lock").read_text(encoding="utf-8"))
    return {pkg["name"] for pkg in lock.get("package", [])}


def test_gitpython_is_not_a_declared_runtime_dependency() -> None:
    """An unused GitPython pin is how 18 Dependabot RCEs stay open."""
    names = _direct_dependency_names()
    assert "gitpython" not in names, (
        "GitPython is not imported anywhere under src/; git I/O is subprocess "
        f"via devcouncil.utils.proc. Declaring it reopens the advisory surface: {names}"
    )


def test_gitpython_is_not_in_the_lockfile() -> None:
    """A stale uv.lock that still pins GitPython is the same open advisory."""
    names = _locked_package_names()
    assert "gitpython" not in names, names


def test_no_source_imports_the_gitpython_package() -> None:
    """The removal is only honest if nothing loads the ``git`` distribution."""
    offenders: list[str] = []
    for path in (REPO_ROOT / "src").rglob("*.py"):
        text = path.read_text(encoding="utf-8")
        if _GITPYTHON_IMPORT.search(text):
            offenders.append(str(path.relative_to(REPO_ROOT)))
    assert not offenders, (
        "src/ imports GitPython; restore the dependency rather than leaving "
        f"an unwired import: {offenders}"
    )


def test_go_toolchain_is_at_least_the_stdlib_security_floor() -> None:
    """CI's setup-go reads this line; 1.26.4 is the govulncheck finding."""
    text = (REPO_ROOT / "backend" / "go_orchestrator" / "go.mod").read_text(encoding="utf-8")
    match = re.search(r"^go (\d+)\.(\d+)(?:\.(\d+))?", text, re.MULTILINE)
    assert match, f"go.mod has no go directive:\n{text}"
    version = (int(match.group(1)), int(match.group(2)), int(match.group(3) or 0))
    assert version >= (1, 26, 6), (
        f"go.mod pins {match.group(0)}; govulncheck's stdlib advisories "
        f"(GO-2026-6088 and siblings) need >= 1.26.6: {text!r}"
    )


def test_root_npm_package_has_a_lockfile_so_audit_can_run() -> None:
    """npm audit requires a shrinkwrap; this package has no npm dependencies."""
    pkg = json.loads((REPO_ROOT / "package.json").read_text(encoding="utf-8"))
    assert pkg["name"] == "devcouncil"
    lockfile = REPO_ROOT / "package-lock.json"
    assert lockfile.is_file(), (
        "npm audit failed with ENOLOCK because package.json has no sibling lockfile"
    )
    lock = json.loads(lockfile.read_text(encoding="utf-8"))
    assert lock.get("name") == "devcouncil", lock


def test_capability_probe_package_json_is_not_an_npm_project() -> None:
    """A lockfile in the probe corpus would be extracted as a probe file.

    ``language_capabilities.rs`` walks every file in
    ``rust-port/testdata/capabilities/``. The ``package.json`` there is a JSON
    language probe named like an npm manifest on purpose. Adding
    ``package-lock.json`` to silence a scanner would change what the extractor
    sees.
    """
    probe = REPO_ROOT / "rust-port" / "testdata" / "capabilities" / "package.json"
    assert probe.is_file()
    assert json.loads(probe.read_text(encoding="utf-8"))["name"] == "probe-config"
    assert not (probe.parent / "package-lock.json").exists(), (
        "testdata/capabilities/package.json is a language probe, not a project; "
        "do not add a lockfile beside it"
    )
