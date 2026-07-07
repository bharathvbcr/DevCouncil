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
