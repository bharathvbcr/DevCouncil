"""devmap_client.py — Thin Python client for devmap (Rust) IPC daemon / CLI.

Task 6.1: Typed request/response, socket/CLI transport, auto-spawn devmap serve.
No analysis logic exists in this file — all graph analysis, extraction, resolution,
and query budgeting are strictly owned by devmap (Rust).
"""

from dataclasses import dataclass, field
import errno
import json
import os
import pathlib
import socket
import subprocess
import threading
import time
from typing import Any, Dict, List, Optional, Union, cast


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
#: Mirrors ``devmap_query::Budget::EXPLORE``. ``explore`` funds four sections
#: from one number — definitions with source, both edge directions per
#: definition, and a blast radius — so it is deliberately larger than the 2000
#: every single-answer surface uses. Kept in sync by
#: ``test_explore_default_budget_matches_the_kernel``.
EXPLORE_BUDGET = 8_000
#: Mirrors ``devmap_query::Budget::AFFECTED``.
AFFECTED_BUDGET = 2_000
#: Mirrors ``devmap_serve::protocol::MAX_EXPLORE_LIMIT``. The kernel refuses a
#: larger limit rather than trimming it, so this is validated here to fail with
#: the local message instead of a round trip.
MAX_EXPLORE_LIMIT = 100
# Per-read socket timeout. Bounds one recv(2) call; the response deadline below
# bounds the exchange as a whole. Without the deadline, a server that dribbles
# one byte per timeout-1 forever keeps this loop alive for weeks — measured in
# stress (slowloris), where only MAX_RESPONSE_BYTES capped the loop.
SOCKET_TIMEOUT_SECONDS = 2.0
# Whole-exchange ceiling for one socket request: connect, send, and receive
# must all finish inside this budget. Without it a peer that trickles one byte
# at a time keeps every recv under its timeout while the exchange runs for
# hours (measured: a 1-byte/0.5s drip holds the client until MAX_RESPONSE_BYTES
# accumulates — months). When the deadline trips, the daemon is treated as
# unavailable and the request degrades to the CLI path. This constant used to
# be defined twice in this module, 5.0 and then 30.0; the second silently won.
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
#: Set to ``0`` to stop every client in the process from spawning a daemon.
AUTOSPAWN_ENV_VAR = "DEVMAP_AUTOSPAWN"


class Unreported:
    """The value of a status field this kernel never sent.

    ``None`` is already one of the kernel's own answers — ``coverage_gaps:
    null`` means "there was no store to inventory" — so "this binary predates
    the field" needs a value of its own. Folded together, a kernel that cannot
    list the files it refused would read exactly like one that read the whole
    corpus and refused none, which is the failure the inventory exists to end.

    Falsy, so ``if not status.coverage_gaps`` still means "nothing to show",
    and identity-compared (``is UNREPORTED``) everywhere it is tested.
    """

    __slots__ = ()

    def __repr__(self) -> str:  # pragma: no cover - debugging aid
        return "UNREPORTED"

    def __bool__(self) -> bool:
        return False


#: The single instance; compare with ``is``.
UNREPORTED = Unreported()


@dataclass
class DevMapStatus:
    generation_id: int
    pending_count: int
    node_count: int
    edge_count: int
    is_fresh: bool = True
    degraded_reason: Optional[str] = None
    quarantined_count: int = 0
    #: The three coverage-gap listings (``discovery_refused`` /
    #: ``parse_failed`` / ``pattern_recovered``), each ``{total, shown,
    #: truncated, paths}``: the paths behind the counts ``degraded_reason``
    #: states. ``None`` when the kernel read no store, ``UNREPORTED`` when the
    #: binary predates the listing.
    coverage_gaps: Union[Dict[str, Any], None, Unreported] = UNREPORTED
    #: ``"stored"`` when this generation's edge confidences carry the evidence
    #: the resolver recorded, ``"reconstructed"`` when they were re-derived for
    #: a generation written before that column existed.
    edge_resolution_source: Union[str, None, Unreported] = UNREPORTED
    #: Stored edges whose confidence contradicts the resolution kind recorded
    #: beside them. ``UNREPORTED`` from a kernel that does not check — which is
    #: "not measured", never 0.
    edge_confidence_mismatches: Union[int, None, Unreported] = UNREPORTED
    raw: Dict[str, Any] = field(default_factory=dict)


#: The graph shows a non-test caller.
REACHED = "reached"
#: The graph was read in full and holds no non-test caller.
UNREACHED = "unreached"
#: The check could not run. Never fold this into :data:`UNREACHED`.
REACH_UNKNOWN = "unknown"


@dataclass(frozen=True)
class SymbolReach:
    """Whether the graph reaches a symbol — in three states, not two.

    A boolean cannot carry this answer. ``False`` would have to mean both "the
    graph was read and nothing calls it" and "the graph could not be read", and
    those are the two readings this kernel exists to keep apart: the first is
    evidence a symbol is dead, the second is the absence of evidence, and a
    caller that treats them alike deletes live code.

    ``__bool__`` therefore raises. ``if reach:`` is exactly the mistake the type
    is here to prevent, and it must fail loudly at the call site rather than
    quietly resolve :data:`REACH_UNKNOWN` to falsy.
    """

    verdict: str
    #: Which caller reached it, or why the check could not run. Carried so a
    #: finding can say *what* was missing rather than only that something was.
    detail: str = ""

    def __bool__(self) -> bool:  # pragma: no cover - the raise is the contract
        raise TypeError(
            "SymbolReach has three states; compare `.verdict` against REACHED, "
            "UNREACHED or REACH_UNKNOWN rather than testing truthiness — "
            "`unknown` is not `unreached`"
        )


