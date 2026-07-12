"""Minimal read-only LSP client (stdio JSON-RPC).

Spawns a detected language server from :class:`~devcouncil.indexing.lsp.LspInspector`
and speaks only ``initialize``, ``textDocument/references``, and
``textDocument/definition``. Off by default — enable via ``indexing.lsp_refs``
config or ``dev map --lsp-refs``. Hard timeouts and clean shutdown; never raises
to callers (methods return ``None`` / empty on failure).
"""

from __future__ import annotations

import json
import logging
import os
import subprocess
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Iterator
from urllib.parse import unquote, urlparse
from urllib.request import url2pathname

from devcouncil.indexing.lsp import LspInspector

logger = logging.getLogger(__name__)

# Default per-request / session budgets. Language servers can be slow to start;
# keep these tight so map/verify never hang when a server is wedged.
_DEFAULT_REQUEST_TIMEOUT = 12.0
_DEFAULT_SHUTDOWN_TIMEOUT = 3.0
_DEFAULT_INIT_TIMEOUT = 20.0


@dataclass(frozen=True)
class LspLocation:
    """A single LSP location (repo-relative path + 1-based line)."""

    path: str
    line: int  # 1-based
    character: int  # 0-based


def _uri_to_rel(project_root: Path, uri: str) -> str | None:
    """Convert a ``file://`` URI to a repo-relative POSIX path, or ``None``."""
    try:
        parsed = urlparse(uri)
        if parsed.scheme != "file":
            return None
        raw = url2pathname(unquote(parsed.path))
        # On Windows url2pathname may yield ``/C:/...``; Path handles it.
        abs_path = Path(raw)
        if os.name == "nt" and raw.startswith("/") and len(raw) > 2 and raw[2] == ":":
            abs_path = Path(raw[1:])
        rel = abs_path.resolve().relative_to(project_root.resolve())
        return rel.as_posix()
    except (OSError, ValueError):
        return None


def _path_to_uri(project_root: Path, rel: str) -> str:
    return (project_root / rel).resolve().as_uri()


def _symbol_position(source: str, line: int, name: str) -> tuple[int, int] | None:
    """Return 0-based ``(line, character)`` for ``name`` on 1-based ``line``."""
    lines = source.splitlines()
    if line < 1 or line > len(lines):
        return None
    text = lines[line - 1]
    col = text.find(name)
    if col < 0:
        # Fall back to first identifier-ish occurrence after ``def``/``class``/``export``.
        col = text.find(name.split(".")[-1]) if "." in name else -1
    if col < 0:
        return None
    return line - 1, col


def _parse_locations(project_root: Path, result: Any) -> list[LspLocation]:
    """Normalize ``Location | Location[] | LocationLink[] | null`` to locations."""
    if result is None:
        return []
    items = result if isinstance(result, list) else [result]
    out: list[LspLocation] = []
    for item in items:
        if not isinstance(item, dict):
            continue
        uri = item.get("uri") or (item.get("targetUri") if "targetUri" in item else None)
        rng = item.get("range") or item.get("targetSelectionRange") or item.get("targetRange")
        if not uri or not isinstance(rng, dict):
            continue
        start = rng.get("start") or {}
        if not isinstance(start, dict):
            continue
        rel = _uri_to_rel(project_root, str(uri))
        if rel is None:
            continue
        line = int(start.get("line", 0)) + 1
        character = int(start.get("character", 0))
        out.append(LspLocation(path=rel, line=line, character=character))
    return out


def language_for_path(path: str) -> str | None:
    return LspInspector._EXTENSIONS.get(Path(path).suffix.lower())


def first_available_command(language: str) -> list[str] | None:
    """First PATH-available command for ``language``, or ``None``."""
    for command in LspInspector._LANGUAGE_SERVERS.get(language, []):
        from shutil import which

        if which(command[0]) is not None:
            return list(command)
    return None


