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
