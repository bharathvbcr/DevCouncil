from pathlib import Path

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


def test_graphify_initialize_does_not_write_agent_guides(tmp_path):
    """Only ``dev map`` / map.py owns AGENTS.md / CLAUDE.md format."""
    # Pre-seed a marker-managed guide so a buggy writer would clobber it.
    marker = "<!-- Managed by dev map: keep this file in sync with .devcouncil/repo_map.json. -->"
    original = marker + "\n\n# Agent Workspace Guide\n\nORIGINAL_CONTENT\n"
    (tmp_path / "AGENTS.md").write_text(original, encoding="utf-8")
    (tmp_path / "CLAUDE.md").write_text(original, encoding="utf-8")

    GraphifyIntegration(tmp_path).initialize()

    assert (tmp_path / "AGENTS.md").read_text(encoding="utf-8") == original
    assert (tmp_path / "CLAUDE.md").read_text(encoding="utf-8") == original
    assert (tmp_path / ".devcouncil" / "graphify.yaml").is_file()


def test_gitnexus_initialize_does_not_write_agent_guides(tmp_path):
    from devcouncil.integrations.gitnexus import GitNexusIntegration

    marker = "<!-- Managed by dev map: keep this file in sync with .devcouncil/repo_map.json. -->"
    original = marker + "\n\n# Agent Workspace Guide\n\nORIGINAL_CONTENT\n"
    (tmp_path / "AGENTS.md").write_text(original, encoding="utf-8")
    (tmp_path / "CLAUDE.md").write_text(original, encoding="utf-8")

    GitNexusIntegration(tmp_path).initialize()

    assert (tmp_path / "AGENTS.md").read_text(encoding="utf-8") == original
    assert (tmp_path / "CLAUDE.md").read_text(encoding="utf-8") == original
    assert (tmp_path / ".devcouncil" / "nexus" / "index_config.json").is_file()
    # Hardcoded DevCouncil paths must not appear via a stub guide writer.
    assert "src/devcouncil/cli/main.py" not in (tmp_path / "AGENTS.md").read_text(encoding="utf-8")
