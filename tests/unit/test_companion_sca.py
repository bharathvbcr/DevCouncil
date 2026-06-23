"""ITEM B: offline-safe SCA scanner, opt-in repo-map field, prompt-builder
dependency-risk segment, and stack-gated CI audit steps."""

from __future__ import annotations

import json

from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution.prompt_builder import PromptBuilder
from devcouncil.repo import ci_scaffold
from devcouncil.repo.sca import (
    AuditorResult,
    DependencyRisk,
    ScaScanner,
    scan_dependency_risks,
)


def _runner_returning(stdout: str, returncode: int = 1):
    def run(argv, project_root):
        return AuditorResult(returncode=returncode, stdout=stdout, stderr="")

    return run


# ---------------------------------------------------------------------------
# Parsing mocked auditor output
# ---------------------------------------------------------------------------


def test_pip_audit_output_parses_into_dependency_risks(tmp_path):
    (tmp_path / "requirements.txt").write_text("requests==2.0.0\n", encoding="utf-8")
    payload = json.dumps(
        {
            "dependencies": [
                {
                    "name": "requests",
                    "version": "2.0.0",
                    "vulns": [
                        {
                            "id": "PYSEC-2023-1",
                            "severity": "high",
                            "description": "A bad bug",
                        }
                    ],
                },
                {"name": "clean", "version": "1.0.0", "vulns": []},
            ]
        }
    )
    scanner = ScaScanner(tmp_path, auditor_runner=_runner_returning(payload))

    risks = scanner.scan()

    assert len(risks) == 1
    risk = risks[0]
    assert isinstance(risk, DependencyRisk)
    assert risk.package == "requests"
    assert risk.installed_version == "2.0.0"
    assert risk.severity == "high"
    assert risk.advisory_id == "PYSEC-2023-1"
    assert "bad bug" in risk.summary


def test_npm_audit_output_parses_into_dependency_risks(tmp_path):
    (tmp_path / "package-lock.json").write_text("{}", encoding="utf-8")
    payload = json.dumps(
        {
            "vulnerabilities": {
                "lodash": {
                    "severity": "critical",
                    "range": "<4.17.21",
                    "via": [
                        {"source": 1065, "title": "Prototype Pollution", "url": "x"}
                    ],
                }
            }
        }
    )
    scanner = ScaScanner(tmp_path, auditor_runner=_runner_returning(payload))

    risks = scanner.scan()

    assert len(risks) == 1
    assert risks[0].package == "lodash"
    assert risks[0].severity == "critical"
    assert risks[0].advisory_id == "1065"
    assert "Prototype Pollution" in risks[0].summary


def test_osv_scanner_output_parses_into_dependency_risks(tmp_path):
    (tmp_path / "requirements.txt").write_text("flask==1.0\n", encoding="utf-8")
    payload = json.dumps(
        {
            "results": [
                {
                    "packages": [
                        {
                            "package": {"name": "flask", "version": "1.0"},
                            "vulnerabilities": [
                                {
                                    "id": "GHSA-xxxx",
                                    "summary": "Flask issue",
                                    "severity": [{"type": "CVSS_V3", "score": "9.8"}],
                                }
                            ],
                        }
                    ]
                }
            ]
        }
    )
    # Force only osv-scanner to run by injecting a runner; pip-audit also matches
    # requirements.txt, so route output via argv inspection.
    def run(argv, project_root):
        if argv and argv[0] == "osv-scanner":
            return AuditorResult(returncode=1, stdout=payload, stderr="")
        return AuditorResult(returncode=0, stdout="", stderr="")

    scanner = ScaScanner(tmp_path, auditor_runner=run)
    risks = scanner.scan()

    osv_hits = [r for r in risks if r.advisory_id == "GHSA-xxxx"]
    assert osv_hits
    assert osv_hits[0].package == "flask"
    assert osv_hits[0].severity == "CVSS_V3"


# ---------------------------------------------------------------------------
# Never-raise / offline safety
# ---------------------------------------------------------------------------


def test_scan_returns_empty_when_no_auditor_installed(tmp_path):
    (tmp_path / "requirements.txt").write_text("requests==2.0.0\n", encoding="utf-8")
    # No injected runner + which() that finds nothing simulates a clean offline box.
    scanner = ScaScanner(tmp_path, which=lambda name: None)

    assert scanner.available_auditors() == []
    assert scanner.scan() == []


def test_scan_never_raises_on_garbage_output(tmp_path):
    (tmp_path / "requirements.txt").write_text("requests==2.0.0\n", encoding="utf-8")
    scanner = ScaScanner(tmp_path, auditor_runner=_runner_returning("not json at all"))

    assert scanner.scan() == []


def test_scan_never_raises_on_timeout_sentinel(tmp_path):
    (tmp_path / "requirements.txt").write_text("x\n", encoding="utf-8")
    scanner = ScaScanner(
        tmp_path, auditor_runner=lambda a, r: AuditorResult(-1, "", "timed out")
    )

    assert scanner.scan() == []


def test_available_auditors_requires_a_lockfile(tmp_path):
    # No lockfiles at all -> nothing is relevant even with an injected runner.
    scanner = ScaScanner(tmp_path, auditor_runner=_runner_returning("{}"))
    assert scanner.available_auditors() == []


