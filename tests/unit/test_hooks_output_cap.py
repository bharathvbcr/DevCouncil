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
