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
import threading
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
# Whole-exchange ceiling for one socket request: connect, send, and receive
# must all finish inside this budget. The per-call socket timeout (2s) bounds a
# single recv; without an overall deadline a peer that trickles one byte at a
# time keeps every recv under its timeout while the exchange runs for hours
# (measured: a 1-byte/0.5s drip holds the client until MAX_RESPONSE_BYTES
# accumulates — months). When the deadline trips, the daemon is treated as
# unavailable and the request degrades to the CLI path.
RESPONSE_DEADLINE_SECONDS = 5.0
# After a transport-level failure (timeout, reset, frame breakage), skip this
# daemon endpoint for a bounded cool-down instead of paying the full deadline
# on every subsequent call. The endpoint is retried after the window so a
# recovered daemon is picked up without restarting the process.
SOCKET_COOLDOWN_SECONDS = 30.0
# Per-read socket timeout. Bounds one recv(2) call; the response deadline below
# bounds the exchange as a whole. Without the deadline, a server that dribbles
# one byte per timeout-1 forever keeps this loop alive for weeks — measured in
# stress (slowloris), where only MAX_RESPONSE_BYTES capped the loop.
SOCKET_TIMEOUT_SECONDS = 2.0
# Total wall clock allowed for one daemon exchange (connect + write + framed
# read). Every transport path must respect it.
RESPONSE_DEADLINE_SECONDS = 30.0
# After a transport-level failure (timeout, reset, deadline) the daemon side is
# skipped entirely for this long: subsequent calls fail over straight to the CLI
# instead of paying the socket stall again on every request.
TRANSPORT_COOLDOWN_SECONDS = 60.0
# After a CLI invocation times out, later CLI calls fail fast for this long so a
# hung binary costs its timeout once, not once per call.
CLI_COOLDOWN_SECONDS = 60.0
DAEMON_READINESS_SECONDS = 3.0
SERVE_PROBE_TIMEOUT_SECONDS = 10.0

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


