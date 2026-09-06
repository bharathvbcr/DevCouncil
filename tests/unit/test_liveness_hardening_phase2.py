"""Unit tests for Phase 2 liveness hardening features.

``test_reachability_gap_points`` lived here and was retired with
``graph.liveness.file_liveness`` in W3.3. Its expectation — a disconnected
island is unwired or unreachable — did not go with it: the unwired half is
``devmap-query/tests/dynamic_references_clear_unwired.rs::
a_file_nothing_references_is_still_unwired`` and the unreachable half is
``devmap-analyze/tests/dead_clusters.rs``, both against the kernel that now
decides it.

The three tests below exercise ``devcouncil.indexing.wiring`` helpers, which
stay: they are still imported by the verification-side gates.
"""

from pathlib import Path
from devcouncil.indexing.wiring import (
    _package_json_entry_targets,
    _cargo_toml_script_targets,
    _conventional_main_seeds,
)


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
