from devcouncil.integrations.clients import grok, hooks, common


def test_grok_integration_preview(tmp_path):
    root = tmp_path
    assert grok._configure_grok(root, apply=False) is True
    command = grok._grok_mcp_command(root)
    assert command[:4] == ["grok", "mcp", "add", "devcouncil"]


def test_grok_integration_apply_writes_toml(tmp_path):
    root = tmp_path
    written = grok._merge_grok_config_toml(root)
    assert written.exists()
    text = written.read_text(encoding="utf-8")
    assert "[mcp_servers.devcouncil]" in text
    assert "mcp-server" in text
    grok._record_grok_config(root)
    from devcouncil.cli.commands.integrate import _load_raw_config

    config = _load_raw_config(root)
    assert config["integrations"]["grok"]["enabled"] is True


def test_grok_hooks_installer(tmp_path):
    paths = hooks._install_grok_hooks(tmp_path)
    assert len(paths) == 1
    data = paths[0].read_text(encoding="utf-8")
    assert "PreToolUse" in data
    assert "devcouncil hook pre-tool-use --client grok" in data
    assert "grok" in common.SUPPORTED_HOOK_TOOLS
