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
    """Language-server detection, with optional live client mode.

    By default this inspector only answers: which language servers for the repo's
    languages are installed on PATH? It does NOT spawn a server or speak the wire
    protocol. When ``client_enabled=True`` (config ``indexing.lsp_refs`` / ``dev map
    --lsp-refs``), summaries report ``mode: "client"`` — the optional
    :mod:`devcouncil.indexing.lsp_client` can then run ``initialize``,
    ``textDocument/references``, and ``textDocument/definition``.

    ``starter_initialize_payload`` remains a never-sent reference payload for
    detection-only mode.
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
        """Build the JSON-RPC ``initialize`` request a client would send.

        In detection-only mode this payload is NEVER sent — it is a reference for
        wiring. When ``mode: "client"``, :mod:`devcouncil.indexing.lsp_client`
        sends an equivalent initialize over stdio.
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

    _DETECTION_ONLY_NOTE = (
        "Detection only: DevCouncil checks which language servers are installed on "
        "PATH; it does not run an LSP client or send any requests. Enable live "
        "references via indexing.lsp_refs or `dev map --lsp-refs`."
    )
    _CLIENT_MODE_NOTE = (
        "Client mode: DevCouncil may spawn detected language servers for "
        "textDocument/references and textDocument/definition (dead-symbol "
        "confirmation and precise MCP impact). Still opt-in per run/config."
    )
    _STARTER_PAYLOAD_NOTE = (
        "Starter payloads only — these initialize requests are NEVER sent in "
        "detection-only mode. In client mode, lsp_client sends initialize live."
    )
    _CLIENT_PAYLOAD_NOTE = (
        "Reference initialize shapes; the live client sends equivalent requests "
        "when indexing.lsp_refs / --lsp-refs is enabled."
    )

    def summary(
        self,
        files: list[str] | None = None,
        *,
        client_enabled: bool = False,
    ) -> dict[str, Any]:
        candidates = self.server_candidates(files)
        mode = "client" if client_enabled else "detection-only"
        note = self._CLIENT_MODE_NOTE if client_enabled else self._DETECTION_ONLY_NOTE
        init_note = self._CLIENT_PAYLOAD_NOTE if client_enabled else self._STARTER_PAYLOAD_NOTE
        return {
            "mode": mode,
            "note": note,
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
                "_note": init_note,
                **{
                    language: self.starter_initialize_payload(language)
                    for language in sorted({candidate.language for candidate in candidates})
                },
            },
        }

    def summary_json(
        self,
        files: list[str] | None = None,
        *,
        client_enabled: bool = False,
    ) -> str:
        return json.dumps(self.summary(files, client_enabled=client_enabled), indent=2)
