"""devmap_client.py — Thin Python client for devmap (Rust) IPC daemon / CLI.

Task 6.1: Typed request/response, socket/CLI transport, auto-spawn devmap serve.
No analysis logic exists in this file — all graph analysis, extraction, resolution,
and query budgeting are strictly owned by devmap (Rust).
"""

from dataclasses import dataclass, field
import errno
import hashlib
import json
import os
import pathlib
import socket
import subprocess
import time
from typing import Any, Dict, List, Optional, Union, cast


DEFAULT_SOCKET_PATH = "/tmp/devmap.sock"

# The Rust kernel's own store, deliberately NOT the Python index.
#
# This previously pointed at `.devcouncil/codeintel/index.sqlite`, which is the
# *Python* schema (`user_version = 2`, tables `node_payloads` / `aliases` /
# `unresolved_references`). The Rust store is at `user_version = 10` with a
# different table shape and fails closed on it — `devmap status` against that
# file returns "unsupported schema version 2".
#
# The effect was that every hybrid consumer's Rust path raised
# `DevMapClientError` and fell through to the Python fallback, on every call, in
# this repository. The hybrid migration was real code that had never once
# executed its primary path. Pointing at a distinct file lets the two stores
# coexist during the cutover instead of contending for one incompatible path.
#
# Fail-safe when absent: `_spawn_daemon` requires the file to exist and be
# non-empty, so a repository that has not run `devmap build` degrades to the
# Python fallback exactly as before.
DEFAULT_DB_PATH = ".devcouncil/codeintel/devmap.sqlite"
PROTOCOL_VERSION = 1
MAX_RESPONSE_BYTES = 16 * 1024 * 1024
MAX_QUERY_BYTES = 4 * 1024
MAX_TOKEN_BUDGET = 100_000
MAX_TRAVERSAL_DEPTH = 64

@dataclass
class DevMapStatus:
    generation_id: int
    pending_count: int
    node_count: int
    edge_count: int
    is_fresh: bool = True
    degraded_reason: Optional[str] = None
    quarantined_count: int = 0
    raw: Dict[str, Any] = field(default_factory=dict)


@dataclass
class BudgetedResponse:
    shown: int
    hidden: int
    total: int
    truncated: bool
    tokens_used: int
    items: List[Dict[str, Any]] = field(default_factory=list)
    resolution: Any = None


def resolution_unavailable_reason(resolution: Any) -> Optional[str]:
    """Return the unavailable reason when resolution is fail-closed (X14)."""
    if resolution is None or resolution == "Available":
        return None
    if isinstance(resolution, dict):
        unavailable = resolution.get("Unavailable")
        if isinstance(unavailable, dict):
            reason = unavailable.get("reason")
            return str(reason) if reason is not None else "resolution unavailable"
        if "Unavailable" in resolution:
            return str(resolution.get("Unavailable") or "resolution unavailable")
    if isinstance(resolution, str) and resolution != "Available":
        return resolution
    return None


def try_connect(
    root_dir: Optional[Union[str, pathlib.Path]] = None,
    *,
    socket_path: Optional[str] = None,
    db_path: Optional[str] = None,
) -> Optional["DevMapClient"]:
    """Return a live client when the Rust store can actually answer; else None.

    A successful `status` is *not* sufficient evidence of availability. `devmap
    status` against a path with no database **creates an empty store** and
    reports success — `generation_id: null`, zero nodes, zero edges. Accepting
    that hands every consumer a confident zero (no callers, no dead symbols, no
    trace) drawn from a store that was never built, instead of falling back to
    Python. A check that could not run must never report what a check that ran
    and passed reports.

    Two conditions gate availability, and both are about the store having
    something to say:

    - `generation_id` is non-zero only once a build has committed a generation.
    - `node_count` is non-zero only if that build actually indexed something. A
      generation over a directory the walker found no sources in commits
      successfully with zero nodes, and answering "no callers, no dead code"
      from it is a confident falsehood about a repository that may be full of
      code. There is no case where answering from an empty index beats falling
      back: if the repository really is empty, the fallback reports the same
      nothing.
    """
    client = DevMapClient(root_dir, socket_path=socket_path, db_path=db_path)
    try:
        status = client.status()
    except DevMapClientError:
        return None
    if not status.generation_id or status.node_count <= 0:
        return None
    return client


