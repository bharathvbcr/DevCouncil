"""Persistent in-process debug-session manager used by MCP and embedded CLI."""

from __future__ import annotations

import re
import threading
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Sequence, cast

from devcouncil.codeintel.debug.discovery import adapter_by_command
from devcouncil.codeintel.debug.fingerprint import build_fingerprint, executable_hash, source_fingerprint
from devcouncil.codeintel.debug.protocol import DAPClient
from devcouncil.codeintel.service import get_codeintel_service

_SECRET = re.compile(r"(?i)(token|secret|password|api[_-]?key|authorization)\s*[:=]\s*([^,;\s]+)")


def redact_value(value: str, *, limit: int = 4096) -> str:
    redacted = _SECRET.sub(lambda match: f"{match.group(1)}=[REDACTED]", value)
    return redacted if len(redacted) <= limit else redacted[:limit] + "…[truncated]"


@dataclass
class DebugSession:
    id: str
    root: Path
    client: DAPClient
    adapter_command: tuple[str, ...]
    adapter_id: str
    adapter_version: str
    adapter_requests: tuple[str, ...]
    request: str
    capabilities: dict[str, Any]
    source_fingerprint: str
    build_fingerprint: str
    executable_hash: str
    breakpoints: dict[str, list[int]] = field(default_factory=dict)

    def as_dict(self) -> dict[str, Any]:
        return {
            "id": self.id,
            "project_root": str(self.root),
            "adapter_command": list(self.adapter_command),
            "adapter_id": self.adapter_id,
            "adapter_version": self.adapter_version,
            "adapter_requests": list(self.adapter_requests),
            "request": self.request,
            "capabilities": self.capabilities,
            "source_fingerprint": self.source_fingerprint,
            "build_fingerprint": self.build_fingerprint,
            "executable_hash": self.executable_hash,
            "breakpoints": self.breakpoints,
        }