#: The resolution rungs the kernel names, strongest first.
#:
#: Mirrored here rather than imported because the kernel is another process, and
#: validated *before* the request goes out for the reason the kernel refuses a
#: typo rather than defaulting: a misspelled floor answered at full breadth is a
#: filtered answer a caller believes is narrow, which is worse than an error.
#: `test_the_client_rung_names_match_the_kernels` reads the label arms out of
#: `rung.rs` and pins the two lists against each other.
MIN_RUNG_NAMES = ("deterministic", "high", "speculative")


def _validated_min_rung(min_rung: Optional[str]) -> Optional[str]:
    """``None`` for no floor, or a name the kernel will accept."""
    if min_rung is None:
        return None
    if min_rung not in MIN_RUNG_NAMES:
        raise DevMapClientError(
            f"devmap min_rung must be one of {', '.join(MIN_RUNG_NAMES)}; got {min_rung!r}"
        )
    return min_rung


@dataclass
class BudgetedResponse:
    shown: int
    hidden: int
    total: int
    truncated: bool
    tokens_used: int
    items: List[Dict[str, Any]] = field(default_factory=list)
    resolution: Any = None
    #: Set when the *producer* of `items` stopped early — a depth or node cap —
    #: as distinct from the token budgeter trimming a complete set.
    #:
    #: These are different failures and only one of them fits the counters
    #: above: a walk that withheld an unknown quantity cannot be expressed in
    #: `shown`/`hidden`/`total` without breaking `shown + hidden == total`, which
    #: `_budgeted` enforces. The kernel has always sent this; the client dropped
    #: it, so a capped walk arrived here wearing the shape of a complete one.
    #: That is the common case rather than an exotic one — at the default depth
    #: of 1 the reverse walk routinely reports it with `truncated: false` and
    #: `hidden: 0`.
    walk_incomplete: Optional[str] = None
    #: Abandoned cycles found in the same generation, for a ``dead`` answer.
    #:
    #: ``None`` means the kernel sent none — the generation predates the
    #: component pass, or this is not a dead-code answer. An empty list means
    #: the pass ran and found none, which is a finding. Keeping the two apart is
    #: the whole reason this is ``Optional`` and not a defaulted list.
    #:
    #: The pass finds what a one-hop inbound-edge join structurally cannot: a
    #: subsystem whose functions call each other has an inbound edge on every
    #: symbol, so ``dead_code`` reports none of it. Until this field existed the
    #: kernel computed the answer and no Python consumer could see it.
    dead_clusters: Optional[List[Dict[str, Any]]] = None
    #: Components found and not listed, because the producer's cap cut them.
    #:
    #: Beside the list rather than folded into ``hidden``, which counts what the
    #: token budget trimmed. Different failures, and ``shown + hidden == total``
    #: is enforced against the second.
    dead_clusters_truncated: int = 0
    #: Why ``dead_clusters`` is ``None``, when the kernel ran the pass and
    #: refused it.
    #:
    #: The scan has three outcomes and an ``Optional[list]`` holds two. A call
    #: graph past the kernel's node ceiling comes back with an empty cluster
    #: list and a refusal flag; carrying it as ``[]`` would say *the pass ran
    #: and found no abandoned subsystems*, which is the reading that gets a
    #: subsystem kept. The kernel therefore sends no list at all and puts the
    #: reason here, so a consumer that predates this field fails closed on the
    #: absence rather than open on an empty finding.
    dead_clusters_incomplete: Optional[str] = None


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


def _norm_repo_path(path: str) -> str:
    """Repo-relative form: forward slashes, no leading ``./``.

    Matches ``devcouncil.indexing.wiring._norm`` exactly. Not imported from
    there because this module sits below ``indexing`` in the dependency order
    and must stay importable without it; the rule is two lines and frozen by
    ``tests/unit/test_dead_symbol_reach.py``'s
    ``test_the_hand_copied_normaliser_answers_what_wiring_answers``, which
    runs both over the same paths and asserts the answers agree.
    """
    normalized = path.replace("\\", "/")
    while normalized.startswith("./"):
        normalized = normalized[2:]
    return normalized


def _is_test_file(path: str) -> bool:
    """``wiring.is_test_path``, imported lazily.

    Lazy because ``devcouncil.indexing.wiring`` pulls in the indexing package
    and this module is imported by tools that have no reason to load it. A
    module that cannot be imported must not silently make every file look
    non-test — that would let a test-only caller clear a symbol — so the
    failure re-raises rather than defaulting.
    """
    from devcouncil.indexing.wiring import is_test_path

    return is_test_path(path)