def test_convenience_helper_returns_dicts(tmp_path):
    (tmp_path / "requirements.txt").write_text("requests==2.0.0\n", encoding="utf-8")
    payload = json.dumps(
        {"dependencies": [{"name": "requests", "version": "2.0.0",
                            "vulns": [{"id": "X", "severity": "low", "description": "d"}]}]}
    )
    out = scan_dependency_risks(tmp_path, auditor_runner=_runner_returning(payload))
    assert out == [
        {
            "package": "requests",
            "installed_version": "2.0.0",
            "severity": "low",
            "advisory_id": "X",
            "summary": "d",
        }
    ]


# ---------------------------------------------------------------------------
# repo_mapper opt-in (default off)
# ---------------------------------------------------------------------------


def test_repo_map_dependency_risks_default_off(tmp_path, monkeypatch):
    from devcouncil.indexing.repo_mapper import RepoMapper

    mapper = RepoMapper(tmp_path)

    called = {"n": 0}

    def boom(self):  # pragma: no cover - must never be invoked by default
        called["n"] += 1
        return [{"package": "x", "installed_version": "1", "severity": "high",
                 "advisory_id": "A", "summary": "s"}]

    monkeypatch.setattr(RepoMapper, "_scan_dependency_risks", boom, raising=True)

    repo_map = mapper.map_repo()  # default: scan_dependencies=False

    assert repo_map.dependency_risks == []
    assert called["n"] == 0


def test_repo_map_dependency_risks_opt_in(tmp_path, monkeypatch):
    from devcouncil.indexing.repo_mapper import RepoMapper

    mapper = RepoMapper(tmp_path)
    monkeypatch.setattr(
        RepoMapper,
        "_scan_dependency_risks",
        lambda self: [{"package": "x", "installed_version": "1", "severity": "high",
                       "advisory_id": "A", "summary": "s"}],
        raising=True,
    )

    repo_map = mapper.map_repo(scan_dependencies=True)

    assert repo_map.dependency_risks and repo_map.dependency_risks[0]["package"] == "x"


# ---------------------------------------------------------------------------
# PromptBuilder dependency-risk segment
# ---------------------------------------------------------------------------


def _write_repo_map(tmp_path, risks):
    map_dir = tmp_path / ".devcouncil"
    map_dir.mkdir(parents=True, exist_ok=True)
    (map_dir / "repo_map.json").write_text(
        json.dumps({"dependency_risks": risks}), encoding="utf-8"
    )


def _task():
    return Task(
        id="T1",
        title="Bump deps",
        description="Update dependencies",
        planned_files=[PlannedFile(path="app.py", reason="edit", allowed_change="modify")],
    )


def test_prompt_builder_emits_dependency_risks_segment(tmp_path):
    (tmp_path / "app.py").write_text("print('hi')\n", encoding="utf-8")
    _write_repo_map(
        tmp_path,
        [{"package": "requests", "installed_version": "2.0.0", "severity": "high",
          "advisory_id": "PYSEC-1", "summary": "remote code execution"}],
    )
    builder = PromptBuilder(tmp_path)

    prompt = builder.build_task_prompt(_task(), [])

    assert "Dependency risks (known vulnerabilities)" in prompt
    assert "requests" in prompt
    assert "PYSEC-1" in prompt


def test_prompt_builder_drops_dependency_risks_under_budget(tmp_path):
    (tmp_path / "app.py").write_text("print('hi')\n", encoding="utf-8")
    _write_repo_map(
        tmp_path,
        [{"package": "requests", "installed_version": "2.0.0", "severity": "high",
          "advisory_id": "PYSEC-1", "summary": "remote code execution"}],
    )
    builder = PromptBuilder(tmp_path)
    task = _task()

    # Sanity: with an ample budget the segment is present.
    assert "Dependency risks (known vulnerabilities)" in builder.build_task_prompt(
        task, [], max_chars=60_000
    )

    # A budget too small for any optional segment forces the lowest-priority
    # dependency-risk segment to be dropped — and named in the omitted marker.
    tight = builder.build_task_prompt(task, [], max_chars=1)

    assert "Dependency risks (known vulnerabilities)" not in tight
    assert "dependency risks" in tight  # named in the omitted-segments marker


# ---------------------------------------------------------------------------
# ci_scaffold stack-gated audit steps
# ---------------------------------------------------------------------------


def _init(tmp_path):
    from devcouncil.cli.commands.init import initialize_project

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)


def test_ci_audit_python_only_for_python_stack(tmp_path):
    (tmp_path / "pyproject.toml").write_text("[project]\nname='x'\n", encoding="utf-8")
    _init(tmp_path)
    workflow = ci_scaffold.render_workflow(tmp_path)

    assert "pip-audit" in workflow
    assert "npm audit" not in workflow


def test_ci_audit_node_only_for_node_stack(tmp_path):
    (tmp_path / "package.json").write_text("{}", encoding="utf-8")
    _init(tmp_path)
    workflow = ci_scaffold.render_workflow(tmp_path)

    assert "npm audit" in workflow
    assert "pip-audit" not in workflow


def test_ci_audit_absent_when_no_stack_detected(tmp_path):
    # No stack markers at all -> no audit step for any stack.
    _init(tmp_path)
    workflow = ci_scaffold.render_workflow(tmp_path)

    assert "pip-audit" not in workflow
    assert "npm audit" not in workflow
