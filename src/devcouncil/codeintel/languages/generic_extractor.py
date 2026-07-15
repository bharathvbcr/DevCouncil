"""Generic Tree-sitter extraction used by the broad language matrix."""

from __future__ import annotations

import re
from typing import Any

from devcouncil.codeintel.languages.registry import detect_language
from devcouncil.codeintel.languages.workers import process_tree_sitter
from devcouncil.indexing.graph.extract_python import (
    ExtractedCall,
    ExtractedImport,
    ExtractedSymbol,
    FileExtraction,
)

_QUOTED = re.compile(r"['\"]([^'\"]+)['\"]")
_CALL = re.compile(r"(?<![\w$])(?:(?P<receiver>[A-Za-z_$][\w$.:]*)\s*[.>:]+\s*)?(?P<name>[A-Za-z_$][\w$]*)\s*\(")
_CALL_KEYWORDS = {"if", "for", "while", "switch", "catch", "return", "sizeof", "typeof", "match", "when"}


def extract_generic(path: str, source: str) -> FileExtraction:
    spec = detect_language(path)
    if spec is None:
        return FileExtraction(path=path, language="")
    output = FileExtraction(path=path, language=spec.grammar)
    result = process_tree_sitter(spec.grammar, source)
    if result is not None:
        _append_result(output, source, result)
    for language, region, line_offset in _embedded_regions(spec.name, source):
        embedded = process_tree_sitter(language, region)
        if embedded is not None:
            _append_result(output, region, embedded, line_offset=line_offset)
    return output


def _append_result(
    output: FileExtraction,
    source: str,
    result: dict[str, Any],
    *,
    line_offset: int = 0,
) -> None:
    """Merge one container or embedded Tree-sitter result into a file extraction."""

    exported_names = {
        str(item.get("name", "")).strip()
        for item in result.get("exports", [])
        if str(item.get("name", "")).strip()
    }

    def visit(item: dict[str, Any], prefix: str = "") -> None:
        name = str(item.get("name", "") or "").strip()
        kind = _kind(str(item.get("kind", "") or ""))
        if name and kind:
            qualname = f"{prefix}.{name}" if prefix else name
            symbol = ExtractedSymbol(
                kind=kind,
                name=name,
                qualname=qualname,
                line=int(item.get("start_line", 0)) + line_offset + 1,
                end_line=int(item.get("end_line", 0)) + line_offset + 1,
                decorators=[str(value) for value in item.get("decorators", [])],
                exported=name in exported_names or not name.startswith("_"),
            )
            if not any(
                existing.qualname == symbol.qualname and existing.line == symbol.line
                for existing in output.symbols
            ):
                output.symbols.append(symbol)
            child_prefix = qualname if kind in {"class", "interface", "struct", "trait", "enum"} else prefix
        else:
            child_prefix = prefix
        for child in item.get("children", []) or []:
            visit(child, child_prefix)

    for item in result.get("structure", []) or []:
        visit(item)

    for item in result.get("imports", []) or []:
        raw_source = str(item.get("source", "") or "")
        match = _QUOTED.search(raw_source)
        module = match.group(1) if match else raw_source.strip()
        if not module:
            continue
        if module not in output.imports:
            output.imports.append(module)
        names = [str(value) for value in item.get("items", []) or []]
        alias = str(item.get("alias", "") or "")
        output.import_details.append(ExtractedImport(
            module=module,
            names=names,
            alias_map={alias: module} if alias else {},
        ))
    output.all_exports = sorted(set(output.all_exports) | exported_names)

    symbol_spans = sorted(output.symbols, key=lambda symbol: (symbol.line, -(symbol.end_line - symbol.line)))
    if "calls" in result:
        for item in result.get("calls", []) or []:
            name = str(item.get("name", "") or "")
            if not name or name in _CALL_KEYWORDS:
                continue
            line_no = int(item.get("line", 0)) + line_offset
            owner = next(
                (
                    symbol.qualname
                    for symbol in reversed(symbol_spans)
                    if symbol.line <= line_no <= symbol.end_line
                ),
                "",
            )
            call = ExtractedCall(
                name=name,
                line=line_no,
                receiver=str(item.get("receiver", "") or ""),
                qualname_hint=owner,
            )
            if not any(
                existing.name == call.name
                and existing.line == call.line
                and existing.receiver == call.receiver
                for existing in output.calls
            ):
                output.calls.append(call)
                output.references.append(name)
        return

    # Compatibility for older companion packs that do not return call rows.
    for line_no, line in enumerate(source.splitlines(), start=line_offset + 1):
        owner = next(
            (symbol.qualname for symbol in reversed(symbol_spans) if symbol.line <= line_no <= symbol.end_line),
            "",
        )
        for match in _CALL.finditer(line):
            name = match.group("name")
            if name in _CALL_KEYWORDS:
                continue
            receiver = match.group("receiver") or ""
            output.calls.append(ExtractedCall(
                name=name,
                line=line_no,
                receiver=receiver,
                qualname_hint=owner,
            ))
            output.references.append(name)


