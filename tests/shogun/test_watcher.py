"""Nudge layer: rising-edge delivery via the deterministic poll path."""

from __future__ import annotations

from typing import List, Tuple

from devcouncil.shogun.mailbox import Mailbox
from devcouncil.shogun.watcher import MailboxWatcher


def _watcher(tmp_path):
    mb = Mailbox(tmp_path)
    nudges: List[Tuple[str, int]] = []
    watcher = MailboxWatcher(mb, ["karo", "ashigaru1"], lambda a, n: nudges.append((a, n)))
    return mb, watcher, nudges


def test_nudge_fires_once_per_new_arrival(tmp_path):
    mb, watcher, nudges = _watcher(tmp_path)

    mb.send("karo", "one", from_agent="shogun")
    assert watcher.poll_once() == ["karo"]
    assert nudges == [("karo", 1)]

    # No new mail -> no repeat nudge.
    assert watcher.poll_once() == []
    assert len(nudges) == 1

    # A second message raises the unread count -> nudge again.
    mb.send("karo", "two", from_agent="shogun")
    assert watcher.poll_once() == ["karo"]
    assert nudges[-1] == ("karo", 2)


def test_drained_inbox_rearms_the_nudge(tmp_path):
    mb, watcher, nudges = _watcher(tmp_path)
    mb.send("ashigaru1", "task", type="task_assigned", from_agent="karo")
    watcher.poll_once()
    assert nudges == [("ashigaru1", 1)]

    mb.drain("ashigaru1")  # agent processed its mail
    watcher.poll_once()    # count fell to 0, re-arm
    mb.send("ashigaru1", "next", type="task_assigned", from_agent="karo")
    watcher.poll_once()
    assert nudges[-1] == ("ashigaru1", 1)


def test_special_messages_do_not_nudge(tmp_path):
    mb, watcher, nudges = _watcher(tmp_path)
    # A context-reset control message is delivered but must not raise a work nudge.
    mb.send("karo", "reset", type="clear_command", from_agent="shogun")
    assert watcher.poll_once() == []
    assert nudges == []


def test_reset_forces_renudge(tmp_path):
    mb, watcher, nudges = _watcher(tmp_path)
    mb.send("karo", "one", from_agent="shogun")
    watcher.poll_once()
    watcher.reset("karo")
    watcher.poll_once()
    assert nudges == [("karo", 1), ("karo", 1)]
