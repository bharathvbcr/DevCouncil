"""SCC cuts: map_artifacts leaf, intel without Verifier, wiki path unification."""

from __future__ import annotations

import ast
import inspect
from pathlib import Path


def test_map_artifacts_does_not_import_cli():
    from devcouncil.indexing import map_artifacts

    src = Path(inspect.getfile(map_artifacts)).read_text(encoding="utf-8")
    tree = ast.parse(src)
    for node in ast.walk(tree):
        if isinstance(node, ast.ImportFrom) and node.module:
            assert not node.module.startswith("devcouncil.cli"), node.module
        if isinstance(node, ast.Import):
            for alias in node.names:
                assert not alias.name.startswith("devcouncil.cli"), alias.name


def test_map_refresh_imports_map_artifacts_not_cli_map():
    from devcouncil.indexing import map_refresh

    src = Path(inspect.getfile(map_refresh)).read_text(encoding="utf-8")
    assert "devcouncil.indexing.map_artifacts" in src
    assert "devcouncil.cli.commands.map" not in src


def test_intel_working_tree_uses_git_diff_fallback_not_verifier():
    from devcouncil.indexing.graph import intel

    src = Path(inspect.getfile(intel)).read_text(encoding="utf-8")
    assert "GitDiffFallback" in src
    assert "from devcouncil.verification.verifier import Verifier" not in src


def test_wiki_read_imports_knowledge_wiki_dir():
    from devcouncil.knowledge import wiki_read

    src = Path(inspect.getfile(wiki_read)).read_text(encoding="utf-8")
    assert "devcouncil.knowledge.wiki" in src
    assert "devcouncil.cli.commands.wiki" not in src


def test_wiki_refresh_imports_knowledge_wiki_dir():
    from devcouncil.verification import wiki_refresh

    src = Path(inspect.getfile(wiki_refresh)).read_text(encoding="utf-8")
    assert "devcouncil.knowledge.wiki" in src
    assert "wiki_dir_for" in src
    assert "devcouncil.cli.commands.wiki" not in src


def test_community_label_lives_in_communities_leaf():
    from devcouncil.indexing.graph.communities import community_label_for_area
    from devcouncil.indexing.graph.schema import CodeGraph, GraphNode, NodeKind

    graph = CodeGraph(
        nodes=[
            GraphNode(
                id="src/app/a.py",
                kind=NodeKind.FILE,
                name="a.py",
                path="src/app/a.py",
                community="auth-flow",
            ),
            GraphNode(
                id="src/app/b.py",
                kind=NodeKind.FILE,
                name="b.py",
                path="src/app/b.py",
                community="auth-flow",
            ),
        ],
        edges=[],
    )
    assert community_label_for_area(graph, "src/app") == "auth-flow"


def test_map_cli_lazily_imports_initialize_project():
    from devcouncil.cli.commands import map as map_cmd

    src = Path(inspect.getfile(map_cmd)).read_text(encoding="utf-8")
    tree = ast.parse(src)
    top_imports = []
    for node in tree.body:
        if isinstance(node, ast.ImportFrom) and node.module == "devcouncil.cli.commands.init":
            top_imports.append(node)
        if isinstance(node, ast.Import):
            for alias in node.names:
                if alias.name == "devcouncil.cli.commands.init":
                    top_imports.append(node)
    assert not top_imports, "initialize_project must be imported inside handlers, not at module top"
    assert "initialize_project" in src
