"""Capability-negotiated Debug Adapter Protocol client."""

from __future__ import annotations

import json
import queue
import socket
import subprocess
import threading
import time
from dataclasses import dataclass
from typing import Any, BinaryIO, Sequence, cast


class DAPError(RuntimeError):
    pass


def encode_message(message: dict[str, Any]) -> bytes:
    body = json.dumps(message, separators=(",", ":")).encode("utf-8")
    return f"Content-Length: {len(body)}\r\n\r\n".encode("ascii") + body


def read_message(stream: BinaryIO) -> dict[str, Any] | None:
    headers: dict[str, str] = {}
    while True:
        line = stream.readline()
        if not line:
            return None
        if line in {b"\r\n", b"\n"}:
            break
        key, _, value = line.decode("ascii", errors="replace").partition(":")
        headers[key.strip().lower()] = value.strip()
    try:
        length = int(headers["content-length"])
    except (KeyError, ValueError) as exc:
        raise DAPError("DAP message is missing a valid Content-Length header") from exc
    body = stream.read(length)
    if len(body) != length:
        raise DAPError("DAP stream ended mid-message")
    parsed = json.loads(body.decode("utf-8"))
    if not isinstance(parsed, dict):
        raise DAPError("DAP message body must be an object")
    return parsed


@dataclass
class PendingRequest:
    sequence: int
    responses: "queue.Queue[dict[str, Any]]"


