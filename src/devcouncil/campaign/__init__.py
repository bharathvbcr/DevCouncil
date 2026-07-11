"""Multi-agent Director orchestration for DevCouncil.

A multi-agent command hierarchy layered over DevCouncil's existing Task / Executor /
Verifier machinery, ported from the ``multi-agent campaign`` pattern:

    operator (you) -> Director -> Coordinator -> Worker x N + Reviewer

* **Director**   relays the operator's order to the Coordinator and steps back.
* **Coordinator**     decomposes the order into subtasks, dispatches them to Worker
               in parallel, routes review to the Reviewer, and owns the dashboard.
* **Worker** workers: each runs one Task through a coding executor.
* **Reviewer**   strategist: quality-controls finished work via the Verifier.

The coordination transport is deliberately *not* an API bus. Every agent has a
YAML "mailbox" file on disk (:mod:`devcouncil.campaign.mailbox`); agents wake each
other with a content-free "you have mail" nudge (:mod:`devcouncil.campaign.watcher`).
This mirrors the original's "files are the API" philosophy and gives a fully
auditable, diffable message log under ``.devcouncil/campaign/``.
"""

from __future__ import annotations

from devcouncil.campaign.bloom import BloomLevel, classify_bloom, route_rank
from devcouncil.campaign.dashboard import DashboardState, DashboardWriter, RosterEntry
from devcouncil.campaign.mailbox import Mailbox, Message
from devcouncil.campaign.notify import Notifier, NullNotifier
from devcouncil.campaign.orchestrator import (
    CampaignResult,
    Campaign,
    TaskOutcome,
    build_coding_executor_factory,
    build_verifier_fn,
)
from devcouncil.campaign.roles import (
    Action,
    ForbiddenActionError,
    Rank,
    Role,
    ROLES,
    assert_allowed,
    get_role,
    load_role_instructions,
)
from devcouncil.campaign.watcher import MailboxWatcher

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
    "Campaign",
    "CampaignResult",
    "TaskOutcome",
    "build_coding_executor_factory",
    "build_verifier_fn",
]