def _positional(*values: str) -> List[str]:
    """Argv tail that the kernel's parser must read as values, never as flags.

    The CLI transport builds argv from caller-supplied strings, and clap reads
    a leading ``-`` as an option wherever one may appear: a search for ``--help``
    made the kernel print its usage text to stdout and exit 0, which this client
    then tried to decode as a JSON response, and a search for ``--db`` was read
    as the store-path option. Everything after ``--`` is a positional value, so
    the terminator goes here — which also means every option flag has to be
    appended *before* this tail, since clap will not accept one after it.

    This is a parser-confusion fix, not a shell-injection one: argv is a list
    and no shell is involved.
    """
    return ["--", *values]


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


def walk_incomplete_reason(response: Any) -> Optional[str]:
    """Return the producer's "I stopped looking" note for *response*, else None.

    The companion to :func:`resolution_unavailable_reason`, and it exists for
    the same reason: the fact that an answer is *not* a verified one has to be
    read the same way by every consumer, from one place.

    `BudgetedResponse.walk_incomplete` is set when the traversal itself stopped
    at a depth or node cap, which `shown`/`hidden`/`total` cannot express
    without breaking the `shown + hidden == total` invariant `_budgeted`
    enforces. The rule every consumer applies to it:

    * an **empty** rendered result plus a note is *unknown*, never `[]` — "I
      stopped looking" must not be published as "there is nothing";
    * a **non-empty** result keeps its rows, and still carries the note so the
      caller knows the list is a floor rather than a total.

    At the default depth of 1 the reverse walk reports this routinely, with
    `truncated: false` and `hidden: 0`, so this is the common case rather than
    an exotic one.
    """
    reason = getattr(response, "walk_incomplete", None)
    if reason is None:
        return None
    text = str(reason).strip()
    return text or None


#: Edge kinds that represent one symbol invoking another. The devmap store also
#: emits structural edges (`Contains` for file→symbol, `MemberOf` for
#: symbol→type, `Imports` for module→module); including those in a caller/callee
#: list makes a symbol look like it calls itself and inflates blast radius with
#: edges nobody can act on.
CALL_EDGE_KINDS = frozenset({"Calls"})


def edge_nodes(items: Any, symbol_key: str, file_key: str) -> List[str]:
    """Call-graph node names from one direction's raw edge list.

    Beside :func:`resolution_unavailable_reason` and
    :func:`walk_incomplete_reason` for the same reason they are here: it decides
    which edge kinds count as calls at all, and two copies of that filter would
    drift silently. `dev map query`'s batched and single-target paths and the
    task prompt's impact block all read one direction's edges the same way.
    """
    edges: List[str] = []
    for edge in items or []:
        if str(edge.get("edge_kind") or "") not in CALL_EDGE_KINDS:
            continue
        node = str(edge.get(symbol_key) or edge.get(file_key) or "")
        if node:
            edges.append(node)
    return edges


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


class DevMapUnsupportedCommand(Exception):
    """The daemon serving this repository does not implement the command.

    A daemon outlives the binary that started it — it retires after 30 idle
    minutes — so a client from a newer kernel routinely meets an older server.
    ``serde`` rejects an unrecognised ``cmd`` tag outright, and that arrives
    here as the daemon's ``invalid_request`` code: a fact about the *server*,
    not about the answer.

    It is raised rather than swallowed so :meth:`DevMapClient._request` can
    retry over the CLI, which is the **same kernel over a different
    transport** — not a second engine. Every other daemon error still surfaces
    as :class:`DevMapClientError`, because a query that genuinely failed must
    not be re-run somewhere else and reported as a success.
    """


class DevMapClientError(Exception):
    """Raised when devmap server/CLI fails or returns an error."""


class DevMapRequestRefused(DevMapClientError):
    """The *request* was refused before any transport was tried.

    A query over :data:`MAX_QUERY_BYTES`, an argument that is not valid UTF-8,
    a depth or budget outside its range: no engine can serve these, so the
    caller must not treat the refusal as "the kernel is unavailable" and
    re-run the request on the Python graph engine. Measured on this
    repository, a 200 KB ``dev map query`` was refused here in microseconds
    and then answered by that fallback in 3.9 s — a whole-graph load of 117k
    payloads scanned for a string no symbol can contain. A subclass of
    :class:`DevMapClientError` so every existing ``except`` keeps catching
    it; callers that fall back on the parent must catch this first.
    """


# The kernel's IPC identity, spelled once here and once in
# `devmap-serve/src/daemon.rs::ipc_identity_for`. `devmap serve
# --print-socket-path <root>` prints the kernel's answer and creates nothing, and
# a test compares the two. Before this the client hashed the root with SHA-256
# into `/tmp/devmap-<hex>.sock` while the kernel used FNV-1a into
# `<tmp>/devmap-<hex>/ipc.sock` — measured: two daemons per repository, one
# spawned by the client on its own path and one by `devmap serve`, each
# reconciling the same store.
FNV1A64_OFFSET_BASIS = 0xCBF29CE484222325
FNV1A64_PRIME = 0x100000001B3
_U64 = 0xFFFFFFFFFFFFFFFF


def fnv1a64(data: bytes) -> int:
    """FNV-1a, 64-bit: ``hash = (hash XOR byte) * prime`` per byte, wrapping."""
    digest = FNV1A64_OFFSET_BASIS
    for byte in data:
        digest = ((digest ^ byte) * FNV1A64_PRIME) & _U64
    return digest


