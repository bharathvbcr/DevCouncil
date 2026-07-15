"""Client/startup helper for the persistent local debug broker."""

from __future__ import annotations

import json
import socket
import subprocess
import sys
import time
from pathlib import Path
from typing import Any


class DebugBrokerClient:
    def __init__(self, root: Path):
        self.root = root.expanduser().resolve()
        self.state_path = self.root / ".devcouncil" / "codeintel" / "debug-broker.json"

    def ensure_started(self, *, timeout: float = 5.0) -> None:
        try:
            self.call("ping", timeout=0.5)
            return
        except Exception:
            pass
        try:
            self.state_path.unlink()
        except OSError:
            pass
        subprocess.Popen(
            [sys.executable, "-m", "devcouncil.codeintel.debug.broker", "--root", str(self.root)],
            cwd=self.root,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                self.call("ping", timeout=0.5)
                return
            except Exception:
                time.sleep(0.05)
        raise TimeoutError("debug broker did not start")

    def call(self, action: str, params: dict[str, Any] | None = None, *, timeout: float = 30.0) -> Any:
        state = json.loads(self.state_path.read_text(encoding="utf-8"))
        request = json.dumps({"token": state["token"], "action": action, "params": params or {}}) + "\n"
        with socket.create_connection((str(state["host"]), int(state["port"])), timeout=timeout) as sock:
            sock.sendall(request.encode("utf-8"))
            sock.settimeout(timeout)
            chunks = bytearray()
            while not chunks.endswith(b"\n"):
                chunk = sock.recv(65536)
                if not chunk:
                    break
                chunks.extend(chunk)
        response = json.loads(bytes(chunks).decode("utf-8"))
        if not response.get("ok"):
            raise RuntimeError(str(response.get("error") or "debug broker request failed"))
        return response.get("result")
