"""MCP 2.0 surface the server advertises, and the contracts behind it.

Four things are pinned here, each written against the *unmodified* code and
observed red before its fix landed:

* Every advertised tool carries behaviour annotations. Without them a client
  that honours ``readOnlyHint`` treats ``devcouncil_read_file`` exactly like
  ``devcouncil_apply_patch`` -- the spec's own default for an unannotated tool
  is "may be destructive".
* A CLI-backed tool does not hold the event loop for the length of its
  subprocess. ``notifications/cancelled`` is applied by the SDK as a scope
  cancel (``mcp/shared/jsonrpc_dispatcher.py:81-85``, mode ``"interrupt"``),
  and a scope cancel needs an await point to land on; a handler that blocks
  has none, and the read loop cannot even receive the notification.
* A CLI call that was killed at the timeout wall never reports what a call
  that ran reports. This repo calls that a Class A defect.
* A graph query that could not be resolved never reports what a query that
  ran and found nothing reports -- same rule, second surface.
"""

from __future__ import annotations

import asyncio
import json
import time
from pathlib import Path

import pytest

from devcouncil.integrations.mcp import server as mcp_server
from devcouncil.integrations.mcp import util as mcp_util
from devcouncil.integrations.mcp.handlers import codeintel, status as status_handlers, tool_specs


@pytest.fixture
def anyio_backend():
    return "asyncio"


def _payload(contents) -> dict:
    return json.loads(contents[0].text)


# --- 1. tool annotations --------------------------------------------------------


def test_every_advertised_tool_declares_all_four_behaviour_hints() -> None:
    """Coverage gate: a new tool cannot ship without saying what it does.

    All four hints are set explicitly rather than relying on the spec's
    "meaningful only when readOnlyHint is false" conditional -- clients differ
    in how carefully they implement that conditional, and an unset
    ``destructiveHint`` defaults to *true*.
    """
    unannotated = [tool.name for tool in tool_specs.all_tools() if tool.annotations is None]
    assert unannotated == [], f"tools advertised with no behaviour annotations: {unannotated}"

    incomplete = {
        tool.name: [
            field
            for field in ("read_only_hint", "destructive_hint", "idempotent_hint", "open_world_hint")
            if getattr(tool.annotations, field) is None
        ]
        for tool in tool_specs.all_tools()
        if tool.annotations is not None
        and any(
            getattr(tool.annotations, field) is None
            for field in ("read_only_hint", "destructive_hint", "idempotent_hint", "open_world_hint")
        )
    }
    assert incomplete == {}, f"tools with partially declared behaviour: {incomplete}"


def test_annotation_table_names_only_tools_that_exist() -> None:
    """A renamed or deleted tool must not leave a stale row behind.

    The row would silently stop applying, and the coverage gate above would
    still pass because the *new* name would simply be missing an annotation --
    two failures that mask each other.
    """
    advertised = {tool.name for tool in tool_specs.all_tools()}
    stale = sorted(set(tool_specs.TOOL_BEHAVIOUR) - advertised)
    assert stale == [], f"annotation rows for tools that no longer exist: {stale}"


def test_read_only_tools_are_never_also_destructive() -> None:
    """Internal consistency: the two hints cannot both be true."""
    contradictory = [
        tool.name
        for tool in tool_specs.all_tools()
        if tool.annotations is not None
        and tool.annotations.read_only_hint
        and (tool.annotations.destructive_hint or not tool.annotations.idempotent_hint)
    ]
    assert contradictory == [], f"read-only tools marked destructive or non-idempotent: {contradictory}"


