import json
from pathlib import Path

from devcouncil.indexing.wiring import _pyproject_script_targets, _package_json_entry_targets

def test_pyproject_script_targets(tmp_path):
    # 1. Missing pyproject.toml
    assert _pyproject_script_targets(tmp_path, set()) == set()
    
    # 2. Valid pyproject.toml
    pyproject_toml = tmp_path / "pyproject.toml"
    pyproject_toml.write_text("""
[project]
scripts = { my_cmd = "my_pkg.cli:main" }
gui-scripts = { my_gui = "my_pkg.gui:main" }
entry-points = { "some.group" = { val = "my_pkg.entry:func" } }

[tool.pytest.ini_options]
pytest_plugins = "my_pkg.plugins.a, my_pkg.plugins.b"
""", encoding="utf-8")
    
    file_set = {
        "my_pkg/cli.py",
        "my_pkg/gui.py",
        "my_pkg/entry.py",
        "my_pkg/plugins/a.py",
        "my_pkg/plugins/b.py",
    }
    
    targets = _pyproject_script_targets(tmp_path, file_set)
    assert "my_pkg/cli.py" in targets
    assert "my_pkg/gui.py" in targets
    assert "my_pkg/entry.py" in targets
    assert "my_pkg/plugins/a.py" in targets
    
    # 3. Malformed pyproject.toml (regex fallback path)
    pyproject_toml.write_text("""
invalid [[] toml syntax
scripts = { my_cmd = "my_pkg.cli:main" }
""", encoding="utf-8")
    targets_fallback = _pyproject_script_targets(tmp_path, file_set)
    assert "my_pkg/cli.py" in targets_fallback


def test_package_json_entry_targets(tmp_path):
    # 1. Missing package.json
    assert _package_json_entry_targets(tmp_path, set()) == set()
    
    # 2. Valid package.json
    package_json = tmp_path / "package.json"
    package_json.write_text(json.dumps({
        "main": "dist/index",
        "module": "./dist/esm",
        "bin": {
            "cmd": "bin/run.js"
        }
    }), encoding="utf-8")
    
    file_set = {
        "dist/index.js",
        "dist/esm.js",
        "bin/run.js",
    }
    
    targets = _package_json_entry_targets(tmp_path, file_set)
    assert "dist/index.js" in targets
    assert "dist/esm.js" in targets
    assert "bin/run.js" in targets
    
    # 3. Invalid package.json
    package_json.write_text("invalid json", encoding="utf-8")
    assert _package_json_entry_targets(tmp_path, file_set) == set()