class DevMapClientError(Exception):
    """Raised when devmap server/CLI fails or returns an error."""


class DevMapClient:
    """Thin IPC/CLI client for devmap (Rust)."""

    def __init__(
        self,
        root_dir: Optional[Union[str, pathlib.Path]] = None,
        socket_path: Optional[str] = None,
        db_path: Optional[str] = None,
    ):
        self.root_dir = pathlib.Path(root_dir or os.getcwd()).resolve()
        self.socket_path = socket_path or self._default_socket_path()
        self.db_path = db_path or DEFAULT_DB_PATH
        self._binary_path: Optional[str] = None
        self._spawn_attempted = False
        self._daemon_process: Optional[subprocess.Popen[bytes]] = None

    def _default_socket_path(self) -> str:
        if os.name == "nt":
            digest = hashlib.sha256(str(self.root_dir).encode("utf-8")).hexdigest()[:16]
            return rf"\\.\pipe\devmap-{digest}"
        digest = hashlib.sha256(str(self.root_dir).encode("utf-8")).hexdigest()[:16]
        return f"/tmp/devmap-{digest}.sock"

    def _find_devmap_binary(self) -> str:
        if self._binary_path:
            return self._binary_path

        rust_port_dir = self.root_dir / "rust-port" / "target"
        for candidate in [
            rust_port_dir / "release" / "devmap",
            rust_port_dir / "debug" / "devmap",
        ]:
            if candidate.is_file() and os.access(candidate, os.X_OK):
                self._binary_path = str(candidate)
                return self._binary_path

        import shutil
        found = shutil.which("devmap")
        if found:
            self._binary_path = found
            return self._binary_path

        self._binary_path = "devmap"
        return self._binary_path

    @staticmethod
    def _strict_nonnegative_int(value: Any, field_name: str) -> int:
        if type(value) is not int or value < 0:
            raise DevMapClientError(
                f"devmap {field_name} must be a non-negative integer, got {value!r}"
            )
        return value

    @staticmethod
    def _validate_query(value: str, field_name: str = "query") -> None:
        if not isinstance(value, str):
            raise DevMapClientError(f"devmap {field_name} must be a string")
        if len(value.encode("utf-8")) > MAX_QUERY_BYTES:
            raise DevMapClientError(
                f"devmap {field_name} exceeds {MAX_QUERY_BYTES} UTF-8 bytes"
            )

    @staticmethod
    def _validate_budget(value: int) -> None:
        if type(value) is not int or not 0 <= value <= MAX_TOKEN_BUDGET:
            raise DevMapClientError(
                f"devmap budget must be an integer within [0, {MAX_TOKEN_BUDGET}]"
            )

    @staticmethod
    def _validate_depth(value: int) -> None:
        if type(value) is not int or not 0 <= value <= MAX_TRAVERSAL_DEPTH:
            raise DevMapClientError(
                f"devmap depth must be an integer within [0, {MAX_TRAVERSAL_DEPTH}]"
            )

    @staticmethod
    def _decode_json_object(raw: str, label: str) -> Dict[str, Any]:
        try:
            value = json.loads(raw)
        except (UnicodeDecodeError, json.JSONDecodeError) as err:
            raise DevMapClientError(f"devmap {label} is invalid JSON: {err}") from err
        if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
            raise DevMapClientError(f"devmap {label} must be a JSON object with string keys")
        return cast(Dict[str, Any], value)

    def _send_socket_request(self, payload: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        if os.name == "nt":
            return self._send_named_pipe_request(payload)
        if not os.path.exists(self.socket_path):
            return None
        request = dict(payload)
        request["version"] = PROTOCOL_VERSION
        client: Optional[socket.socket] = None
        try:
            client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            client.settimeout(2.0)
            client.connect(self.socket_path)
            data_bytes = json.dumps(request, separators=(",", ":")).encode("utf-8") + b"\n"
            client.sendall(data_bytes)
            response_bytes = b""
            while b"\n" not in response_bytes:
                chunk = client.recv(4096)
                if not chunk:
                    break
                response_bytes += chunk
                if len(response_bytes) > MAX_RESPONSE_BYTES:
                    raise DevMapClientError(
                        f"devmap daemon response exceeds {MAX_RESPONSE_BYTES} bytes"
                    )
            if not response_bytes:
                raise DevMapClientError("devmap daemon closed without a response")
            line, separator, trailing = response_bytes.partition(b"\n")
            if not separator:
                raise DevMapClientError("devmap daemon response was not newline-terminated")
            if trailing.strip():
                raise DevMapClientError("devmap daemon returned trailing protocol data")
            return self._decode_envelope(line)
        except OSError as err:
            if err.errno in {errno.ENOENT, errno.ECONNREFUSED}:
                return None
            raise DevMapClientError(f"devmap daemon transport failed: {err}") from err
        finally:
            if client is not None:
                client.close()

    def _send_named_pipe_request(self, payload: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        request = dict(payload)
        request["version"] = PROTOCOL_VERSION
        data = json.dumps(request, separators=(",", ":")).encode("utf-8") + b"\n"
        try:
            with open(self.socket_path, "r+b", buffering=0) as pipe:
                pipe.write(data)
                response = bytearray()
                while b"\n" not in response:
                    chunk = pipe.read(4096)
                    if not chunk:
                        break
                    response.extend(chunk)
                    if len(response) > MAX_RESPONSE_BYTES:
                        raise DevMapClientError(
                            f"devmap daemon response exceeds {MAX_RESPONSE_BYTES} bytes"
                        )
        except FileNotFoundError:
            return None
        except OSError as err:
            raise DevMapClientError(f"devmap named-pipe transport failed: {err}") from err
        if not response:
            raise DevMapClientError("devmap daemon closed without a response")
        line, separator, trailing = bytes(response).partition(b"\n")
        if not separator or trailing.strip():
            raise DevMapClientError("devmap daemon returned a malformed framed response")
        return self._decode_envelope(line)

    @staticmethod
    def _decode_envelope(line: bytes) -> Dict[str, Any]:
        try:
            envelope = json.loads(line.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as err:
            raise DevMapClientError(f"devmap daemon returned invalid JSON: {err}") from err
        if not isinstance(envelope, dict):
            raise DevMapClientError("devmap daemon envelope must be an object")
        if envelope.get("protocol_version") != PROTOCOL_VERSION:
            raise DevMapClientError(
                "devmap daemon protocol mismatch: "
                f"expected {PROTOCOL_VERSION}, got {envelope.get('protocol_version')!r}"
            )
        if envelope.get("ok") is not True:
            error = envelope.get("error")
            if isinstance(error, dict):
                code = error.get("code", "unknown")
                message = error.get("message", "unspecified daemon error")
                raise DevMapClientError(f"devmap daemon error [{code}]: {message}")
            raise DevMapClientError("devmap daemon returned an invalid error envelope")
        result = envelope.get("result")
        if not isinstance(result, dict):
            raise DevMapClientError("devmap daemon result must be an object")
        return result

    def _start_daemon(self) -> bool:
        if self._spawn_attempted:
            return False
        self._spawn_attempted = True
        binary = self._find_devmap_binary()
        db = self.root_dir / self.db_path
        if not db.is_file() or db.stat().st_size == 0:
            return False
        command = [
            binary,
            "--db",
            str(db),
            "serve",
            str(self.root_dir),
            "--socket",
            self.socket_path,
        ]
        try:
            self._daemon_process = subprocess.Popen(
                command,
                cwd=str(self.root_dir),
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                start_new_session=True,
            )
        except OSError:
            return False
        deadline = time.monotonic() + 3.0
        while time.monotonic() < deadline:
            if self._daemon_process.poll() is not None:
                self._reap_daemon_process()
                return False
            response = self._send_socket_request({"cmd": "status"})
            if response is not None:
                return True
            time.sleep(0.05)
        self._reap_daemon_process()
        return False

    def _reap_daemon_process(self) -> None:
        process = self._daemon_process
        if process is None:
            return
        try:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=1.0)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=1.0)
            else:
                process.wait(timeout=0)
        finally:
            self._daemon_process = None

    def _run_cli_command(self, cmd_args: List[str], timeout: float = 120.0) -> Dict[str, Any]:
        binary = self._find_devmap_binary()
        full_cmd = [
            binary,
            "--json",
            "--db",
            str(self.root_dir / self.db_path),
        ] + cmd_args
        try:
            res = subprocess.run(
                full_cmd,
                cwd=str(self.root_dir),
                capture_output=True,
                text=True,
                timeout=timeout,
            )
            if res.returncode != 0:
                raise DevMapClientError(
                    f"devmap CLI failed (code {res.returncode}): {res.stderr or res.stdout}"
                )
            return self._decode_json_object(res.stdout, "CLI response")
        except (OSError, subprocess.SubprocessError, json.JSONDecodeError) as err:
            raise DevMapClientError(f"devmap CLI invocation failed: {err}") from err

    def _request(self, payload: Dict[str, Any], cli_args: List[str], timeout: float = 120.0) -> Dict[str, Any]:
        response = self._send_socket_request(payload)
        if response is not None:
            return response
        if self._start_daemon():
            response = self._send_socket_request(payload)
            if response is not None:
                return response
        return self._run_cli_command(cli_args, timeout=timeout)

    def _budgeted(self, resp: Dict[str, Any], budget: int) -> BudgetedResponse:
        required = {"shown", "hidden", "total", "truncated", "tokens_used", "items"}
        missing = sorted(required.difference(resp))
        if missing:
            raise DevMapClientError(f"devmap response is missing fields: {', '.join(missing)}")
        shown = self._strict_nonnegative_int(resp["shown"], "response shown")
        hidden = self._strict_nonnegative_int(resp["hidden"], "response hidden")
        total = self._strict_nonnegative_int(resp["total"], "response total")
        tokens_used = self._strict_nonnegative_int(
            resp["tokens_used"], "response tokens_used"
        )
        if type(resp["truncated"]) is not bool:
            raise DevMapClientError("devmap response truncated must be a boolean")
        items = resp["items"]
        if not isinstance(items, list) or not all(isinstance(item, dict) for item in items):
            raise DevMapClientError("devmap response items must be a list of objects")
        if shown != len(items) or shown + hidden != total:
            raise DevMapClientError(
                "devmap response count invariant failed: "
                f"shown={shown} hidden={hidden} total={total} items={len(items)}"
            )
        if resp["truncated"] != (hidden > 0):
            raise DevMapClientError(
                "devmap response truncation invariant failed: "
                f"truncated={resp['truncated']!r} hidden={hidden}"
            )
        if tokens_used > budget:
            raise DevMapClientError(
                f"devmap response exceeded budget: used={tokens_used} budget={budget}"
            )
        return BudgetedResponse(
            shown=shown,
            hidden=hidden,
            total=total,
            truncated=resp["truncated"],
            tokens_used=tokens_used,
            items=list(items),
            resolution=resp.get("resolution"),
        )

    def status(self) -> DevMapStatus:
        req = {"cmd": "status"}
        resp = self._request(req, ["status"])
        required = {"generation_id", "pending_count", "node_count", "edge_count", "is_fresh"}
        missing = sorted(required.difference(resp))
        if missing:
            raise DevMapClientError(f"devmap status is missing fields: {', '.join(missing)}")
        generation = resp["generation_id"]
        if generation is None:
            generation_id = 0
        else:
            generation_id = self._strict_nonnegative_int(generation, "status generation_id")
        if type(resp["is_fresh"]) is not bool:
            raise DevMapClientError("devmap status is_fresh must be a boolean")
        degraded = resp.get("degraded_reason")
        if degraded is not None and not isinstance(degraded, str):
            raise DevMapClientError("devmap status degraded_reason must be a string or null")
        return DevMapStatus(
            generation_id=generation_id,
            pending_count=self._strict_nonnegative_int(
                resp["pending_count"], "status pending_count"
            ),
            node_count=self._strict_nonnegative_int(resp["node_count"], "status node_count"),
            edge_count=self._strict_nonnegative_int(resp["edge_count"], "status edge_count"),
            is_fresh=resp["is_fresh"],
            degraded_reason=(
                degraded if degraded is not None else None
            ),
            quarantined_count=self._strict_nonnegative_int(
                resp.get("quarantined_count", 0), "status quarantined_count"
            ),
            raw=resp,
        )

    def build(self, affected: Optional[List[str]] = None, deleted: Optional[List[str]] = None) -> Dict[str, Any]:
        args = ["build", str(self.root_dir)]
        if affected:
            args.extend(["--affected", ",".join(affected)])
        if deleted:
            args.extend(["--deleted", ",".join(deleted)])
        return self._run_cli_command(args, timeout=600.0)

    def search(self, query: str, limit: int = 2000) -> BudgetedResponse:
        self._validate_query(query)
        self._validate_budget(limit)
        resp = self._request(
            {"cmd": "search", "query": query, "budget": limit},
            ["search", query, "--budget", str(limit)],
        )
        return self._budgeted(resp, limit)

    def deps(self, target: str, depth: int = 1) -> BudgetedResponse:
        self._validate_query(target, "target")
        if depth != 1:
            raise DevMapClientError("devmap deps depth must be 1")
        budget = 2000
        resp = self._request(
            {"cmd": "deps", "target": target, "budget": budget},
            ["deps", target, "--budget", str(budget)],
        )
        return self._budgeted(resp, budget)

    def impact(self, target: str, depth: int = 3) -> BudgetedResponse:
        self._validate_query(target, "target")
        self._validate_depth(depth)
        budget = 2000
        resp = self._request(
            {"cmd": "impact", "target": target, "budget": budget, "depth": depth},
            ["impact", target, "--budget", str(budget), "--depth", str(depth)],
        )
        return self._budgeted(resp, budget)

    def trace(
        self, from_symbol: str, depth: int = 3, to_symbol: Optional[str] = None
    ) -> BudgetedResponse:
        budget = 2000
        self._validate_query(from_symbol, "trace source")
        self._validate_depth(depth)
        if to_symbol is not None:
            self._validate_query(to_symbol, "trace destination")
        payload: Dict[str, Any] = {
            "cmd": "trace",
            "from": from_symbol,
            "budget": budget,
            "depth": depth,
        }
        args = ["trace", from_symbol]
        if to_symbol is not None:
            payload["to"] = to_symbol
            args.append(to_symbol)
        args.extend(["--budget", str(budget), "--depth", str(depth)])
        resp = self._request(
            payload,
            args,
        )
        return self._budgeted(resp, budget)

    def dead_symbols(self, budget: int = 2000) -> BudgetedResponse:
        self._validate_budget(budget)
        resp = self._request(
            {"cmd": "dead", "budget": budget},
            ["dead", "--budget", str(budget)],
        )
        return self._budgeted(resp, budget)

    def manifest(self, write_path: Optional[pathlib.Path] = None) -> Dict[str, Any]:
        out = write_path or (self.root_dir / ".devcouncil" / "repo_map.json")
        self._run_cli_command(["manifest", str(self.root_dir), "--output", str(out)])
        if not out.is_file():
            raise DevMapClientError(f"devmap manifest was not created at {out}")
        try:
            raw_manifest = out.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as err:
            raise DevMapClientError(f"devmap manifest is unreadable: {err}") from err
        return self._decode_json_object(raw_manifest, "manifest")

    def read_repo_map(self) -> Dict[str, Any]:
        path = self.root_dir / ".devcouncil" / "repo_map.json"
        if path.is_file():
            try:
                raw_map = path.read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError) as err:
                raise DevMapClientError(f"devmap repo map is unreadable: {err}") from err
            return self._decode_json_object(raw_map, "repo map")
        return self.manifest()

    def semantic_snapshots(self, file_path: str = "", budget: int = 2000) -> BudgetedResponse:
        self._validate_query(file_path, "snapshot file")
        self._validate_budget(budget)
        resp = self._run_cli_command(["snapshots", file_path, "--budget", str(budget)])
        return self._budgeted(resp, budget)

    def is_map_stale(self) -> bool:
        st = self.status()
        return not st.is_fresh or st.pending_count > 0
