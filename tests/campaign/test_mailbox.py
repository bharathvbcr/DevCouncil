"""Mailbox transport: atomicity, capping, self-send rejection, concurrency."""

from __future__ import annotations

import threading

import pytest

from devcouncil.campaign.mailbox import Mailbox, MailboxError, MailboxLockTimeout


def test_send_and_read(tmp_path):
    mb = Mailbox(tmp_path)
    msg = mb.send("coordinator", "advance", type="cmd_new", from_agent="director")
    assert msg.from_agent == "director"
    stored = mb.all("coordinator")
    assert len(stored) == 1
    assert stored[0].content == "advance"
    assert stored[0].read is False
    # Round-trips through the on-disk ``from`` key, not the python attr name.
    assert mb.path_for("coordinator").exists()


def test_self_send_is_rejected(tmp_path):
    mb = Mailbox(tmp_path)
    with pytest.raises(MailboxError):
        mb.send("coordinator", "note to self", from_agent="coordinator")


def test_unread_count_excludes_special_types(tmp_path):
    mb = Mailbox(tmp_path)
    mb.send("worker1", "work", type="task_assigned", from_agent="coordinator")
    mb.send("worker1", "reset context", type="clear_command", from_agent="coordinator")
    # clear_command is delivered but not counted as pending work.
    assert mb.count_unread("worker1") == 1
    assert mb.count_unread("worker1", exclude_special=False) == 2


def test_mark_read_and_drain(tmp_path):
    mb = Mailbox(tmp_path)
    a = mb.send("reviewer", "one", from_agent="worker1")
    mb.send("reviewer", "two", from_agent="worker2")
    assert mb.mark_read("reviewer", [a.id]) == 1
    assert mb.count_unread("reviewer") == 1
    drained = mb.drain("reviewer")
    assert [m.content for m in drained] == ["two"]
    assert mb.count_unread("reviewer") == 0


def test_capping_bounds_read_history_when_mixed(tmp_path):
    from devcouncil.campaign.mailbox import MAX_MESSAGES, MAX_READ_RETAINED

    mb = Mailbox(tmp_path)
    for i in range(40):
        mb.send("coordinator", f"read-{i}", from_agent="director")
    mb.mark_read("coordinator")
    for i in range(20):
        mb.send("coordinator", f"unread-{i}", from_agent="director")
    mb.send("coordinator", "trigger-cap", from_agent="director")

    stored = mb.all("coordinator")
    read = [m for m in stored if m.read]
    assert len(stored) <= MAX_MESSAGES
    assert len(read) <= MAX_READ_RETAINED
    assert mb.count_unread("coordinator") <= MAX_MESSAGES


def test_unread_only_mailbox_capped_at_max_messages(tmp_path):
    from devcouncil.campaign.mailbox import MAX_MESSAGES

    mb = Mailbox(tmp_path)
    for i in range(60):
        mb.send("coordinator", f"unread-{i}", from_agent="director")

    stored = mb.all("coordinator")
    assert len(stored) == MAX_MESSAGES
    assert mb.count_unread("coordinator") == MAX_MESSAGES
    assert stored[0].content == f"unread-{60 - MAX_MESSAGES}"
    assert stored[-1].content == "unread-59"


def test_concurrent_sends_do_not_lose_messages(tmp_path):
    mb = Mailbox(tmp_path)
    n = 40

    def worker(i):
        mb.send("coordinator", f"msg-{i}", from_agent=f"worker{i % 7 + 1}")

    threads = [threading.Thread(target=worker, args=(i,)) for i in range(n)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()

    # The locked, atomic append must not drop writes under contention.
    assert len(mb.all("coordinator")) == n


def test_clear_resets_mailbox(tmp_path):
    mb = Mailbox(tmp_path)
    mb.send("coordinator", "x", from_agent="director")
    mb.clear("coordinator")
    assert mb.all("coordinator") == []


def test_lock_timeout_raises_when_held(tmp_path):
    """A lock held past LOCK_TIMEOUT_S must raise, not force-through and write."""
    mb = Mailbox(tmp_path)
    lock_path = mb.path_for("coordinator")
    mutex_dir = lock_path.with_suffix(lock_path.suffix + ".lock.d")
    mutex_dir.parent.mkdir(parents=True, exist_ok=True)
    mutex_dir.mkdir()

    with pytest.raises(MailboxLockTimeout):
        mb.send("coordinator", "should not arrive", from_agent="director")

    assert mb.all("coordinator") == []