@pytest.mark.parametrize(
    "name",
    [
        # Verified read-only at the source, not inferred from the description:
        "devcouncil_read_file",       # read.py: within_root + read_bytes
        "devcouncil_code_search",     # codeintel.py: DevMapClient.search
        "devcouncil_next_task",       # task_gate_ops.py:444 discards client_id, takes no lease
        "devcouncil_graph_cypher",    # indexing/graph/cypher.py:46 rejects every mutating clause
        "devcouncil_get_evidence",
        "devcouncil_list_leases",
    ],
)
def test_query_tools_are_advertised_read_only(name: str) -> None:
    tool = next(t for t in tool_specs.all_tools() if t.name == name)
    assert tool.annotations is not None
    assert tool.annotations.read_only_hint is True
    assert tool.annotations.destructive_hint is False


@pytest.mark.parametrize(
    "name",
    [
        "devcouncil_write_file",       # overwrites a file in place
        "devcouncil_apply_patch",      # rewrites every target of the diff
        "devcouncil_run_command",      # runs an arbitrary allowlisted command
        "devcouncil_cli",              # cli_gate.py:11-18 allows write / apply-patch / rollback
        "devcouncil_debug_evaluate",   # debug.py:84 "side-effectful"
    ],
)
def test_mutating_tools_are_not_advertised_read_only(name: str) -> None:
    tool = next(t for t in tool_specs.all_tools() if t.name == name)
    assert tool.annotations is not None
    assert tool.annotations.read_only_hint is False
    assert tool.annotations.destructive_hint is True


def test_annotations_survive_the_additional_properties_closing() -> None:
    """``_closed`` rebuilds each Tool; the annotations must come through it."""
    closed = {t.name: t for t in tool_specs.all_tools()}
    assert closed["devcouncil_read_file"].input_schema["additionalProperties"] is False
    assert closed["devcouncil_read_file"].annotations is not None


# --- 2. the event loop stays live across a CLI-backed tool ----------------------


class _SlowCLI:
    """A stand-in for ``run_cli_command`` that blocks the way a subprocess does."""

    def __init__(self, seconds: float, stdout: str = '{"ok": true, "tasks": []}') -> None:
        self.seconds = seconds
        self.stdout = stdout
        self.calls = 0

    def __call__(self, *_args, **_kwargs):
        self.calls += 1
        time.sleep(self.seconds)
        return {
            "ok": True,
            "returncode": 0,
            "stdout": self.stdout,
            "stderr": "",
            "stdout_truncated": False,
            "stderr_truncated": False,
            "timed_out": False,
        }


async def _count_ticks_during(coro, *, tick: float = 0.005) -> tuple[object, int]:
    """Run ``coro`` while a second task tries to tick, and report how far it got."""
    ticks = 0

    async def _ticker() -> None:
        nonlocal ticks
        while True:
            await asyncio.sleep(tick)
            ticks += 1

    ticker = asyncio.create_task(_ticker())
    await asyncio.sleep(tick * 2)  # let the ticker reach its first await
    ticks = 0
    try:
        result = await coro
    finally:
        ticker.cancel()
        with pytest.raises(asyncio.CancelledError):
            await ticker
    return result, ticks


