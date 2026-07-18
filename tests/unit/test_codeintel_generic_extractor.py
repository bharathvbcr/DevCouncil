from __future__ import annotations

import sys
from types import SimpleNamespace

import devcouncil.codeintel.languages.workers as workers
from devcouncil.codeintel.languages.generic_extractor import (
    _embedded_regions,
    extract_generic,
)
from devcouncil.codeintel.languages.workers import ParserWorkerPool
from devcouncil.indexing.graph.build import _CODE_SUFFIXES


def test_code_suffixes_cover_broad_language_matrix() -> None:
    assert {".java", ".cs", ".cpp", ".swift", ".kt", ".svelte", ".astro", ".sol", ".tf", ".nix"} <= _CODE_SUFFIXES


def test_generic_extractor_uses_local_pack_without_download(monkeypatch) -> None:
    monkeypatch.setattr(
        "devcouncil.codeintel.languages.generic_extractor.process_tree_sitter",
        lambda language, source: {
            "structure": [{
                "name": "Worker",
                "kind": "Class",
                "start_line": 0,
                "end_line": 3,
                "decorators": [],
                "children": [{
                    "name": "run",
                    "kind": "Method",
                    "start_line": 1,
                    "end_line": 2,
                    "decorators": [],
                    "children": [],
                }],
            }],
            "imports": [{"source": 'import "pkg"', "items": ["Thing"], "alias": ""}],
            "exports": [{"name": "Worker"}],
            "calls": [{"name": "helper", "receiver": "", "line": 1}],
        },
    )

    result = extract_generic("Worker.java", "class Worker { void run() { helper(); } }")

    assert [(symbol.kind, symbol.qualname) for symbol in result.symbols] == [
        ("class", "Worker"),
        ("method", "Worker.run"),
    ]
    assert result.imports == ["pkg"]
    assert any(call.name == "helper" for call in result.calls)


def test_embedded_vue_script_uses_region_grammar_and_line_offsets(monkeypatch) -> None:
    def process(language: str, source: str):
        if language == "vue":
            return None
        if language == "html":
            assert "<div />" in source
            return {"structure": [], "imports": [], "exports": [], "calls": []}
        assert language == "typescript"
        assert "function handler" in source
        return {
            "structure": [{
                "name": "handler",
                "kind": "Function",
                "start_line": 1,
                "end_line": 1,
                "decorators": [],
                "children": [],
            }],
            "imports": [],
            "exports": [],
            "calls": [{"name": "run", "receiver": "svc", "line": 2}],
        }

    monkeypatch.setattr(
        "devcouncil.codeintel.languages.generic_extractor.process_tree_sitter",
        process,
    )
    result = extract_generic(
        "Component.vue",
        "<template><div /></template>\n"
        "<script lang='ts'>\n"
        "function handler() { svc.run(); }\n"
        "</script>\n",
    )

    assert result.symbols[0].line == 3
    assert result.calls[0].line == 3
    assert result.calls[0].receiver == "svc"


def test_embedded_style_template_frontmatter_and_liquid_offsets(monkeypatch) -> None:
    seen: list[tuple[str, str]] = []

    def process(language: str, source: str):
        seen.append((language, source.strip()))
        return {"structure": [], "imports": [], "exports": [], "calls": []}

    monkeypatch.setattr(
        "devcouncil.codeintel.languages.generic_extractor.process_tree_sitter",
        process,
    )
    extract_generic(
        "Component.svelte",
        "<script>run()</script>\n<style>\n.item {}\n</style>\n"
        "<template>\n<div>ready</div>\n</template>\n",
    )
    extract_generic(
        "Page.astro",
        "---\nconst value: number = 1\n---\n<style>.item {}</style>\n",
    )
    extract_generic(
        "page.liquid",
        "<main>{{ value }}</main>\n{% javascript %}run(){% endjavascript %}\n"
        "<style>.item {}</style>\n",
    )

    assert ("javascript", "run()") in seen
    assert ("css", ".item {}") in seen
    assert ("html", "<div>ready</div>") in seen
    assert ("typescript", "const value: number = 1") in seen
    assert ("html", "<main>{{ value }}</main>\n{% javascript %}run(){% endjavascript %}\n"
                    "<style>.item {}</style>") in seen


def test_embedded_region_source_offsets() -> None:
    svelte = (
        "<script>run()</script>\n<style>\n.item {}\n</style>\n"
        "<template>\n<div>ready</div>\n</template>\n"
    )
    astro = "---\nconst value: number = 1\n---\n<div>ready</div>\n"
    liquid = "<main>{{ value }}</main>\n"

    assert ("css", "\n.item {}\n", 1) in _embedded_regions("Svelte", svelte)
    assert ("html", "\n<div>ready</div>\n", 4) in _embedded_regions("Svelte", svelte)
    assert ("typescript", "const value: number = 1", 1) in _embedded_regions("Astro", astro)
    assert ("html", "<div>ready</div>\n", 3) in _embedded_regions("Astro", astro)
    assert ("html", liquid, 0) in _embedded_regions("Liquid", liquid)


def test_tree_sitter_empty_call_rows_do_not_use_regex_fallback(monkeypatch) -> None:
    monkeypatch.setattr(
        "devcouncil.codeintel.languages.generic_extractor.process_tree_sitter",
        lambda language, source: {
            "structure": [],
            "imports": [],
            "exports": [],
            "calls": [],
        },
    )

    result = extract_generic("Worker.java", "class Worker { void run() { helper(); } }")

    assert result.calls == []
    assert result.references == []


