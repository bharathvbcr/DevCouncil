import json

from devcouncil.integrations.actions import (
    VALID_INTEGRATION_TARGETS,
    IntegrationActionReport,
    apply_integration_target,
    normalize_apply_target,
)


def test_normalize_apply_target_accepts_known_targets():
    assert normalize_apply_target("codex") == "codex"
    assert normalize_apply_target("hooks") == "hooks"
    assert normalize_apply_target("all") == "all"
    assert "opencode" in VALID_INTEGRATION_TARGETS


def test_normalize_apply_target_rejects_unknown_targets():
    try:
        normalize_apply_target("python -c bad")
    except ValueError as exc:
        assert "Unsupported integration target" in str(exc)
    else:
        raise AssertionError("unknown target should fail")


def test_action_report_serializes_to_dashboard_payload():
    report = IntegrationActionReport(
        target="cursor",
        ok=True,
        results=[{"target": "cursor", "ok": True, "path": ".cursor/mcp.json"}],
        warnings=[],
        check={"ok": True, "checks": []},
    )

    payload = report.as_dict()

    assert payload["ok"] is True
    assert payload["target"] == "cursor"
    assert payload["results"][0]["path"] == ".cursor/mcp.json"
    assert json.loads(report.to_json())["target"] == "cursor"


def test_apply_cursor_writes_project_config(tmp_path, monkeypatch):
    monkeypatch.setattr("devcouncil.cli.commands.integrate.shutil.which", lambda _cmd: None)

    report = apply_integration_target(tmp_path, "cursor", include_hooks=False)

    assert report.ok is True
    assert (tmp_path / ".cursor" / "mcp.json").exists()
    data = json.loads((tmp_path / ".cursor" / "mcp.json").read_text(encoding="utf-8"))
    server = data["mcpServers"]["devcouncil"]
    assert server["command"] == "devcouncil"
    assert server["args"] == ["mcp-server"]
    assert server["env"]["DEVCOUNCIL_PROJECT_ROOT"] == str(tmp_path)
