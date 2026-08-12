"""Unit tests for Phase 2 liveness hardening features."""

from pathlib import Path
from devcouncil.indexing.wiring import (
    _package_json_entry_targets,
    _cargo_toml_script_targets,
    _conventional_main_seeds,
)
from devcouncil.indexing.graph.liveness import file_liveness


def test_package_json_depth_sort(tmp_path: Path):
    file_set = {
        "deep/nested/pkg/package.json",
        "package.json",
        "apps/web/package.json",
        "index.js",
    }
    (tmp_path / "package.json").write_text('{"main": "index.js"}')
    (tmp_path / "index.js").write_text("console.log('root')")

    targets = _package_json_entry_targets(tmp_path, file_set)
    assert "index.js" in targets


def test_cargo_toml_bin_targets(tmp_path: Path):
    file_set = {"Cargo.toml", "src/bin/my_cli.rs", "src/lib.rs"}
    (tmp_path / "Cargo.toml").write_text(
        '[package]\nname = "my_crate"\n\n[[bin]]\nname = "my_cli"\npath = "src/bin/my_cli.rs"\n'
    )
    targets = _cargo_toml_script_targets(tmp_path, file_set)
    assert "src/bin/my_cli.rs" in targets
    assert "src/lib.rs" in targets


def test_go_cmd_main_seeds(tmp_path: Path):
    file_set = {"cmd/server/app.go", "pkg/util/helper.go"}
    (tmp_path / "cmd" / "server").mkdir(parents=True)
    (tmp_path / "cmd" / "server" / "app.go").write_text(
        "package main\n\nfunc main() {\n\tprintln(\"hello\")\n}\n"
    )

    seeds = _conventional_main_seeds(tmp_path, file_set)
    assert "cmd/server/app.go" in seeds
    assert "pkg/util/helper.go" not in seeds


def test_reachability_gap_points(tmp_path: Path):
    files = ["src/main.py", "src/wired.py", "src/orphan.py"]
    call_edges = [("src/main.py", "src/wired.py"), ("src/wired.py", "src/orphan.py")]
    # Note: if src/main.py is entry_root, all are reachable.
    # If we have entry root = src/main.py, but disconnected island:
    files = ["src/main.py", "src/entry.py", "src/island_a.py", "src/island_b.py"]
    (tmp_path / "src").mkdir(parents=True)
    (tmp_path / "src" / "entry.py").write_text("def main(): pass\nif __name__ == '__main__': main()")
    (tmp_path / "src" / "island_a.py").write_text("import island_b")
    (tmp_path / "src" / "island_b.py").write_text("def foo(): pass")

    call_edges = [("src/entry.py", "src/entry.py"), ("src/island_a.py", "src/island_b.py")]
    prod_roots, unwired, unreachable, unreliable = file_liveness(
        tmp_path, files, call_edges
    )
    assert "src/island_b.py" in unreachable or "src/island_a.py" in unwired
