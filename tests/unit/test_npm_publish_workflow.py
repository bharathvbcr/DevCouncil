"""F-11: tag → npm publish → registry smoke → GitHub Release wiring."""

from __future__ import annotations

import json
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github" / "workflows" / "npm-publish.yml"
PACKAGE_JSON = ROOT / "package.json"
JUDGE_NOTES = ROOT / "docs" / "judge-release-notes.md"
REGISTRY_SMOKE = ROOT / "scripts" / "npm-registry-smoke.mjs"
README = ROOT / "README.md"


def _jobs() -> dict:
    doc = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
    assert isinstance(doc, dict)
    jobs = doc.get("jobs") or {}
    assert isinstance(jobs, dict)
    return jobs


def test_npm_publish_has_release_job_after_publish():
    jobs = _jobs()
    assert "publish" in jobs
    assert "release" in jobs
    release = jobs["release"]
    assert release.get("needs") == "publish"
    assert "startsWith(github.ref, 'refs/tags/v')" in str(release.get("if", ""))


def test_registry_smoke_runs_before_github_release():
    jobs = _jobs()
    steps = jobs["release"]["steps"]
    names = [s.get("name", "") for s in steps]
    smoke_idx = next(i for i, n in enumerate(names) if "registry smoke" in n.lower())
    release_idx = next(i for i, n in enumerate(names) if "github release" in n.lower())
    assert smoke_idx < release_idx
    smoke = steps[smoke_idx]
    assert "npm run smoke:registry" in str(smoke.get("run", ""))
    body = steps[release_idx].get("run", "")
    assert "docs/judge-release-notes.md" in body
    assert "gh release create" in body


def test_package_json_wires_registry_smoke_script():
    pkg = json.loads(PACKAGE_JSON.read_text(encoding="utf-8"))
    assert pkg["scripts"]["smoke:registry"] == "node scripts/npm-registry-smoke.mjs"
    assert REGISTRY_SMOKE.is_file()
    text = REGISTRY_SMOKE.read_text(encoding="utf-8")
    assert "NPM_REGISTRY_SMOKE_VERSION" in text
    assert "devcouncil --help" in text or "devcouncil" in text


def test_judge_release_notes_exist():
    assert JUDGE_NOTES.is_file()
    text = JUDGE_NOTES.read_text(encoding="utf-8")
    assert "registry smoke" in text.lower()
    assert "release-health" in text.lower()


def test_readme_has_ci_and_npm_badges():
    text = README.read_text(encoding="utf-8")
    assert "actions/workflows/ci.yml/badge.svg" in text
    assert "img.shields.io/npm/v/devcouncil" in text