class LspClient:
    """One stdio language-server session (read-only: initialize / refs / definition)."""

    def __init__(
        self,
        project_root: Path,
        language: str,
        command: list[str],
        *,
        request_timeout: float = _DEFAULT_REQUEST_TIMEOUT,
        init_timeout: float = _DEFAULT_INIT_TIMEOUT,
    ) -> None:
        self.project_root = project_root.resolve()
        self.language = language
        self.command = list(command)
        self.request_timeout = request_timeout
        self.init_timeout = init_timeout
        self._proc: subprocess.Popen[bytes] | None = None
        self._reader: threading.Thread | None = None
        self._lock = threading.Lock()
        self._next_id = 1
        self._pending: dict[int, dict[str, Any]] = {}
        self._events: dict[int, threading.Event] = {}
        self._alive = False
        self._opened: set[str] = set()

    def start(self) -> bool:
        """Spawn the server and complete ``initialize``. Returns False on failure."""
        if self._alive:
            return True
        try:
            self._proc = subprocess.Popen(
                self.command,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                cwd=str(self.project_root),
                bufsize=0,
            )
        except OSError:
            logger.debug("Failed to spawn LSP %s", self.command, exc_info=True)
            self._proc = None
            return False
        self._reader = threading.Thread(target=self._read_loop, name=f"lsp-{self.language}", daemon=True)
        self._reader.start()
        try:
            init_result = self._request(
                "initialize",
                {
                    "processId": os.getpid(),
                    "rootUri": self.project_root.as_uri(),
                    "rootPath": str(self.project_root),
                    "capabilities": {
                        "textDocument": {
                            "definition": {"linkSupport": True},
                            "references": {},
                        },
                        "workspace": {},
                    },
                    "initializationOptions": {},
                    "trace": "off",
                },
                timeout=self.init_timeout,
            )
            if init_result is None and self._proc is not None and self._proc.poll() is not None:
                self.shutdown()
                return False
            self._notify("initialized", {})
            self._alive = True
            return True
        except Exception:
            logger.debug("LSP initialize failed for %s", self.language, exc_info=True)
            self.shutdown()
            return False

    def shutdown(self) -> None:
        """Best-effort ``shutdown`` / ``exit`` then terminate the process."""
        proc = self._proc
        self._alive = False
        if proc is None:
            return
        try:
            if proc.poll() is None:
                try:
                    self._request("shutdown", None, timeout=_DEFAULT_SHUTDOWN_TIMEOUT)
                except Exception:
                    pass
                try:
                    self._notify("exit", None)
                except Exception:
                    pass
                try:
                    proc.wait(timeout=_DEFAULT_SHUTDOWN_TIMEOUT)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    try:
                        proc.wait(timeout=1.0)
                    except subprocess.TimeoutExpired:
                        pass
        finally:
            self._proc = None
            self._opened.clear()
            with self._lock:
                for ev in self._events.values():
                    ev.set()
                self._pending.clear()
                self._events.clear()

    def __enter__(self) -> LspClient:
        if not self.start():
            raise RuntimeError(f"Failed to start LSP for {self.language}")
        return self

    def __exit__(self, *exc: object) -> None:
        self.shutdown()

    def references(
        self,
        rel_path: str,
        line: int,
        character: int,
        *,
        include_declaration: bool = False,
    ) -> list[LspLocation] | None:
        """``textDocument/references`` at 0-based position. ``None`` on failure."""
        if not self._ensure_open(rel_path):
            return None
        result = self._request(
            "textDocument/references",
            {
                "textDocument": {"uri": _path_to_uri(self.project_root, rel_path)},
                "position": {"line": line, "character": character},
                "context": {"includeDeclaration": include_declaration},
            },
        )
        if result is None and not self._alive:
            return None
        return _parse_locations(self.project_root, result)

    def _ensure_open(self, rel_path: str) -> bool:
        if not self._alive:
            return False
        norm = rel_path.replace("\\", "/")
        if norm in self._opened:
            return True
        abs_path = self.project_root / norm
        try:
            text = abs_path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            return False
        lang_id = {
            "python": "python",
            "typescript": "typescript",
            "javascript": "javascript",
            "go": "go",
            "rust": "rust",
        }.get(self.language, self.language)
        self._notify(
            "textDocument/didOpen",
            {
                "textDocument": {
                    "uri": _path_to_uri(self.project_root, norm),
                    "languageId": lang_id,
                    "version": 1,
                    "text": text,
                }
            },
        )
        self._opened.add(norm)
        return True

    def _read_loop(self) -> None:
        proc = self._proc
        if proc is None or proc.stdout is None:
            return
        stdout = proc.stdout
        try:
            while True:
                headers: dict[str, str] = {}
                while True:
                    line = stdout.readline()
                    if not line:
                        return
                    if line in (b"\r\n", b"\n"):
                        break
                    try:
                        key, _, value = line.decode("utf-8", errors="replace").partition(":")
                        headers[key.strip().lower()] = value.strip()
                    except Exception:
                        continue
                length_s = headers.get("content-length")
                if not length_s:
                    continue
                try:
                    length = int(length_s)
                except ValueError:
                    continue
                body = b""
                while len(body) < length:
                    chunk = stdout.read(length - len(body))
                    if not chunk:
                        return
                    body += chunk
                try:
                    message = json.loads(body.decode("utf-8"))
                except json.JSONDecodeError:
                    continue
                if not isinstance(message, dict):
                    continue
                msg_id = message.get("id")
                if msg_id is not None and ("result" in message or "error" in message):
                    with self._lock:
                        self._pending[int(msg_id)] = message
                        ev = self._events.get(int(msg_id))
                        if ev is not None:
                            ev.set()
                # Notifications / server requests are ignored (read-only client).
        except Exception:
            logger.debug("LSP read loop ended for %s", self.language, exc_info=True)

    def _write(self, payload: dict[str, Any]) -> None:
        proc = self._proc
        if proc is None or proc.stdin is None or proc.poll() is not None:
            raise RuntimeError("LSP process not running")
        raw = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        header = f"Content-Length: {len(raw)}\r\n\r\n".encode("ascii")
        proc.stdin.write(header + raw)
        proc.stdin.flush()

    def _notify(self, method: str, params: Any) -> None:
        msg: dict[str, Any] = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        with self._lock:
            self._write(msg)

    def _request(self, method: str, params: Any, *, timeout: float | None = None) -> Any:
        wait = self.request_timeout if timeout is None else timeout
        with self._lock:
            req_id = self._next_id
            self._next_id += 1
            event = threading.Event()
            self._events[req_id] = event
            msg: dict[str, Any] = {"jsonrpc": "2.0", "id": req_id, "method": method}
            if params is not None:
                msg["params"] = params
            try:
                self._write(msg)
            except Exception:
                self._events.pop(req_id, None)
                raise
        if not event.wait(timeout=wait):
            with self._lock:
                self._events.pop(req_id, None)
                self._pending.pop(req_id, None)
            logger.debug("LSP request %s timed out after %.1fs", method, wait)
            return None
        with self._lock:
            self._events.pop(req_id, None)
            response = self._pending.pop(req_id, None)
        if response is None:
            return None
        if "error" in response:
            logger.debug("LSP %s error: %s", method, response.get("error"))
            return None
        return response.get("result")


