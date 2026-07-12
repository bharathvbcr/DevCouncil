"""Phase 0 audit-fix regression tests for liveness/wiring."""

from __future__ import annotations

import subprocess

from devcouncil.indexing.repo_mapper import RepoMapper
from devcouncil.indexing.wiring import (
    build_dynamic_import_index,
    entry_roots,
    module_tokens_for,
    reference_cleared,
    strip_js_comments,
    strip_py_comments,
)
from devcouncil.verification.checks.liveness_ratchet import (
    delete_liveness_baseline,
    detect_liveness_regressions,
    load_liveness_baseline,
    snapshot_liveness_baseline,
)


def _git(root, *args):
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True)


def _commit(root):
    _git(root, "init")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init")


def _write(tmp_path, files):
    for rel, content in files.items():
        p = tmp_path / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content, encoding="utf-8")


def test_strip_py_comments_preserves_line_count():
    src = (
        "# leading comment\n"
        "def foo():\n"
        "    return 1  # trailing\n"
        "# another\n"
        "x = 2\n"
    )
    cleaned = strip_py_comments(src)
    assert cleaned.count("\n") == src.count("\n")
    assert "def foo" in cleaned
    assert "leading comment" not in cleaned
    # Line 1 blanked, so foo still starts at line 2
    assert cleaned.splitlines()[1].startswith("def foo")


def test_strip_js_comments_preserves_newlines_in_blocks():
    src = "/* line1\nline2\n*/\nexport function f() { return 1; }\n"
    cleaned = strip_js_comments(src)
    assert cleaned.count("\n") == src.count("\n")
    assert "export function f" in cleaned
    # Function still on line 4 after 3-line block comment
    assert "export function f" in cleaned.splitlines()[3]


def test_comment_strip_does_not_skew_dead_symbol_detection(tmp_path):
    """Leading comments must not falsely clear a dead symbol via line skew."""
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/mod.py": (
            "# header comment\n"
            "# another header\n"
            "def never_called():\n"
            "    return 1\n"
        ),
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    joined = " ".join(repo_map.dead_symbol_candidates)
    assert "never_called" in joined


def test_entry_roots_exclude_structural_exemptions(tmp_path):
    _write(tmp_path, {
        "pyproject.toml": (
            "[project]\nname = \"x\"\nversion = \"0\"\n"
            "[project.scripts]\nmycli = \"pkg.cli:main\"\n"
        ),
        "pkg/__init__.py": "",
        "pkg/cli.py": "def main():\n    pass\n",
        "pkg/__main__.py": "print('hi')\n",
        "benchmarks/bench_foo.py": "def run():\n    pass\n",
        "app/dashboard/page.tsx": "export default function Page() { return null; }\n",
        "migrations/001_init.py": "def upgrade():\n    pass\n",
    })
    _commit(tmp_path)
    files = [
        "pkg/__init__.py", "pkg/cli.py", "pkg/__main__.py",
        "benchmarks/bench_foo.py", "app/dashboard/page.tsx", "migrations/001_init.py",
    ]
    roots = set(entry_roots(tmp_path, files, production_only=True))
    assert "pkg/cli.py" in roots
    assert "pkg/__main__.py" in roots
    assert "benchmarks/bench_foo.py" not in roots
    assert "app/dashboard/page.tsx" not in roots
    assert "migrations/001_init.py" not in roots


def test_entry_roots_not_capped_in_map(tmp_path):
    """Debt lists are capped; entry_roots must keep every config/convention seed."""
    scripts = "\n".join(f'cli{i} = "pkg.cli{i}:main"' for i in range(220))
    files = {
        "pyproject.toml": (
            f"[project]\nname = \"x\"\nversion = \"0\"\n[project.scripts]\n{scripts}\n"
        ),
        "pkg/__init__.py": "",
    }
    for i in range(220):
        files[f"pkg/cli{i}.py"] = "def main():\n    pass\n"
    _write(tmp_path, files)
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    assert len(repo_map.entry_roots) >= 220
    assert len(repo_map.unwired_candidates) <= RepoMapper(tmp_path)._LIVENESS_CAP


def test_short_stem_does_not_overmatch(tmp_path):
    """importlib('other.config') must not clear pkg/config.py via bare stem."""
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/config.py": "SETTING = 1\n",
        "pkg/other.py": (
            "import importlib\n"
            "importlib.import_module('other.config')\n"
        ),
    })
    _commit(tmp_path)
    tokens = module_tokens_for("pkg/config.py")
    assert "config" not in tokens or "pkg.config" in tokens
    assert not reference_cleared(tmp_path, "pkg/config.py", git_files=[
        "pkg/__init__.py", "pkg/config.py", "pkg/other.py",
    ])
    # Exact module path still clears
    (tmp_path / "pkg" / "loader.py").write_text(
        "import importlib\nimportlib.import_module('pkg.config')\n",
        encoding="utf-8",
    )
    assert reference_cleared(tmp_path, "pkg/config.py", git_files=[
        "pkg/__init__.py", "pkg/config.py", "pkg/other.py", "pkg/loader.py",
    ])


def test_dynamic_import_index_shared_scan(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/plugin.py": "def hook():\n    return 1\n",
        "pkg/loader.py": (
            "import importlib\n"
            "importlib.import_module('pkg.plugin')\n"
        ),
    })
    _commit(tmp_path)
    files = ["pkg/__init__.py", "pkg/plugin.py", "pkg/loader.py"]
    index = build_dynamic_import_index(tmp_path, files)
    assert reference_cleared(
        tmp_path, "pkg/plugin.py", git_files=files, dynamic_index=index,
    )


