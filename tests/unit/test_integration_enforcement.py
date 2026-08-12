"""Rank 17 (targeted) — honest containment posture surfaced per coding-CLI client."""

from devcouncil.executors.agent_registry import CODING_CLI_INTEGRATION_INFO
from devcouncil.integrations.check import integration_capability_rows


def test_enforcement_reflects_hook_support():
    # Assist is default: registry capability is advisory+verify, not pre-action.
    assert CODING_CLI_INTEGRATION_INFO["codex"].enforcement == "advisory+verify"
    assert CODING_CLI_INTEGRATION_INFO["claude"].enforcement == "advisory+verify"
    assert CODING_CLI_INTEGRATION_INFO["cursor"].enforcement == "advisory+verify"
    assert CODING_CLI_INTEGRATION_INFO["grok"].enforcement == "advisory+verify"
    assert CODING_CLI_INTEGRATION_INFO["opencode"].enforcement == "advisory+verify"
    verify_only = [i.name for i in CODING_CLI_INTEGRATION_INFO.values() if not i.hooks]
    assert verify_only, "expected some verify-only clients in the registry"
    for name in verify_only:
        assert CODING_CLI_INTEGRATION_INFO[name].enforcement == "verify-only"


def test_capability_rows_expose_enforcement(tmp_path):
    rows = {r["name"]: r for r in integration_capability_rows(tmp_path)}
    assert rows["codex"]["enforcement"] == "advisory+verify"
    assert rows["claude"]["enforcement"] == "advisory+verify"
    # Every row reports an honest posture (no silent assumption of hard containment).
    assert all(r["enforcement"] in {"pre-action", "advisory+verify", "verify-only"} for r in rows.values())


def test_capability_rows_pre_action_when_write_gate_installed(tmp_path):
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    (dev / "config.yaml").write_text(
        "project:\n  name: t\nintegrations:\n  claude:\n    write_gate: true\n  cursor:\n    write_gate: true\n",
        encoding="utf-8",
    )
    rows = {r["name"]: r for r in integration_capability_rows(tmp_path)}
    assert rows["claude"]["enforcement"] == "pre-action"
    assert rows["cursor"]["enforcement"] == "pre-action"
    assert rows["codex"]["enforcement"] == "advisory+verify"
