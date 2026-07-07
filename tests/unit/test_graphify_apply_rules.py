
from devcouncil.integrations.graphify import GraphifyIntegration


def test_apply_rules_writes_status_json(tmp_path):
    integration = GraphifyIntegration(tmp_path)
    integration.initialize()
    integration.apply_rules()

    out_path = tmp_path / ".devcouncil" / "graphify" / "rules_applied.json"
    assert out_path.exists()
    import json
    payload = json.loads(out_path.read_text(encoding="utf-8"))
    assert payload["status"] == "applied"
    assert payload["engine"] == "internal"
    assert "applied_at" in payload
