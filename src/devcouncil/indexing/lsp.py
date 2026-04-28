from __future__ import annotations

import json
import shutil
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class LspServerCandidate:
    language: str
    command: list[str]
    available: bool
    reason: str


class LspInspector:
    """Starter LSP integration focused on discovery and safe initialize payloads."""

    _LANGUAGE_SERVERS: dict[str, list[list[str]]] = {
        "python": [["pyright-langserver", "--stdio"], ["pylsp"]],
        "typescript": [["typescript-language-server", "--stdio"]],
        "javascript": [["typescript-language-server", "--stdio"]],
        "go": [["gopls"]],
        "rust": [["rust-analyzer"]],
    }

    _EXTENSIONS: dict[str, str] = {
        ".py": "python",
        ".ts": "typescript",
        ".tsx": "typescript",
        ".js": "javascript",
        ".jsx": "javascript",
        ".go": "go",
        ".rs": "rust",
    }
    _IGNORED_DIRS = {".git", ".devcouncil", "__pycache__", ".venv", "node_modules", "dist", "build", "target", "vendor"}

    def __init__(self, project_root: Path):
        self.project_root = project_root

    def _is_ignored_path(self, file: str) -> bool:
        return any(part in self._IGNORED_DIRS for part in Path(file).parts)

    def detect_languages(self, files: list[str] | None = None) -> list[str]:
        if files is None:
            try:
                discovered: list[str] = []
                for path in self.project_root.rglob("*"):
                    if path.is_file() and not any(part in self._IGNORED_DIRS for part in path.parts):
                        discovered.append(str(path.relative_to(self.project_root)))
                files = discovered
            except OSError:
                files = []
        languages = {
            self._EXTENSIONS[Path(file).suffix.lower()]
            for file in files
            if Path(file).suffix.lower() in self._EXTENSIONS and not self._is_ignored_path(file)
        }
        return sorted(languages)

    def server_candidates(self, files: list[str] | None = None) -> list[LspServerCandidate]:
        candidates: list[LspServerCandidate] = []
        for language in self.detect_languages(files):
            for command in self._LANGUAGE_SERVERS.get(language, []):
                executable = command[0]
                available = shutil.which(executable) is not None
                candidates.append(
                    LspServerCandidate(
                        language=language,
                        command=command,
                        available=available,
                        reason="found on PATH" if available else "not found on PATH",
                    )
                )
        return candidates

    def initialize_request(self, language: str) -> dict[str, Any]:
        return {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "processId": None,
                "rootUri": self.project_root.resolve().as_uri(),
                "capabilities": {
                    "textDocument": {
                        "publishDiagnostics": {"relatedInformation": True},
                        "definition": {"linkSupport": True},
                        "references": {},
                        "documentSymbol": {"hierarchicalDocumentSymbolSupport": True},
                    },
                    "workspace": {"symbol": {}},
                },
                "initializationOptions": {"language": language},
            },
        }

    def summary(self, files: list[str] | None = None) -> dict[str, Any]:
        candidates = self.server_candidates(files)
        return {
            "languages": self.detect_languages(files),
            "servers": [
                {
                    "language": candidate.language,
                    "command": candidate.command,
                    "available": candidate.available,
                    "reason": candidate.reason,
                }
                for candidate in candidates
            ],
            "initialize_requests": {
                language: self.initialize_request(language)
                for language in sorted({candidate.language for candidate in candidates})
            },
        }

    def summary_json(self, files: list[str] | None = None) -> str:
        return json.dumps(self.summary(files), indent=2)
