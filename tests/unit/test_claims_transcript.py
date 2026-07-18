"""Unit tests for claims transcript helpers."""

from __future__ import annotations

import json
from pathlib import Path

from devcouncil.verification.claims.transcript import (
    _text_of,
    ends_on_open_question,
    last_assistant_sentence,
    last_assistant_text,
)


def test_text_of_variants():
    assert _text_of({}) is None
    assert _text_of({"message": "x"}) is None
    assert _text_of({"message": {"content": "  hi  "}}) == "hi"
    assert _text_of({"message": {"content": "   "}}) is None
    assert (
        _text_of(
            {
                "message": {
                    "content": [
                        {"type": "text", "text": "a"},
                        {"type": "tool_use"},
                        {"type": "text", "text": "b"},
                    ]
                }
            }
        )
        == "a\nb"
    )
    assert _text_of({"message": {"content": [{"type": "tool_use"}]}}) is None
    assert _text_of({"message": {"content": 123}}) is None


def test_last_assistant_text_and_sentence(tmp_path: Path):
    path = tmp_path / "t.jsonl"
    assert last_assistant_text(path) is None
    assert last_assistant_sentence(path) is None

    path.write_text(
        "\n".join(
            [
                "not-json",
                json.dumps({"type": "user", "message": {"content": "q"}}),
                json.dumps({"type": "assistant", "message": {"content": [{"type": "tool_use"}]}}),
                json.dumps(
                    {
                        "type": "assistant",
                        "message": {"content": "Done. All good?"},
                    }
                ),
            ]
        ),
        encoding="utf-8",
    )
    assert last_assistant_text(path) == "Done. All good?"
    assert last_assistant_sentence(path) == "All good?"


def test_ends_on_open_question(tmp_path: Path):
    missing = tmp_path / "missing.jsonl"
    assert ends_on_open_question(missing) is False

    path = tmp_path / "q.jsonl"
    path.write_text(
        "\n".join(
            [
                json.dumps({"type": "assistant", "message": {"content": "Ready?"}}),
            ]
        ),
        encoding="utf-8",
    )
    assert ends_on_open_question(path) is True

    path.write_text(
        "\n".join(
            [
                json.dumps({"type": "assistant", "message": {"content": "Ready?"}}),
                json.dumps({"type": "user", "message": {"content": "yes"}}),
            ]
        ),
        encoding="utf-8",
    )
    assert ends_on_open_question(path) is False

    path.write_text(
        "\n".join(
            [
                json.dumps({"type": "assistant", "message": {"content": "Done."}}),
            ]
        ),
        encoding="utf-8",
    )
    assert ends_on_open_question(path) is False
