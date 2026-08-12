from pathlib import Path

from devcouncil.indexing.walk import should_skip_path


def test_should_skip_path_node_modules():
    assert should_skip_path("src/foo/node_modules/bar.js")
    assert should_skip_path(Path("node_modules/pkg/index.js"))


def test_should_skip_path_git():
    assert should_skip_path(".git/config")
    assert should_skip_path(Path("src/.git/objects/abc"))


def test_should_skip_path_normal_source():
    assert not should_skip_path("src/devcouncil/foo.py")
    assert not should_skip_path(Path("tests/unit/test_walk.py"))


def test_should_skip_path_nested_claude_worktree():
    assert should_skip_path(".claude/worktrees/nervous-volhard-80a90b/src/devcouncil/a.py")
    assert should_skip_path(Path(".claude/worktrees/x/README.md"))


def test_should_skip_path_keeps_claude_assets_and_plain_worktrees_dirs():
    # Only the nested-checkout pair is skipped, not .claude assets or a source
    # directory that happens to be named "worktrees".
    assert not should_skip_path(".claude/skills/foo/SKILL.md")
    assert not should_skip_path("src/worktrees/manager.py")