@pytest.mark.anyio
async def test_run_cli_command_handler_does_not_hold_the_event_loop(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    slow = _SlowCLI(0.35, stdout="coverage report body")
    monkeypatch.setattr(status_handlers, "run_cli_command", slow)

    _result, ticks = await _count_ticks_during(status_handlers.handle_report(tmp_path, None, {}))

    assert slow.calls == 1, "the monkeypatched seam must still be the one that runs"
    # ~70 ticks are available in 0.35s; anything above a handful proves the loop
    # was scheduling other work while the blocking call ran.
    assert ticks > 10, f"event loop was blocked for the whole CLI call (ticks={ticks})"


@pytest.mark.anyio
async def test_run_cli_json_handler_does_not_hold_the_event_loop(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    slow = _SlowCLI(0.35)
    monkeypatch.setattr(status_handlers, "run_cli_json", lambda *a, **k: (json.loads(slow(*a, **k)["stdout"]), None))

    _result, ticks = await _count_ticks_during(status_handlers.handle_list_tasks(tmp_path, None, {}))

    assert slow.calls == 1
    assert ticks > 10, f"event loop was blocked for the whole CLI call (ticks={ticks})"


@pytest.mark.anyio
async def test_a_slow_cli_tool_observes_cancellation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The SDK cancels the handler's scope; the handler needs an await to land on.

    This pins that the cancellation is *observed* -- the handler raises rather
    than returning a result the peer will never be sent. The subprocess itself
    still runs to its own ``_CLI_TIMEOUT_SECONDS`` bound; Python cannot kill the
    worker thread, and that limit is unchanged by this test.
    """
    monkeypatch.setattr(status_handlers, "run_cli_command", _SlowCLI(0.3, stdout="body"))

    task = asyncio.create_task(status_handlers.handle_report(tmp_path, None, {}))
    await asyncio.sleep(0.05)
    task.cancel()

    with pytest.raises(asyncio.CancelledError):
        await task


# --- 3. a timed-out CLI call is not a result -----------------------------------


def _timed_out(stdout: str = "") -> dict:
    return {
        "ok": False,
        "returncode": None,
        "stdout": stdout,
        "stderr": "",
        "stdout_truncated": False,
        "stderr_truncated": False,
        "timed_out": True,
        "timeout_seconds": mcp_util.CLI_TIMEOUT_SECONDS,
    }


def test_a_killed_cli_call_is_never_parsed_into_a_successful_payload() -> None:
    """Whatever a killed process managed to flush is not an answer.

    ``parse_cli_json`` read stdout before it read ``ok``, so a command killed at
    the 120s wall that had already flushed a complete JSON object was returned
    as a finished result -- a truncated view of the tasks table presented as
    the whole of it.
    """
    payload, error = mcp_util.parse_cli_json(_timed_out('{"tasks": [], "total": 0}'))

    assert payload is None, "a timed-out command's partial output was returned as a result"
    assert error is not None
    assert _payload(error)["ok"] is False


def test_a_timeout_is_distinguishable_from_a_command_that_ran_and_failed() -> None:
    """A check that could not run must not report what a check that ran reports."""
    _payload_timeout, timeout_error = mcp_util.parse_cli_json(_timed_out())
    _payload_failure, failure_error = mcp_util.parse_cli_json(
        {"ok": False, "returncode": 2, "stdout": "", "stderr": "boom", "timed_out": False}
    )

    assert timeout_error is not None and failure_error is not None
    timed_out = _payload(timeout_error)
    failed = _payload(failure_error)

    assert timed_out["code"] != failed["code"], (
        "a killed command and a command that ran and exited non-zero report the same code"
    )
    assert timed_out["timed_out"] is True
    assert timed_out["timeout_seconds"] == mcp_util.CLI_TIMEOUT_SECONDS
    assert failed["code"] == "cli_failed"


@pytest.mark.anyio
async def test_report_reports_its_own_timeout_rather_than_a_generic_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """``handle_report`` returns prose, so it never went through ``parse_cli_json``.

    That left it as the one CLI surface where a killed command still arrived as
    ``cli_failed`` with an empty stderr -- the same answer a report that ran and
    refused produces.
    """
    monkeypatch.setattr(status_handlers, "run_cli_command", lambda *a, **k: _timed_out())

    payload = _payload(await status_handlers.handle_report(tmp_path, None, {}))

    assert payload["code"] == "cli_timeout"
    assert payload["timed_out"] is True
    assert payload["timeout_seconds"] == mcp_util.CLI_TIMEOUT_SECONDS


def test_a_successful_call_still_parses_normally() -> None:
    """The characterization the fix must not break."""
    payload, error = mcp_util.parse_cli_json(
        {"ok": True, "returncode": 0, "stdout": '{"tasks": []}', "stderr": "", "timed_out": False}
    )
    assert error is None
    assert payload == {"tasks": []}


# --- 4. an unresolvable graph path is not "no path" ----------------------------


class _UnavailableTrace:
    """A kernel response whose resolution says the query could not be answered."""

    items: list = []
    total = 0
    truncated = False
    resolution = {"Unavailable": {"reason": "symbol 'foo' is not in the index"}}


class _Status:
    generation_id = 7
    pending_count = 0
    node_count = 1
    edge_count = 1
    is_fresh = True
    degraded_reason = None
    quarantined_count = 0
    raw: dict = {"schema_version": "1", "analyzer_version": "1"}


class _Client:
    def status(self):
        return _Status()

    def trace(self, *_args, **_kwargs):
        return _UnavailableTrace()


def test_unresolvable_path_query_is_not_reported_as_a_verified_absence(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """``found: false`` with ``ok: true`` is "there is no path"; that was a lie.

    Every sibling operation (search, impact, dead) routes an Unavailable
    resolution through ``_unavailable``; ``_path_via_client`` alone answered
    with a success envelope, and dropped ``operation`` on the way -- the exact
    gap ``_client_envelope``'s own docstring says the fallback used to hide.
    """
    monkeypatch.setattr(codeintel, "try_connect", lambda _root: _Client())

    out = codeintel._path_via_client(Path("/tmp"), "foo", "bar", 8)

    assert out["ok"] is False, "an unanswerable path query reported a verified negative"
    assert out["operation"] == "path"
    from devcouncil.devmap_client import resolution_unavailable_reason

    assert resolution_unavailable_reason(out["resolution"])


class _TruncatedEmptyTrace:
    """A trace capped to nothing with work still outstanding."""

    items: list = []
    total = 12
    truncated = True
    resolution = None


class _TruncatingClient(_Client):
    def trace(self, *_args, **_kwargs):
        return _TruncatedEmptyTrace()


def test_path_capped_to_nothing_is_not_reported_as_a_verified_absence(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Same class as above: ``total=12`` with zero rows shown is not "no path".

    ``search``, ``impact`` and ``dead`` all guard this with ``_require_usable``;
    ``path`` did not, so a budget-truncated trace answered ``found: false``.
    """
    monkeypatch.setattr(codeintel, "try_connect", lambda _root: _TruncatingClient())

    out = codeintel._path_via_client(Path("/tmp"), "foo", "bar", 8)

    assert out["ok"] is False, "a capped-to-empty trace reported a verified negative"
    assert out["operation"] == "path"


# --- 5. what the tools/list cache hint actually reaches -------------------------


def test_the_tools_list_cache_hint_cannot_reach_a_handshake_client() -> None:
    """Characterization, not a fix: the hint is inert on this server's transport.

    ``server.py`` runs over ``stdio_server`` + ``app.run``, which negotiates via
    ``initialize``; ``ServerRunner._negotiate_initialize`` (mcp
    ``server/runner.py:425``) can only pick from ``HANDSHAKE_PROTOCOL_VERSIONS``,
    and ``ttlMs``/``cacheScope`` are 2026-07-28 vocabulary that the per-version
    sieve strips from every handshake-era result.

    When this test starts failing the hint has gone live -- update the note at
    ``server.py``'s ``_TOOLS_LIST_CACHE_TTL_MS`` rather than deleting the test.
    """
    from mcp.server.runner import ServerRunner
    from mcp_types.version import HANDSHAKE_PROTOCOL_VERSIONS

    assert "2026-07-28" not in HANDSHAKE_PROTOCOL_VERSIONS

    runner = ServerRunner(mcp_server.app, connection=None, lifespan_state=None)
    for version in HANDSHAKE_PROTOCOL_VERSIONS:
        dumped = runner._serialize(
            "tools/list", version, asyncio.run(mcp_server._on_list_tools(None, None))
        )
        assert "ttlMs" not in dumped, version
        assert "cacheScope" not in dumped, version
