"""Fixture-driven regression tests for the claim mapper."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from devcouncil.verification.claims.mapper import map_claims

FIXTURE_DIR = Path(__file__).parent.parent / "fixtures" / "claims"


def _load_cases(pattern: str):
    cases = []
    for path in sorted(FIXTURE_DIR.glob(pattern)):
        for i, line in enumerate(path.read_text(encoding="utf-8").splitlines()):
            line = line.strip()
            if not line:
                continue
            case = json.loads(line)
            case_id = f"{path.stem}:{i}"
            cases.append(pytest.param(case, id=case_id))
    return cases


@pytest.mark.parametrize("case", _load_cases("seed.jsonl"))
def test_seed_fixture(case):
    actual = {(a.kind.value, a.target) for a in map_claims(case["text"])}
    expected = {(e["kind"], e.get("target")) for e in case["expected"]}
    assert actual == expected, f"claim text: {case['text']!r}"


@pytest.mark.parametrize("case", _load_cases("redteam-01.jsonl"))
def test_redteam_fixture(case):
    actual = {(a.kind.value, a.target) for a in map_claims(case["text"])}
    expected = {(e["kind"], e.get("target")) for e in case["expected"]}
    assert actual == expected, f"claim text: {case['text']!r}"
