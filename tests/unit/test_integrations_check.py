import yaml

from devcouncil.integrations.check import (
    IntegrationCheckReport,
    IntegrationCheckRow,
    build_integration_check_report,
    integration_status_summary,
    probe_coding_cli_version,
    recommended_executor_status,
)
from devcouncil.executors.agent_registry import resolve_coding_cli_probe_order


def test_probe_coding_cli_version_missing(tmp_path, monkeypatch):
    monkeypatch.setattr("devcouncil.integrations.check.shutil.which", lambda _cmd: None)
    ok, details = probe_coding_cli_version("codex")
    assert ok is False
    assert "Codex" in details


def test_recommended_executor_status_without_cli(tmp_path, monkeypatch):
    monkeypatch.setattr("devcouncil.integrations.check.shutil.which", lambda _cmd: None)
    ok, details = recommended_executor_status(tmp_path)
    assert ok is False
    assert "recommend" in details


def test_resolve_coding_cli_probe_order_uses_config(tmp_path):
    config_dir = tmp_path / ".devcouncil"
    config_dir.mkdir()
    (config_dir / "config.yaml").write_text(
        yaml.safe_dump({"execution": {"coding_cli_probe_order": ["aider", "codex"]}}),
        encoding="utf-8",
    )
    assert resolve_coding_cli_probe_order(tmp_path) == ("aider", "codex")


def test_integration_status_summary_includes_probe_order(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    summary = integration_status_summary(tmp_path)
    assert summary["probe_order"]
    assert summary["default_executor"] == "manual"


def test_integration_status_summary_includes_capabilities(tmp_path):
    (tmp_path / ".devcouncil").mkdir()

    summary = integration_status_summary(tmp_path)

    assert "capabilities" in summary
    assert any(row["name"] == "codex" for row in summary["capabilities"])
    assert all("launcher_shim" in row for row in summary["capabilities"])


def test_build_integration_check_report_returns_rows(tmp_path, monkeypatch):
    (tmp_path / ".devcouncil").mkdir()
    monkeypatch.setattr("devcouncil.integrations.check.shutil.which", lambda _cmd: None)
    monkeypatch.setattr("devcouncil.cli.commands.integrate._run_capture", lambda _cmd: (0, "help"))
    monkeypatch.setattr(
        "devcouncil.cli.commands.integrate._probe_mcp_tools",
        lambda _root: ["devcouncil_status", "devcouncil_report", "devcouncil_get_task"],
    )

    report = build_integration_check_report(tmp_path, strict=False)

    assert report.ok is True
    assert any(row.name == "Project state" for row in report.checks)
    assert any(row.name == "Recommended coding CLI" for row in report.checks)


def test_integration_status_summary_marks_configured_project_files(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        "integrations:\n  cursor:\n    enabled: true\n    config_path: .cursor/mcp.json\n",
        encoding="utf-8",
    )
    (tmp_path / ".cursor").mkdir()
    (tmp_path / ".cursor" / "mcp.json").write_text(
        '{"mcpServers":{"devcouncil":{"type":"stdio","command":"devcouncil","args":["mcp-server"],"env":{"DEVCOUNCIL_PROJECT_ROOT":"'
        + str(tmp_path).replace("\\", "\\\\")
        + '"}}}}\n',
        encoding="utf-8",
    )

    summary = integration_status_summary(tmp_path)
    cursor = next(row for row in summary["capabilities"] if row["name"] == "cursor")

    assert cursor["configured"] is True
    assert cursor["config_status"] == "ok"
    assert cursor["fixable"] is False


def test_integration_check_report_json():
    report = IntegrationCheckReport(
        (IntegrationCheckRow(name="Codex CLI", status="missing", details="not installed"),),
        None,
        0,
    )
    payload = report.as_dict()
    assert payload["ok"] is True
    assert payload["recommended_executor"] is None