def ipc_identity(root: Union[str, pathlib.Path]) -> str:
    """The 16-hex-digit identity the kernel derives for a repository root.

    Canonical root (symlinks resolved, ``.``/``..`` removed, absolute) as UTF-8
    with no trailing separator, hashed with FNV-1a 64 and rendered as lowercase
    zero-padded hex. A root that cannot be canonicalized -- it does not exist --
    is hashed as given, exactly as the kernel does: there is no daemon for a
    missing repository to collide with.
    """
    path = pathlib.Path(root)
    try:
        canonical = path.resolve(strict=True)
    except OSError:
        canonical = path if path.is_absolute() else pathlib.Path(os.getcwd()) / path
    return f"{fnv1a64(str(canonical).encode('utf-8')):016x}"


def _kernel_temp_dir() -> str:
    """Where Rust's ``std::env::temp_dir`` points on this platform, as a string.

    On Unix that is ``$TMPDIR`` when set, else ``/tmp`` -- *not* Python's
    ``tempfile.gettempdir()``, which also consults ``TEMP``/``TMP`` and falls
    through ``/var/tmp`` and ``/usr/tmp``; the two must agree on the same
    directory or the client connects to a socket the daemon never bound.
    """
    return os.environ.get("TMPDIR") or "/tmp"


def default_socket_path(root: Union[str, pathlib.Path]) -> str:
    """``<temp dir>/devmap-<identity>/ipc.sock``; a named pipe of the same identity on Windows."""
    identity = ipc_identity(root)
    if os.name == "nt":
        return "\\\\.\\pipe\\devmap-" + identity
    return os.path.join(_kernel_temp_dir(), f"devmap-{identity}", "ipc.sock")


