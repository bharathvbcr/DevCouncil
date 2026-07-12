"""Graph-based dead-code precision: methods, __all__, confidence tiers."""

from __future__ import annotations

import subprocess

from devcouncil.indexing.graph.build import build_code_graph
from devcouncil.indexing.graph.schema import Confidence
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


def test_dead_symbol_methods_detected(tmp_path):
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n[project.scripts]\ncli="pkg.main:main"\n',
        "pkg/__init__.py": "",
        "pkg/main.py": (
            "class Svc:\n"
            "    def used(self):\n"
            "        return 1\n"
            "    def unused_method(self):\n"
            "        return 2\n"
            "def main():\n"
            "    Svc().used()\n"
        ),
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    method_dead = [
        d for d in graph.dead_code
        if d.kind == "method" and "unused_method" in d.id
    ]
    assert method_dead
    assert method_dead[0].confidence == Confidence.INFERRED
    assert not any("Svc.used" in d.id for d in graph.dead_code)


def test_all_export_keeps_symbol_live(tmp_path):
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n[project.scripts]\ncli="pkg.main:main"\n',
        "pkg/__init__.py": "",
        "pkg/main.py": (
            '__all__ = ["exported"]\n'
            "def exported():\n"
            "    return 1\n"
            "def main():\n"
            "    return 0\n"
        ),
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    assert not any(d.id.endswith("::exported") for d in graph.dead_code)


def test_unused_symbol_in_imported_file_is_dead(tmp_path):
    """File-level import must not keep unused sibling symbols alive."""
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n[project.scripts]\ncli="pkg.b:main"\n',
        "pkg/__init__.py": "",
        "pkg/a.py": "def used():\n    return 1\ndef dead():\n    return 2\n",
        "pkg/b.py": "from pkg.a import used\ndef main():\n    return used()\n",
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    dead = [d for d in graph.dead_code if d.id.endswith("::dead")]
    assert dead, f"expected dead in {graph.dead_code}"
    assert dead[0].confidence == Confidence.EXTRACTED
    assert not any(d.id.endswith("::used") for d in graph.dead_code)
    assert not any(d.id.endswith("::main") for d in graph.dead_code)


def test_transitive_dead_island_propagates(tmp_path):
    """alpha_entry → beta_helper island: both dead; cascade reason on beta."""
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n[project.scripts]\ncli="pkg.main:main"\n',
        "pkg/__init__.py": "",
        "pkg/island.py": (
            "def alpha_entry():\n"
            "    return beta_helper()\n"
            "def beta_helper():\n"
            "    return 1\n"
        ),
        "pkg/main.py": "def main():\n    return 0\n",
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    alpha = [d for d in graph.dead_code if d.id.endswith("::alpha_entry")]
    beta = [d for d in graph.dead_code if d.id.endswith("::beta_helper")]
    assert alpha, f"expected alpha_entry dead in {graph.dead_code}"
    assert beta, f"expected beta_helper dead in {graph.dead_code}"
    assert beta[0].reason == "only callers are dead"
    assert beta[0].confidence == Confidence.INFERRED
    assert alpha[0].confidence == Confidence.EXTRACTED


def test_cross_file_reexport_protects_defining_symbol(tmp_path):
    """Barrel ``__init__`` re-export keeps defining-file symbol live."""
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n[project.scripts]\ncli="pkg.main:main"\n',
        "pkg/__init__.py": "from pkg.impl import Foo\n",
        "pkg/impl.py": "def Foo():\n    return 1\n",
        "pkg/main.py": "def main():\n    return 0\n",
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    assert not any(d.id.endswith("::Foo") for d in graph.dead_code), graph.dead_code


def test_non_init_import_is_not_reexport(tmp_path):
    """Module-level import in a non-init file must not mark names as re-exports."""
    from devcouncil.indexing.graph.extract_python import extract_python

    ext = extract_python(
        "pkg/util.py",
        "from pkg.impl import Helper\ndef util():\n    return Helper\n",
    )
    assert "Helper" not in ext.reexports


def test_getattr_dynamic_seeds_live(tmp_path):
    """getattr(x, \"name\") in production code seeds the named symbol live."""
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n[project.scripts]\ncli="pkg.main:main"\n',
        "pkg/__init__.py": "",
        "pkg/main.py": (
            "def hidden_api():\n"
            "    return 1\n"
            "def main():\n"
            "    return getattr(None, 'hidden_api')\n"
        ),
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    assert not any(d.id.endswith("::hidden_api") for d in graph.dead_code), graph.dead_code


def test_override_method_live_when_base_live(tmp_path):
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n[project.scripts]\ncli="pkg.main:main"\n',
        "pkg/__init__.py": "",
        "pkg/main.py": (
            "class Base:\n"
            "    def hook(self):\n"
            "        return 1\n"
            "class Child(Base):\n"
            "    def hook(self):\n"
            "        return 2\n"
            "def main():\n"
            "    return Base().hook()\n"
        ),
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    assert not any("Child.hook" in d.id for d in graph.dead_code), graph.dead_code


