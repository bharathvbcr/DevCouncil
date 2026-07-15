"""Language manifests and Tree-sitter grammar diagnostics."""

from devcouncil.codeintel.languages.registry import (
    LANGUAGE_SPECS,
    LanguageSpec,
    detect_language,
    grammar_status,
    supported_languages,
)

__all__ = [
    "LANGUAGE_SPECS",
    "LanguageSpec",
    "detect_language",
    "grammar_status",
    "supported_languages",
]
