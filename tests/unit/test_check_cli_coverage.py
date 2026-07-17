from __future__ import annotations

import json
import subprocess
from pathlib import Path
from types import SimpleNamespace

from rich.console import Console
from typer.testing import CliRunner

from devcouncil.cli.commands import check as check_cmd
from devcouncil.cli.main import app
from devcouncil.domain.evidence import DiffCoverageEvidence
from devcouncil.domain.gap import Gap
from devcouncil.verification.ad_hoc_check import AdHocCheckResult
from devcouncil.verification.next_actions import NextAction


runner = CliRunner()


def _gap(
    gap_id: str = "G1",
    gap_type: str = "security_risk",
    blocking: bool = True,
    severity: str = "high",
) -> Gap:
    return Gap(
        id=gap_id,
        severity=severity,
        gap_type=gap_type,
        description=f"{gap_type} description",
        recommended_fix="Fix it.",
        blocking=blocking,
        file="app.py",
        line=3,
    )


def test_diff_uses_verifier_without_base(monkeypatch, tmp_path: Path) -> None:
    class FakeVerifier:
        def __init__(self, root):
            self.root = root

        def get_diff(self):
            return "working diff"

    monkeypatch.setattr(check_cmd, "Verifier", FakeVerifier)

    assert check_cmd._diff(tmp_path, None) == "working diff"


def test_diff_with_base_success_and_failure(monkeypatch, tmp_path: Path) -> None:
    captured: dict[str, object] = {}

    def fake_check_output(command, cwd, text, encoding, errors):
        captured["command"] = command
        captured["cwd"] = cwd
        return "base diff"

    monkeypatch.setattr(check_cmd.subprocess, "check_output", fake_check_output)
    assert check_cmd._diff(tmp_path, "HEAD~1") == "base diff"
    assert captured["command"] == ["git", "diff", "HEAD~1", "--"]
    assert captured["cwd"] == tmp_path

    monkeypatch.setattr(
        check_cmd.subprocess,
        "check_output",
        lambda *args, **kwargs: (_ for _ in ()).throw(subprocess.CalledProcessError(1, "git")),
    )
    assert check_cmd._diff(tmp_path, "missing") == ""


def _patch_cli_common(
    monkeypatch,
    tmp_path: Path,
    *,
    diff: str = "diff --git a/app.py b/app.py\n",
    secret_gaps: list[Gap] | None = None,
):
    monkeypatch.setattr("devcouncil.telemetry.logging_setup.set_log_dir", lambda root: None)
    monkeypatch.setattr(check_cmd, "initialize_project", lambda root, quiet=True: None)
    monkeypatch.setattr(check_cmd, "_diff", lambda root, base: diff)

    class FakeSecretScanner:
        def __init__(self, gaps):
            self._gaps = gaps

        def scan_diff(self, diff_text, task_id):
            return list(self._gaps)

    class FakeVerifier:
        def __init__(self, root):
            self.root = root
            self.secret_scanner = FakeSecretScanner(
                [_gap("S1", "security_risk")] if secret_gaps is None else secret_gaps
            )

        def get_changed_files(self):
            return ["app.py", "tests/test_app.py"]

    monkeypatch.setattr(check_cmd, "Verifier", FakeVerifier)

    class FakeGraphAdapter:
        def __init__(self, root):
            self.root = root

        def get_context(self, changed_files):
            return SimpleNamespace(
                available=True,
                impacted_files=["service.py"],
                related_tests=["tests/test_service.py"],
            )

    monkeypatch.setattr(check_cmd, "CodeReviewGraphAdapter", FakeGraphAdapter)
    monkeypatch.setattr(
        check_cmd,
        "load_config",
        lambda root: SimpleNamespace(
            models=SimpleNamespace(provider="openrouter", roles={}),
            provider=SimpleNamespace(),
        ),
    )
    monkeypatch.setattr(check_cmd, "validate_model_provider", lambda provider: provider)
    monkeypatch.setattr(check_cmd, "get_api_key", lambda provider, root: "key")
    monkeypatch.setattr(check_cmd, "create_provider", lambda *args, **kwargs: object())
    monkeypatch.setattr(check_cmd, "ModelRouter", lambda *args, **kwargs: object())


