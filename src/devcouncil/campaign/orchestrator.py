"""The Director campaign — hierarchical, parallel task execution over DevCouncil.

``Campaign`` layers the multi-agent command hierarchy on top of DevCouncil's
existing machinery:

* the **Director** relays the order (goal) to the Coordinator via the mailbox;
* the **Coordinator** orders the plan's tasks topologically, classifies each by Bloom
  level, and dispatches them to the worker pool **in parallel** — never releasing
  a task before its ``depends_on`` prerequisites are verified;
* each **Worker** runs one task through an :class:`~devcouncil.execution.executor.Executor`
  (Bloom Analyze-and-above tasks are routed to the **Reviewer** instead);
* the **Reviewer** quality-controls every finished task through the Verifier and
  reports the verdict back up to the Coordinator, who updates the dashboard and pushes a
  notification to the operator.

Every collaborator that touches the outside world — the coding executor, the
verifier, the notifier — is injected, so the whole control flow (routing,
dependency waves, QC gating, role-hierarchy enforcement, dashboard/mailbox
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
from devcouncil.campaign.bloom import BloomLevel, classify_bloom, route_rank, summarize_routing
from devcouncil.campaign.dashboard import DashboardState, DashboardWriter, RosterEntry
from devcouncil.campaign.mailbox import Mailbox
from devcouncil.campaign.notify import NullNotifier, Notifier
from devcouncil.campaign.roles import Action, Rank, assert_allowed

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
    halted: bool = False
    halt_reason: str = ""

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
        """True when every non-skipped task verified and the campaign was not halted."""
        if self.halted:
            return False
        actionable = [o for o in self.outcomes if o.status != "skipped"]
        return bool(actionable) and all(o.status == "verified" for o in actionable)

    def summary_line(self) -> str:
        return (
            f"campaign complete — {len(self.verified)} verified, "
            f"{len(self.blocked)} blocked, {len(self.skipped)} skipped"
        )


class Campaign:
    """Run a set of already-planned DevCouncil tasks as a multi-agent campaign."""

    def __init__(
        self,
        root: Path | str,
        *,
        goal: str,
        tasks: Sequence[Task],
        requirements: Optional[Sequence[Requirement]] = None,
        num_workers: int = 4,
        max_parallel: int = 4,
        executor_factory: Optional[ExecutorFactory] = None,
        verify_fn: Optional[VerifyFn] = None,
        mailbox: Optional[Mailbox] = None,
        dashboard: Optional[DashboardWriter] = None,
        notifier: Optional[Notifier] = None,
        verify_serialized: bool = True,
        on_event: Optional[EventSink] = None,
        on_task_update: Optional[TaskUpdateSink] = None,
        use_leases: bool = False,
        cost_budget_usd: Optional[float] = None,
    ):
        self.root = Path(root)
        self.goal = goal
        self.tasks: List[Task] = list(tasks)
        self.requirements: List[Requirement] = list(requirements or [])
        self.num_workers = max(1, num_workers)
        self.max_parallel = max(1, max_parallel)
        self.executor_factory = executor_factory or _echo_executor_factory
        self.verify_fn = verify_fn or _passthrough_verify
        self.mailbox = mailbox or Mailbox(self.root)
        self.dashboard = dashboard or DashboardWriter(self.root)
        self.notifier = notifier or NullNotifier()
        # Worker execute in parallel, but the Reviewer's Verifier runs git diff /
        # coverage against the shared working tree — concurrent QC would race. By
        # default QC is serialized behind a lock while execution stays parallel.
        self._verify_serialized = verify_serialized
        self._verify_lock = threading.Lock()
        self._state_lock = threading.Lock()
        self._on_event = on_event
        self._on_task_update = on_task_update
        self._use_leases = use_leases
        self._cost_budget_usd = cost_budget_usd
        self._lease_tokens: Dict[str, str] = {}

        self.events: List[str] = []
        self._roster_lock = threading.Lock()
        self._worker_ids = [f"worker{i}" for i in range(1, self.num_workers + 1)]
        self._rr = 0  # round-robin cursor over worker
        self._state = DashboardState(goal=goal)
        self._state.roster = (
            [RosterEntry("director", str(Rank.DIRECTOR)), RosterEntry("coordinator", str(Rank.COORDINATOR))]
            + [RosterEntry(a, str(Rank.WORKER)) for a in self._worker_ids]
            + [RosterEntry("reviewer", str(Rank.REVIEWER))]
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

    def _next_worker(self) -> str:
        agent = self._worker_ids[self._rr % len(self._worker_ids)]
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
        # Only the Coordinator may write the dashboard.
        assert_allowed(Rank.COORDINATOR, Action.WRITE_DASHBOARD)
        with self._roster_lock:
            self.dashboard.write(self._state)

    # -- main flow -------------------------------------------------------------

    def run(self) -> CampaignResult:
        self._relay_order()
        self._state.total_tasks = len(self.tasks)
        self._state.cost_budget_usd = self._cost_budget_usd
        self._refresh_cost()
        ordered = self._coordinator_plan()
        outcomes, halted, halt_reason = self._dispatch_waves(ordered)
        self._coordinator_rollup(outcomes)
        path = self.dashboard.path if self.dashboard.path.exists() else None
        return CampaignResult(
            goal=self.goal,
            outcomes=outcomes,
            dashboard_path=path,
            events=list(self.events),
            halted=halted,
            halt_reason=halt_reason,
        )

    def _relay_order(self) -> None:
        # Director accepts the operator's order and hands it to the Coordinator, then steps back.
        assert_allowed(Rank.DIRECTOR, Action.RELAY_ORDER)
        self.mailbox.send("coordinator", self.goal, type="cmd_new", from_agent="director")
        self._emit(f"Director relays the order to the Coordinator: “{self.goal}”")

    def _coordinator_plan(self) -> List[Task]:
        assert_allowed(Rank.COORDINATOR, Action.DECOMPOSE)
        ordered = _topological(self.tasks)
        self._state.routing = summarize_routing(f"{t.title}. {t.description}" for t in ordered)
        r = self._state.routing
        self._emit(
            f"Coordinator schedules {len(ordered)} task(s) "
            f"→ {r.get('worker', 0)} to Worker, {r.get('reviewer', 0)} to Reviewer"
        )
        self._write_dashboard()
        return ordered

    def _dispatch_waves(self, ordered: List[Task]) -> Tuple[List[TaskOutcome], bool, str]:
        outcomes: Dict[str, TaskOutcome] = {}
        halted = False
        halt_reason = ""
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

            wave = self._select_wave(ready)
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
                        self._state.completed_tasks += 1
                    else:
                        failed_deps.add(outcome.task_id)

            # Reconcile dashboard after the wave, then move on.
            self._state.in_progress = [
                row for row in self._state.in_progress if row.split(" · ")[0] not in outcomes
            ]
            self._write_dashboard()
            self._refresh_cost()
            if self._over_budget():
                halted = True
                halt_reason = "cost budget exceeded"
                self._emit("Campaign halted — cost budget exceeded.")
                for t in remaining:
                    if t.id in outcomes:
                        continue
                    outcomes[t.id] = TaskOutcome(
                        task_id=t.id,
                        title=t.title,
                        owner="-",
                        bloom=self._bloom_of(t).label,
                        executed=False,
                        verified=False,
                        status="skipped",
                        message="campaign halted: cost budget exceeded",
                    )
                    self._state.skipped.append(f"{t.id} — campaign halted: cost budget exceeded")
                    self._emit(f"⏭️  {t.id} skipped — campaign halted: cost budget exceeded")
                break
            remaining = [t for t in remaining if t.id not in outcomes]

        # Preserve the original topological order in the returned list.
        return [outcomes[t.id] for t in ordered if t.id in outcomes], halted, halt_reason

    def _writable_planned_paths(self, task: Task) -> set[str]:
        return {
            pf.path.replace("\\", "/")
            for pf in task.planned_files
            if pf.allowed_change != "read_only"
        }

    def _select_wave(self, ready: List[Task]) -> List[Task]:
        """Pick up to ``max_parallel`` ready tasks with no overlapping writable scopes."""
        if self.max_parallel <= 1:
            return ready[:1]

        selected: List[Task] = []
        claimed: set[str] = set()
        deferred: List[Task] = []

        for task in ready:
            if len(selected) >= self.max_parallel:
                deferred.append(task)
                continue
            paths = self._writable_planned_paths(task)
            if paths and paths & claimed:
                deferred.append(task)
                continue
            selected.append(task)
            claimed |= paths

        if deferred and self.max_parallel > 1:
            overlap_ids = [t.id for t in deferred if self._writable_planned_paths(t) & claimed]
            if overlap_ids:
                self._emit(
                    "Parallel dispatch deferred tasks with overlapping planned_files "
                    f"({', '.join(overlap_ids)}) — they share the git working tree."
                )
        return selected

    def _assign_owner(self, task: Task) -> str:
        assert_allowed(Rank.COORDINATOR, Action.ASSIGN)
        rank = route_rank(self._bloom_of(task))
        owner = "reviewer" if rank is Rank.REVIEWER else self._next_worker()
        self.mailbox.send(owner, f"task {task.id} assigned: {task.title}", type="task_assigned", from_agent="coordinator")
        self._emit(f"Coordinator → {owner}: task {task.id} ({self._bloom_of(task).label})")
        return owner

    def _run_one(self, task: Task, owner: str) -> TaskOutcome:
        bloom = self._bloom_of(task).label
        owner_rank = Rank.REVIEWER if owner == "reviewer" else Rank.WORKER
        self._set_roster(owner, status="working", current=task.id)

        # -- execution (Worker / Reviewer does the work) -----------------------
        if owner_rank is Rank.WORKER:
            assert_allowed(Rank.WORKER, Action.EXECUTE_TASK)
        else:
            assert_allowed(Rank.REVIEWER, Action.DEEP_ANALYSIS)
        task.status = "running"
        self._notify_task_update(task)
        executed, exec_msg = self._execute(owner, task)

        # -- report up: worker → Reviewer for QC --------------------------------
        if owner != "reviewer":
            assert_allowed(owner_rank, Action.WRITE_REPORT)
            self.mailbox.send(
                "reviewer",
                f"{task.id} finished by {owner} — request QC",
                type="report_received",
                from_agent=owner,
            )

        # -- Reviewer quality control -------------------------------------------
        assert_allowed(Rank.REVIEWER, Action.QC_REVIEW)
        passed, gaps = self._quality_control(task)
        verified = bool(passed and executed)

        self.mailbox.send(
            "coordinator",
            f"{task.id}: {'verified' if verified else 'blocked'}",
            type="qc_result",
            from_agent="reviewer",
        )
        self._set_roster(owner, status="idle", current="-")

        if verified:
            task.status = "verified"
            with self._state_lock:
                self._state.achievements.append(f"{task.id} · {task.title} · {owner} · {bloom}")
            self._emit(f"Reviewer verifies {task.id} — worked by {owner}")
            status = "verified"
        else:
            task.status = "blocked"
            reason = "; ".join(gaps) if gaps else ("execution failed" if not executed else "verification failed")
            with self._state_lock:
                self._state.blocked.append(f"{task.id} — {reason}")
            self._emit(f"Reviewer blocks {task.id}: {reason}")
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
        lease_token: Optional[str] = None
        if self._use_leases:
            lease_token = self._checkout_task(owner, task.id)
            if lease_token is None:
                return False, f"lease checkout failed for {task.id}"
        try:
            executor = self.executor_factory(owner)
            result = executor.run_task(task, self._reqs_for(task))
            success = bool(getattr(result, "success", False))
            message = str(getattr(result, "message", ""))
            return success, message
        except Exception as exc:
            return False, f"executor error: {exc}"
        finally:
            if self._use_leases and lease_token:
                self._release_task(task.id, lease_token)

    def _checkout_task(self, owner: str, task_id: str) -> Optional[str]:
        from devcouncil.execution.lease_ops import checkout_task_payload

        payload = checkout_task_payload(self.root, task_id=task_id, client_id=f"campaign:{owner}")
        if not payload.get("ok"):
            self._emit(f"Lease checkout failed for {task_id}: {payload.get('error')}")
            return None
        token = str(payload["lease_token"])
        with self._state_lock:
            self._lease_tokens[task_id] = token
        return token

    def _release_task(self, task_id: str, lease_token: str) -> None:
        from devcouncil.execution.lease_ops import release_task_payload

        release_task_payload(self.root, task_id=task_id, lease_token=lease_token)
        with self._state_lock:
            self._lease_tokens.pop(task_id, None)

    def _refresh_cost(self) -> None:
        try:
            from devcouncil.telemetry.cost import group_cost

            self._state.cost_usd = float(group_cost(self.root).get("total_cost", 0.0))
        except Exception:
            pass

    def _over_budget(self) -> bool:
        if self._cost_budget_usd is None:
            return False
        return self._state.cost_usd > self._cost_budget_usd

    def _quality_control(self, task: Task) -> Tuple[bool, List[str]]:
        """Run the Reviewer's verify gate, serialized against git races by default."""
        try:
            if self._verify_serialized:
                with self._verify_lock:
                    return self.verify_fn(task, self._reqs_for(task))
            return self.verify_fn(task, self._reqs_for(task))
        except Exception as exc:  # a broken gate must not crash the campaign
            return False, [f"verifier error: {exc}"]

    def _coordinator_rollup(self, outcomes: List[TaskOutcome]) -> None:
        assert_allowed(Rank.COORDINATOR, Action.ROLLUP)
        self._write_dashboard()
        verified = sum(1 for o in outcomes if o.status == "verified")
        blocked = sum(1 for o in outcomes if o.status in {"blocked", "failed"})
        skipped = sum(1 for o in outcomes if o.status == "skipped")
        summary = (
            f"Campaign complete — {verified} verified, {blocked} blocked, {skipped} skipped. "
            f"Goal: {self.goal}"
        )
        assert_allowed(Rank.COORDINATOR, Action.NOTIFY)
        self.notifier.notify(summary, title="Director campaign", tags=["white_check_mark"])
        self._emit(f"Coordinator reports to the operator: {verified} verified / {blocked} blocked / {skipped} skipped")
        # Director reads the dashboard to answer for the operator (never writes it).
        assert_allowed(Rank.DIRECTOR, Action.READ_DASHBOARD)

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
    """Real Worker: a fresh :class:`CodingCliExecutor` per worker."""
    from devcouncil.executors.coding_cli import CodingCliExecutor

    def factory(owner: str) -> ExecutorLike:
        return CodingCliExecutor(root, cli_client, profile=profile, stream_output=stream or None)

    return factory


def build_verifier_fn(root: Path, router: object = None) -> VerifyFn:
    """Real Reviewer QC: run the DevCouncil :class:`Verifier` and surface gaps."""
    import asyncio

    from devcouncil.verification.verifier import Verifier

    # asyncio.run() must not be invoked from worker pool threads; funnel async
    # verify through a single dedicated thread with its own event loop.
    _async_executor = ThreadPoolExecutor(max_workers=1, thread_name_prefix="campaign-qc")

    def _run_verify(task: Task, requirements: List[Requirement]) -> Tuple[bool, List[str]]:
        verifier = Verifier(root, router=router)  # type: ignore[arg-type]

        def _thread_main() -> Tuple[bool, List[str]]:
            loop = asyncio.new_event_loop()
            try:
                asyncio.set_event_loop(loop)
                gaps, _evidence = loop.run_until_complete(verifier.verify_task(task, requirements))
            finally:
                loop.close()
            blocking = [
                str(getattr(g, "description", g))
                for g in gaps
                if getattr(g, "blocking", True)
            ]
            return (len(blocking) == 0, blocking)

        return _async_executor.submit(_thread_main).result()

    return _run_verify
