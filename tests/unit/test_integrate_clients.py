from devcouncil.integrations.clients import codex, common, hooks, aider, claude


def test_integrate_client_modules_expose_commands(tmp_path):
    root = tmp_path
    assert codex._codex_command(root)[0] == "codex"
    assert common._project_root(str(root)) == root.resolve()
    assert hooks.SUPPORTED_HOOK_TOOLS == common.SUPPORTED_HOOK_TOOLS
    assert aider._configure_aider(root, apply=False) is True
    assert claude._claude_command(root, "local")[0] == "claude"