@dataclass
class CloneReport:
    """Duplicate-code findings plus the coverage they were computed over.

    ``groups`` alone is not a readable answer: an empty list says the same thing
    for a tree with no duplication and for one where nothing could be signed.
    ``signed_symbols`` is the denominator that separates them, so it is carried
    beside the findings rather than left for the caller to go looking for.
    """

    groups: BudgetedResponse
    signed_symbols: int
    unsigned_symbols: int


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
        response_deadline_seconds: float = RESPONSE_DEADLINE_SECONDS,
    ):
        self.root_dir = pathlib.Path(root_dir or os.getcwd()).resolve()
        self.socket_path = socket_path or self._default_socket_path()
        self.db_path = db_path or DEFAULT_DB_PATH
        if not 0 < response_deadline_seconds <= 3600:
            raise DevMapClientError(
                f"response deadline must be within (0, 3600] seconds, got {response_deadline_seconds!r}"
            )
        self._response_deadline_seconds = float(response_deadline_seconds)
        self._binary_path: Optional[str] = None
        self._serve_capable: Optional[bool] = None
        self._spawn_attempted = False
        self._daemon_process: Optional[subprocess.Popen[bytes]] = None
        # Transport health, set by observed failures — never by assumption. A
        # cooldown that starts on a guess would route around a healthy daemon
        # for a minute for no reason; a check that did not run must not report
        # the same verdict as one that ran and failed.
        self._transport_unhealthy_until = 0.0
        self._cli_unhealthy_until = 0.0

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

    def _supports_serve(self, binary: str) -> bool:
        """Probe whether *binary* can actually host the IPC daemon.

        Version strings cannot answer this — every build of devmap reports
        `0.1.0`, so an outdated install is indistinguishable from a fresh one by
        name. The only honest evidence is asking the binary what it supports.
        A binary without `serve --socket` must never be spawned: the readiness
        poll would burn its full window on a process that can never listen, and
        the fallback CLI would then run the same stale kernel against a schema
        it may predate. Result is cached per instance; the probe runs at most
        once per client, only when a spawn is actually about to happen.
        """
        if self._serve_capable is not None:
            return self._serve_capable
        try:
            probe = subprocess.run(
                [binary, "serve", "--help"],
                capture_output=True,
                text=True,
                timeout=SERVE_PROBE_TIMEOUT_SECONDS,
            )
        except subprocess.TimeoutExpired:
            # A binary too wedged to answer --help inside 10s will hang real
            # commands the same way. Marking the CLI cooldown here turns the
            # measured 123s-per-call outdated-binary stall into one bounded
            # probe plus immediate fail-fast.
            self._cli_unhealthy_until = time.monotonic() + CLI_COOLDOWN_SECONDS
            self._serve_capable = False
            return False
        except (OSError, subprocess.SubprocessError):
            self._serve_capable = False
            return False
        self._serve_capable = probe.returncode == 0 and "--socket" in (
            probe.stdout or ""
        )
        return self._serve_capable

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
            client.settimeout(SOCKET_TIMEOUT_SECONDS)
            client.connect(self.socket_path)
            data_bytes = json.dumps(request, separators=(",", ":")).encode("utf-8") + b"\n"
            client.sendall(data_bytes)
            deadline = time.monotonic() + self._response_deadline_seconds
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
                # The per-recv timeout bounds one read; a server that dribbles
                # bytes slower than that timeout resets it forever. The total
                # deadline is what makes the exchange bounded, not the recv.
                if time.monotonic() > deadline:
                    raise TimeoutError("response deadline exceeded")
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
            self._transport_unhealthy_until = (
                time.monotonic() + TRANSPORT_COOLDOWN_SECONDS
            )
            return None
        finally:
            if client is not None:
                client.close()

    def _read_named_pipe_response(self, pipe: Any) -> bytes:
        """Read one newline-framed response from *pipe*, bounded by the deadline.

        A synchronous named-pipe read has no timeout of its own and can block
        forever on a wedged server. A daemon watchdog thread closes the handle
        at the deadline, which unblocks the read with an error; the belt-and-
        braces monotonic check covers servers that answer just often enough to
        keep the loop alive without ever finishing a frame.
        """
        deadline = time.monotonic() + self._response_deadline_seconds
        watchdog = threading.Timer(self._response_deadline_seconds, pipe.close)
        watchdog.daemon = True
        watchdog.start()
        response = bytearray()
        try:
            while b"\n" not in response:
                chunk = pipe.read(4096)
                if not chunk:
                    break
                response.extend(chunk)
                if len(response) > MAX_RESPONSE_BYTES:
                    raise DevMapClientError(
                        f"devmap daemon response exceeds {MAX_RESPONSE_BYTES} bytes"
                    )
                if time.monotonic() > deadline:
                    raise TimeoutError("response deadline exceeded")
        finally:
            watchdog.cancel()
        return bytes(response)

    def _send_named_pipe_request(self, payload: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        request = dict(payload)
        request["version"] = PROTOCOL_VERSION
        data = json.dumps(request, separators=(",", ":")).encode("utf-8") + b"\n"
        try:
            with open(self.socket_path, "r+b", buffering=0) as pipe:
                pipe.write(data)
                response = self._read_named_pipe_response(pipe)
        except FileNotFoundError:
            return None
        except (OSError, ValueError) as err:
            # ValueError covers reads unwound by the watchdog's close(); OSError
            # covers every other transport failure including the deadline.
            self._transport_unhealthy_until = (
                time.monotonic() + TRANSPORT_COOLDOWN_SECONDS
            )
            return None
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
        # An outdated binary must never host the IPC endpoint: it cannot serve,
        # so spawning it only burns the readiness window before falling through
        # anyway — and worse, a *stale* serve-capable binary would keep owning
        # the socket with old behavior. Capability, not the version string,
        # decides; every build reports the same version number. Checked after
        # the cheap db guard, so repositories with no store never pay for it.
        if not self._supports_serve(binary):
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
        deadline = time.monotonic() + DAEMON_READINESS_SECONDS
        while time.monotonic() < deadline:
            if self._daemon_process.poll() is not None:
                self._reap_daemon_process()
                return False
            response = self._send_socket_request({"cmd": "status"})
            if response is not None:
                # The answer may have come from a pre-existing daemon rather
                # than our child. If our own child already lost the bind race
                # and exited, reap it here instead of leaking it.
                if self._daemon_process.poll() is not None:
                    self._reap_daemon_process()
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

    def _run_cli_command(
        self,
        cmd_args: List[str],
        timeout: float = 120.0,
        *,
        fail_fast_on_timeout: bool = True,
    ) -> Dict[str, Any]:
        # A CLI that already timed out recently is presumed hung: fail fast
        # rather than paying the full timeout again on every call. Long-running
        # commands (build) opt out — one slow build must not poison the next.
        if time.monotonic() < self._cli_unhealthy_until:
            raise DevMapClientError(
                "devmap CLI timed out recently; failing fast for a cooldown before retrying"
            )
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
        except subprocess.TimeoutExpired as err:
            if fail_fast_on_timeout:
                self._cli_unhealthy_until = time.monotonic() + CLI_COOLDOWN_SECONDS
            raise DevMapClientError(f"devmap CLI invocation failed: {err}") from err
        except (OSError, json.JSONDecodeError) as err:
            raise DevMapClientError(f"devmap CLI invocation failed: {err}") from err

    def _request(self, payload: Dict[str, Any], cli_args: List[str], timeout: float = 120.0) -> Dict[str, Any]:
        if time.monotonic() >= self._transport_unhealthy_until:
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
        return self._run_cli_command(args, timeout=600.0, fail_fast_on_timeout=False)

    def search(self, query: str, limit: int = 2000, semantic: bool = False) -> BudgetedResponse:
        """Symbol search. ``semantic`` ranks by name similarity in the kernel.

        Both modes go to the same engine, so a caller does not have to know
        whether an embedding index was built — there isn't one. The kernel
        derives the ranking from the symbol names it already stores.
        """
        self._validate_query(query)
        self._validate_budget(limit)
        payload = {"cmd": "search", "query": query, "budget": limit}
        args = ["search", query, "--budget", str(limit)]
        if semantic:
            payload["semantic"] = True
            args.append("--semantic")
        resp = self._request(payload, args)
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

    def preview(
        self,
        file: str,
        content: str,
        budget: int = 2000,
        min_confidence: float = 0.5,
    ) -> Dict[str, Any]:
        """What an unsaved edit to ``file`` would do to the graph.

        Nothing is written: the buffer is parsed in memory and compared against
        the file currently on disk, and the index is consulted only for the
        caller graph.
        """
        self._validate_budget(budget)
        if not isinstance(file, str) or not file:
            raise DevMapClientError("preview requires a file path")
        if not isinstance(content, str):
            raise DevMapClientError("preview content must be a string")
        if (
            not isinstance(min_confidence, (int, float))
            or isinstance(min_confidence, bool)
            or not 0.0 <= float(min_confidence) <= 1.0
        ):
            raise DevMapClientError("preview min_confidence must be within [0, 1]")

        # The CLI fallback reads the buffer from stdin, which `_run_cli_command`
        # cannot supply — so this surface is socket-only, and says so rather
        # than silently shelling out to a command that would block on a tty.
        payload = {
            "cmd": "preview",
            "file": file,
            "content": content,
            "budget": budget,
            "min_confidence": float(min_confidence),
        }
        if time.monotonic() < self._transport_unhealthy_until:
            raise DevMapClientError("devmap transport is unavailable; preview needs the daemon")
        response = self._send_socket_request(payload)
        if response is None and self._start_daemon():
            response = self._send_socket_request(payload)
        if response is None:
            raise DevMapClientError("devmap daemon is unavailable; preview needs the daemon")
        return response

    def savings(self, query: Optional[str] = None, budget: int = 2000) -> Dict[str, Any]:
        """What the map cost against what reading the files would have.

        CLI-only: the figures come from sizing files on disk, which the daemon
        has no cheaper access to than a subprocess does, and there is no
        long-lived state to reuse.
        """
        self._validate_budget(budget)
        args = ["savings", "--budget", str(budget)]
        if query is not None:
            self._validate_query(query)
            args += ["--query", query]
        return self._run_cli_command(args)

    def workspace(self, action: List[str]) -> Dict[str, Any]:
        """Run a `devmap workspace` subcommand.

        CLI-only, deliberately: the registry is a file the command owns, and
        routing mutations through a long-lived daemon would put two writers on
        it for no gain.
        """
        if not action or not all(isinstance(part, str) for part in action):
            raise DevMapClientError("workspace requires a subcommand")
        return self._run_cli_command(["workspace", *action])

    def clones(
        self,
        budget: int = 2000,
        kind: Optional[str] = None,
        min_nodes: int = 0,
    ) -> CloneReport:
        """Duplicate symbol bodies in the latest generation.

        ``kind`` is ``exact`` (the same code) or ``structural`` (the same shape
        under renaming); ``None`` reports both. An unrecognised value is
        rejected here rather than sent, because a server that ignored it would
        return an unfiltered report the caller would read as filtered.
        """
        self._validate_budget(budget)
        if kind is not None and kind not in ("exact", "structural"):
            raise DevMapClientError(
                f"clone kind must be 'exact' or 'structural', got {kind!r}"
            )
        if not isinstance(min_nodes, int) or isinstance(min_nodes, bool) or min_nodes < 0:
            raise DevMapClientError("clone min_nodes must be a non-negative integer")

        payload: Dict[str, Any] = {"cmd": "clones", "budget": budget, "min_nodes": min_nodes}
        cli_args = ["clones", "--budget", str(budget), "--min-nodes", str(min_nodes)]
        if kind is not None:
            payload["kind"] = kind
            cli_args += ["--kind", kind]

        resp = self._request(payload, cli_args)
        if not isinstance(resp.get("groups"), dict):
            raise DevMapClientError("devmap clone response is missing a groups object")
        return CloneReport(
            groups=self._budgeted(resp["groups"], budget),
            signed_symbols=self._strict_nonnegative_int(
                resp.get("signed_symbols"), "clone signed_symbols"
            ),
            unsigned_symbols=self._strict_nonnegative_int(
                resp.get("unsigned_symbols"), "clone unsigned_symbols"
            ),
        )

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
