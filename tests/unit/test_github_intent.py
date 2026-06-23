import json
import subprocess

import devcouncil.integrations.github_intent as gi
from devcouncil.integrations.github_intent import (
    IntentRef,
    parse_intent_ref,
    resolve_goal_intent,
)


def test_parse_intent_ref_recognizes_supported_forms():
    assert parse_intent_ref("#142") == IntentRef(142, "auto", None)
    assert parse_intent_ref("GH-7") == IntentRef(7, "auto", None)
    assert parse_intent_ref("owner/repo#9") == IntentRef(9, "auto", "owner/repo")
    assert parse_intent_ref("https://github.com/o/r/issues/3") == IntentRef(3, "issue", "o/r")
    assert parse_intent_ref("https://github.com/o/r/pull/5") == IntentRef(5, "pull", "o/r")


def test_parse_intent_ref_ignores_plain_text_and_embedded_refs():
    assert parse_intent_ref("add a feature") is None
    # A ref must be the WHOLE goal, not embedded — otherwise we'd hijack normal text.
    assert parse_intent_ref("fix #12 please") is None


def test_resolve_goal_intent_passes_plain_text_through(tmp_path):
    goal, note = resolve_goal_intent("add a hello function", tmp_path)
    assert goal == "add a hello function"
    assert note is None


def test_resolve_goal_intent_reports_when_gh_missing(monkeypatch, tmp_path):
    monkeypatch.setattr(gi.shutil, "which", lambda _cmd: None)
    goal, note = resolve_goal_intent("#142", tmp_path)
    assert goal == "#142"  # unchanged — expansion is strictly additive
    assert note is not None and "gh" in note


def test_resolve_goal_intent_expands_issue_via_gh(monkeypatch, tmp_path):
    monkeypatch.setattr(gi.shutil, "which", lambda _cmd: "gh")

    payload = {
        "title": "Add password reset",
        "body": "Users must be able to reset via email link.",
        "url": "https://github.com/o/r/issues/142",
        "state": "OPEN",
        "comments": [{"body": "Token should expire in 1 hour."}],
    }

    def fake_run(cmd, **kwargs):
        assert "issue" in cmd and "view" in cmd
        return subprocess.CompletedProcess(cmd, 0, stdout=json.dumps(payload), stderr="")

    monkeypatch.setattr(gi.subprocess, "run", fake_run)
    goal, note = resolve_goal_intent("#142", tmp_path)

    assert "Add password reset" in goal
    assert "reset via email link" in goal
    assert "expire in 1 hour" in goal
    assert "issue #142" in (note or "")


def test_resolve_goal_intent_caps_long_comments(monkeypatch, tmp_path):
    monkeypatch.setattr(gi.shutil, "which", lambda _cmd: "gh")
    long_comment = "x" * 5000
    payload = {"title": "T", "body": "B", "url": "u", "comments": [{"body": long_comment}]}
    monkeypatch.setattr(
        gi.subprocess, "run",
        lambda cmd, **kw: subprocess.CompletedProcess(cmd, 0, stdout=json.dumps(payload), stderr=""),
    )
    goal, _note = resolve_goal_intent("#1", tmp_path)
    assert "[…]" in goal
    # The 5000-char comment must not survive verbatim.
    assert long_comment not in goal
    assert len(goal) < 1500


def test_resolve_goal_intent_falls_back_to_pr_when_issue_missing(monkeypatch, tmp_path):
    monkeypatch.setattr(gi.shutil, "which", lambda _cmd: "gh")
    calls = []

    def fake_run(cmd, **kwargs):
        sub = "pr" if "pr" in cmd else "issue"
        calls.append(sub)
        if sub == "issue":
            return subprocess.CompletedProcess(cmd, 1, stdout="", stderr="not found")
        return subprocess.CompletedProcess(
            cmd, 0, stdout=json.dumps({"title": "A PR", "body": "PR body", "url": "u"}), stderr=""
        )

    monkeypatch.setattr(gi.subprocess, "run", fake_run)
    goal, note = resolve_goal_intent("#5", tmp_path)

    assert calls == ["issue", "pr"]
    assert "A PR" in goal
    assert "pull request #5" in (note or "")


def test_resolve_goal_intent_keeps_literal_when_lookup_fails(monkeypatch, tmp_path):
    monkeypatch.setattr(gi.shutil, "which", lambda _cmd: "gh")
    monkeypatch.setattr(
        gi.subprocess, "run",
        lambda cmd, **kw: subprocess.CompletedProcess(cmd, 1, stdout="", stderr="nope"),
    )
    goal, note = resolve_goal_intent("#999", tmp_path)
    assert goal == "#999"
    assert note is not None and "#999" in note
