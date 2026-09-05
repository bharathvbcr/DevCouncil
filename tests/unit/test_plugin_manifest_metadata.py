"""Plugins-1.0 conformance for the generated Claude Code plugin bundle.

Two kinds of check live here:

* **Publication metadata** — the plugin manifest must carry the fields an installed
  plugin needs to identify itself (``homepage``, ``repository``, ``license``), and those
  values must stay in sync with ``pyproject.toml`` / ``LICENSE`` so the copies in
  ``claude_assets`` cannot silently drift.
* **The real validator** — ``claude plugin validate --strict`` run against an actually
  built bundle. The previous pass asserted a *proxy* for this (``manifest["description"]``
  is truthy) and recorded "both manifests pass strict" in a docstring; the proxy passed
  while the shipped bundle still failed, because nothing ever ran the tool. A check that
  cannot fail the way the real thing fails is not a check, so this module runs the tool.
"""

from __future__ import annotations

import json
import re
import shutil
import subprocess
import tomllib
from pathlib import Path

import pytest

from devcouncil.integrations import claude_assets

_REPO_ROOT = Path(__file__).resolve().parents[2]
_PLUGIN_REL = Path(".devcouncil") / "claude-plugin"


def _pyproject_urls() -> dict[str, str]:
    data = tomllib.loads((_REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    return data["project"]["urls"]


def test_plugin_manifest_declares_publication_metadata():
    """plugin.json must carry homepage, repository and license.

    All three are recognized ``plugin.json`` fields — verified empirically against
    ``claude plugin validate --strict --json`` (Claude Code 2.1.259), which emits
    ``Unknown field '<x>'. Claude Code ignores it at load time.`` for anything it does not
    know, and emitted no such warning for these. The bundle is a *published, installable*
    artifact: without them an installed plugin cannot tell the user its licence terms or
    where the source lives.
    """
    manifest = json.loads(claude_assets._plugin_json("1.2.3"))

    for field in ("homepage", "repository", "license"):
        value = manifest.get(field)
        assert isinstance(value, str) and value.strip(), (
            f"plugin.json is missing {field!r}; a published plugin manifest should declare "
            f"it (recognized by `claude plugin validate --strict`): {manifest!r}"
        )


def test_plugin_manifest_metadata_matches_pyproject_and_license():
    """The manifest's URLs/licence must not drift from the project's own metadata."""
    manifest = json.loads(claude_assets._plugin_json("1.2.3"))
    urls = _pyproject_urls()

    assert manifest["homepage"] == urls["Homepage"], (
        "plugin.json homepage drifted from pyproject.toml [project.urls] Homepage"
    )
    assert manifest["repository"] == urls["Repository"], (
        "plugin.json repository drifted from pyproject.toml [project.urls] Repository"
    )

    licence_text = (_REPO_ROOT / "LICENSE").read_text(encoding="utf-8")
    assert "Apache License" in licence_text and "Version 2.0" in licence_text, (
        "LICENSE is no longer Apache 2.0 — update the plugin manifest's license field"
    )
    assert manifest["license"] == "Apache-2.0", (
        f"plugin.json license {manifest['license']!r} does not match the repo's LICENSE"
    )


# --- the real validator ----------------------------------------------------------

def _validate(target: Path) -> dict:
    """Run `claude plugin validate <target> --strict --json` and return the report."""
    proc = subprocess.run(
        ["claude", "plugin", "validate", str(target), "--strict", "--json"],
        capture_output=True,
        text=True,
        timeout=120,
    )
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError:  # pragma: no cover - only if the CLI's --json contract changes
        pytest.fail(
            f"`claude plugin validate` produced non-JSON output (exit {proc.returncode}):\n"
            f"stdout={proc.stdout!r}\nstderr={proc.stderr!r}"
        )


def _findings(report: dict) -> str:
    manifest = report.get("manifest") or {}
    lines = [f"  {kind}: {item.get('path')}: {item.get('message')}"
             for kind in ("errors", "warnings")
             for item in manifest.get(kind) or []]
    return "\n".join(lines) or "  (no findings reported)"


@pytest.mark.skipif(shutil.which("claude") is None, reason="Claude Code CLI not installed")
def test_built_bundle_passes_real_strict_plugin_validation(tmp_path):
    """A really-built bundle must pass the real `claude plugin validate --strict`.

    ``--strict`` is what a publishing pipeline runs: it promotes warnings (unrecognized
    fields, missing metadata) to a non-zero exit. Both the marketplace manifest and the
    plugin manifest it points at are checked, because validating the marketplace descends
    into the referenced ``plugin.json`` but validating the plugin root does not descend
    into components.
    """
    for asset in claude_assets.build_plugin_bundle(tmp_path, version="1.2.3", skill_assets=[]):
        asset.write_if_changed()

    market_root = tmp_path / _PLUGIN_REL
    plugin_root = market_root / "devcouncil"

    for target in (market_root, plugin_root):
        report = _validate(target)
        assert report.get("success") is True, (
            f"`claude plugin validate {target.name} --strict` failed:\n{_findings(report)}"
        )


_COMMAND_NAMES = (
    "status", "next", "verify", "repair", "plan", "review", "report", "map", "wiki", "supervise",
)


@pytest.mark.skipif(shutil.which("claude") is None, reason="Claude Code CLI not installed")
def test_built_bundle_command_inventory_loads(tmp_path):
    """Claude Code must actually *load* every bundled slash command.

    `claude plugin validate --strict` cannot gate this: the marketplace docs state the
    validator does not open a plugin's command files, and it passed identically while the
    bundle emitted `commands/devcouncil/*.md`, which the loader discovers as nothing. So
    this asserts against the loader's own component inventory (`claude plugin details`,
    Claude Code 2.1.259) instead of the manifest — the check the previous layout would
    have failed and `--strict` did not.
    """
    for asset in claude_assets.build_plugin_bundle(tmp_path, version="1.2.3", skill_assets=[]):
        asset.write_if_changed()
    plugin_root = tmp_path / _PLUGIN_REL / "devcouncil"

    proc = subprocess.run(
        ["claude", "--plugin-dir", str(plugin_root), "plugin", "details", "devcouncil"],
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert proc.returncode == 0, f"`claude plugin details` failed:\n{proc.stdout}\n{proc.stderr}"

    # Parse the inventory line rather than substring-matching the whole report: "plan" and
    # "review" both occur elsewhere in it ("planning", "devcouncil-reviewer"), so a naive
    # `in proc.stdout` would pass for two commands that never loaded.
    match = re.search(r"^\s*Skills \((\d+)\)\s*(.*)$", proc.stdout, re.MULTILINE)
    assert match, f"`claude plugin details` reported no component inventory:\n{proc.stdout}"
    loaded = {name.strip() for name in match.group(2).split(",") if name.strip()}

    missing = sorted(set(_COMMAND_NAMES) - loaded)
    assert not missing, (
        f"Claude Code loaded {match.group(1)} command(s) and is missing {missing}. "
        f"Inventory reported:\n{proc.stdout}"
    )