def test_module_alias_attr_call_resolves(tmp_path):
    """``import mod as alias; alias.fn()`` must not bind to a same-file ``fn``."""
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n[project.scripts]\ncli="pkg.main:main"\n',
        "pkg/__init__.py": "",
        "pkg/handlers.py": (
            "def get_prompt():\n"
            "    return helper()\n"
            "def helper():\n"
            "    return 1\n"
        ),
        "pkg/main.py": (
            "from pkg import handlers as prompt_handlers\n"
            "def get_prompt():\n"
            "    return prompt_handlers.get_prompt()\n"
            "def main():\n"
            "    return get_prompt()\n"
        ),
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    assert not any(d.id.endswith("handlers.py::helper") for d in graph.dead_code), graph.dead_code
    assert not any(d.id.endswith("handlers.py::get_prompt") for d in graph.dead_code), graph.dead_code


def test_property_ref_in_tests_keeps_symbol_live(tmp_path):
    """Non-call attribute access in tests (``card.flag``) seeds the property live."""
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n[project.scripts]\ncli="pkg.main:main"\n',
        "pkg/__init__.py": "",
        "pkg/models.py": (
            "class Card:\n"
            "    def __init__(self):\n"
            "        self.blocks_gate = True\n"
            "    @property\n"
            "    def blocks_completion(self):\n"
            "        return self.blocks_gate\n"
            "def main():\n"
            "    return 0\n"
        ),
        "tests/test_card.py": (
            "from pkg.models import Card\n"
            "def test_blocks():\n"
            "    assert Card().blocks_completion\n"
        ),
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    assert not any("blocks_completion" in d.id for d in graph.dead_code), graph.dead_code


def test_callback_method_ref_keeps_callee_live(tmp_path):
    """``pool.submit(self._job)`` must seed ``_job`` so its callees stay live."""
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n[project.scripts]\ncli="pkg.main:main"\n',
        "pkg/__init__.py": "",
        "pkg/main.py": (
            "from concurrent.futures import ThreadPoolExecutor\n"
            "class Worker:\n"
            "    def run(self):\n"
            "        with ThreadPoolExecutor() as pool:\n"
            "            pool.submit(self._job)\n"
            "    def _job(self):\n"
            "        return helper()\n"
            "def helper():\n"
            "    return 1\n"
            "def main():\n"
            "    Worker().run()\n"
        ),
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    assert not any(d.id.endswith("::helper") for d in graph.dead_code), graph.dead_code


def test_ambiguous_attr_call_fans_out_to_all_candidates(tmp_path):
    """``obj.to_dict()`` with multiple ``to_dict`` methods must not drop any."""
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n[project.scripts]\ncli="pkg.main:main"\n',
        "pkg/__init__.py": "",
        "pkg/a.py": (
            "class A:\n"
            "    def to_dict(self):\n"
            "        return {}\n"
        ),
        "pkg/b.py": (
            "class B:\n"
            "    def to_dict(self):\n"
            "        return {}\n"
        ),
        "pkg/main.py": (
            "from pkg.a import A\n"
            "from pkg.b import B\n"
            "def main():\n"
            "    x = A()\n"
            "    return x.to_dict()\n"
        ),
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    assert not any("A.to_dict" in d.id for d in graph.dead_code), graph.dead_code
    # B.to_dict is unused; fan-out from main keeps A live but B may stay dead
    # (ambiguous edges only from callers that invoke to_dict).
    assert not any("A.to_dict" in d.id for d in graph.dead_code)


def test_dunder_method_exempt(tmp_path):
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n[project.scripts]\ncli="pkg.main:main"\n',
        "pkg/__init__.py": "",
        "pkg/main.py": (
            "class Box:\n"
            "    def __enter__(self):\n"
            "        return self\n"
            "    def __exit__(self, *a):\n"
            "        return False\n"
            "def main():\n"
            "    return 0\n"
        ),
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    assert not any("__enter__" in d.id or "__exit__" in d.id for d in graph.dead_code)


def test_legacy_dead_format_preserved(tmp_path):
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n[project.scripts]\ncli="pkg.main:main"\n',
        "pkg/__init__.py": "",
        "pkg/main.py": "def main():\n    return 0\n",
        "pkg/orphan.py": "def lonely():\n    return 1\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    for entry in repo_map.dead_symbol_candidates:
        # path:line name
        assert " " in entry
        loc, name = entry.split(" ", 1)
        assert ":" in loc
        assert name


def test_main_guard_script_not_flagged(tmp_path):
    """Module-level call from a reachable (imported) file keeps the callee live."""
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n[project.scripts]\ncli="pkg.main:main"\n',
        "pkg/__init__.py": "",
        "pkg/setup_hooks.py": (
            "def init_db():\n"
            "    return 1\n"
            "\n"
            "init_db()\n"
        ),
        "pkg/main.py": (
            "import pkg.setup_hooks  # noqa: F401\n"
            "def main():\n"
            "    return 0\n"
        ),
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    assert not any(d.id.endswith("::init_db") for d in graph.dead_code), graph.dead_code


