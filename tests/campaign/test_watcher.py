"""Nudge layer: rising-edge delivery via the deterministic poll path."""

from __future__ import annotations

from typing import List, Tuple

from devcouncil.campaign.mailbox import Mailbox
from devcouncil.campaign.watcher import MailboxWatcher


def _watcher(tmp_path):
    mb = Mailbox(tmp_path)
    nudges: List[Tuple[str, int]] = []
    watcher = MailboxWatcher(mb, ["coordinator", "worker1"], lambda a, n: nudges.append((a, n)))
    return mb, watcher, nudges


def test_nudge_fires_once_per_new_arrival(tmp_path):
    mb, watcher, nudges = _watcher(tmp_path)

    mb.send("coordinator", "one", from_agent="director")
    assert watcher.poll_once() == ["coordinator"]
    assert nudges == [("coordinator", 1)]

    # No new mail -> no repeat nudge.
    assert watcher.poll_once() == []
    assert len(nudges) == 1

    # A second message raises the unread count -> nudge again.
    mb.send("coordinator", "two", from_agent="director")
    assert watcher.poll_once() == ["coordinator"]
    assert nudges[-1] == ("coordinator", 2)


def test_drained_inbox_rearms_the_nudge(tmp_path):
    mb, watcher, nudges = _watcher(tmp_path)
    mb.send("worker1", "task", type="task_assigned", from_agent="coordinator")
    watcher.poll_once()
    assert nudges == [("worker1", 1)]

    mb.drain("worker1")  # agent processed its mail
    watcher.poll_once()    # count fell to 0, re-arm
    mb.send("worker1", "next", type="task_assigned", from_agent="coordinator")
    watcher.poll_once()
    assert nudges[-1] == ("worker1", 1)


def test_special_messages_do_not_nudge(tmp_path):
    mb, watcher, nudges = _watcher(tmp_path)
    # A context-reset control message is delivered but must not raise a work nudge.
    mb.send("coordinator", "reset", type="clear_command", from_agent="director")
    assert watcher.poll_once() == []
    assert nudges == []


def test_reset_forces_renudge(tmp_path):
    mb, watcher, nudges = _watcher(tmp_path)
    mb.send("coordinator", "one", from_agent="director")
    watcher.poll_once()
    watcher.reset("coordinator")
    watcher.poll_once()
    assert nudges == [("coordinator", 1), ("coordinator", 1)]
