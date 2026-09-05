"""The 10,000-character hook-output cap applies to *every* string DevCouncil emits.

Claude Code hooks reference (https://code.claude.com/docs/en/hooks.md, fetched
2026-09-04):

    Hook output strings, including ``additionalContext``, ``systemMessage``, and
    plain stdout, are capped at 10,000 characters. Output that exceeds this limit is
    saved to a file and replaced with a preview and file path[.]

The Stop / SubagentStop blocking ``reason`` is the one string DevCouncil emits that
is built from *foreign* output: a failing claim check embeds the failing command's
output tail (``verification/claims/checks.py`` bounds it to 50 **lines**, not
characters), once per failing claim. So the corrective message the stop gate exists
to deliver is exactly the string most able to exceed the cap — and when it does, the
gate still blocks while Claude receives a file path instead of the instructions.

``_emit_hook_specific`` and ``_emit_system_message`` already cap; these tests pin the
same rule on the stop-result path so there is one bound, not two behaviours.
"""

from __future__ import annotations

import contextlib
import io
import json

from devcouncil.cli.commands.hook import HOOK_OUTPUT_MAX_CHARS, _emit_stop_result
from devcouncil.execution.stop_gate import StopGateResult


def _emit(client: str, result: StopGateResult) -> dict:
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        _emit_stop_result(client, result)
    out = buf.getvalue().strip()
    assert out, "stop result emitted nothing"
    return json.loads(out)


def _huge(marker: str) -> str:
    """One very long line, the shape a 50-line tail cap does not bound."""
    return f"{marker}: " + "x" * (HOOK_OUTPUT_MAX_CHARS * 3)


def test_block_reason_is_capped():
    payload = _emit(
        "claude",
        StopGateResult(decision="block", reason=_huge("CLAIM VERIFICATION FAILED")),
    )
    assert payload["decision"] == "block"
    assert len(payload["reason"]) <= HOOK_OUTPUT_MAX_CHARS


def test_block_reason_keeps_its_leading_instructions():
    """Truncation must keep the head: that is where the corrective ask lives."""
    payload = _emit(
        "claude",
        StopGateResult(decision="block", reason=_huge("CLAIM VERIFICATION FAILED")),
    )
    assert payload["reason"].startswith("CLAIM VERIFICATION FAILED")


def test_block_system_message_is_capped():
    payload = _emit(
        "claude",
        StopGateResult(
            decision="block", reason="short", system_message=_huge("devcouncil")
        ),
    )
    assert len(payload["systemMessage"]) <= HOOK_OUTPUT_MAX_CHARS


def test_pass_system_message_is_capped():
    payload = _emit(
        "claude", StopGateResult(decision="pass", system_message=_huge("devcouncil"))
    )
    assert len(payload["systemMessage"]) <= HOOK_OUTPUT_MAX_CHARS


def test_codex_stop_reason_is_capped():
    payload = _emit(
        "codex", StopGateResult(decision="block", reason=_huge("verification failed"))
    )
    assert payload["continue"] is False
    assert len(payload["stopReason"]) <= HOOK_OUTPUT_MAX_CHARS


def test_gemini_system_message_is_capped():
    payload = _emit(
        "gemini", StopGateResult(decision="pass", system_message=_huge("devcouncil"))
    )
    assert len(payload["systemMessage"]) <= HOOK_OUTPUT_MAX_CHARS


def test_short_reason_is_untouched():
    """The cap must not rewrite output that was already within bounds."""
    payload = _emit(
        "claude", StopGateResult(decision="block", reason="two claims failed")
    )
    assert payload["reason"] == "two claims failed"


# --- the PreToolUse deny reason: the one emit path that skipped the cap -----------


def _deny_stderr(reason: str) -> str:
    """Run ``_emit_decision(..., "deny", ...)`` and return what Claude would read.

    A PreToolUse hook that blocks by exiting 2 has its *stderr* handed to Claude as
    the denial reason, so stderr is a Claude-facing output string and the same
    10,000-character rule applies to it.
    """
    import sys

    import typer

    from devcouncil.cli.commands.hook import _emit_decision

    buf = io.StringIO()
    saved, sys.stderr = sys.stderr, buf
    try:
        try:
            _emit_decision("claude", "deny", reason)
        except typer.Exit as exit_exc:
            assert exit_exc.exit_code == 2
        else:  # pragma: no cover - a deny that does not exit is its own bug
            raise AssertionError("deny did not exit 2")
    finally:
        sys.stderr = saved
    return buf.getvalue().rstrip("\n")


def test_deny_reason_is_capped():
    """Measured pre-fix: 30,006 chars in, 30,007 out — uncapped on the deny path.

    Every other Claude-facing emit already capped; this one printed straight to
    stderr, so past the limit Claude Code spills it to a file and hands Claude a
    preview plus a path. The block still lands, but the corrective instruction the
    gate exists to deliver does not.
    """
    out = _deny_stderr(_huge("DENY"))
    assert len(out) == HOOK_OUTPUT_MAX_CHARS
    assert out.startswith("DENY")


def test_deny_reason_cap_matches_the_warn_path_on_the_same_call():
    """One cap, one owner: deny and warn must bound the same reason identically."""
    import contextlib

    from devcouncil.cli.commands.hook import _emit_decision

    reason = _huge("SAME")
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        _emit_decision("claude", "warn", reason)
    warned = json.loads(buf.getvalue().strip())["systemMessage"]
    assert len(warned) == HOOK_OUTPUT_MAX_CHARS
    assert len(_deny_stderr(reason)) == len(warned)


def test_short_deny_reason_is_untouched():
    assert _deny_stderr("Write blocked: no active task.") == "Write blocked: no active task."


# --- an un-evaluable write gate must not be silent on every channel ---------------


def test_empty_tool_call_payload_is_reported_not_silently_allowed():
    """``pre_tool_use`` with an empty payload allowed the call and emitted nothing.

    No stdout, no stderr, no trace — a gate invocation that examined nothing was
    byte-identical to one that examined the call and approved it. Fails against the
    pre-fix code with ``stdout == ''``.
    """
    from typer.testing import CliRunner

    from devcouncil.cli.main import app

    result = CliRunner().invoke(app, ["hook", "pre-tool-use", "   ", "--client", "claude"])
    assert result.exit_code == 0, result.output
    out = result.stdout.strip()
    assert out, "an un-evaluable write gate emitted nothing, exactly like a pass"
    assert "nothing to evaluate" in json.loads(out)["systemMessage"]