class DevMapClient:
    """Thin IPC/CLI client for devmap (Rust)."""

    def __init__(
        self,
        root_dir: Optional[Union[str, pathlib.Path]] = None,
        socket_path: Optional[str] = None,
        db_path: Optional[str] = None,
        response_deadline_seconds: float = RESPONSE_DEADLINE_SECONDS,
        autospawn: bool = True,
    ):
        """``autospawn=False`` never starts a daemon: a request that finds no
        live socket goes straight to the CLI. A status probe after a build, or
        from `dev map status`, must not leave a 30-minute daemon behind as a
        side effect of asking a question — measured: a probe-spawned daemon
        reconciled the tree and committed a generation of its own seconds
        after the CLI build, so "one build, one generation" no longer held."""
        self.root_dir = pathlib.Path(root_dir or os.getcwd()).resolve()
        self.socket_path = socket_path or self._default_socket_path()
        self.db_path = db_path or DEFAULT_DB_PATH
        # `DEVMAP_AUTOSPAWN=0` disables spawning process-wide. Test suites set
        # it: measured, one unit-test run left 20 daemons behind, one per
        # temporary repository, each holding a store for its 30-minute idle
        # bound after the directory had been deleted.
        env_allows = os.environ.get(AUTOSPAWN_ENV_VAR, "1").strip().lower() not in {
            "0", "false", "no", "off",
        }
        self._autospawn = bool(autospawn) and env_allows
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
        return default_socket_path(self.root_dir)

    def _find_devmap_binary(self) -> str:
        """The kernel this client drives — by the engine's rule, not a second one.

        This used to search `<root_dir>/rust-port/target` and then `PATH` with
        no capability probe, while `devmap_engine.find_engine_binary` searched
        the *package* and probed. Two rules, two binaries: on any repository
        other than DevCouncil a query ran `~/.cargo/bin/devmap` while a build
        ran the packaged kernel, with no signal that the answers came from
        different code. One owner for the question now.

        The engine raises when nothing capable exists; this client must keep
        degrading (``try_connect`` returns ``None``), so the bare name is kept
        as the last resort — it fails on first use with a clear error rather
        than here, where callers expect no exception.
        """
        if self._binary_path:
            return self._binary_path
        from devcouncil.devmap_engine import DevMapEngineError, find_engine_binary

        try:
            self._binary_path = find_engine_binary(self.root_dir)
        except DevMapEngineError:
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
    def _optional_field(
        resp: Dict[str, Any],
        field_name: str,
        types: tuple[type, ...],
        expected: str,
    ) -> Any:
        """A status field a kernel may legitimately not send at all.

        Three outcomes, kept apart: absent is ``UNREPORTED`` (this binary
        predates the field), ``null`` stays ``None`` (the kernel looked and has
        nothing to report), and anything else must match *types* or the whole
        status is refused. Coercing a wrong type here would hand the doctor a
        value it would render as a finding.
        """
        if field_name not in resp:
            return UNREPORTED
        value = resp[field_name]
        if value is None or isinstance(value, types):
            return value
        raise DevMapClientError(
            f"devmap status {field_name} must be {expected}, got {value!r}"
        )

    @staticmethod
    def _optional_count(resp: Dict[str, Any], field_name: str) -> Union[int, None, Unreported]:
        """An optional non-negative integer, with ``bool`` refused like elsewhere."""
        if field_name not in resp:
            return UNREPORTED
        value = resp[field_name]
        if value is None:
            return None
        if type(value) is not int or value < 0:
            raise DevMapClientError(
                f"devmap status {field_name} must be a non-negative integer, got {value!r}"
            )
        return value

    @staticmethod
    def _validate_query(value: str, field_name: str = "query") -> None:
        if not isinstance(value, str):
            raise DevMapRequestRefused(f"devmap {field_name} must be a string")
        try:
            encoded = value.encode("utf-8")
        except UnicodeEncodeError as err:
            # `sys.argv` hands a CLI undecodable bytes as surrogate escapes;
            # they cannot be sent to the kernel as UTF-8, and before this the
            # `UnicodeEncodeError` itself was the answer — a traceback.
            raise DevMapRequestRefused(
                f"devmap {field_name} is not valid UTF-8 (byte {err.start})"
            ) from err
        if len(encoded) > MAX_QUERY_BYTES:
            raise DevMapRequestRefused(
                f"devmap {field_name} exceeds {MAX_QUERY_BYTES} UTF-8 bytes"
            )

    @staticmethod
    def _validate_budget(value: int) -> None:
        if type(value) is not int or not 0 <= value <= MAX_TOKEN_BUDGET:
            raise DevMapRequestRefused(
                f"devmap budget must be an integer within [0, {MAX_TOKEN_BUDGET}]"
            )

    @staticmethod
    def _validate_depth(value: int) -> None:
        if type(value) is not int or not 0 <= value <= MAX_TRAVERSAL_DEPTH:
            raise DevMapRequestRefused(
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
        except (OSError, ValueError):
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
                if code == "invalid_request":
                    # The frame did not deserialize into this daemon's request
                    # type. The client only ever sends well-formed requests for
                    # the protocol it was built against, so this means the two
                    # kernels differ — an older daemon that predates a command.
                    raise DevMapUnsupportedCommand(
                        f"devmap daemon rejected the request as unrecognised: {message}"
                    )
                raise DevMapClientError(f"devmap daemon error [{code}]: {message}")
            raise DevMapClientError("devmap daemon returned an invalid error envelope")
        result = envelope.get("result")
        if not isinstance(result, dict):
            raise DevMapClientError("devmap daemon result must be an object")
        return result

    def _start_daemon(self) -> bool:
        if self._spawn_attempted or not self._autospawn:
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
            try:
                response = self._send_socket_request(payload)
                if response is not None:
                    return response
                if self._start_daemon():
                    response = self._send_socket_request(payload)
                    if response is not None:
                        return response
            except DevMapUnsupportedCommand:
                # A daemon older than this client. The CLI below is the same
                # kernel reached another way, so the retry answers the question
                # that was asked rather than substituting a different engine's
                # answer for it.
                pass
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
        walk_incomplete = resp.get("walk_incomplete")
        if walk_incomplete is not None and not isinstance(walk_incomplete, str):
            raise DevMapClientError("devmap response walk_incomplete must be a string")
        # Validated, not merely copied. A cluster list is read as "delete this
        # subsystem", and a malformed one arriving as a silently-dropped `None`
        # would be indistinguishable from a generation that has no clusters —
        # the one distinction this field exists to carry.
        dead_clusters = resp.get("dead_clusters")
        if dead_clusters is not None:
            if not isinstance(dead_clusters, list) or not all(
                isinstance(cluster, dict) for cluster in dead_clusters
            ):
                raise DevMapClientError(
                    "devmap response dead_clusters must be a list of objects"
                )
            dead_clusters = list(dead_clusters)
        dead_clusters_truncated = self._strict_nonnegative_int(
            resp.get("dead_clusters_truncated", 0), "response dead_clusters_truncated"
        )
        dead_clusters_incomplete = resp.get("dead_clusters_incomplete")
        if dead_clusters_incomplete is not None:
            if not isinstance(dead_clusters_incomplete, str):
                raise DevMapClientError(
                    "devmap response dead_clusters_incomplete must be a string"
                )
            if dead_clusters is not None:
                # Both at once is the kernel claiming a scan it also says did
                # not run. Refusing beats picking one, because either choice
                # would be a guess presented as the kernel's answer.
                raise DevMapClientError(
                    "devmap response carries both dead_clusters and "
                    "dead_clusters_incomplete; a refused scan has no list"
                )
        return BudgetedResponse(
            shown=shown,
            hidden=hidden,
            total=total,
            truncated=resp["truncated"],
            tokens_used=tokens_used,
            items=list(items),
            resolution=resp.get("resolution"),
            walk_incomplete=walk_incomplete,
            dead_clusters=dead_clusters,
            dead_clusters_truncated=dead_clusters_truncated,
            dead_clusters_incomplete=dead_clusters_incomplete,
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
            # Optional: a kernel that predates any of these sends no key, and
            # the absence is carried through as `UNREPORTED` rather than
            # flattened into the `null` the kernel itself uses for "no store".
            coverage_gaps=self._optional_field(
                resp, "coverage_gaps", (dict,), "an object or null"
            ),
            edge_resolution_source=self._optional_field(
                resp, "edge_resolution_source", (str,), "a string or null"
            ),
            edge_confidence_mismatches=self._optional_count(
                resp, "edge_confidence_mismatches"
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
        args = ["search", "--budget", str(limit)]
        if semantic:
            payload["semantic"] = True
            args.append("--semantic")
        args += _positional(query)
        resp = self._request(payload, args)
        return self._budgeted(resp, limit)

    def deps(
        self, target: str, depth: int = 1, min_rung: Optional[str] = None
    ) -> BudgetedResponse:
        """Callees of ``target``, optionally floored at a resolution rung.

        ``min_rung`` is W2.3's point and it stopped at the kernel: the CLI and
        the IPC command have taken a floor since it landed, and no Python or MCP
        surface exposed one, so an agent asking for deterministic-only edges
        still could not. The histogram the kernel returns describes the
        population *before* the floor, so a narrowed answer stays readable as
        one.
        """
        self._validate_query(target, "target")
        if depth != 1:
            raise DevMapClientError("devmap deps depth must be 1")
        budget = 2000
        rung = _validated_min_rung(min_rung)
        payload: Dict[str, Any] = {"cmd": "deps", "target": target, "budget": budget}
        args = ["deps", "--budget", str(budget)]
        if rung is not None:
            payload["min_rung"] = rung
            args += ["--min-rung", rung]
        args += _positional(target)
        resp = self._request(payload, args)
        return self._budgeted(resp, budget)

    def neighbors(
        self,
        targets: List[str],
        depth: int = 1,
        min_confidence: float = 0.0,
        min_rung: Optional[str] = None,
    ) -> List[Dict[str, Any]]:
        """Callers and callees for several targets in one exchange.

        Composes what :meth:`impact` and :meth:`deps` answer per target, in the
        kernel rather than here. The point is transport, not analysis: this
        client falls back to a ``devmap`` subprocess whenever no daemon socket
        is live, so a five-definition view cost eleven process spawns and stayed
        at ~1.1 s however fast the store got. It now costs one.

        No target-count limit is duplicated here on purpose. The kernel owns the
        bound and *refuses* an over-long list rather than trimming it, so drift
        between the two surfaces shows up as a loud error instead of a short
        answer that reads as a complete one.

        Each direction is validated by :meth:`_budgeted` exactly as it would be
        on its own, so a composed answer cannot smuggle past the count and
        truncation invariants the separate calls enforce.

        ``min_rung`` is accepted for the same reason: this is the two queries
        composed, both of them take a floor, and a composition that drops a
        filter its parts accept answers a broader question than the caller
        asked while looking like it answered theirs.
        """
        if not isinstance(targets, list):
            raise DevMapClientError("devmap neighbors targets must be a list")
        for target in targets:
            self._validate_query(target, "target")
        rung = _validated_min_rung(min_rung)
        budget = 2000
        payload: Dict[str, Any] = {
            "cmd": "neighbors",
            "targets": list(targets),
            "budget": budget,
            "depth": depth,
            "min_confidence": min_confidence,
        }
        args = [
            "neighbors",
            "--budget",
            str(budget),
            "--depth",
            str(depth),
            "--min-confidence",
            str(min_confidence),
        ]
        if rung is not None:
            payload["min_rung"] = rung
            args += ["--min-rung", rung]
        # Positional last: `--min-rung` takes a value, and clap would read the
        # first target as it if the flag trailed the list.
        args += _positional(*targets)
        resp = self._request(payload, args)
        entries = resp.get("neighbors")
        if not isinstance(entries, list):
            raise DevMapClientError("devmap neighbors response must carry a list")
        if len(entries) != len(targets):
            raise DevMapClientError(
                "devmap neighbors returned "
                f"{len(entries)} entries for {len(targets)} targets"
            )
        answers: List[Dict[str, Any]] = []
        for entry, requested in zip(entries, targets):
            if not isinstance(entry, dict):
                raise DevMapClientError("devmap neighbors entry must be an object")
            # The kernel echoes the target back; if it ever stops matching, the
            # caller would silently attribute one symbol's callers to another.
            if entry.get("target") != requested:
                raise DevMapClientError(
                    "devmap neighbors answer is misaligned: asked for "
                    f"{requested!r}, got {entry.get('target')!r}"
                )
            for side in ("callers", "callees"):
                if not isinstance(entry.get(side), dict):
                    raise DevMapClientError(
                        f"devmap neighbors entry is missing {side}"
                    )
            answers.append({
                "target": requested,
                "callers": self._budgeted(entry["callers"], budget),
                "callees": self._budgeted(entry["callees"], budget),
            })
        return answers

    def impact(
        self, target: str, depth: int = 3, min_rung: Optional[str] = None
    ) -> BudgetedResponse:
        """Callers of ``target``, optionally floored at a resolution rung.

        See :meth:`deps` for why the floor is here.
        """
        self._validate_query(target, "target")
        self._validate_depth(depth)
        budget = 2000
        rung = _validated_min_rung(min_rung)
        payload: Dict[str, Any] = {
            "cmd": "impact",
            "target": target,
            "budget": budget,
            "depth": depth,
        }
        args = ["impact", "--budget", str(budget), "--depth", str(depth)]
        if rung is not None:
            payload["min_rung"] = rung
            args += ["--min-rung", rung]
        args += _positional(target)
        resp = self._request(payload, args)
        return self._budgeted(resp, budget)

    def trace(
        self,
        from_symbol: str,
        depth: int = 3,
        to_symbol: Optional[str] = None,
        min_rung: Optional[str] = None,
    ) -> BudgetedResponse:
        """A path between two symbols, optionally floored at a resolution rung.

        See :meth:`deps` for why the floor is here.
        """
        budget = 2000
        self._validate_query(from_symbol, "trace source")
        self._validate_depth(depth)
        if to_symbol is not None:
            self._validate_query(to_symbol, "trace destination")
        rung = _validated_min_rung(min_rung)
        payload: Dict[str, Any] = {
            "cmd": "trace",
            "from": from_symbol,
            "budget": budget,
            "depth": depth,
        }
        # Options first, then the terminator, then FROM (and TO) in order: clap
        # accepts no option after `--`, and the two endpoints are positional so
        # their order still carries meaning.
        args = ["trace", "--budget", str(budget), "--depth", str(depth)]
        if rung is not None:
            payload["min_rung"] = rung
            args += ["--min-rung", rung]
        if to_symbol is None:
            args += _positional(from_symbol)
        else:
            payload["to"] = to_symbol
            args += _positional(from_symbol, to_symbol)
        resp = self._request(
            payload,
            args,
        )
        return self._budgeted(resp, budget)

    def symbol_is_reached(self, path: str, name: str, *, depth: int = 1) -> SymbolReach:
        """Whether any non-test file calls ``name`` as defined in ``path``.

        A first-class query rather than an ``impact`` call each caller
        re-interprets. The inbound walk, the target matching, the test-path
        exclusion and — most of all — the distinction between "no caller" and
        "could not look" were previously reassembled at the call site, and the
        gate that did so folded a kernel outage into a `False` that read as
        "never referenced".

        Returns :data:`REACH_UNKNOWN` for every condition under which the
        question was not actually answered: the kernel could not be reached, the
        walk reported itself unavailable, or the walk stopped early. That last
        one matters and was previously invisible — a capped walk that found no
        caller has not established there is none.
        """
        self._validate_query(path, "path")
        self._validate_query(name, "name")
        try:
            resp = self.impact(path, depth=depth)
        except DevMapClientError as exc:
            return SymbolReach(REACH_UNKNOWN, f"kernel unavailable: {exc}")

        if isinstance(resp.resolution, dict) and "Unavailable" in resp.resolution:
            reason = resp.resolution.get("Unavailable")
            detail = ""
            if isinstance(reason, dict):
                detail = str(reason.get("reason") or "")
            return SymbolReach(REACH_UNKNOWN, detail or f"impact unavailable for {path}")

        wanted = _norm_repo_path(path)
        for item in resp.items:
            if _norm_repo_path(str(item.get("target_file") or "")) != wanted:
                continue
            target = str(item.get("target_symbol") or "")
            if target != name and not target.endswith(f"::{name}"):
                continue
            source = str(item.get("source_file") or "")
            if _is_test_file(source):
                continue
            return SymbolReach(REACHED, f"{source}::{item.get('source_symbol') or ''}")

        # Only now, and only if the walk actually finished. A depth cap or a
        # node cap that stopped short found no caller *in what it searched*,
        # which is not the same statement as "there is none" — and it is the
        # statement a delete-this verdict would be built on.
        if resp.walk_incomplete:
            return SymbolReach(
                REACH_UNKNOWN,
                f"the inbound walk stopped early: {resp.walk_incomplete}",
            )
        if resp.truncated:
            return SymbolReach(
                REACH_UNKNOWN,
                f"the inbound edge list was truncated at {resp.shown} of {resp.total}",
            )
        return SymbolReach(UNREACHED, f"no non-test caller among {resp.total} inbound edge(s)")

    def dead_symbols(self, budget: int = 2000) -> BudgetedResponse:
        self._validate_budget(budget)
        resp = self._request(
            {"cmd": "dead", "budget": budget},
            ["dead", "--budget", str(budget)],
        )
        return self._budgeted(resp, budget)

    def _validate_budgeted_sections(self, payload: Dict[str, Any], budget: int) -> None:
        """Run every budgeted section of a composed answer past :meth:`_budgeted`.

        A composed response is several responses in a trench coat, and each of
        them carries its own counters. Checking only the outer list would let an
        edge list arrive with ``shown + hidden != total`` — the exact invariant
        the separate ``impact``/``trace`` calls have always been held to — and a
        caller would read a broken count as a measured one.
        """
        definitions = payload.get("definitions")
        if isinstance(definitions, dict):
            checked = self._budgeted(definitions, budget)
            for item in checked.items:
                for side in ("callers", "callees"):
                    section = item.get(side)
                    if not isinstance(section, dict):
                        raise DevMapClientError(
                            f"devmap explore definition is missing {side}"
                        )
                    self._budgeted(section, budget)
        tests = payload.get("tests")
        if isinstance(tests, dict):
            self._budgeted(tests, budget)
        radius = payload.get("blast_radius")
        if isinstance(radius, dict):
            layers = radius.get("layers")
            if not isinstance(layers, dict):
                raise DevMapClientError("devmap blast radius is missing layers")
            self._budgeted(layers, budget)

    def explore(
        self,
        query: str,
        limit: int = 20,
        budget: int = EXPLORE_BUDGET,
        depth: int = 3,
        min_confidence: float = 0.0,
    ) -> Dict[str, Any]:
        """Definitions matching ``query``, with source, edges and blast radius.

        The composition happens in the kernel. It used to happen in a second
        Python engine that loaded the whole graph into process memory — and
        which, since the Python writer was retired, had no store to load and
        raised ``FileNotFoundError`` on every call.

        Returned as the kernel's own payload rather than a reshaped one: this
        client owns transport and invariant checking, not presentation. Every
        budgeted section is validated, so a caller can trust the counters
        without re-deriving them.
        """
        self._validate_query(query)
        self._validate_budget(budget)
        self._validate_depth(depth)
        if type(limit) is not int or not 1 <= limit <= MAX_EXPLORE_LIMIT:
            raise DevMapClientError(
                f"devmap explore limit must be an integer within [1, {MAX_EXPLORE_LIMIT}]"
            )
        if (
            not isinstance(min_confidence, (int, float))
            or isinstance(min_confidence, bool)
            or not 0.0 <= float(min_confidence) <= 1.0
        ):
            raise DevMapClientError("devmap explore min_confidence must be within [0, 1]")
        resp = self._request(
            {
                "cmd": "explore",
                "query": query,
                "limit": limit,
                "budget": budget,
                "depth": depth,
                "min_confidence": float(min_confidence),
            },
            [
                "explore",
                "--limit",
                str(limit),
                "--budget",
                str(budget),
                "--depth",
                str(depth),
                "--min-confidence",
                str(float(min_confidence)),
                *_positional(query),
            ],
        )
        self._validate_budgeted_sections(resp, budget)
        return resp

    def affected_tests(
        self,
        targets: List[str],
        budget: int = AFFECTED_BUDGET,
        depth: int = 3,
        min_confidence: float = 0.0,
    ) -> Dict[str, Any]:
        """Test files reachable through the inbound blast radius of ``targets``.

        No target-count limit is duplicated here on purpose: the kernel owns the
        bound and *refuses* an over-long list rather than trimming it, so drift
        between the two surfaces shows up as a loud error instead of a short
        answer that reads as a complete one.
        """
        if not isinstance(targets, list):
            raise DevMapClientError("devmap affected targets must be a list")
        if not targets:
            raise DevMapClientError("devmap affected requires at least one target")
        for target in targets:
            self._validate_query(target, "target")
        self._validate_budget(budget)
        self._validate_depth(depth)
        if (
            not isinstance(min_confidence, (int, float))
            or isinstance(min_confidence, bool)
            or not 0.0 <= float(min_confidence) <= 1.0
        ):
            raise DevMapClientError("devmap affected min_confidence must be within [0, 1]")
        resp = self._request(
            {
                "cmd": "affected",
                "targets": list(targets),
                "budget": budget,
                "depth": depth,
                "min_confidence": float(min_confidence),
            },
            [
                "affected",
                "--budget",
                str(budget),
                "--depth",
                str(depth),
                "--min-confidence",
                str(float(min_confidence)),
                *_positional(*targets),
            ],
        )
        self._validate_budgeted_sections(resp, budget)
        return resp

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
        resp = self._run_cli_command(
            ["snapshots", "--budget", str(budget), *_positional(file_path)]
        )
        return self._budgeted(resp, budget)

    # --- HTTP route surfaces ------------------------------------------------
    #
    # These three go over the CLI rather than :meth:`_request`. The daemon's
    # `IpcCommand` (`devmap-serve/src/protocol.rs`) has no `routes`,
    # `shape_check` or `api_impact` variant, so a socket attempt would be
    # rejected as `invalid_request` and retried on the CLI anyway — one wasted
    # round trip per call, for a command whose own bounded file scan dominates
    # its cost. The CLI is the same kernel over a different transport, not a
    # second engine.
    #
    # Each answer carries the kernel's own coverage record (`capabilities` on
    # `routes`, `scan` on `shape-check` and `api-impact`): what the client scan
    # read, and whether it finished. The Python implementations these replace
    # carried no such record, so a scan that stopped at its file cap was
    # published as a complete route inventory.

    def routes(self, route_filter: Optional[str] = None) -> Dict[str, Any]:
        """HTTP routes, their handlers, and the clients that call them."""
        args = ["routes"]
        if route_filter:
            self._validate_query(route_filter, "route filter")
            args += ["--filter", route_filter]
        return self._run_cli_command(args + _positional(str(self.root_dir)))

    def shape_check(self, route_filter: Optional[str] = None) -> Dict[str, Any]:
        """What each handler returns against what its callers read."""
        args = ["shape-check"]
        if route_filter:
            self._validate_query(route_filter, "route filter")
            args += ["--filter", route_filter]
        return self._run_cli_command(args + _positional(str(self.root_dir)))

    def api_impact(self, route: str) -> Dict[str, Any]:
        """What changing one route reaches: callers, shape, and a risk band."""
        self._validate_query(route, "route")
        # `route` is positional and may begin with `/`, which clap reads as a
        # path and not a flag — but `--` is cheap and the rule here is uniform.
        return self._run_cli_command(
            ["api-impact", *_positional(route, str(self.root_dir))]
        )

    def is_map_stale(self) -> bool:
        st = self.status()
        return not st.is_fresh or st.pending_count > 0
