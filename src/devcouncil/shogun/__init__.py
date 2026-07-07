"""Multi-agent Shogun orchestration for DevCouncil.

A feudal command hierarchy layered over DevCouncil's existing Task / Executor /
Verifier machinery, ported from the ``multi-agent-shogun`` pattern:

    Lord (you) -> Shogun -> Karo -> Ashigaru x N + Gunshi

* **Shogun**   relays the Lord's order to the Karo and steps back.
* **Karo**     decomposes the order into subtasks, dispatches them to Ashigaru
               in parallel, routes review to the Gunshi, and owns the dashboard.
* **Ashigaru** foot-soldiers: each runs one Task through a coding executor.
* **Gunshi**   strategist: quality-controls finished work via the Verifier.

The coordination transport is deliberately *not* an API bus. Every agent has a
YAML "mailbox" file on disk (:mod:`devcouncil.shogun.mailbox`); agents wake each
other with a content-free "you have mail" nudge (:mod:`devcouncil.shogun.watcher`).
This mirrors the original's "files are the API" philosophy and gives a fully
auditable, diffable message log under ``.devcouncil/shogun/``.
"""

from __future__ import annotations

from devcouncil.shogun.bloom import BloomLevel, classify_bloom, route_rank
from devcouncil.shogun.dashboard import DashboardState, DashboardWriter, RosterEntry
from devcouncil.shogun.mailbox import Mailbox, Message
from devcouncil.shogun.notify import Notifier, NullNotifier
from devcouncil.shogun.orchestrator import (
    CampaignResult,
    ShogunCampaign,
    TaskOutcome,
    build_coding_executor_factory,
    build_verifier_fn,
)
from devcouncil.shogun.roles import (
    Action,
    ForbiddenActionError,
    Rank,
    Role,
    ROLES,
    assert_allowed,
    get_role,
    load_role_instructions,
)
from devcouncil.shogun.watcher import MailboxWatcher

__all__ = [
    "Mailbox",
    "Message",
    "MailboxWatcher",
    "Rank",
    "Role",
    "ROLES",
    "Action",
    "get_role",
    "assert_allowed",
    "load_role_instructions",
    "ForbiddenActionError",
    "BloomLevel",
    "classify_bloom",
    "route_rank",
    "DashboardState",
    "DashboardWriter",
    "RosterEntry",
    "Notifier",
    "NullNotifier",
    "ShogunCampaign",
    "CampaignResult",
    "TaskOutcome",
    "build_coding_executor_factory",
    "build_verifier_fn",
]
