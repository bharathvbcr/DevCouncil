"""Code graph extraction, resolution, and confidence ladder."""

from __future__ import annotations

import subprocess

from devcouncil.indexing.graph.build import build_code_graph, load_code_graph, write_code_graph
from devcouncil.indexing.graph.extract_python import extract_python
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


def test_extract_python_symbols_and_calls():
    src = '''
class Foo:
    def bar(self):
        helper()
        self.baz()

    def baz(self):
        pass

def helper():
    return 1

def entry():
    Foo().bar()
'''
    ext = extract_python("pkg/m.py", src)
    names = {s.qualname for s in ext.symbols}
    assert "Foo" in names
    assert "Foo.bar" in names
    assert "helper" in names
    assert any(c.name == "helper" for c in ext.calls)
    assert any(c.name == "baz" and c.receiver == "self" for c in ext.calls)


def test_build_code_graph_writes_artifact(tmp_path):
    _write(tmp_path, {
        "pyproject.toml": '[project]\nname = "t"\nversion = "0"\n[project.scripts]\ncli = "pkg.main:main"\n',
        "pkg/__init__.py": "",
        "pkg/main.py": "from pkg import util\ndef main():\n    util.run()\n",
        "pkg/util.py": "def run():\n    return 1\n\ndef orphan():\n    return 2\n",
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path)
    write_code_graph(tmp_path, graph)
    loaded = load_code_graph(tmp_path)
    assert loaded is not None
    assert loaded.schema_version == 2
    assert any(n.id.endswith("::run") for n in loaded.nodes)
    assert any(e.kind == "imports" for e in loaded.edges)
    assert any(e.kind == "calls" for e in loaded.edges)


def test_call_resolution_same_file_extracted(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/a.py": "def helper():\n    return 1\ndef caller():\n    return helper()\n",
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path, liveness=False)
    calls = [e for e in graph.edges if e.kind == "calls"]
    assert any(
        e.target.endswith("::helper") and e.confidence == Confidence.EXTRACTED for e in calls
    )


def test_map_repo_facade_writes_both_artifacts(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/a.py": "x = 1\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo(liveness=False)
    assert repo_map.content_fingerprint
    assert (tmp_path / ".devcouncil" / "graph" / "code_graph.json").is_file()