class DAPClient:
    def __init__(
        self,
        reader: BinaryIO,
        writer: BinaryIO,
        *,
        process: subprocess.Popen[bytes] | None = None,
        sock: socket.socket | None = None,
    ):
        self.reader = reader
        self.writer = writer
        self.process = process
        self.sock = sock
        self.capabilities: dict[str, Any] = {}
        self._sequence = 0
        self._write_lock = threading.Lock()
        self._pending: dict[int, queue.Queue[dict[str, Any]]] = {}
        self._events: "queue.Queue[dict[str, Any]]" = queue.Queue()
        self._closed = threading.Event()
        self._reader_thread = threading.Thread(target=self._read_loop, name="devcouncil-dap-reader", daemon=True)
        self._reader_thread.start()

    @classmethod
    def start_stdio(cls, command: Sequence[str], *, cwd: str | None = None) -> "DAPClient":
        process = subprocess.Popen(
            list(command),
            cwd=cwd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        if process.stdin is None or process.stdout is None:
            process.kill()
            raise DAPError("debug adapter did not expose stdio pipes")
        return cls(cast(BinaryIO, process.stdout), cast(BinaryIO, process.stdin), process=process)

    @classmethod
    def connect_tcp(cls, host: str, port: int, *, timeout: float = 10.0) -> "DAPClient":
        sock = socket.create_connection((host, port), timeout=timeout)
        reader = sock.makefile("rb")
        writer = sock.makefile("wb")
        return cls(reader, writer, sock=sock)

    def begin_request(self, command: str, arguments: dict[str, Any] | None = None) -> PendingRequest:
        with self._write_lock:
            self._sequence += 1
            sequence = self._sequence
            responses: "queue.Queue[dict[str, Any]]" = queue.Queue(maxsize=1)
            self._pending[sequence] = responses
            message = {
                "seq": sequence,
                "type": "request",
                "command": command,
                "arguments": arguments or {},
            }
            self.writer.write(encode_message(message))
            self.writer.flush()
        return PendingRequest(sequence, responses)

    def wait_response(self, pending: PendingRequest, *, timeout: float = 15.0) -> dict[str, Any]:
        try:
            response = pending.responses.get(timeout=timeout)
        except queue.Empty as exc:
            self._pending.pop(pending.sequence, None)
            raise TimeoutError(f"DAP request {pending.sequence} timed out") from exc
        if not response.get("success", False):
            message = response.get("message") or response.get("body", {}).get("error", {}).get("format")
            raise DAPError(str(message or "debug adapter rejected request"))
        return dict(response.get("body") or {})

    def request(self, command: str, arguments: dict[str, Any] | None = None, *, timeout: float = 15.0) -> dict[str, Any]:
        return self.wait_response(self.begin_request(command, arguments), timeout=timeout)

    def initialize(self, *, client_name: str = "DevCouncil", adapter_id: str = "devcouncil") -> dict[str, Any]:
        self.capabilities = self.request("initialize", {
            "clientID": "devcouncil",
            "clientName": client_name,
            "adapterID": adapter_id,
            "pathFormat": "path",
            "linesStartAt1": True,
            "columnsStartAt1": True,
            "supportsVariableType": True,
            "supportsVariablePaging": True,
            "supportsRunInTerminalRequest": False,
        })
        return self.capabilities

    def wait_event(
        self,
        event: str | None = None,
        *,
        timeout: float = 15.0,
        fail_on: PendingRequest | None = None,
    ) -> dict[str, Any]:
        deadline = time.monotonic() + timeout
        deadline_items: list[dict[str, Any]] = []
        while True:
            if fail_on is not None:
                try:
                    response = fail_on.responses.get_nowait()
                except queue.Empty:
                    pass
                else:
                    fail_on.responses.put(response)
                    if not response.get("success", False):
                        self.wait_response(fail_on, timeout=0)
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                for item in deadline_items:
                    self._events.put(item)
                raise TimeoutError(f"DAP event {event or '*'} timed out")
            try:
                message = self._events.get(timeout=min(0.05, remaining) if fail_on is not None else remaining)
            except queue.Empty as exc:
                if fail_on is not None:
                    continue
                for item in deadline_items:
                    self._events.put(item)
                raise TimeoutError(f"DAP event {event or '*'} timed out") from exc
            if event is None or message.get("event") == event:
                for item in deadline_items:
                    self._events.put(item)
                return message
            deadline_items.append(message)

    def evaluate(
        self,
        expression: str,
        *,
        frame_id: int | None = None,
        context: str = "repl",
        allow_side_effects: bool = False,
    ) -> dict[str, Any]:
        if not allow_side_effects:
            raise PermissionError("DAP evaluate may execute code; set allow_side_effects=True explicitly")
        arguments: dict[str, Any] = {"expression": expression, "context": context}
        if frame_id is not None:
            arguments["frameId"] = frame_id
        return self.request("evaluate", arguments)

    def close(self) -> None:
        if self._closed.is_set():
            return
        self._closed.set()
        if self.sock is not None:
            try:
                self.sock.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
        try:
            self.writer.close()
        except OSError:
            pass
        try:
            self.reader.close()
        except OSError:
            pass
        if self.sock is not None:
            self.sock.close()
        if self.process is not None and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.process.kill()

    def _read_loop(self) -> None:
        try:
            while not self._closed.is_set():
                message = read_message(self.reader)
                if message is None:
                    break
                message_type = message.get("type")
                if message_type == "response":
                    request_seq = int(message.get("request_seq", -1))
                    responses = self._pending.pop(request_seq, None)
                    if responses is not None:
                        responses.put(message)
                elif message_type == "event":
                    self._events.put(message)
                elif message_type == "request":
                    self._respond_unsupported(message)
        except Exception as exc:
            error = {
                "type": "response",
                "success": False,
                "message": f"DAP transport failed: {type(exc).__name__}: {exc}",
            }
            for responses in list(self._pending.values()):
                responses.put(error)
            self._pending.clear()
        finally:
            self._closed.set()

    def _respond_unsupported(self, request: dict[str, Any]) -> None:
        with self._write_lock:
            self._sequence += 1
            response = {
                "seq": self._sequence,
                "type": "response",
                "request_seq": request.get("seq"),
                "success": False,
                "command": request.get("command"),
                "message": "DevCouncil does not support this reverse request",
            }
            self.writer.write(encode_message(response))
            self.writer.flush()
