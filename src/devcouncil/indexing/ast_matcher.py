from __future__ import annotations

import ast
import re
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class AstMatch:
    path: str
    language: str
    kind: str
    name: str
    line: int
    text: str
    engine: str

    def model_dump(self) -> dict[str, object]:
        return {
            "path": self.path,
            "language": self.language,
            "kind": self.kind,
            "name": self.name,
            "line": self.line,
            "text": self.text,
            "engine": self.engine,
        }


class AstMatcher:
    """Structural symbol search with tree-sitter when available, regex/AST fallbacks."""

    _EXT_LANGUAGE = {
        ".py": "python",
        ".ts": "typescript",
        ".tsx": "typescript",
        ".js": "javascript",
        ".jsx": "javascript",
        ".go": "go",
        ".rs": "rust",
    }

    _SYMBOL_PATTERNS: dict[str, re.Pattern[str]] = {
        "typescript": re.compile(
            r"^\s*(?:export\s+)?(?:(?:async\s+)?(?:function|class|interface|type)\s+|const\s+)([A-Za-z_$][\w$]*)"
        ),
        "javascript": re.compile(
            r"^\s*(?:export\s+)?(?:(?:async\s+)?(?:function|class)\s+|const\s+)([A-Za-z_$][\w$]*)"
        ),
        "go": re.compile(r"^\s*func\s+(?:\([^)]+\)\s*)?([A-Za-z_]\w*)\s*\("),
        "rust": re.compile(r"^\s*(?:pub\s+)?(?:async\s+)?(?:fn|struct|enum|trait)\s+([A-Za-z_]\w*)"),
    }
    _IGNORED_DIRS = {".git", ".devcouncil", "__pycache__", ".venv", "node_modules", "dist", "build", "target", "vendor"}

    def __init__(self, project_root: Path):
        self.project_root = project_root
        self.tree_sitter_available = self._has_tree_sitter()

    def _has_tree_sitter(self) -> bool:
        try:
            from devcouncil.indexing.ts_imports import tree_sitter_available

            return tree_sitter_available()
        except Exception:
            return False

    def match(
        self,
        *,
        query: str = "",
        language: str | None = None,
        kind: str | None = None,
        limit: int = 100,
        files: list[Path] | None = None,
    ) -> list[AstMatch]:
        language = language.lower() if language else None
        kind = kind.lower() if kind else None
        limit = max(1, limit)
        matches: list[AstMatch] = []
        for path in self._candidate_files(language, files):
            try:
                text = path.read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError):
                continue
            rel = path.relative_to(self.project_root).as_posix()
            file_language = self._EXT_LANGUAGE.get(path.suffix.lower(), path.suffix.lower().lstrip("."))
            matches.extend(self._match_file(rel, file_language, text, query=query, kind=kind))
            if len(matches) >= limit:
                return matches[:limit]
        return matches[:limit]

    def _candidate_files(self, language: str | None, files: list[Path] | None = None) -> list[Path]:
        allowed_exts = {
            ext for ext, ext_language in self._EXT_LANGUAGE.items()
            if language is None or ext_language == language
        }
        # When the caller already walked the tree (e.g. SemanticIndex.create_snapshot
        # shares one traversal across all collectors), filter that list in memory
        # instead of re-globbing. The filter is identical to the rglob path below.
        if files is not None:
            return sorted(
                path for path in files
                if path.is_file() and path.suffix.lower() in allowed_exts
                and not any(part in self._IGNORED_DIRS for part in path.parts)
            )
        candidates: list[Path] = []
        try:
            for path in self.project_root.rglob("*"):
                if not path.is_file() or path.suffix.lower() not in allowed_exts:
                    continue
                if any(part in self._IGNORED_DIRS for part in path.parts):
                    continue
                candidates.append(path)
        except OSError:
            return candidates
        return sorted(candidates)

    def _match_file(self, rel: str, language: str, text: str, *, query: str, kind: str | None) -> list[AstMatch]:
        if language == "python":
            return self._match_python(rel, text, query=query, kind=kind)
        if self.tree_sitter_available and language in self._SYMBOL_PATTERNS:
            ts_hits = self._match_tree_sitter(rel, language, text, query=query, kind=kind)
            if ts_hits is not None:
                return ts_hits
        return self._match_regex(rel, language, text, query=query, kind=kind)

    def _match_tree_sitter(
        self, rel: str, language: str, text: str, *, query: str, kind: str | None
    ) -> list[AstMatch] | None:
        try:
            from devcouncil.indexing.ts_imports import extract_symbols

            hits = extract_symbols(language, text)
        except Exception:
            return None
        if hits is None:
            return None
        results: list[AstMatch] = []
        q = query.lower() if query else ""
        for symbol_kind, symbol_name, lineno, source in hits:
            if kind and kind != symbol_kind:
                continue
            if q and q not in symbol_name.lower() and q not in source.lower():
                continue
            results.append(
                AstMatch(rel, language, symbol_kind, symbol_name, lineno, source, "tree-sitter")
            )
        return sorted(results, key=lambda item: (item.path, item.line))

    def _match_regex(
        self, rel: str, language: str, text: str, *, query: str, kind: str | None
    ) -> list[AstMatch]:
        pattern = self._SYMBOL_PATTERNS.get(language)
        if not pattern:
            return []
        results: list[AstMatch] = []
        for lineno, line in enumerate(text.splitlines(), start=1):
            match = pattern.match(line)
            if not match:
                continue
            symbol_name = match.group(1)
            symbol_kind = self._line_kind(line)
            if kind and kind != symbol_kind:
                continue
            if query and query.lower() not in symbol_name.lower() and query.lower() not in line.lower():
                continue
            results.append(
                AstMatch(rel, language, symbol_kind, symbol_name, lineno, line.strip(), self._engine())
            )
        return results

    def _match_python(self, rel: str, text: str, *, query: str, kind: str | None) -> list[AstMatch]:
        try:
            tree = ast.parse(text)
        except SyntaxError:
            return []
        lines = text.splitlines()
        results: list[AstMatch] = []
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                symbol_kind = "function"
            elif isinstance(node, ast.ClassDef):
                symbol_kind = "class"
            else:
                continue
            symbol_name = node.name
            source = lines[node.lineno - 1].strip() if 0 < node.lineno <= len(lines) else symbol_name
            if kind and kind != symbol_kind:
                continue
            if query and query.lower() not in symbol_name.lower() and query.lower() not in source.lower():
                continue
            results.append(AstMatch(rel, "python", symbol_kind, symbol_name, node.lineno, source, self._engine()))
        return sorted(results, key=lambda item: (item.path, item.line))

    def _line_kind(self, line: str) -> str:
        stripped = line.strip()
        if "class " in stripped:
            return "class"
        if stripped.startswith(("type ", "export type ")):
            return "type"
        if stripped.startswith(("interface ", "export interface ")):
            return "interface"
        if stripped.startswith(("struct ", "pub struct ")):
            return "struct"
        if stripped.startswith(("enum ", "pub enum ")):
            return "enum"
        if stripped.startswith(("trait ", "pub trait ")):
            return "trait"
        return "function"

    def _engine(self) -> str:
        # Python always uses the stdlib AST; other languages report tree-sitter when
        # the optional extra is installed even if a given file fell back to regex.
        return "tree-sitter-optional" if self.tree_sitter_available else "fallback-ast"
