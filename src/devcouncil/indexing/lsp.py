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
    """Language-server *detection* only — DevCouncil does not run an LSP client.

    This inspector exists to answer one honest question: which language servers
    for the repo's languages are installed on PATH? It does NOT spawn a server,
    does NOT speak the LSP wire protocol, and does NOT send the ``initialize``
    handshake. ``starter_initialize_payload`` builds the JSON-RPC ``initialize``
    request a *future* client would send, but it is never transmitted — it is
    surfaced purely as a reference/starter payload, clearly labelled as such, so
    no consumer mistakes detection for a live LSP capability.
    """

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

    def starter_initialize_payload(self, language: str) -> dict[str, Any]:
        """Build the JSON-RPC ``initialize`` request a future LSP client *would* send.

        This payload is NEVER sent — DevCouncil has no LSP client. It is provided
        only as a reference/starter for anyone wiring up real LSP support. See the
        ``initialize_requests`` block in :meth:`summary`, which carries the same
        non-sent payloads under an explicit ``"_note"`` disclaimer.
        """
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

    # Made explicit in every summary so no consumer reads this as a live LSP feature.
    _DETECTION_ONLY_NOTE = (
        "Detection only: DevCouncil checks which language servers are installed on "
        "PATH; it does not run an LSP client or send any requests."
    )
    _STARTER_PAYLOAD_NOTE = (
        "Starter payloads only — these initialize requests are NEVER sent. They are "
        "a reference for wiring up a real LSP client later."
    )

    def summary(self, files: list[str] | None = None) -> dict[str, Any]:
        candidates = self.server_candidates(files)
        return {
            "mode": "detection-only",
            "note": self._DETECTION_ONLY_NOTE,
            "languages": self.detect_languages(files),
            "servers_detected": [
                {
                    "language": candidate.language,
                    "command": candidate.command,
                    "available": candidate.available,
                    "reason": candidate.reason,
                }
                for candidate in candidates
            ],
            # Back-compat alias for "detected_servers" (older consumers/tests).
            "detected_servers": [
                {
                    "language": candidate.language,
                    "command": candidate.command,
                    "available": candidate.available,
                    "reason": candidate.reason,
                }
                for candidate in candidates
            ],
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
                "_note": self._STARTER_PAYLOAD_NOTE,
                **{
                    language: self.starter_initialize_payload(language)
                    for language in sorted({candidate.language for candidate in candidates})
                },
            },
        }

    def summary_json(self, files: list[str] | None = None) -> str:
        return json.dumps(self.summary(files), indent=2)
