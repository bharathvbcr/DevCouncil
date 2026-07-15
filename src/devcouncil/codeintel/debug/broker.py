"""Loopback JSON broker that keeps CLI debug sessions alive across invocations."""

from __future__ import annotations

import argparse
import json
import os
import secrets
import socketserver
from pathlib import Path
from typing import Any

from devcouncil.codeintel.debug.session import get_debug_manager


class BrokerServer(socketserver.ThreadingTCPServer):
    allow_reuse_address = False
    daemon_threads = True

    def __init__(self, root: Path, token: str):
        super().__init__(("127.0.0.1", 0), BrokerHandler)
        self.root = root
        self.token = token


class BrokerHandler(socketserver.StreamRequestHandler):
    def handle(self) -> None:
        try:
            request = json.loads(self.rfile.readline(4 * 1024 * 1024).decode("utf-8"))
            if request.get("token") != self.server.token:  # type: ignore[attr-defined]
                raise PermissionError("invalid debug broker token")
            result = dispatch(self.server.root, str(request.get("action", "")), dict(request.get("params") or {}))  # type: ignore[attr-defined]
            response = {"ok": True, "result": result}
        except Exception as exc:
            response = {"ok": False, "error": f"{type(exc).__name__}: {exc}"}
        self.wfile.write((json.dumps(response, default=str) + "\n").encode("utf-8"))


def dispatch(root: Path, action: str, params: dict[str, Any]) -> Any:
    manager = get_debug_manager()
    if action == "ping":
        return {"pid": os.getpid(), "sessions": manager.list()}
    if action == "start":
        session = manager.start(
            root,
            [str(value) for value in params["adapter_command"]],
            request=str(params.get("request", "launch")),
            arguments=dict(params.get("arguments") or {}),
            initial_breakpoints={
                str(source): [int(line) for line in lines]
                for source, lines in dict(params.get("initial_breakpoints") or {}).items()
            },
            timeout=float(params.get("timeout", 30.0)),
        )
        return session.as_dict()
    if action == "list":
        return manager.list()
    if action == "breakpoints":
        return manager.set_breakpoints(str(params["session_id"]), str(params["source"]), params.get("lines") or [])
    if action == "control":
        return manager.control(
            str(params["session_id"]),
            str(params["debug_action"]),
            thread_id=int(params["thread_id"]) if params.get("thread_id") is not None else None,
        )
    if action == "inspect":
        return manager.inspect(str(params["session_id"]), str(params["operation"]), dict(params.get("arguments") or {}))
    if action == "evaluate":
        return manager.evaluate(
            str(params["session_id"]),
            str(params["expression"]),
            frame_id=int(params["frame_id"]) if params.get("frame_id") is not None else None,
            allow_side_effects=bool(params.get("allow_side_effects")),
        )
    if action == "capture_stack":
        return manager.capture_stack(str(params["session_id"]), thread_id=int(params["thread_id"]))
    if action == "stop":
        manager.stop(str(params["session_id"]), terminate_debuggee=bool(params.get("terminate_debuggee", True)))
        return {"stopped": params["session_id"]}
    raise ValueError(f"unknown broker action: {action}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    ns = parser.parse_args()
    root = ns.root.expanduser().resolve()
    credential = secrets.token_urlsafe(32)
    server = BrokerServer(root, credential)
    state_path = root / ".devcouncil" / "codeintel" / "debug-broker.json"
    state_path.parent.mkdir(parents=True, exist_ok=True)
    state_path.write_text(json.dumps({
        "pid": os.getpid(),
        "host": "127.0.0.1",
        "port": server.server_address[1],
        "token": credential,
    }), encoding="utf-8")
    try:
        state_path.chmod(0o600)
    except OSError:
        pass
    try:
        server.serve_forever(poll_interval=0.25)
    finally:
        server.server_close()
        try:
            state_path.unlink()
        except OSError:
            pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
