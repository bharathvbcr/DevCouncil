"""Mailbox transport: atomicity, capping, self-send rejection, concurrency."""

from __future__ import annotations

import threading

import pytest

from devcouncil.shogun.mailbox import Mailbox, MailboxError


def test_send_and_read(tmp_path):
    mb = Mailbox(tmp_path)
    msg = mb.send("karo", "advance", type="cmd_new", from_agent="shogun")
    assert msg.from_agent == "shogun"
    stored = mb.all("karo")
    assert len(stored) == 1
    assert stored[0].content == "advance"
    assert stored[0].read is False
    # Round-trips through the on-disk ``from`` key, not the python attr name.
    assert mb.path_for("karo").exists()


def test_self_send_is_rejected(tmp_path):
    mb = Mailbox(tmp_path)
    with pytest.raises(MailboxError):
        mb.send("karo", "note to self", from_agent="karo")


def test_unread_count_excludes_special_types(tmp_path):
    mb = Mailbox(tmp_path)
    mb.send("ashigaru1", "work", type="task_assigned", from_agent="karo")
    mb.send("ashigaru1", "reset context", type="clear_command", from_agent="karo")
    # clear_command is delivered but not counted as pending work.
    assert mb.count_unread("ashigaru1") == 1
    assert mb.count_unread("ashigaru1", exclude_special=False) == 2


def test_mark_read_and_drain(tmp_path):
    mb = Mailbox(tmp_path)
    a = mb.send("gunshi", "one", from_agent="ashigaru1")
    mb.send("gunshi", "two", from_agent="ashigaru2")
    assert mb.mark_read("gunshi", [a.id]) == 1
    assert mb.count_unread("gunshi") == 1
    drained = mb.drain("gunshi")
    assert [m.content for m in drained] == ["two"]
    assert mb.count_unread("gunshi") == 0


def test_capping_bounds_read_history_but_never_drops_unread(tmp_path):
    from devcouncil.shogun.mailbox import MAX_READ_RETAINED

    mb = Mailbox(tmp_path)
    for i in range(60):
        mb.send("karo", f"m{i}", from_agent="shogun")
    # Mark the first 40 read, leaving 20 unread; then one more send triggers the cap.
    ids = [m.id for m in mb.all("karo")[:40]]
    mb.mark_read("karo", ids)
    mb.send("karo", "trigger-cap", from_agent="shogun")

    stored = mb.all("karo")
    read = [m for m in stored if m.read]
    # Unread messages are sacrosanct — every one is preserved (20 + the trigger send
    # was unread too, but the trigger send is unread so 21 unread survive).
    assert mb.count_unread("karo") == 21
    # Read history is bounded so a mailbox can't grow without limit.
    assert len(read) <= MAX_READ_RETAINED


def test_concurrent_sends_do_not_lose_messages(tmp_path):
    mb = Mailbox(tmp_path)
    n = 40

    def worker(i):
        mb.send("karo", f"msg-{i}", from_agent=f"ashigaru{i % 7 + 1}")

    threads = [threading.Thread(target=worker, args=(i,)) for i in range(n)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()

    # The locked, atomic append must not drop writes under contention.
    assert len(mb.all("karo")) == n


def test_clear_resets_mailbox(tmp_path):
    mb = Mailbox(tmp_path)
    mb.send("karo", "x", from_agent="shogun")
    mb.clear("karo")
    assert mb.all("karo") == []