class DebugSessionManager:
    def __init__(self):
        self._sessions: dict[str, DebugSession] = {}
        self._lock = threading.Lock()

    def start(
        self,
        root: Path,
        adapter_command: Sequence[str],
        *,
        request: str,
        arguments: dict[str, Any],
        initial_breakpoints: dict[str, Sequence[int]] | None = None,
        timeout: float = 30.0,
    ) -> DebugSession:
        if request not in {"launch", "attach"}:
            raise ValueError("request must be launch or attach")
        root = root.expanduser().resolve()
        adapter = adapter_by_command(list(adapter_command))
        client = DAPClient.start_stdio(adapter_command, cwd=str(root))
        try:
            capabilities = client.initialize(adapter_id=adapter.id if adapter else "devcouncil")
            pending = client.begin_request(request, arguments)
            client.wait_event("initialized", timeout=timeout, fail_on=pending)
            normalized_breakpoints: dict[str, list[int]] = {}
            for source, lines in (initial_breakpoints or {}).items():
                normalized = str(Path(source).expanduser().resolve())
                normalized_breakpoints[normalized] = [int(line) for line in lines]
                client.request("setBreakpoints", {
                    "source": {"path": normalized},
                    "breakpoints": [{"line": line} for line in normalized_breakpoints[normalized]],
                    "sourceModified": False,
                }, timeout=timeout)
            client.request("configurationDone", timeout=timeout)
            client.wait_response(pending, timeout=timeout)
        except Exception:
            client.close()
            raise
        executable = adapter_command[0]
        session = DebugSession(
            id=uuid.uuid4().hex,
            root=root,
            client=client,
            adapter_command=tuple(adapter_command),
            adapter_id=adapter.id if adapter else "custom",
            adapter_version=adapter.version if adapter else "",
            adapter_requests=adapter.requests if adapter else (request,),
            request=request,
            capabilities=capabilities,
            source_fingerprint=source_fingerprint(root),
            build_fingerprint=build_fingerprint(root, executable),
            executable_hash=executable_hash(executable),
            breakpoints=normalized_breakpoints,
        )
        with self._lock:
            self._sessions[session.id] = session
        return session

    def get(self, session_id: str) -> DebugSession:
        with self._lock:
            session = self._sessions.get(session_id)
        if session is None:
            raise KeyError(f"unknown debug session: {session_id}")
        return session

    def list(self) -> list[dict[str, Any]]:
        with self._lock:
            return [session.as_dict() for session in self._sessions.values()]

    def set_breakpoints(self, session_id: str, source: str, lines: Sequence[int]) -> dict[str, Any]:
        session = self.get(session_id)
        normalized = str(Path(source).expanduser().resolve())
        body = session.client.request("setBreakpoints", {
            "source": {"path": normalized},
            "breakpoints": [{"line": int(line)} for line in lines],
            "sourceModified": False,
        })
        session.breakpoints[normalized] = [int(line) for line in lines]
        return body

    def control(self, session_id: str, action: str, *, thread_id: int | None = None) -> dict[str, Any]:
        session = self.get(session_id)
        commands = {
            "continue": "continue",
            "pause": "pause",
            "next": "next",
            "stepIn": "stepIn",
            "stepOut": "stepOut",
        }
        command = commands.get(action)
        if command is None:
            raise ValueError(f"unsupported debug action: {action}")
        arguments = {"threadId": thread_id} if thread_id is not None else {}
        return session.client.request(command, arguments)

    def inspect(self, session_id: str, operation: str, arguments: dict[str, Any]) -> dict[str, Any]:
        session = self.get(session_id)
        allowed = {"threads", "stackTrace", "scopes", "variables", "source", "disassemble"}
        if operation not in allowed:
            raise ValueError(f"unsupported inspect operation: {operation}")
        body = session.client.request(operation, arguments)
        return cast(dict[str, Any], self._redact(body))

    def evaluate(
        self,
        session_id: str,
        expression: str,
        *,
        frame_id: int | None,
        allow_side_effects: bool,
    ) -> dict[str, Any]:
        session = self.get(session_id)
        return cast(dict[str, Any], self._redact(session.client.evaluate(
            expression,
            frame_id=frame_id,
            allow_side_effects=allow_side_effects,
        )))

    def capture_stack(self, session_id: str, *, thread_id: int) -> dict[str, Any]:
        session = self.get(session_id)
        body = session.client.request("stackTrace", {"threadId": thread_id})
        frames = list(body.get("stackFrames") or [])
        observations = []
        for caller, callee in zip(reversed(frames[1:]), reversed(frames[:-1])):
            observations.append({
                "source": self._frame_id(caller),
                "target": self._frame_id(callee),
                "kind": "sampled_stack",
                "evidence": {"thread_id": thread_id, "provider": "dap-stack"},
            })
        store = get_codeintel_service(session.root).store
        runtime_id = store.start_runtime_session(
            provider="dap-stack",
            source_fingerprint=session.source_fingerprint,
            build_fingerprint=session.build_fingerprint,
            executable_hash=session.executable_hash,
            metadata={
                "debug_session": session.id,
                "thread_id": thread_id,
                "adapter_id": session.adapter_id,
                "adapter_version": session.adapter_version,
                "adapter_requests": list(session.adapter_requests),
                "adapter_capabilities": session.capabilities,
            },
        )
        store.add_runtime_observations(runtime_id, observations)
        store.end_runtime_session(runtime_id)
        return {"session_id": runtime_id, "observations": observations, "frames": self._redact(frames)}

    def stop(self, session_id: str, *, terminate_debuggee: bool = True) -> None:
        session = self.get(session_id)
        try:
            session.client.request("disconnect", {"terminateDebuggee": terminate_debuggee}, timeout=5.0)
        except Exception:
            pass
        session.client.close()
        with self._lock:
            self._sessions.pop(session_id, None)

    @classmethod
    def _redact(cls, value: Any) -> Any:
        if isinstance(value, str):
            return redact_value(value)
        if isinstance(value, list):
            return [cls._redact(item) for item in value]
        if isinstance(value, dict):
            return {key: cls._redact(item) for key, item in value.items()}
        return value

    @staticmethod
    def _frame_id(frame: dict[str, Any]) -> str:
        source = frame.get("source") or {}
        path = source.get("path") or source.get("name") or "<unknown>"
        return f"{path}:{frame.get('name', '<anonymous>')}:{frame.get('line', 0)}"


_MANAGER = DebugSessionManager()


def get_debug_manager() -> DebugSessionManager:
    return _MANAGER