class LspSessionPool:
    """Lazy per-language :class:`LspClient` pool for one project root."""

    def __init__(
        self,
        project_root: Path,
        *,
        request_timeout: float = _DEFAULT_REQUEST_TIMEOUT,
        client_factory: Callable[[Path, str, list[str]], LspClient] | None = None,
    ) -> None:
        self.project_root = project_root.resolve()
        self.request_timeout = request_timeout
        self._clients: dict[str, LspClient] = {}
        self._failed: set[str] = set()
        self._factory = client_factory

    def close(self) -> None:
        for client in list(self._clients.values()):
            client.shutdown()
        self._clients.clear()
        self._failed.clear()

    def __enter__(self) -> LspSessionPool:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    def client_for(self, rel_path: str) -> LspClient | None:
        language = language_for_path(rel_path)
        if not language or language in self._failed:
            return None
        existing = self._clients.get(language)
        if existing is not None:
            return existing if existing._alive else None
        command = first_available_command(language)
        if command is None:
            self._failed.add(language)
            return None
        if self._factory is not None:
            client = self._factory(self.project_root, language, command)
        else:
            client = LspClient(
                self.project_root,
                language,
                command,
                request_timeout=self.request_timeout,
            )
        if not client.start():
            self._failed.add(language)
            return None
        self._clients[language] = client
        return client

    def confirm_unreferenced(self, rel_path: str, line: int, name: str) -> bool | None:
        """Confirm a token-scan dead symbol has zero external LSP references.

        Returns:
            ``True`` — LSP confirmed unreferenced (still dead).
            ``False`` — LSP found external references (not dead).
            ``None`` — no server / request failed (keep token-scan result).
        """
        client = self.client_for(rel_path)
        if client is None:
            return None
        abs_path = self.project_root / rel_path
        try:
            source = abs_path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            return None
        pos = _symbol_position(source, line, name)
        if pos is None:
            return None
        locs = client.references(rel_path, pos[0], pos[1], include_declaration=False)
        if locs is None:
            return None
        norm = rel_path.replace("\\", "/")
        for loc in locs:
            if loc.path.replace("\\", "/") != norm:
                return False
            # Same-file hit outside the defining line → a real use.
            if loc.line != line:
                return False
        return True

    def dependents_of_file(self, rel_path: str) -> list[str] | None:
        """Files that reference any top-level public symbol in ``rel_path``.

        ``None`` when no server is available or all queries fail.
        """
        client = self.client_for(rel_path)
        if client is None:
            return None
        abs_path = self.project_root / rel_path
        try:
            source = abs_path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            return None
        symbols = list(_iter_public_symbols(rel_path, source))
        if not symbols:
            return []
        norm = rel_path.replace("\\", "/")
        dependents: set[str] = set()
        any_ok = False
        for line, name in symbols:
            pos = _symbol_position(source, line, name)
            if pos is None:
                continue
            locs = client.references(rel_path, pos[0], pos[1], include_declaration=False)
            if locs is None:
                continue
            any_ok = True
            for loc in locs:
                other = loc.path.replace("\\", "/")
                if other != norm:
                    dependents.add(other)
        if not any_ok:
            return None
        return sorted(dependents)