def test_ratchet_skips_new_dead_symbol_in_existing_file():
    """New unused def in an existing file is dead_symbol, not stranded_code."""
    from devcouncil.indexing.wiring import LIVENESS_SCAN_VERSION

    baseline = {
        "complete": True,
        "scan_version": LIVENESS_SCAN_VERSION,
        "unwired_candidates": [],
        "unreachable_files": [],
        "dead_symbol_candidates": [],
        "symbol_index": ["pkg/mod.py::used"],
    }
    current = {
        "unwired_candidates": [],
        "unreachable_files": [],
        "dead_symbol_candidates": ["pkg/mod.py:10 helper"],
        "symbol_index": ["pkg/mod.py::used", "pkg/mod.py::helper"],
    }
    gaps = detect_liveness_regressions(
        baseline, current, set(), blocking=True,
    )
    assert gaps == []


def test_ratchet_flags_preexisting_symbol_newly_dead():
    from devcouncil.indexing.wiring import LIVENESS_SCAN_VERSION

    baseline = {
        "complete": True,
        "scan_version": LIVENESS_SCAN_VERSION,
        "unwired_candidates": [],
        "unreachable_files": [],
        "dead_symbol_candidates": [],
        "symbol_index": ["pkg/mod.py::helper"],
    }
    current = {
        "unwired_candidates": [],
        "unreachable_files": [],
        "dead_symbol_candidates": ["pkg/mod.py:3 helper"],
    }
    gaps = detect_liveness_regressions(
        baseline, current, set(), blocking=True,
    )
    assert len(gaps) == 1
    assert gaps[0].gap_type == "stranded_code"
    assert "helper" in gaps[0].description


def test_ratchet_skips_symbol_whose_def_line_in_diff():
    from devcouncil.indexing.wiring import LIVENESS_SCAN_VERSION

    baseline = {
        "complete": True,
        "scan_version": LIVENESS_SCAN_VERSION,
        "unwired_candidates": [],
        "unreachable_files": [],
        "dead_symbol_candidates": [],
        "symbol_index": ["pkg/mod.py::helper"],  # even if wrongly indexed
    }
    current = {
        "unwired_candidates": [],
        "unreachable_files": [],
        "dead_symbol_candidates": ["pkg/mod.py:3 helper"],
    }
    gaps = detect_liveness_regressions(
        baseline,
        current,
        set(),
        blocking=True,
        diff_added_lines={"pkg/mod.py": {3}},
    )
    assert gaps == []


def test_baseline_write_once(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/orphan.py": "x = 1\n",
    })
    _commit(tmp_path)
    first = snapshot_liveness_baseline(tmp_path, "TASK-1")
    assert first is not None
    loaded = load_liveness_baseline(tmp_path, "TASK-1")
    assert loaded is not None
    assert loaded.get("complete") is True
    original_dead = list(loaded.get("dead_symbol_candidates") or [])

    # Mutate tree then snapshot again — write-once must keep the first baseline.
    (tmp_path / "pkg" / "orphan.py").write_text("x = 2\ny = 3\n", encoding="utf-8")
    second = snapshot_liveness_baseline(tmp_path, "TASK-1")
    assert second == first
    loaded2 = load_liveness_baseline(tmp_path, "TASK-1")
    assert loaded2.get("dead_symbol_candidates") == original_dead

    # Explicit reset rewrites.
    reset = snapshot_liveness_baseline(tmp_path, "TASK-1", reset=True)
    assert reset is not None


def test_baseline_incomplete_treated_as_missing(tmp_path):
    base_dir = tmp_path / ".devcouncil" / "liveness_baseline"
    base_dir.mkdir(parents=True)
    (base_dir / "TASK-1.json").write_text(
        '{"unwired_candidates": [], "complete": false}\n',
        encoding="utf-8",
    )
    assert load_liveness_baseline(tmp_path, "TASK-1") is None


def test_delete_baseline_on_demand(tmp_path):
    _write(tmp_path, {"pkg/__init__.py": "", "pkg/a.py": "x = 1\n"})
    _commit(tmp_path)
    snapshot_liveness_baseline(tmp_path, "TASK-1")
    assert load_liveness_baseline(tmp_path, "TASK-1") is not None
    assert delete_liveness_baseline(tmp_path, "TASK-1") is True
    assert load_liveness_baseline(tmp_path, "TASK-1") is None


def test_incomplete_baseline_skips_ratchet():
    gaps = detect_liveness_regressions(
        {"unwired_candidates": [], "complete": False},
        {"unwired_candidates": ["pkg/a.py"]},
        set(),
        blocking=True,
    )
    assert gaps == []


def test_tsconfig_parent_relative_not_mangled_by_lstrip(tmp_path):
    """``../shared/*`` must not become ``shared/*`` via character-class lstrip."""
    _write(tmp_path, {
        "tsconfig.json": (
            '{"compilerOptions": {"baseUrl": ".", '
            '"paths": {"@shared/*": ["../shared/*"]}}}\n'
        ),
        "package.json": '{"name": "app"}\n',
        "src/app.ts": "export const x = 1;\n",
    })
    _commit(tmp_path)
    mapper = RepoMapper(tmp_path)
    rules = mapper._load_js_path_aliases()
    has_parent = any(
        any(t.startswith("../") or t.startswith("/../") or "/../" in f"/{t}"
            for t in targets)
        for _, targets in rules
    )
    assert has_parent, f"expected parent-relative target preserved, got {rules}"
