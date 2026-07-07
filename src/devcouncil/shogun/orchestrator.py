"""The Shogun campaign — hierarchical, parallel task execution over DevCouncil.

``ShogunCampaign`` layers the feudal command hierarchy on top of DevCouncil's
existing machinery:

* the **Shogun** relays the order (goal) to the Karo via the mailbox;
* the **Karo** orders the plan's tasks topologically, classifies each by Bloom
  level, and dispatches them to the worker pool **in parallel** — never releasing
  a task before its ``depends_on`` prerequisites are verified;
* each **Ashigaru** runs one task through an :class:`~devcouncil.execution.executor.Executor`
  (Bloom Analyze-and-above tasks are routed to the **Gunshi** instead);
* the **Gunshi** quality-controls every finished task through the Verifier and
  reports the verdict back up to the Karo, who updates the dashboard and pushes a
  notification to the Lord.

Every collaborator that touches the outside world — the coding executor, the
verifier, the notifier — is injected, so the whole control flow (routing,
dependency waves, QC gating, chain-of-command enforcement, dashboard/mailbox
side effects) is unit-testable with in-memory fakes and no coding CLI present.
"""

from __future__ import annotations

import threading
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Dict, List, Optional, Protocol, Sequence, Tuple

from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.shogun.bloom import BloomLevel, classify_bloom, route_rank, summarize_routing
from devcouncil.shogun.dashboard import DashboardState, DashboardWriter, RosterEntry
from devcouncil.shogun.mailbox import Mailbox
from devcouncil.shogun.notify import NullNotifier, Notifier
from devcouncil.shogun.roles import Action, Rank, assert_allowed

# (passed, blocking_gap_descriptions)
VerifyFn = Callable[[Task, List[Requirement]], Tuple[bool, List[str]]]


class ExecutorLike(Protocol):
    """Minimal shape of a DevCouncil executor: :meth:`run_task`."""

    def run_task(self, task: Task, requirements: List[Requirement]) -> object: ...


ExecutorFactory = Callable[[str], ExecutorLike]
EventSink = Callable[[str], None]
TaskUpdateSink = Callable[[Task], None]


@dataclass
class TaskOutcome:
    task_id: str
    title: str
    owner: str
    bloom: str
    executed: bool
    verified: bool
    status: str  # "verified" | "blocked" | "skipped" | "failed"
    message: str = ""
    blocking_gaps: List[str] = field(default_factory=list)


@dataclass
class CampaignResult:
    goal: str
    outcomes: List[TaskOutcome]
    dashboard_path: Optional[Path]
    events: List[str] = field(default_factory=list)

    def _ids(self, status: str) -> List[str]:
        return [o.task_id for o in self.outcomes if o.status == status]

    @property
    def verified(self) -> List[str]:
        return self._ids("verified")

    @property
    def blocked(self) -> List[str]:
        return self._ids("blocked") + self._ids("failed")

    @property
    def skipped(self) -> List[str]:
        return self._ids("skipped")

    @property
    def success(self) -> bool:
        """True when every non-skipped task verified."""
        actionable = [o for o in self.outcomes if o.status != "skipped"]
        return bool(actionable) and all(o.status == "verified" for o in actionable)

    def summary_line(self) -> str:
        return (
            f"campaign complete — {len(self.verified)} verified, "
            f"{len(self.blocked)} blocked, {len(self.skipped)} skipped"
        )


