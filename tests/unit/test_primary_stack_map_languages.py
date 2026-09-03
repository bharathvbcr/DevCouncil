"""Primary-stack map language coverage: TS/Go/Py/Rust/Swift/Kotlin/Markdown/HTML."""

from __future__ import annotations

from devcouncil.codeintel.languages import (
    code_extensions,
    language_id_for_suffix,
    markup_extensions,
)
from devcouncil.execution.prompt_builder import PromptBuilder
from devcouncil.verification.gate_selector import _CODE_EXTS


_PRIMARY_CODE = {
    ".ts": "typescript",
    ".tsx": "typescript",
    ".go": "go",
    ".py": "python",
    ".rs": "rust",
    ".swift": "swift",
    ".kt": "kotlin",
    ".kts": "kotlin",
}

_PRIMARY_MARKUP = {
    ".md": "markdown",
    ".markdown": "markdown",
    ".html": "html",
    ".htm": "html",
}

_SAMPLE_FILES = [
    "src/app.ts",
    "src/App.tsx",
    "cmd/server/main.go",
    "src/pkg/main.py",
    "crates/core/src/lib.rs",
    "Sources/App/App.swift",
    "app/src/main/java/MainActivity.kt",
    "build.gradle.kts",
    "README.md",
    "docs/guide.markdown",
    "public/index.html",
    "legacy/page.htm",
]


def test_code_extensions_include_primary_code_stack():
    exts = code_extensions()
    for suffix in _PRIMARY_CODE:
        assert suffix in exts, f"{suffix} missing from code_extensions"
    # HTML/markdown must stay out of code_extensions (no junk subsystems).
    for suffix in _PRIMARY_MARKUP:
        assert suffix not in exts, f"{suffix} must not be in code_extensions"


def test_markup_extensions_include_markdown_html():
    markup = markup_extensions()
    for suffix in _PRIMARY_MARKUP:
        assert suffix in markup


def test_language_ids_for_primary_stack():
    for suffix, expected in _PRIMARY_CODE.items():
        assert language_id_for_suffix(suffix) == expected
    for suffix, expected in _PRIMARY_MARKUP.items():
        assert language_id_for_suffix(suffix) is None
        assert language_id_for_suffix(suffix, include_markup=True) == expected


def test_gate_selector_and_prompt_fences_primary_stack():
    for suffix in _PRIMARY_CODE:
        assert suffix in _CODE_EXTS
    assert ".html" not in _CODE_EXTS
    assert ".md" not in _CODE_EXTS

    assert PromptBuilder._lang_for("src/app.ts") == "typescript"
    assert PromptBuilder._lang_for("src/App.tsx") == "tsx"  # fence override
    assert PromptBuilder._lang_for("main.go") == "go"
    assert PromptBuilder._lang_for("main.py") == "python"
    assert PromptBuilder._lang_for("lib.rs") == "rust"
    assert PromptBuilder._lang_for("App.swift") == "swift"
    assert PromptBuilder._lang_for("Main.kt") == "kotlin"
    assert PromptBuilder._lang_for("README.md") == "markdown"
    assert PromptBuilder._lang_for("index.html") == "html"
    assert PromptBuilder._lang_for("page.htm") == "html"


def test_hook_refresh_uses_code_not_markup():
    """Hook map refresh stays on code_extensions; markup labels via full/header rebuild."""
    from devcouncil.codeintel.languages import code_extensions as exts

    assert ".swift" in exts()
    assert ".html" not in exts()
    assert ".md" not in exts()


def test_map_refresh_header_exts_include_markup():
    from devcouncil.codeintel.languages import code_extensions, markup_extensions

    header = code_extensions() | markup_extensions()
    assert ".html" in header
    assert ".md" in header
    assert ".go" in header