def test_duplicate_rows_dedup_across_container_and_embedded_regions(monkeypatch) -> None:
    payload = {
        "structure": [{
            "name": "Worker",
            "kind": "Class",
            "start_line": 0,
            "end_line": 5,
            "decorators": [],
            "children": [],
        }],
        "imports": [],
        "exports": [],
        "calls": [
            {"name": "helper", "receiver": "", "line": 2},
            {"name": "helper", "receiver": "", "line": 2},
            {"name": "helper", "receiver": "svc", "line": 2},
        ],
    }
    monkeypatch.setattr(
        "devcouncil.codeintel.languages.generic_extractor.process_tree_sitter",
        lambda language, source: payload,
    )

    result = extract_generic("Worker.java", "class Worker {\n  helper();\n}\n")

    assert [(symbol.qualname, symbol.line) for symbol in result.symbols] == [("Worker", 1)]
    # Identical (name, line, receiver) rows collapse; a different receiver survives.
    assert [(call.name, call.receiver) for call in result.calls] == [
        ("helper", ""),
        ("helper", "svc"),
    ]


def test_call_rows_use_callee_hints_and_dedup(monkeypatch) -> None:
    """Native call rows carry callee-style hints (receiver.name / bare name).

    Owner-style hints (the call's enclosing symbol) made resolve_calls bind
    every bare in-function call back to its own caller as a self-loop, so the
    replaced ``_owner_lookup`` helper must stay gone from the call path.
    """
    monkeypatch.setattr(
        "devcouncil.codeintel.languages.generic_extractor.process_tree_sitter",
        lambda _language, _source: {
            "structure": [
                {
                    "name": "run",
                    "kind": "function",
                    "start_line": 0,
                    "end_line": 9,
                    "children": [],
                }
            ],
            "imports": [],
            "exports": [],
            "calls": [
                {"name": "helper", "receiver": "", "line": 3},
                {"name": "helper", "receiver": "", "line": 3},
                {"name": "send", "receiver": "svc", "line": 5},
            ],
        },
    )
    extracted = extract_generic("worker.c", "int run(void) { return helper(); }\n")
    assert [(c.name, c.qualname_hint) for c in extracted.calls] == [
        ("helper", "helper"),
        ("send", "svc.send"),
    ]


def test_parser_worker_process_is_bounded_and_handles_missing_grammar() -> None:
    pool = ParserWorkerPool(max_workers=1, timeout=10)
    try:
        assert pool.max_workers == 1
        assert pool.process("definitely-not-installed", "source") is None
    finally:
        pool.close()


def test_companion_activation_is_attempted_once_per_process(monkeypatch) -> None:
    calls: list[bool] = []
    companion = SimpleNamespace(
        activate=lambda: calls.append(True) or {"ok": True, "activated": True}
    )
    monkeypatch.setitem(sys.modules, "devcouncil_codeintel_grammars", companion)
    monkeypatch.setattr(workers, "_ACTIVATION_ATTEMPTED", False)
    monkeypatch.setattr(
        workers,
        "_ACTIVATION_STATUS",
        {"installed": False, "activated": False},
    )

    first = workers._activate_companion_once()
    second = workers._activate_companion_once()

    assert first == second
    assert first["activated"] is True
    assert calls == [True]


def _c_grammar_loadable() -> bool:
    try:
        import tree_sitter_language_pack as pack

        workers._activate_companion_once()
        pack.get_language("c")
        return True
    except Exception:
        return False


def test_fill_missing_structure_names_follows_declarator_chain() -> None:
    """C-family definitions arrive with name=None from the language pack; the
    fill pass must dig the identifier out of the declarator chain."""

    class Node:
        def __init__(self, type_, start=0, sb=0, eb=0, fields=None, children=()):
            self.type = type_
            self.start_point = (start, 0)
            self.start_byte = sb
            self.end_byte = eb
            self._fields = fields or {}
            self.children = list(children)

        def child_by_field_name(self, field):
            return self._fields.get(field)

    raw = b"int helper(int x) { return x; }"
    ident = Node("identifier", start=0, sb=4, eb=10)
    func_decl = Node("function_declarator", start=0, fields={"declarator": ident})
    func_def = Node("function_definition", start=0, fields={"declarator": func_decl})
    root = Node("translation_unit", start=0, children=[func_def])
    tree = SimpleNamespace(root_node=root)

    rows = [{
        "name": "",
        "kind": "Function",
        "start_line": 0,
        "end_line": 0,
        "decorators": [],
        "children": [],
    }]
    workers._fill_missing_structure_names(tree, raw, rows)
    assert rows[0]["name"] == "helper"


def test_c_extraction_end_to_end_with_real_grammar(monkeypatch) -> None:
    import pytest

    if not _c_grammar_loadable():
        pytest.skip("C grammar companion not installed")
    # In-process worker path (skip the spawn pool) through the full generic
    # extraction, so symbol names, kinds, and callee hints are all covered.
    monkeypatch.setattr(
        "devcouncil.codeintel.languages.generic_extractor.process_tree_sitter",
        workers._native_process,
    )
    source = (
        "int helper(int x) {\n"
        "    return x * 2;\n"
        "}\n"
        "\n"
        "int main(void) {\n"
        "    return helper(21);\n"
        "}\n"
    )
    result = extract_generic("native/compute.c", source)
    symbols = {(s.name, s.kind) for s in result.symbols}
    assert ("helper", "function") in symbols
    assert ("main", "function") in symbols
    calls = [(c.name, c.qualname_hint) for c in result.calls]
    assert ("helper", "helper") in calls