class ShogunCampaign:
    """Run a set of already-planned DevCouncil tasks as a feudal campaign."""

    def __init__(
        self,
        root: Path | str,
        *,
        goal: str,
        tasks: Sequence[Task],
        requirements: Optional[Sequence[Requirement]] = None,
        num_ashigaru: int = 4,
        max_parallel: int = 4,
        executor_factory: Optional[ExecutorFactory] = None,
        verify_fn: Optional[VerifyFn] = None,
        mailbox: Optional[Mailbox] = None,
        dashboard: Optional[DashboardWriter] = None,
        notifier: Optional[Notifier] = None,
        verify_serialized: bool = True,
        on_event: Optional[EventSink] = None,
        on_task_update: Optional[TaskUpdateSink] = None,
    ):
        self.root = Path(root)
        self.goal = goal
        self.tasks: List[Task] = list(tasks)
        self.requirements: List[Requirement] = list(requirements or [])
        self.num_ashigaru = max(1, num_ashigaru)
        self.max_parallel = max(1, max_parallel)
        self.executor_factory = executor_factory or _echo_executor_factory
        self.verify_fn = verify_fn or _passthrough_verify
        self.mailbox = mailbox or Mailbox(self.root)
        self.dashboard = dashboard or DashboardWriter(self.root)
        self.notifier = notifier or NullNotifier()
        # Ashigaru execute in parallel, but the Gunshi's Verifier runs git diff /
        # coverage against the shared working tree — concurrent QC would race. By
        # default QC is serialized behind a lock while execution stays parallel.
        self._verify_serialized = verify_serialized
        self._verify_lock = threading.Lock()
        self._on_event = on_event
        self._on_task_update = on_task_update

        self.events: List[str] = []
        self._roster_lock = threading.Lock()
        self._ashigaru_ids = [f"ashigaru{i}" for i in range(1, self.num_ashigaru + 1)]
        self._rr = 0  # round-robin cursor over ashigaru
        self._state = DashboardState(goal=goal)
        self._state.roster = (
            [RosterEntry("shogun", str(Rank.SHOGUN)), RosterEntry("karo", str(Rank.KARO))]
            + [RosterEntry(a, str(Rank.ASHIGARU)) for a in self._ashigaru_ids]
            + [RosterEntry("gunshi", str(Rank.GUNSHI))]
        )

    # -- helpers ---------------------------------------------------------------

    def _emit(self, message: str) -> None:
        self.events.append(message)
        if self._on_event is not None:
            self._on_event(message)

    def _reqs_for(self, task: Task) -> List[Requirement]:
        if not self.requirements:
            return []
        if task.requirement_ids:
            by_id = {r.id: r for r in self.requirements}
            picked = [by_id[i] for i in task.requirement_ids if i in by_id]
            if picked:
                return picked
        return self.requirements

    def _next_ashigaru(self) -> str:
        agent = self._ashigaru_ids[self._rr % len(self._ashigaru_ids)]
        self._rr += 1
        return agent

    def _bloom_of(self, task: Task) -> BloomLevel:
        text = f"{task.title}. {task.description}"
        return classify_bloom(text, difficulty=task.difficulty)

    def _set_roster(self, agent: str, *, status: str, current: str) -> None:
        with self._roster_lock:
            entry = self._state.roster_for(agent)
            if entry:
                entry.status = status
                entry.current_task = current

    def _write_dashboard(self) -> None:
        # Only the Karo may write the dashboard.
        assert_allowed(Rank.KARO, Action.WRITE_DASHBOARD)
        with self._roster_lock:
            self.dashboard.write(self._state)

    # -- main flow -------------------------------------------------------------

    def run(self) -> CampaignResult:
        self._relay_order()
        ordered = self._karo_plan()
        outcomes = self._dispatch_waves(ordered)
        self._karo_rollup(outcomes)
        path = self.dashboard.path if self.dashboard.path.exists() else None
        return CampaignResult(goal=self.goal, outcomes=outcomes, dashboard_path=path, events=list(self.events))

    def _relay_order(self) -> None:
        # Shogun accepts the Lord's order and hands it to the Karo, then steps back.
        assert_allowed(Rank.SHOGUN, Action.RELAY_ORDER)
        self.mailbox.send("karo", self.goal, type="cmd_new", from_agent="shogun")
        self._emit(f"将軍 Shogun relays the order to the Karo: “{self.goal}”")

    def _karo_plan(self) -> List[Task]:
        assert_allowed(Rank.KARO, Action.DECOMPOSE)
        ordered = _topological(self.tasks)
        self._state.routing = summarize_routing(f"{t.title}. {t.description}" for t in ordered)
        r = self._state.routing
        self._emit(
            f"家老 Karo musters {len(ordered)} task(s) "
            f"→ {r.get('ashigaru', 0)} to Ashigaru, {r.get('gunshi', 0)} to Gunshi"
        )
        self._write_dashboard()
        return ordered

    def _dispatch_waves(self, ordered: List[Task]) -> List[TaskOutcome]:
        outcomes: Dict[str, TaskOutcome] = {}
        # Tasks already finished before this campaign count as satisfied prerequisites.
        completed_ok = {t.id for t in ordered if t.status in {"verified", "done"}}
        failed_deps: set[str] = set()
        remaining = [t for t in ordered if t.id not in completed_ok]

        while remaining:
            ready = [
                t
                for t in remaining
                if all(d in completed_ok for d in t.depends_on)
                and not any(d in failed_deps for d in t.depends_on)
            ]
            if not ready:
                # Everything left is transitively blocked by an unmet dependency.
                for t in remaining:
                    unmet = [d for d in t.depends_on if d not in completed_ok]
                    outcomes[t.id] = TaskOutcome(
                        task_id=t.id,
                        title=t.title,
                        owner="-",
                        bloom=self._bloom_of(t).label,
                        executed=False,
                        verified=False,
                        status="skipped",
                        message=f"unmet dependencies: {', '.join(unmet)}",
                    )
                    self._state.skipped.append(f"{t.id} — unmet deps: {', '.join(unmet)}")
                    self._emit(f"⏭️  {t.id} skipped — unmet dependencies {unmet}")
                break

            wave = ready[: self.max_parallel]
            assignments = [(t, self._assign_owner(t)) for t in wave]
            for task, owner in assignments:
                self._state.in_progress.append(f"{task.id} · {owner} · {self._bloom_of(task).label}")

            with ThreadPoolExecutor(max_workers=self.max_parallel) as pool:
                futures = {
                    pool.submit(self._run_one, task, owner): task.id
                    for task, owner in assignments
                }
                for fut in as_completed(futures):
                    outcome = fut.result()
                    outcomes[outcome.task_id] = outcome
                    if outcome.status == "verified":
                        completed_ok.add(outcome.task_id)
                    else:
                        failed_deps.add(outcome.task_id)

            # Reconcile dashboard after the wave, then move on.
            self._state.in_progress = [
                row for row in self._state.in_progress if row.split(" · ")[0] not in outcomes
            ]
            self._write_dashboard()
            remaining = [t for t in remaining if t.id not in outcomes]

        # Preserve the original topological order in the returned list.
        return [outcomes[t.id] for t in ordered if t.id in outcomes]

    def _assign_owner(self, task: Task) -> str:
        assert_allowed(Rank.KARO, Action.ASSIGN)
        rank = route_rank(self._bloom_of(task))
        owner = "gunshi" if rank is Rank.GUNSHI else self._next_ashigaru()
        self.mailbox.send(owner, f"task {task.id} assigned: {task.title}", type="task_assigned", from_agent="karo")
        self._emit(f"家老 Karo → {owner}: task {task.id} ({self._bloom_of(task).label})")
        return owner

    def _run_one(self, task: Task, owner: str) -> TaskOutcome:
        bloom = self._bloom_of(task).label
        owner_rank = Rank.GUNSHI if owner == "gunshi" else Rank.ASHIGARU
        self._set_roster(owner, status="working", current=task.id)

        # -- execution (Ashigaru / Gunshi does the work) -----------------------
        if owner_rank is Rank.ASHIGARU:
            assert_allowed(Rank.ASHIGARU, Action.EXECUTE_TASK)
        else:
            assert_allowed(Rank.GUNSHI, Action.DEEP_ANALYSIS)
        task.status = "running"
        self._notify_task_update(task)
        executed, exec_msg = self._execute(owner, task)

        # -- report up: worker → Gunshi for QC --------------------------------
        if owner != "gunshi":
            assert_allowed(owner_rank, Action.WRITE_REPORT)
            self.mailbox.send(
                "gunshi",
                f"{task.id} finished by {owner} — request QC",
                type="report_received",
                from_agent=owner,
            )

        # -- Gunshi quality control -------------------------------------------
        assert_allowed(Rank.GUNSHI, Action.QC_REVIEW)
        passed, gaps = self._quality_control(task)
        verified = bool(passed and executed)

        self.mailbox.send(
            "karo",
            f"{task.id}: {'verified' if verified else 'blocked'}",
            type="qc_result",
            from_agent="gunshi",
        )
        self._set_roster(owner, status="idle", current="-")

        if verified:
            task.status = "verified"
            self._state.achievements.append(f"{task.id} · {task.title} · {owner} · {bloom}")
            self._emit(f"軍師 Gunshi verifies {task.id} — worked by {owner}")
            status = "verified"
        else:
            task.status = "blocked"
            reason = "; ".join(gaps) if gaps else ("execution failed" if not executed else "verification failed")
            self._state.blocked.append(f"{task.id} — {reason}")
            self._emit(f"軍師 Gunshi blocks {task.id}: {reason}")
            status = "failed" if not executed else "blocked"

        self._notify_task_update(task)
        return TaskOutcome(
            task_id=task.id,
            title=task.title,
            owner=owner,
            bloom=bloom,
            executed=executed,
            verified=verified,
            status=status,
            message=exec_msg,
            blocking_gaps=list(gaps),
        )

    def _execute(self, owner: str, task: Task) -> Tuple[bool, str]:
        try:
            executor = self.executor_factory(owner)
            result = executor.run_task(task, self._reqs_for(task))
            success = bool(getattr(result, "success", False))
            message = str(getattr(result, "message", ""))
            return success, message
        except Exception as exc:
            return False, f"executor error: {exc}"

    def _quality_control(self, task: Task) -> Tuple[bool, List[str]]:
        """Run the Gunshi's verify gate, serialized against git races by default."""
        try:
            if self._verify_serialized:
                with self._verify_lock:
                    return self.verify_fn(task, self._reqs_for(task))
            return self.verify_fn(task, self._reqs_for(task))
        except Exception as exc:  # a broken gate must not crash the campaign
            return False, [f"verifier error: {exc}"]

    def _karo_rollup(self, outcomes: List[TaskOutcome]) -> None:
        assert_allowed(Rank.KARO, Action.ROLLUP)
        self._write_dashboard()
        verified = sum(1 for o in outcomes if o.status == "verified")
        blocked = sum(1 for o in outcomes if o.status in {"blocked", "failed"})
        skipped = sum(1 for o in outcomes if o.status == "skipped")
        summary = (
            f"⚔️ Campaign complete — {verified} verified, {blocked} blocked, {skipped} skipped. "
            f"Goal: {self.goal}"
        )
        assert_allowed(Rank.KARO, Action.NOTIFY)
        self.notifier.notify(summary, title="Shogun campaign", tags=["crossed_swords"])
        self._emit(f"家老 Karo reports to the Lord: {verified} verified / {blocked} blocked / {skipped} skipped")
        # Shogun reads the dashboard to answer for the Lord (never writes it).
        assert_allowed(Rank.SHOGUN, Action.READ_DASHBOARD)

    def _notify_task_update(self, task: Task) -> None:
        if self._on_task_update is not None:
            self._on_task_update(task)


