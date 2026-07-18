"""Deterministic unit tests for campaign mailbox, watcher, and roles."""

from __future__ import annotations

from pathlib import Path

import pytest

from devcouncil.campaign.mailbox import Mailbox, MailboxError, Message
from devcouncil.campaign.roles import Action, ForbiddenActionError, Rank, assert_allowed, get_role, load_role_instructions
from devcouncil.campaign.watcher import MailboxWatcher


def test_message_roundtrip_and_special():
    msg = Message(
        id="1",
        from_agent="director",
        timestamp="t",
        type="clear_command",
        content="x",
    )
    assert msg.is_special is True
    restored = Message.from_dict(msg.to_dict())
    assert restored.from_agent == "director"
    assert Message.from_dict({"from_agent": "a", "type": "info"}).from_agent == "a"


def test_mailbox_send_read_count_and_cap(tmp_path: Path):
    box = Mailbox(tmp_path)
    with pytest.raises(MailboxError):
        box.send("worker", "hello", from_agent="worker")

    mid = box.send("worker", "do this", type="task", from_agent="director")
    assert mid.id
    assert box.count_unread("worker") == 1
    unread = box.unread("worker")
    assert len(unread) == 1
    assert unread[0].content == "do this"

    box.mark_read("worker", [mid.id])
    assert box.count_unread("worker") == 0

    box.send("worker", "clear", type="clear_command", from_agent="director")
    assert box.count_unread("worker") == 0

    for i in range(60):
        box.send("worker", f"m{i}", type="task", from_agent="director")
    assert len(box.all("worker")) <= 50
    drained = box.drain("worker")
    assert drained
    box.clear("worker")
    assert box.all("worker") == []


def test_mailbox_watcher_poll_once_rising_edge(tmp_path: Path):
    box = Mailbox(tmp_path)
    nudges: list[tuple[str, int]] = []

    watcher = MailboxWatcher(box, ["worker"], nudge=lambda a, n: nudges.append((a, n)))
    assert watcher.poll_once() == []

    box.send("worker", "task-1", from_agent="director")
    assert watcher.poll_once() == ["worker"]
    assert nudges[-1][0] == "worker"
    assert nudges[-1][1] >= 1

    assert watcher.poll_once() == []

    box.send("worker", "task-2", from_agent="director")
    assert watcher.poll_once() == ["worker"]

    box.mark_read("worker")
    assert watcher.poll_once() == []
    watcher.reset("worker")
    watcher.reset()


def test_mailbox_watcher_context_manager(tmp_path: Path):
    box = Mailbox(tmp_path)
    seen: list[str] = []
    with MailboxWatcher(box, ["worker"], nudge=lambda a, _n: seen.append(a)) as watcher:
        box.send("worker", "hello", from_agent="director")
        watcher.poll_once()
    assert isinstance(seen, list)


def test_roles_assert_allowed():
    role = get_role(Rank.WORKER)
    assert Action.EXECUTE_TASK in role.allowed
    assert_allowed(Rank.WORKER, Action.EXECUTE_TASK)
    with pytest.raises(ForbiddenActionError):
        assert_allowed(Rank.WORKER, Action.WRITE_DASHBOARD)
    director = get_role(Rank.DIRECTOR)
    assert director.reports_to is None
    text = load_role_instructions(Rank.WORKER)
    assert isinstance(text, str)
