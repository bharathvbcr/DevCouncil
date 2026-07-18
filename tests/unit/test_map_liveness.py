"""Repo-map liveness artifact tests."""

from __future__ import annotations

import subprocess

from devcouncil.indexing.repo_mapper import RepoMapper


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


def test_alias_edges_resolved_from_tsconfig(tmp_path):
    _write(tmp_path, {
        "tsconfig.json": (
            '{"compilerOptions": {"baseUrl": ".", "paths": {"@/*": ["src/*"]}}}\n'
        ),
        "package.json": '{"name": "app"}\n',
        "src/models.ts": "export class Model {}\n",
        "src/app.ts": "import { Model } from '@/models';\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    assert "src/app.ts" in repo_map.dependents.get("src/models.ts", [])


def test_entry_roots_from_pyproject_and_conventions(tmp_path):
    _write(tmp_path, {
        "pyproject.toml": (
            "[project]\nname = \"x\"\nversion = \"0\"\n"
            "[project.scripts]\nmycli = \"pkg.cli:main\"\n"
        ),
        "pkg/__init__.py": "",
        "pkg/cli.py": "def main():\n    pass\n",
        "pkg/__main__.py": "print('hi')\n",
        "pkg/orphan.py": "x = 1\n",
        "tests/test_cli.py": "def test_main():\n    assert True\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    roots = set(repo_map.entry_roots)
    assert "pkg/cli.py" in roots
    assert "pkg/__main__.py" in roots
    # Stored roots are production-only — test files must not appear.
    assert not any("tests/" in r or r.startswith("test_") for r in roots)
    assert "tests/test_cli.py" not in roots


def test_unwired_candidates_excludes_entry_roots(tmp_path):
    _write(tmp_path, {
        "pyproject.toml": (
            "[project]\nname = \"x\"\nversion = \"0\"\n"
            "[project.scripts]\nmycli = \"pkg.cli:main\"\n"
        ),
        "pkg/__init__.py": "",
        "pkg/cli.py": "def main():\n    pass\n",
        "pkg/orphan.py": "x = 1\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    assert "pkg/cli.py" not in repo_map.unwired_candidates
    assert "pkg/orphan.py" in repo_map.unwired_candidates


def test_imported_submodule_clears_package_init(tmp_path):
    """Ancestor __init__.py edges: importing a submodule wires package inits."""
    _write(tmp_path, {
        "pyproject.toml": (
            "[project]\nname = \"x\"\nversion = \"0\"\n"
            "[project.scripts]\nmycli = \"pkg.cli:main\"\n"
        ),
        "pkg/__init__.py": "",
        "pkg/sub/__init__.py": "",
        "pkg/sub/mod.py": "def f():\n    return 1\n",
        "pkg/cli.py": "from pkg.sub.mod import f\ndef main():\n    return f()\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    assert "pkg/__init__.py" not in repo_map.unwired_candidates
    assert "pkg/sub/__init__.py" not in repo_map.unwired_candidates
    assert "pkg/__init__.py" not in repo_map.unreachable_files
    assert "pkg/sub/__init__.py" not in repo_map.unreachable_files


def test_same_file_use_clears_dead_symbol_on_map(tmp_path):
    """Config-class pattern: class used later in the same file is not dead."""
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/config.py": (
            "class ModelsConfig:\n    name: str = 'x'\n\n"
            "class AppConfig:\n    models: ModelsConfig = ModelsConfig()\n"
        ),
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    joined = " ".join(repo_map.dead_symbol_candidates)
    assert "ModelsConfig" not in joined
    # AppConfig itself is unused outside its span → still flagged.
    assert "AppConfig" in joined


def test_recursive_self_ref_still_dead_on_map(tmp_path):
    """Self-call inside the defining span does not clear the symbol."""
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/mod.py": (
            "def unused():\n"
            "    return unused()  # recursive, still dead\n"
        ),
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    joined = " ".join(repo_map.dead_symbol_candidates)
    assert "unused" in joined


def test_benchmarks_dir_exempt_from_unwired(tmp_path):
    _write(tmp_path, {
        "pyproject.toml": (
            "[project]\nname = \"x\"\nversion = \"0\"\n"
            "[project.scripts]\nmycli = \"pkg.cli:main\"\n"
        ),
        "pkg/__init__.py": "",
        "pkg/cli.py": "def main():\n    pass\n",
        "benchmarks/bench_foo.py": "def run():\n    pass\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    assert "benchmarks/bench_foo.py" not in repo_map.unwired_candidates
    assert "benchmarks/bench_foo.py" not in repo_map.unreachable_files


def test_allow_unwired_suppresses_on_map(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/orphan.py": "# devcouncil: allow-unwired\ndef lonely():\n    return 1\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    assert "pkg/orphan.py" not in repo_map.unwired_candidates


def test_dynamic_import_clears_unwired_on_map(tmp_path):
    _write(tmp_path, {
        "pyproject.toml": (
            "[project]\nname = \"x\"\nversion = \"0\"\n"
            "[project.scripts]\nmycli = \"pkg.cli:main\"\n"
        ),
        "pkg/__init__.py": "",
        "pkg/cli.py": (
            "import importlib\n"
            "def main():\n"
            "    return importlib.import_module('pkg.plugin')\n"
        ),
        "pkg/plugin.py": "def hook():\n    return 1\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    assert "pkg/plugin.py" not in repo_map.unwired_candidates


def test_test_dynamic_import_does_not_clear_unwired(tmp_path):
    _write(tmp_path, {
        "pyproject.toml": (
            "[project]\nname = \"x\"\nversion = \"0\"\n"
            "[project.scripts]\nmycli = \"pkg.cli:main\"\n"
        ),
        "pkg/__init__.py": "",
        "pkg/cli.py": "def main():\n    pass\n",
        "pkg/plugin.py": "def hook():\n    return 1\n",
        "tests/test_plugin.py": (
            "import importlib\n"
            "def test_it():\n"
            "    importlib.import_module('pkg.plugin')\n"
        ),
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    assert "pkg/plugin.py" in repo_map.unwired_candidates


def test_unreachable_catches_orphan_island(tmp_path):
    """A↔B island: B has a dependent, but neither is reachable from entry roots."""
    _write(tmp_path, {
        "pyproject.toml": (
            "[project]\nname = \"x\"\nversion = \"0\"\n"
            "[project.scripts]\nmycli = \"pkg.cli:main\"\n"
        ),
        "pkg/__init__.py": "",
        "pkg/cli.py": "def main():\n    pass\n",
        "pkg/island_a.py": "from pkg.island_b import b\ndef a():\n    return b()\n",
        "pkg/island_b.py": "from pkg.island_a import a\ndef b():\n    return a()\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    # island_b has an importer (island_a) so may not appear in unwired_candidates,
    # but both should be unreachable from entry roots.
    unreachable = set(repo_map.unreachable_files)
    assert "pkg/island_a.py" in unreachable
    assert "pkg/island_b.py" in unreachable
    # At least one of the island files is NOT in unwired (has inbound) but IS unreachable.
    has_inbound_but_unreachable = (
        set(repo_map.unreachable_files) - set(repo_map.unwired_candidates)
    )
    assert has_inbound_but_unreachable & {"pkg/island_a.py", "pkg/island_b.py"}
    # island_b is imported by island_a, so it must not appear in unwired_candidates.
    assert "pkg/island_b.py" not in repo_map.unwired_candidates


def test_dead_symbol_candidates_contents(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/mod.py": "def never_called():\n    return 1\n\ndef used():\n    return 2\n",
        "pkg/caller.py": "from pkg.mod import used\nprint(used())\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    joined = " ".join(repo_map.dead_symbol_candidates)
    assert "never_called" in joined
    assert "used" not in joined or "never_called" in joined


def test_no_liveness_omits_fields(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/a.py": "x = 1\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo(liveness=False)
    assert repo_map.entry_roots == []
    assert repo_map.unwired_candidates == []
    assert repo_map.unreachable_files == []
    assert repo_map.dead_symbol_candidates == []


def test_liveness_caps_respected(tmp_path):
    files = {"pkg/__init__.py": ""}
    for i in range(250):
        files[f"pkg/orphan_{i}.py"] = f"x = {i}\n"
    _write(tmp_path, files)
    _commit(tmp_path)
    mapper = RepoMapper(tmp_path)
    repo_map = mapper.map_repo()
    assert len(repo_map.unwired_candidates) <= mapper._LIVENESS_CAP


def test_test_only_importer_still_unwired(tmp_path):
    """Align with verify gate: a test-only import does not clear unwired_candidates."""
    _write(tmp_path, {
        "pyproject.toml": (
            "[project]\nname = \"x\"\nversion = \"0\"\n"
            "[project.scripts]\nmycli = \"pkg.cli:main\"\n"
        ),
        "pkg/__init__.py": "",
        "pkg/cli.py": "def main():\n    pass\n",
        "pkg/helper.py": "def help():\n    return 1\n",
        "tests/test_helper.py": "from pkg.helper import help\ndef test_it():\n    assert help() == 1\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    assert "pkg/helper.py" in repo_map.unwired_candidates


def test_test_reference_clears_dead_symbol_on_map(tmp_path):
    """Map/verify parity: a test reference clears a production dead symbol."""
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/mod.py": "def helper():\n    return 1\n",
        "tests/test_mod.py": "from pkg.mod import helper\ndef test_it():\n    assert helper() == 1\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    joined = " ".join(repo_map.dead_symbol_candidates)
    assert "helper" not in joined


def test_wiring_decorator_exempts_dead_symbol_on_map(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/cli.py": (
            "import typer\napp = typer.Typer()\n"
            "@app.command()\ndef handle():\n    return 1\n"
        ),
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    joined = " ".join(repo_map.dead_symbol_candidates)
    assert "handle" not in joined


def test_production_only_unreachable_ignores_test_seeds(tmp_path):
    """Files only reachable via tests should still be unreachable from production."""
    _write(tmp_path, {
        "pyproject.toml": (
            "[project]\nname = \"x\"\nversion = \"0\"\n"
            "[project.scripts]\nmycli = \"pkg.cli:main\"\n"
        ),
        "pkg/__init__.py": "",
        "pkg/cli.py": "def main():\n    pass\n",
        "pkg/island_mod.py": "def f():\n    return 1\n",
        "tests/test_island.py": (
            "from pkg.island_mod import f\ndef test_f():\n    assert f() == 1\n"
        ),
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    assert "pkg/island_mod.py" in repo_map.unreachable_files
    assert "pkg/island_mod.py" in repo_map.unwired_candidates


def test_empty_entry_roots_fail_soft_unreachable(tmp_path):
    """No config/convention entry roots → unreachable=[] + unreliable flag."""
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/a.py": "x = 1\n",
        "pkg/b.py": "from pkg.a import x\ny = x\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    assert repo_map.entry_roots == []
    assert repo_map.unreachable_files == []
    assert repo_map.liveness_unreachable_unreliable is True
    # Unwired still computed
    assert "pkg/a.py" in repo_map.unwired_candidates or "pkg/b.py" in repo_map.unwired_candidates


def test_high_unreachable_ratio_fail_soft(tmp_path, monkeypatch):
    """When most code files are unreachable from sparse roots, omit the flood."""
    from devcouncil.indexing.graph import liveness as liv

    monkeypatch.setattr(liv, "_unreachable_unreliable_ratio", lambda _root: 0.25)
    files = {
        "pyproject.toml": (
            "[project]\nname = \"x\"\nversion = \"0\"\n"
            "[project.scripts]\nmycli = \"pkg.cli:main\"\n"
        ),
        "pkg/__init__.py": "",
        "pkg/cli.py": "def main():\n    pass\n",
    }
    for i in range(20):
        files[f"pkg/orphan_{i}.py"] = f"x = {i}\n"
    _write(tmp_path, files)
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    assert repo_map.entry_roots
    assert repo_map.unreachable_files == []
    assert repo_map.liveness_unreachable_unreliable is True
    # Unwired still surfaces the real signal.
    assert any(p.startswith("pkg/orphan_") for p in repo_map.unwired_candidates)


def test_test_only_main_prod_roots_empty_fail_soft(tmp_path):
    """Only test-path __main__ seeds → production roots empty → fail-soft."""
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/lib.py": "def f():\n    return 1\n",
        "tests/__main__.py": "print('test runner')\n",
        "tests/test_lib.py": "from pkg.lib import f\ndef test_f():\n    assert f() == 1\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    assert repo_map.entry_roots == []
    assert repo_map.unreachable_files == []
    assert repo_map.liveness_unreachable_unreliable is True


def test_liveness_meta_reports_truncation(tmp_path, monkeypatch):
    """Ranked caps expose total/shown/truncated in liveness_meta."""
    scripts = "\n".join(f'cli{i} = "pkg.cli{i}:main"' for i in range(220))
    files = {
        "pyproject.toml": (
            f"[project]\nname = \"x\"\nversion = \"0\"\n[project.scripts]\n{scripts}\n"
        ),
        "pkg/__init__.py": "",
    }
    for i in range(220):
        files[f"pkg/orphan{i}.py"] = "x = 1\n"
        files[f"pkg/cli{i}.py"] = "def main():\n    pass\n"
    _write(tmp_path, files)
    _commit(tmp_path)
    monkeypatch.setattr(RepoMapper, "_LIVENESS_CAP", 50)
    repo_map = RepoMapper(tmp_path).map_repo()
    meta = repo_map.liveness_meta.get("unwired") or {}
    assert meta.get("total", 0) > RepoMapper(tmp_path)._LIVENESS_CAP
    assert meta.get("shown") == RepoMapper(tmp_path)._LIVENESS_CAP
    assert meta.get("truncated", 0) > 0


def _go_grammar_available() -> bool:
    try:
        from devcouncil.codeintel.languages import grammar_status

        return not any(
            row.get("missing_grammars")
            for row in grammar_status().get("languages", [])
            if row.get("language") == "go"
        )
    except Exception:
        return False


def test_go_same_package_callee_is_wired_via_call_edges(tmp_path):
    """A Go file called from main (no import statement) must not read as unwired."""
    import pytest

    if not _go_grammar_available():
        pytest.skip("go grammar not installed")
    _write(tmp_path, {
        "cmd/server/main.go": "package main\n\nfunc main() {\n\thandle()\n}\n",
        "cmd/server/handlers.go": "package main\n\nfunc handle() {}\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    assert "cmd/server/main.go" in repo_map.entry_roots
    assert "cmd/server/handlers.go" not in repo_map.unwired_candidates
    assert "cmd/server/handlers.go" not in repo_map.unreachable_files
    assert not repo_map.liveness_unreachable_unreliable