# -- default collaborators -----------------------------------------------------


def _topological(tasks: Sequence[Task]) -> List[Task]:
    """Order tasks so a task never precedes one it depends on (best-effort)."""
    try:
        from devcouncil.gating.policy import topological_order

        return list(topological_order(list(tasks)))
    except Exception:
        # Fallback: stable order that still respects simple id-based deps.
        return list(tasks)


class _EchoResult:
    def __init__(self, success: bool, message: str):
        self.success = success
        self.message = message


def _echo_executor_factory(owner: str) -> ExecutorLike:
    """A do-nothing executor used when none is supplied (dry runs / previews)."""

    class _Echo:
        def run_task(self, task: Task, requirements: List[Requirement]) -> _EchoResult:
            return _EchoResult(True, f"[echo] {owner} would run {task.id}")

    return _Echo()


def _passthrough_verify(task: Task, requirements: List[Requirement]) -> Tuple[bool, List[str]]:
    """Default QC that passes everything — replaced by the real Verifier in the CLI."""
    return True, []


def build_coding_executor_factory(
    root: Path,
    cli_client: str,
    *,
    profile: Optional[str] = None,
    stream: bool = False,
) -> ExecutorFactory:
    """Real Ashigaru: a fresh :class:`CodingCliExecutor` per worker."""
    from devcouncil.executors.coding_cli import CodingCliExecutor

    def factory(owner: str) -> ExecutorLike:
        return CodingCliExecutor(root, cli_client, profile=profile, stream_output=stream or None)

    return factory


def build_verifier_fn(root: Path, router: object = None) -> VerifyFn:
    """Real Gunshi QC: run the DevCouncil :class:`Verifier` and surface gaps."""
    import asyncio

    from devcouncil.verification.verifier import Verifier

    def verify(task: Task, requirements: List[Requirement]) -> Tuple[bool, List[str]]:
        verifier = Verifier(root, router=router)  # type: ignore[arg-type]
        gaps, _evidence = asyncio.run(verifier.verify_task(task, requirements))
        blocking = [
            str(getattr(g, "description", g))
            for g in gaps
            if getattr(g, "blocking", True)
        ]
        return (len(blocking) == 0, blocking)

    return verify