def test_check_llm_audit_json_success_includes_findings_and_graph(monkeypatch, tmp_path: Path) -> None:
    _patch_cli_common(monkeypatch, tmp_path)

    class FakeReviewer:
        def __init__(self, router):
            self.router = router

        async def review_changes(self, task, requirements, diff):
            return SimpleNamespace(findings=[_gap("R1", "architecture_drift", blocking=False, severity="medium")])

    monkeypatch.setattr(check_cmd, "ImplementationReviewer", FakeReviewer)

    result = runner.invoke(app, ["check", "--json", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    data = json.loads(result.output[result.output.find("{") :])
    assert data["ok"] is False
    assert data["changed_files"] == ["app.py", "tests/test_app.py"]
    assert data["secret_findings"][0]["id"] == "S1"
    assert data["review_findings"][0]["id"] == "R1"
    assert data["blast_radius"]["impacted_files"] == ["service.py"]
    assert data["blast_radius"]["related_tests"] == ["tests/test_service.py"]


def test_check_llm_audit_human_output_renders_secrets_findings_graph_and_note(
    monkeypatch, tmp_path: Path
) -> None:
    _patch_cli_common(monkeypatch, tmp_path)

    class FakeReviewer:
        def __init__(self, router):
            self.router = router

        async def review_changes(self, task, requirements, diff):
            raise check_cmd.ProviderRequestError("provider down")

    monkeypatch.setattr(check_cmd, "ImplementationReviewer", FakeReviewer)

    result = runner.invoke(app, ["check", "--goal", "review this", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert "Changed files (2)" in result.output
    assert "Possible secrets" in result.output
    assert "Blast radius" in result.output
    assert "provider down" in result.output
    assert "Tip:" in result.output


def test_check_llm_audit_human_output_renders_review_findings(monkeypatch, tmp_path: Path) -> None:
    _patch_cli_common(monkeypatch, tmp_path, secret_gaps=[])

    class FakeReviewer:
        def __init__(self, router):
            self.router = router

        async def review_changes(self, task, requirements, diff):
            return SimpleNamespace(
                findings=[
                    _gap("R-high", "architecture_drift", blocking=True, severity="high"),
                    _gap("R-low", "assumption_violated", blocking=False, severity="low"),
                ]
            )

    monkeypatch.setattr(check_cmd, "ImplementationReviewer", FakeReviewer)

    result = runner.invoke(app, ["check", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert "Review findings (2)" in result.output
    assert "high" in result.output
    assert "low" in result.output


def test_check_llm_audit_human_output_renders_no_concerns(monkeypatch, tmp_path: Path) -> None:
    _patch_cli_common(monkeypatch, tmp_path, secret_gaps=[])

    class FakeGraphAdapter:
        def __init__(self, root):
            self.root = root

        def get_context(self, changed_files):
            return SimpleNamespace(available=False, impacted_files=[], related_tests=[])

    class FakeReviewer:
        def __init__(self, router):
            self.router = router

        async def review_changes(self, task, requirements, diff):
            return SimpleNamespace(findings=[])

    monkeypatch.setattr(check_cmd, "CodeReviewGraphAdapter", FakeGraphAdapter)
    monkeypatch.setattr(check_cmd, "ImplementationReviewer", FakeReviewer)

    result = runner.invoke(app, ["check", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert "No secrets or review concerns" in result.output


def test_check_llm_audit_clean_tree_json(monkeypatch, tmp_path: Path) -> None:
    _patch_cli_common(monkeypatch, tmp_path, diff="")

    result = runner.invoke(app, ["check", "--json", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    data = json.loads(result.output)
    assert data == {"ok": True, "message": "No changes to check (clean working tree)."}


def test_check_goal_reference_note_suppressed_in_json(monkeypatch, tmp_path: Path) -> None:
    captured: dict[str, object] = {}

    def fake_gate(root, requirement, test_commands, enforce_coverage, min_ratio):
        captured["requirement"] = requirement
        return AdHocCheckResult(requirement=requirement or "", passed=True, reason="no_changes")

    monkeypatch.setattr("devcouncil.telemetry.logging_setup.set_log_dir", lambda root: None)
    monkeypatch.setattr(check_cmd, "initialize_project", lambda root, quiet=True: None)
    monkeypatch.setattr(check_cmd, "resolve_goal_intent", lambda goal, root: ("expanded goal", "intent note"))
    monkeypatch.setattr(check_cmd, "run_working_tree_check", fake_gate)

    result = runner.invoke(app, ["check", "--goal", "#1", "--verify", "--json", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert "intent note" not in result.output
    assert captured["requirement"] == "expanded goal"


def test_check_goal_reference_note_printed_for_human_gate(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setattr("devcouncil.telemetry.logging_setup.set_log_dir", lambda root: None)
    monkeypatch.setattr(check_cmd, "initialize_project", lambda root, quiet=True: None)
    monkeypatch.setattr(check_cmd, "resolve_goal_intent", lambda goal, root: ("expanded goal", "intent note"))
    monkeypatch.setattr(
        check_cmd,
        "run_working_tree_check",
        lambda root, requirement, test_commands, enforce_coverage, min_ratio: AdHocCheckResult(
            requirement=requirement or "", passed=True, reason="no_changes"
        ),
    )

    result = runner.invoke(app, ["check", "--goal", "#1", "--verify", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert "intent note" in result.output
    assert "No working-tree changes" in result.output


def _render_to_text(monkeypatch, result: AdHocCheckResult) -> str:
    import io

    output = io.StringIO()
    monkeypatch.setattr(check_cmd, "console", Console(file=output, force_terminal=False, width=200))
    check_cmd._render_gate(result)
    return output.getvalue()


def test_render_gate_no_changes(monkeypatch) -> None:
    text = _render_to_text(
        monkeypatch,
        AdHocCheckResult(requirement="", reason="no_changes", passed=True),
    )

    assert "No working-tree changes" in text


def test_render_gate_verified_with_diff_coverage(monkeypatch) -> None:
    text = _render_to_text(
        monkeypatch,
        AdHocCheckResult(
            requirement="feature works",
            changed_files=["app.py"],
            diff_coverage=DiffCoverageEvidence(
                task_id="CHECK",
                measured=True,
                summary="1/1 changed lines covered",
            ),
            gaps=[],
            passed=True,
        ),
    )

    assert "Checking:" in text
    assert "Diff coverage: 1/1 changed lines covered" in text
    assert "Verified: the change is backed" in text


def test_render_gate_blocking_and_nonblocking_findings_with_next_actions(monkeypatch) -> None:
    text = _render_to_text(
        monkeypatch,
        AdHocCheckResult(
            requirement="feature works",
            changed_files=["app.py"],
            gaps=[_gap("G1", "diff_not_exercised", True), _gap("G2", "architecture_drift", False, "low")],
            next_actions=[
                NextAction(
                    gap_id="G1",
                    gap_type="diff_not_exercised",
                    category="add_test",
                    severity="high",
                    blocking=True,
                    action="Add a test",
                    file="app.py",
                    line=3,
                ),
                NextAction(
                    gap_id="G2",
                    gap_type="architecture_drift",
                    category="review",
                    severity="low",
                    blocking=True,
                    action="Review the change",
                    file="service.py",
                )
            ],
            passed=False,
        ),
    )

    assert "Findings" in text
    assert "diff_not_exercised" in text
    assert "Next actions" in text
    assert "app.py:3" in text
    assert "Not verified" in text


def test_render_gate_passed_with_nonblocking_signals(monkeypatch) -> None:
    text = _render_to_text(
        monkeypatch,
        AdHocCheckResult(
            requirement="feature works",
            changed_files=["app.py"],
            gaps=[_gap("G2", "architecture_drift", False, "low")],
            passed=True,
        ),
    )

    assert "Verified with non-blocking signals only" in text
