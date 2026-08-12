"""Language manifests and Tree-sitter grammar diagnostics."""

from devcouncil.codeintel.languages.registry import (
    LANGUAGE_SPECS,
    LanguageSpec,
    code_extensions,
    detect_language,
    grammar_status,
    language_id_for_path,
    language_id_for_suffix,
    markup_extensions,
    supported_languages,
)

__all__ = [
    "LANGUAGE_SPECS",
    "LanguageSpec",
    "code_extensions",
    "detect_language",
    "grammar_status",
    "language_id_for_path",
    "language_id_for_suffix",
    "markup_extensions",
    "supported_languages",
]
