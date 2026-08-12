"""Explicit language/grammar manifest for the code-intelligence release."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class LanguageSpec:
    name: str
    grammar: str
    extensions: tuple[str, ...]
    embedded: tuple[str, ...] = ()


def _spec(name: str, grammar: str, extensions: str, *, embedded: tuple[str, ...] = ()) -> LanguageSpec:
    return LanguageSpec(name, grammar, tuple(extensions.split()), embedded)


LANGUAGE_SPECS: tuple[LanguageSpec, ...] = (
    _spec("TypeScript", "typescript", ".ts .mts .cts"),
    _spec("TSX", "tsx", ".tsx"),
    _spec("JavaScript", "javascript", ".js .jsx .mjs .cjs"),
    _spec("ArkTS", "typescript", ".ets"),
    _spec("Python", "python", ".py .pyi"),
    _spec("Go", "go", ".go"),
    _spec("Rust", "rust", ".rs"),
    _spec("Java", "java", ".java"),
    _spec("C#", "csharp", ".cs"),
    _spec("VB.NET", "vb", ".vb"),
    _spec("PHP", "php", ".php .phtml"),
    _spec("Ruby", "ruby", ".rb .rake"),
    _spec("C", "c", ".c .h"),
    _spec("C++", "cpp", ".cc .cpp .cxx .hh .hpp .hxx"),
    _spec("Objective-C", "objc", ".m .mm"),
    # Metal is a C++14-derived language; the language pack ships no separate
    # Metal grammar, so use its maintained C++ parser plus Metal augmenters.
    _spec("Metal", "cpp", ".metal"),
    _spec("CUDA", "cuda", ".cu .cuh"),
    _spec("Swift", "swift", ".swift"),
    _spec("Kotlin", "kotlin", ".kt .kts"),
    _spec("Scala", "scala", ".scala .sc"),
    _spec("Dart", "dart", ".dart"),
    _spec("Svelte", "svelte", ".svelte", embedded=("typescript", "javascript", "css", "html")),
    _spec("Vue", "vue", ".vue", embedded=("typescript", "javascript", "css", "html")),
    _spec("Astro", "astro", ".astro", embedded=("typescript", "javascript", "css", "html")),
    _spec("Liquid", "liquid", ".liquid", embedded=("html", "javascript", "css")),
    _spec("Pascal/Delphi", "pascal", ".pas .pp .dpr"),
    _spec("Lua", "lua", ".lua"),
    _spec("Luau", "luau", ".luau"),
    _spec("R", "r", ".r .R"),
    _spec("CFML", "cfml", ".cfm .cfc"),
    _spec("COBOL", "cobol", ".cob .cbl .cpy"),
    _spec("Erlang", "erlang", ".erl .hrl"),
    _spec("Solidity", "solidity", ".sol"),
    _spec("Terraform/OpenTofu", "hcl", ".tf .tfvars .hcl"),
    _spec("Nix", "nix", ".nix"),
)

_BY_EXTENSION = {
    extension.lower(): spec
    for spec in LANGUAGE_SPECS
    for extension in spec.extensions
}

# Overlay for markup/config/shell suffixes that are not LANGUAGE_SPECS code
# languages but still need stable map/fence language ids.
#
# HTML intentionally lives here — not in LANGUAGE_SPECS / code_extensions().
# Labeling .html/.htm in languages[] and files[].language is useful; putting
# them in code_extensions would promote static-asset trees (public/, dist/,
# email templates) into code subsystems via _primary_code_files. Markdown is
# the same class: first-class map labels via include_markup, never subsystem
# seeds.
_MARKUP_LANGUAGE_IDS: dict[str, str] = {
    ".md": "markdown",
    ".markdown": "markdown",
    ".html": "html",
    ".htm": "html",
    ".yaml": "yaml",
    ".yml": "yaml",
    ".toml": "toml",
    ".json": "json",
    ".sh": "shell",
    ".ps1": "powershell",
}

# Grammar ids that should coalesce to a sibling map/fence id.
_GRAMMAR_ALIASES: dict[str, str] = {
    "tsx": "typescript",
}


def supported_languages() -> list[str]:
    return [spec.name for spec in LANGUAGE_SPECS]


def detect_language(path: str | Path) -> LanguageSpec | None:
    return _BY_EXTENSION.get(Path(path).suffix.lower())


def code_extensions() -> frozenset[str]:
    """All source extensions declared in ``LANGUAGE_SPECS`` (lowercase)."""
    return frozenset(
        extension.lower()
        for spec in LANGUAGE_SPECS
        for extension in spec.extensions
    )


def markup_extensions() -> frozenset[str]:
    """Markup/config/shell suffixes from the overlay (not code subsystem seeds)."""
    return frozenset(_MARKUP_LANGUAGE_IDS)


def language_id_for_suffix(
    suffix: str,
    *,
    include_markup: bool = False,
) -> str | None:
    """Map a file suffix to a stable lowercase language id.

    Prefers ``LANGUAGE_SPECS`` grammar ids, coalescing ``tsx`` → ``typescript``.
    When ``include_markup`` is True, also resolves markdown/html/yaml/toml/json/shell.
    """
    key = suffix if suffix.startswith(".") else f".{suffix}"
    key = key.lower()
    spec = _BY_EXTENSION.get(key)
    if spec is not None:
        return _GRAMMAR_ALIASES.get(spec.grammar, spec.grammar)
    if include_markup:
        return _MARKUP_LANGUAGE_IDS.get(key)
    return None


def language_id_for_path(
    path: str | Path,
    *,
    include_markup: bool = False,
) -> str | None:
    """Convenience wrapper around :func:`language_id_for_suffix`."""
    return language_id_for_suffix(Path(path).suffix, include_markup=include_markup)


def grammar_status() -> dict[str, Any]:
    """Report local grammar availability without downloading anything."""

    companion: dict[str, Any] = {"installed": False}
    try:
        try:
            import devcouncil_codeintel_grammars as grammar_assets

            companion = {"installed": True, **grammar_assets.activate()}
        except ImportError:
            pass
        import tree_sitter_language_pack as pack

        available = set(pack.available_languages())
        pack_version = str(getattr(pack, "__version__", "unknown"))
        cache_dir = str(pack.cache_dir())
        error = ""
    except Exception as exc:
        available = set()
        pack_version = "unavailable"
        cache_dir = ""
        error = f"{type(exc).__name__}: {exc}"
    rows = []
    for spec in LANGUAGE_SPECS:
        required = (spec.grammar, *spec.embedded)
        missing = sorted(set(required) - available)
        rows.append({
            "language": spec.name,
            "grammar": spec.grammar,
            "required_grammars": list(required),
            "missing_grammars": missing,
            "available": not missing,
            "extensions": list(spec.extensions),
            "embedded": list(spec.embedded),
        })
    missing_required = sorted({
        grammar
        for spec in LANGUAGE_SPECS
        for grammar in (spec.grammar, *spec.embedded)
        if grammar not in available
    })
    companion_installed = bool(companion.get("installed"))
    companion_ok = bool(companion.get("ok"))
    if missing_required and not companion_installed:
        action = (
            "Install the platform-matched devcouncil-codeintel-grammars wheel; "
            "runtime grammar downloads are disabled."
        )
    elif missing_required and not companion_ok:
        action = (
            "Reinstall the platform-matched devcouncil-codeintel-grammars wheel; "
            "its manifest or checksums are invalid."
        )
    elif missing_required:
        action = (
            "The installed companion wheel is incomplete for this platform; "
            "install a wheel containing every required grammar."
        )
    else:
        action = ""
    return {
        "ok": all(row["available"] for row in rows),
        "pack_version": pack_version,
        "cache_dir": cache_dir,
        "available_count": sum(bool(row["available"]) for row in rows),
        "required_count": len(rows),
        "languages": rows,
        "missing_grammars": missing_required,
        "action": action,
        "error": error,
        "downloaded_at_runtime": False,
        "companion": companion,
    }