def test_scripts_dir_symbol_skipped(tmp_path):
    """Symbols under scripts/ are structurally exempt at symbol level."""
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n[project.scripts]\ncli="pkg.main:main"\n',
        "pkg/__init__.py": "",
        "pkg/main.py": "def main():\n    return 0\n",
        "scripts/tool.py": (
            "def categorize():\n"
            "    return 1\n"
            "def main():\n"
            "    return categorize()\n"
            "\n"
            "if __name__ == '__main__':\n"
            "    main()\n"
        ),
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    assert not any("scripts/" in d.path for d in graph.dead_code), graph.dead_code


def test_getattr_method_live(tmp_path):
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n[project.scripts]\ncli="pkg.main:main"\n',
        "pkg/__init__.py": "",
        "pkg/compiler.py": (
            "class AcceptanceTestCompiler:\n"
            "    def repair(self):\n"
            "        return 1\n"
        ),
        "pkg/main.py": (
            "from pkg.compiler import AcceptanceTestCompiler\n"
            "def main():\n"
            "    c = AcceptanceTestCompiler()\n"
            "    fn = getattr(c, 'repair', None)\n"
            "    return fn\n"
        ),
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    assert not any("repair" in d.id for d in graph.dead_code), graph.dead_code


def test_unrelated_same_name_local_does_not_demote(tmp_path):
    """Bare-name references in unrelated files must not demote a dead method."""
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n[project.scripts]\ncli="pkg.main:main"\n',
        "pkg/__init__.py": "",
        "pkg/role.py": (
            "class Role:\n"
            "    def matches(self, x):\n"
            "        return False\n"
        ),
        "pkg/other.py": (
            "def helper():\n"
            "    matches = []\n"
            "    return matches\n"
        ),
        "pkg/main.py": (
            "from pkg.other import helper\n"
            "def main():\n"
            "    return helper()\n"
        ),
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    hits = [d for d in graph.dead_code if "Role.matches" in d.id or d.id.endswith("::matches")]
    assert hits, f"expected Role.matches dead in {graph.dead_code}"
    assert hits[0].confidence == Confidence.INFERRED
    assert "referenced by name" not in (hits[0].reason or "")


def test_string_literal_key_does_not_clear_token_scan(tmp_path):
    """Dict-key string literals must not clear the token-scan dead list."""
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n[project.scripts]\ncli="pkg.main:main"\n',
        "pkg/__init__.py": "",
        "pkg/stats.py": "def cost_by_task():\n    return {}\n",
        "pkg/status.py": (
            "def report():\n"
            "    return {'cost_by_task': 1}\n"
        ),
        "pkg/main.py": (
            "from pkg.status import report\n"
            "def main():\n"
            "    return report()\n"
        ),
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    hits = [d for d in graph.dead_code if d.id.endswith("::cost_by_task")]
    assert hits, f"expected cost_by_task dead in {graph.dead_code}"
    assert hits[0].confidence == Confidence.EXTRACTED
    assert "token-scan agrees" in (hits[0].reason or "")


def test_vendor_min_js_excluded_from_graph(tmp_path):
    """Vendored ``.min.js`` must not produce nodes or cross-language call edges."""
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname="t"\nversion="0"\n[project.scripts]\ncli="pkg.main:main"\n',
        "pkg/__init__.py": "",
        "pkg/main.py": (
            "class EventBus:\n"
            "    def emit(self):\n"
            "        return 1\n"
            "def main():\n"
            "    return EventBus().emit()\n"
        ),
        "assets/vendor/force-graph.min.js": (
            "function emit(){return 1}\n"
            "emit();\n"
            "var matches = 1;\n"
        ),
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    assert not any("vendor" in (n.path or "") for n in graph.nodes), [
        n.path for n in graph.nodes if "vendor" in (n.path or "")
    ]
    assert not any(
        "vendor" in (e.source or "") or "vendor" in (e.target or "")
        for e in graph.edges
    )


def test_js_bare_name_cannot_bind_python_symbol(tmp_path):
    """Cross-language unique-global fallback must not bind JS calls to Python."""
    from devcouncil.indexing.graph.extract_python import (
        ExtractedCall,
        ExtractedSymbol,
        FileExtraction,
    )
    from devcouncil.indexing.graph.resolve import resolve_calls

    py_ext = FileExtraction(
        path="pkg/bus.py",
        language="python",
        symbols=[
            ExtractedSymbol(
                kind="function", name="emit", qualname="emit", line=1, end_line=2
            ),
        ],
    )
    js_ext = FileExtraction(
        path="src/app.js",
        language="javascript",
        calls=[ExtractedCall(name="emit", line=1, receiver="", qualname_hint="emit")],
    )
    symbol_index = {"pkg/bus.py::emit": "pkg/bus.py::emit"}
    edges = resolve_calls(
        {"pkg/bus.py": py_ext, "src/app.js": js_ext},
        symbol_index,
        [],
    )
    assert not any(
        e.target == "pkg/bus.py::emit" and "app.js" in e.source for e in edges
    )