def _embedded_regions(container: str, source: str) -> list[tuple[str, str, int]]:
    regions: list[tuple[str, str, int]] = []
    if container in {"Svelte", "Vue", "Astro", "Liquid"}:
        for match in re.finditer(
            r"<script\b(?P<attrs>[^>]*)>(?P<body>.*?)</script\s*>",
            source,
            re.IGNORECASE | re.DOTALL,
        ):
            attrs = match.group("attrs")
            language = (
                "typescript"
                if re.search(r"\blang\s*=\s*['\"](?:ts|typescript)['\"]", attrs, re.I)
                else "javascript"
            )
            regions.append((
                language,
                match.group("body"),
                source[:match.start("body")].count("\n"),
            ))
        for match in re.finditer(
            r"<style\b[^>]*>(?P<body>.*?)</style\s*>",
            source,
            re.IGNORECASE | re.DOTALL,
        ):
            regions.append((
                "css",
                match.group("body"),
                source[:match.start("body")].count("\n"),
            ))
    if container in {"Svelte", "Vue", "Astro"}:
        for match in re.finditer(
            r"<template\b[^>]*>(?P<body>.*?)</template\s*>",
            source,
            re.IGNORECASE | re.DOTALL,
        ):
            regions.append((
                "html",
                match.group("body"),
                source[:match.start("body")].count("\n"),
            ))
    if container == "Astro":
        frontmatter = re.match(r"^---\s*\n(?P<body>.*?)\n---(?:\s*\n|$)", source, re.DOTALL)
        if frontmatter is not None:
            regions.append((
                "typescript",
                frontmatter.group("body"),
                source[:frontmatter.start("body")].count("\n"),
            ))
            regions.append((
                "html",
                source[frontmatter.end():],
                source[:frontmatter.end()].count("\n"),
            ))
        else:
            regions.append(("html", source, 0))
    if container == "Svelte":
        regions.append(("html", source, 0))
    if container == "Liquid":
        for match in re.finditer(
            r"{%\s*javascript\s*%}(?P<body>.*?){%\s*endjavascript\s*%}",
            source,
            re.IGNORECASE | re.DOTALL,
        ):
            regions.append((
                "javascript",
                match.group("body"),
                source[:match.start("body")].count("\n"),
            ))
        regions.append(("html", source, 0))
    return regions


def _kind(raw: str) -> str:
    normalized = raw.lower().replace("_", "").replace(" ", "")
    if "method" in normalized or "constructor" in normalized:
        return "method"
    if "function" in normalized or "procedure" in normalized or normalized in {"func", "subroutine"}:
        return "function"
    if "interface" in normalized or "protocol" in normalized:
        return "interface"
    if "struct" in normalized or "record" in normalized:
        return "struct"
    if "trait" in normalized:
        return "trait"
    if "enum" in normalized:
        return "enum"
    if "class" in normalized or "object" in normalized or "module" in normalized:
        return "class"
    if "type" in normalized or "alias" in normalized:
        return "type"
    if "property" in normalized or "field" in normalized:
        return "property"
    if "variable" in normalized or "constant" in normalized:
        return "variable"
    return ""