def _iter_public_symbols(rel_path: str, source: str) -> Iterator[tuple[int, str]]:
    """Yield ``(1-based line, name)`` for public top-level defs (Python / JS-ish)."""
    suffix = Path(rel_path).suffix.lower()
    if suffix == ".py":
        try:
            import ast

            tree = ast.parse(source)
        except SyntaxError:
            return
        for node in tree.body:
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                name = node.name
                if name.startswith("_"):
                    continue
                yield int(getattr(node, "lineno", 1) or 1), name
        return
    if suffix in {".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"}:
        import re

        export_re = re.compile(
            r"(?m)^\s*export\s+(?:async\s+)?(?:function|class|const|let|var)\s+([A-Za-z_][A-Za-z0-9_]*)"
        )
        for m in export_re.finditer(source):
            name = m.group(1)
            if name.startswith("_"):
                continue
            line = source[: m.start()].count("\n") + 1
            yield line, name


def lsp_refs_enabled(project_root: Path) -> bool:
    """Read ``indexing.lsp_refs`` from config; default False. Never raises."""
    try:
        from devcouncil.app.config import load_config

        return bool(load_config(project_root).indexing.lsp_refs)
    except Exception:
        return False


def filter_dead_symbols_with_lsp(
    project_root: Path,
    candidates: list[str],
    *,
    pool: LspSessionPool | None = None,
) -> list[str]:
    """Drop token-scan dead symbols that LSP finds referenced.

    When LSP is unavailable for a candidate, keep it (token-scan stands).
    """
    if not candidates:
        return candidates
    own_pool = pool is None
    session = pool or LspSessionPool(project_root)
    try:
        kept: list[str] = []
        for entry in candidates:
            loc, _, name = entry.partition(" ")
            path, _, line_s = loc.rpartition(":")
            if not path or not name or not line_s.isdigit():
                kept.append(entry)
                continue
            confirmed = session.confirm_unreferenced(path.replace("\\", "/"), int(line_s), name.strip())
            if confirmed is False:
                continue
            kept.append(entry)
        return kept
    finally:
        if own_pool:
            session.close()
