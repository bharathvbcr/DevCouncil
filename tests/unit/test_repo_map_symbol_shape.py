"""`devcouncil_repo_map` must answer with the same shape whether or not a path was asked about.

The payload carries `symbols: []` unconditionally, but the fields that say what
that empty list *means* — `symbols_available`, `symbols_reason`, `symbols_total`
— were only attached on the branch where a `path` argument was supplied. So a
machine consumer reading `payload["symbols_available"]`, the field that exists
precisely to separate "this path has no symbols" from "I could not list them",
raised `KeyError` on the no-path branch instead of getting an answer.

`[]` with no discriminator is the same conflation this codebase fails closed on
everywhere else; the fix is to always publish the discriminator, with `None`
meaning "not determined" rather than absent.
"""

from __future__ import annotations

import asyncio
import json
from pathlib import Path

import pytest

from devcouncil.integrations.mcp.handlers.map import handle_repo_map

#: Every field that describes the symbol listing. Present on both branches or
#: neither — a consumer must not have to test for the key's existence.
SYMBOL_FIELDS = (
    "symbols",
    "symbols_available",
    "symbols_shown",
    "symbols_total",
    "symbols_truncated",
)


def _payload(root: Path, **args) -> dict:
    return json.loads(asyncio.run(handle_repo_map(root, args))[0].text)


@pytest.fixture
def project(tmp_path: Path) -> Path:
    """A project with a real `repo_map.json`, so the handler answers rather than
    failing closed on a missing map (which it correctly does, and which would
    make these shape assertions vacuous)."""
    root = tmp_path / "proj"
    (root / ".devcouncil").mkdir(parents=True)
    (root / "pkg").mkdir()
    (root / "pkg" / "mod.py").write_text("def widget():\n    return 1\n", encoding="utf-8")
    (root / ".devcouncil" / "repo_map.json").write_text(
        json.dumps({
            "languages": ["python"],
            "frameworks": [],
            "package_managers": ["uv"],
            "files": [{"path": "pkg/mod.py", "area": "pkg", "kind": "code", "summary": "widget"}],
            "subsystems": [{
                "area": "pkg",
                "summary": "Package",
                "entry_points": ["pkg/mod.py"],
                "critical_files": ["pkg/mod.py"],
                "neighbors": [],
                "handoff_paths": [],
                "role_files": {},
            }],
            "dependents": {},
            "entry_roots": ["pkg/mod.py"],
        }),
        encoding="utf-8",
    )
    return root


def test_the_no_path_branch_publishes_the_same_symbol_fields(project):
    without = _payload(project)
    missing = [field for field in SYMBOL_FIELDS if field not in without]
    assert not missing, (
        f"asking without a path omits {missing}; a consumer reading them to tell "
        "'no symbols' from 'could not list' gets a KeyError instead of an answer"
    )


def test_not_asking_is_reported_as_undetermined_not_as_a_negative_finding(project):
    without = _payload(project)
    assert without["symbols"] == []
    assert without["symbols_available"] is None, (
        "not asking about a path came back as a determined answer; None means "
        "'not determined', which is what actually happened"
    )
    assert without["symbols_total"] is None
    assert "symbols_reason" in without and without["symbols_reason"]


def test_both_branches_agree_on_the_field_set(project):
    without = _payload(project)
    with_path = _payload(project, path="pkg/mod.py")
    for field in SYMBOL_FIELDS:
        assert field in with_path, f"the path branch lost {field}"
        assert field in without, f"the no-path branch lost {field}"
    assert with_path["symbols_available"] in (True, False), (
        "asking about a real path must produce a determined answer"
    )
