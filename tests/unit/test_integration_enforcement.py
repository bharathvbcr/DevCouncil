"""Rank 17 (targeted) — honest containment posture surfaced per coding-CLI client."""

from devcouncil.executors.agent_registry import CODING_CLI_INTEGRATION_INFO
from devcouncil.integrations.check import integration_capability_rows


def test_enforcement_reflects_hook_support():
    # Hook-enabled clients gate pre-action; the rest are verify-only.
    assert CODING_CLI_INTEGRATION_INFO["codex"].enforcement == "pre-action"
    assert CODING_CLI_INTEGRATION_INFO["claude"].enforcement == "pre-action"
    verify_only = [i.name for i in CODING_CLI_INTEGRATION_INFO.values() if not i.hooks]
    assert verify_only, "expected some verify-only clients in the registry"
    for name in verify_only:
        assert CODING_CLI_INTEGRATION_INFO[name].enforcement == "verify-only"


def test_capability_rows_expose_enforcement(tmp_path):
    rows = {r["name"]: r for r in integration_capability_rows(tmp_path)}
    assert rows["codex"]["enforcement"] == "pre-action"
    # Every row reports an honest posture (no silent assumption of hard containment).
    assert all(r["enforcement"] in {"pre-action", "verify-only"} for r in rows.values())
