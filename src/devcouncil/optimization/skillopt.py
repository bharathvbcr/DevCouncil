"""SkillOpt: a text-space optimization loop for DevCouncil skill + guidance documents.

This is a DevCouncil-native implementation of the loop popularized by Microsoft
SkillOpt (https://github.com/microsoft/SkillOpt): a document is treated as the
trainable state of a frozen agent and improved over epochs by

    rollout -> reflect -> aggregate -> propose-edits -> update -> validate -> evaluate

Unlike weight-space training, the only thing that changes is markdown text. DevCouncil
has **two** such artifacts that steer a coding agent: the *guidance* (an agent
profile's prompt preamble) and the *skill* document. They are optimized **together,
simultaneously** — each epoch the optimizer proposes a single batch of bounded edits
that may touch either document, the combined candidate runs the rollout, and the
*whole* candidate is accepted only when it strictly improves a held-out validation
score (the validation gate). Co-optimizing them in one loop keeps guidance and skill
mutually consistent instead of drifting apart across two separate runs.

Each epoch a *target* agent runs the current documents on the training tasks
(rollout), the trajectories are scored (reflect), the low-scoring cases are aggregated
into feedback, and an *optimizer* model proposes a small number of add/delete/replace
edits across both documents. Rejected edits are remembered so the optimizer doesn't
re-propose them.

Both the rollout and the edit proposer are pluggable callables so the loop is fully
testable without a live model; :func:`make_llm_rollout` and :func:`make_llm_optimizer`
provide the default :class:`~devcouncil.llm.router.ModelRouter`-backed implementations.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import logging
from collections.abc import Awaitable, Callable
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Literal

from pydantic import BaseModel, Field

# Reuse the small list-coercion helper from the agent-profile optimizer so both
# optimizers share one eval-dataset field contract (required_terms, forbidden_terms…).
from devcouncil.optimization.gepa_agent import _string_list

logger = logging.getLogger(__name__)

# The two co-optimized document slots. ``guidance`` is the agent profile preamble that
# steers the agent; ``skill`` is the engineering skill document. Edits without an
# explicit target default to ``skill``.
GUIDANCE = "guidance"
SKILL = "skill"
DOC_TARGETS = (GUIDANCE, SKILL)

# ---------------------------------------------------------------------------
# Edit model — the optimizer's bounded action space (add / delete / replace),
# targeting either the guidance or the skill document.
# ---------------------------------------------------------------------------


class SkillEdit(BaseModel):
    """One bounded edit to a co-optimized document body.

    ``target`` selects which document the edit applies to (``guidance`` or ``skill``).
    ``replace`` swaps the first occurrence of ``find`` with ``text``; ``delete``
    removes the first occurrence of ``find``; ``add`` inserts ``text`` after the
    first occurrence of ``find`` (or appends it when ``find`` is empty). Anchoring on
    existing text keeps every edit local and reviewable, which is what lets the
    validation gate attribute a score change to a specific batch of edits.
    """

    op: Literal["add", "delete", "replace"]
    target: Literal["guidance", "skill"] = SKILL
    find: str = ""
    text: str = ""
    reason: str = ""


class SkillEditProposal(BaseModel):
    """A batch of edits proposed by the optimizer for one epoch, spanning both docs."""

    edits: list[SkillEdit] = Field(default_factory=list)
    rationale: str = ""


def _edit_signature(edit: SkillEdit) -> str:
    """Stable identity for the rejected-edit buffer (target + op + anchor + payload).

    Fields are JSON-encoded rather than space-joined so distinct edits can't collide
    on an ambiguous field boundary (e.g. find='a', text='b c' vs find='a b', text='c').
    A collision would let a rejected edit silently suppress a different, untried one.
    """
    raw = json.dumps([edit.target, edit.op, edit.find.strip(), edit.text.strip()])
    return hashlib.sha256(raw.encode("utf-8")).hexdigest()[:16]


def _batch_signature(edits: list[SkillEdit]) -> str:
    """Order-independent identity for a *combination* of edits.

    When the validation gate rejects a multi-edit batch, only this combined signature is
    blacklisted — not each component — so a genuinely good edit that was merely dragged
    below the gate by a bad partner stays eligible to be re-proposed in a different
    combination. The select step uses it to avoid re-applying the identical losing batch."""
    raw = json.dumps(sorted(_edit_signature(edit) for edit in edits))
    return hashlib.sha256(raw.encode("utf-8")).hexdigest()[:16]


def _apply_one(body: str, edit: SkillEdit) -> str | None:
    """Apply a single edit to one document body; return new body or None if no-op."""
    find = edit.find
    if edit.op == "add":
        text = edit.text.strip("\n")
        if not text:
            return None
        if find and find in body:
            idx = body.index(find) + len(find)
            candidate = body[:idx] + "\n\n" + text + body[idx:]
        elif not find:
            candidate = body.rstrip("\n") + "\n\n" + text + "\n"
        else:
            return None  # anchor requested but absent -> skip
    elif edit.op == "delete":
        if not find or find not in body:
            return None
        candidate = body.replace(find, "", 1)
    else:  # replace
        if not find or find not in body:
            return None
        candidate = body.replace(find, edit.text, 1)
    return None if candidate == body else candidate


def _is_permanently_redundant(edit: SkillEdit, docs: dict[str, str]) -> bool:
    """Whether an edit no-ops for a reason that can never change as the docs evolve.

    Used to decide what the no-op branch may add to the rejected-edit buffer: an empty
    append or an identity replace will *always* no-op and is safe to blacklist forever.
    An edit that no-ops only because its ``find`` anchor isn't present *yet* is NOT
    redundant — a later accepted edit may introduce that anchor — so it stays eligible.
    """
    body = docs.get(edit.target)
    if body is None:
        return False
    if edit.op == "add":
        return not edit.text.strip("\n")  # only an empty append is permanently a no-op
    if edit.op == "replace":
        return bool(edit.find) and edit.find in body and edit.find == edit.text
    return False  # a delete only no-ops when its anchor is absent (may appear later)


def apply_edits(docs: dict[str, str], edits: list[SkillEdit]) -> tuple[dict[str, str], list[SkillEdit]]:
    """Apply ``edits`` to the right document in ``docs``, skipping ones that no-op.

    Returns updated copies of the document bodies and the subset of edits that
    actually changed something. A hallucinated anchor (or an edit targeting a missing
    document) is skipped rather than corrupting the documents.
    """
    updated = dict(docs)
    applied: list[SkillEdit] = []
    for edit in edits:
        if edit.target not in updated:
            continue
        result = _apply_one(updated[edit.target], edit)
        if result is None:
            continue
        updated[edit.target] = result
        applied.append(edit)
    return updated, applied


# ---------------------------------------------------------------------------
# Loop configuration and records.
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class SkillOptConfig:
    """Hyper-parameters for the SkillOpt loop.

    ``max_edits_per_epoch`` is the textual learning-rate budget — the cap on how many
    edits may land in a single epoch (across both documents combined). ``min_improvement``
    is the validation gate margin; the default ``0.0`` means *strictly greater*
    validation score is required to accept a candidate. ``rollout_concurrency`` bounds how
    many rollouts in one evaluation run concurrently (rollouts are independent LLM calls;
    results are still collected in task order so the mean and feedback stay deterministic).
    """

    epochs: int = 5
    max_edits_per_epoch: int = 3
    val_fraction: float = 0.5
    min_improvement: float = 0.0
    seed: int = 0
    rollout_concurrency: int = 8
    # Stop after this many consecutive *unproductive* epochs (a no-op proposal or a batch
    # already known-rejected). A stuck optimizer otherwise burns one LLM call per remaining
    # epoch for zero gain. Gate-rejected epochs are productive (they grow the rejected set
    # and force exploration) and do NOT count toward this patience.
    noop_patience: int = 2


@dataclass
class EpochRecord:
    epoch: int
    train_score: float
    val_score_before: float
    val_score_after: float | None
    proposed_edits: int
    applied_edits: int
    edited_targets: list[str]
    accepted: bool
    note: str = ""


@dataclass
class SkillOptResult:
    skill_name: str
    seed_docs: dict[str, str]
    best_docs: dict[str, str]
    seed_val_score: float
    best_val_score: float
    epochs: list[EpochRecord] = field(default_factory=list)
    accepted_edit_count: int = 0
    rejected_edit_count: int = 0
    train_size: int = 0
    val_size: int = 0
    artifact_path: Path | None = None
    applied: bool = False

    @property
    def improved(self) -> bool:
        return self.best_val_score > self.seed_val_score

    @property
    def best_skill_body(self) -> str:
        return self.best_docs.get(SKILL, "")

    @property
    def best_guidance_body(self) -> str:
        return self.best_docs.get(GUIDANCE, "")


# Pluggable hooks. ``RolloutFn(docs, task) -> trajectory text`` where ``docs`` maps
# each document name to its current body; the trajectory is then handed to
# ``ScoreFn(task, trajectory) -> score`` which should return a value in [0, 1] (the loop
# clamps to that range, so a custom scorer with a wider range can't break the gate).
RolloutFn = Callable[[dict[str, str], dict[str, Any]], Awaitable[str]]
ScoreFn = Callable[[dict[str, Any], str], float]
OptimizerFn = Callable[[dict[str, str], list[dict[str, Any]], list[str], str], Awaitable[SkillEditProposal]]


def _clamp01(value: float) -> float:
    return 0.0 if value < 0.0 else 1.0 if value > 1.0 else value


# ---------------------------------------------------------------------------
# Default scoring — term coverage on the rollout trajectory.
# ---------------------------------------------------------------------------


def default_score(task: dict[str, Any], trajectory: str) -> float:
    """Score a rollout by required/forbidden term coverage and expected substrings.

    Returns a neutral ``0.5`` when a task carries no scoring signal so an unlabeled
    task never silently drags the mean to zero.
    """
    text = trajectory.lower()
    required = _string_list(task, "required_terms", "expected_terms", "must_include")
    forbidden = _string_list(task, "forbidden_terms", "must_avoid")
    expected = _string_list(task, "expected", "expected_output", "expected_substrings")

    if not (required or forbidden or expected):
        return 0.5
    if not trajectory.strip():
        return 0.0

    parts: list[tuple[float, float]] = []  # (score, weight)
    if required:
        hits = sum(1 for term in required if term.lower() in text)
        parts.append((hits / len(required), 0.6))
    if expected:
        hits = sum(1 for term in expected if term.lower() in text)
        parts.append((hits / len(expected), 0.3))
    if forbidden:
        hits = sum(1 for term in forbidden if term.lower() in text)
        parts.append((1.0 - hits / len(forbidden), 0.2))

    total_weight = sum(weight for _, weight in parts)
    score = sum(value * weight for value, weight in parts) / total_weight
    return max(0.0, min(1.0, score))


def _task_prompt(task: dict[str, Any]) -> str:
    for key in ("prompt", "task", "goal", "instruction", "question"):
        value = task.get(key)
        if isinstance(value, str) and value.strip():
            return value.strip()
    return ""


def _split_dataset(
    dataset: list[dict[str, Any]], val_fraction: float, seed: int
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """Deterministically split into (train, val).

    A stable hash of each example id (salted by ``seed``) decides the side, so the
    same dataset + seed always yields the same split without importing ``random`` or
    depending on input order. At least one example is always kept on each side.
    """
    if len(dataset) < 2:
        return dataset, dataset
    val: list[dict[str, Any]] = []
    train: list[dict[str, Any]] = []
    for index, item in enumerate(dataset):
        key = f"{seed}:{item.get('id', index)}".encode("utf-8")
        bucket = int.from_bytes(hashlib.sha256(key).digest()[:4], "big") / 0xFFFFFFFF
        (val if bucket < val_fraction else train).append(item)
    # Keep at least one example on each side. Both fallbacks *move* (pop) rather than
    # copy, so the same example never lands in both splits — a copy would leak a
    # validation example into training and quietly defeat the held-out gate.
    if not val:
        val.append(train.pop())
    if not train:
        train.append(val.pop())
    return train, val


# ---------------------------------------------------------------------------
# The loop.
# ---------------------------------------------------------------------------


async def _evaluate(
    docs: dict[str, str],
    tasks: list[dict[str, Any]],
    rollout: RolloutFn,
    score: ScoreFn,
    concurrency: int = 8,
) -> tuple[float, list[tuple[dict[str, Any], str, float]]]:
    # Rollouts are independent LLM round-trips, so run them concurrently under a semaphore.
    # ``gather`` preserves input order, so trajectories realign with ``tasks`` and the mean
    # and downstream ``_reflect`` ordering remain deterministic regardless of finish order.
    if not tasks:
        return 0.0, []
    sem = asyncio.Semaphore(max(1, concurrency))

    async def _one(task: dict[str, Any]) -> str:
        async with sem:
            try:
                return await rollout(docs, task)
            except Exception:
                # A single flaky rollout (transient network error, a raising custom
                # RolloutFn) must not abort the whole run and discard every accepted
                # improvement. Degrade to an empty trajectory — deterministically scored
                # 0.0 for a labeled task — instead of propagating.
                logger.warning("rollout failed for task %s; scoring it 0.0", task.get("id"), exc_info=True)
                return ""

    trajectories = await asyncio.gather(*(_one(task) for task in tasks))
    # Clamp into the documented [0, 1] contract so a misbehaving custom ScoreFn can't break
    # the loop's control flow (the >= 1.0 early-stop and the _reflect "perfect" threshold).
    rollouts = [
        (task, traj, _clamp01(score(task, traj))) for task, traj in zip(tasks, trajectories)
    ]
    mean = sum(item[2] for item in rollouts) / len(rollouts)
    return mean, rollouts


def _term_gaps(task: dict[str, Any], trajectory: str) -> dict[str, list[str]]:
    """Per-case scoring gaps: which expected terms are missing and which forbidden ones
    are present in the trajectory. This is the *actionable* signal — it tells the
    optimizer exactly what to add or remove — versus dumping the full term lists. Derived
    from the same term contract the default scorer uses; empty for custom datasets with no
    term fields (those fall back to the trajectory excerpt)."""
    text = trajectory.lower()
    required = _string_list(task, "required_terms", "expected_terms", "must_include")
    expected = _string_list(task, "expected", "expected_output", "expected_substrings")
    forbidden = _string_list(task, "forbidden_terms", "must_avoid")
    return {
        "missing_required": [t for t in required if t.lower() not in text],
        "missing_expected": [t for t in expected if t.lower() not in text],
        "present_forbidden": [t for t in forbidden if t.lower() in text],
    }


def _reflect(rollouts: list[tuple[dict[str, Any], str, float]]) -> list[dict[str, Any]]:
    """Aggregate the weakest rollouts into compact feedback for the optimizer.

    Worst-scoring first, so the optimizer spends its bounded edit budget on the
    cases that are actually failing. Each case carries the concrete scoring gaps
    (missing/forbidden terms) so the optimizer can target them directly.
    """
    feedback: list[dict[str, Any]] = []
    for task, trajectory, value in sorted(rollouts, key=lambda item: item[2]):
        if value >= 1.0:
            continue
        case = {
            "task": _task_prompt(task),
            "score": round(value, 3),
            "observed_failure": task.get("observed_failure", ""),
            "desired_behavior": task.get("desired_behavior", ""),
            "trajectory_excerpt": trajectory.strip()[:400],
        }
        gaps = _term_gaps(task, trajectory)
        case.update({key: terms for key, terms in gaps.items() if terms})  # only non-empty gaps
        feedback.append(case)
    return feedback


async def optimize_skill(
    *,
    skill_name: str,
    docs: dict[str, str],
    dataset: list[dict[str, Any]],
    rollout: RolloutFn,
    optimizer: OptimizerFn,
    score: ScoreFn = default_score,
    config: SkillOptConfig | None = None,
) -> SkillOptResult:
    """Co-optimize a skill document and its guidance preamble over the SkillOpt loop.

    ``docs`` maps document names (typically :data:`GUIDANCE` and :data:`SKILL`) to their
    current bodies; both are improved **simultaneously**. ``rollout`` runs the target
    agent with a candidate set of documents on one task; ``optimizer`` turns aggregated
    feedback into a :class:`SkillEditProposal` whose edits may target either document.
    Neither needs a live model — inject fakes to test the loop, or use
    :func:`make_llm_rollout` / :func:`make_llm_optimizer` for the router-backed defaults.
    """
    cfg = config or SkillOptConfig()
    seed_docs = dict(docs)
    train, val = _split_dataset(dataset, cfg.val_fraction, cfg.seed)

    seed_val_score, _ = await _evaluate(seed_docs, val, rollout, score, cfg.rollout_concurrency)
    best_docs = dict(seed_docs)
    best_val_score = seed_val_score

    rejected: set[str] = set()
    rejected_count = 0
    accepted_count = 0
    epochs: list[EpochRecord] = []
    # Memoized train evaluation for the current best_docs. The train rollouts are the
    # dominant cost of the loop, and best_docs only changes when the validation gate
    # accepts a candidate — so a rejected/no-op epoch would otherwise re-run a
    # byte-identical train _evaluate. Cache (train_score, feedback) and invalidate it
    # exactly where best_docs is reassigned below.
    cached_train: tuple[float, list[dict[str, Any]]] | None = None
    consecutive_noops = 0

    for epoch in range(1, cfg.epochs + 1):
        if best_val_score >= 1.0:
            break  # nothing left to gain on the validation set

        if cached_train is None:
            train_score, train_rollouts = await _evaluate(
                best_docs, train, rollout, score, cfg.rollout_concurrency
            )
            cached_train = (train_score, _reflect(train_rollouts))
        train_score, feedback = cached_train
        if not feedback:
            epochs.append(
                EpochRecord(epoch, train_score, best_val_score, None, 0, 0, [], False, "no failing train tasks")
            )
            break

        proposal = await optimizer(best_docs, feedback, sorted(rejected), skill_name)
        # Select: drop already-rejected edits, then clamp to the learning-rate budget.
        fresh = [e for e in proposal.edits if _edit_signature(e) not in rejected]
        budgeted = fresh[: cfg.max_edits_per_epoch]

        candidate_docs, applied = apply_edits(best_docs, budgeted)
        # A multi-edit batch already rejected by the gate must not be re-applied verbatim;
        # its components stay individually eligible (see _batch_signature), so the optimizer
        # can recombine them, but the exact losing combination is skipped.
        known_rejected_batch = len(applied) > 1 and _batch_signature(applied) in rejected
        if not applied or candidate_docs == best_docs or known_rejected_batch:
            # Unproductive epoch. Blacklist only permanently-redundant single edits (empty
            # append / identity replace); an edit whose anchor is merely absent now may
            # apply once a later epoch introduces it, so keep it eligible.
            if not known_rejected_batch:
                for edit in budgeted:
                    if _is_permanently_redundant(edit, best_docs):
                        rejected.add(_edit_signature(edit))
            note = "known-rejected batch" if known_rejected_batch else "no-op proposal"
            epochs.append(
                EpochRecord(epoch, train_score, best_val_score, None, len(proposal.edits), 0, [], False, note)
            )
            consecutive_noops += 1
            if consecutive_noops >= cfg.noop_patience:
                break  # optimizer is stuck producing nothing applicable
            continue

        consecutive_noops = 0  # an applied batch (accepted or gate-rejected) is productive
        val_after, _ = await _evaluate(candidate_docs, val, rollout, score, cfg.rollout_concurrency)
        accepted = val_after > best_val_score + cfg.min_improvement
        edited_targets = sorted({edit.target for edit in applied})
        if accepted:
            score_before = best_val_score
            best_docs = candidate_docs
            best_val_score = val_after
            cached_train = None  # best_docs changed -> the train eval must be recomputed
            accepted_count += len(applied)
            note = "accepted"
        else:
            score_before = best_val_score
            # Gate rejected the batch. Attribute the blame at the right granularity: a lone
            # edit is blacklisted directly; a multi-edit batch blacklists only its
            # *combination* so a good edit isn't banned for a bad partner's sake.
            if len(applied) == 1:
                rejected.add(_edit_signature(applied[0]))
            else:
                rejected.add(_batch_signature(applied))
            rejected_count += len(applied)
            note = "rejected by validation gate"

        epochs.append(
            EpochRecord(
                epoch, train_score, score_before, val_after,
                len(proposal.edits), len(applied), edited_targets, accepted, note,
            )
        )

    return SkillOptResult(
        skill_name=skill_name,
        seed_docs=seed_docs,
        best_docs=best_docs,
        seed_val_score=seed_val_score,
        best_val_score=best_val_score,
        epochs=epochs,
        accepted_edit_count=accepted_count,
        rejected_edit_count=rejected_count,
        train_size=len(train),
        val_size=len(val),
    )


# ---------------------------------------------------------------------------
# Router-backed default rollout + optimizer.
# ---------------------------------------------------------------------------

DEFAULT_OBJECTIVE = (
    "Improve the guidance preamble and the engineering skill document together so a "
    "coding agent following them produces correct, in-scope, well-verified work on the "
    "evaluation tasks. Keep both documents compact and actionable; prefer small, "
    "targeted edits over rewrites, and keep guidance and skill mutually consistent."
)


class _AgentAnswer(BaseModel):
    answer: str = ""


def _compose_context(docs: dict[str, str]) -> str:
    blocks = []
    guidance = docs.get(GUIDANCE, "").strip()
    skill = docs.get(SKILL, "").strip()
    if guidance:
        blocks.append(f"<guidance>\n{guidance}\n</guidance>")
    if skill:
        blocks.append(f"<skill>\n{skill}\n</skill>")
    for name, body in docs.items():
        if name in DOC_TARGETS or not body.strip():
            continue
        blocks.append(f"<{name}>\n{body.strip()}\n</{name}>")
    return "\n\n".join(blocks)


def _pick_role(role_config: dict[str, Any], preferred: tuple[str, ...]) -> str:
    for role in preferred:
        if role in role_config:
            return role
    if role_config:
        return next(iter(role_config))
    raise ValueError("Router has no configured roles for SkillOpt.")


def make_llm_rollout(
    router: Any,
    *,
    preferred_roles: tuple[str, ...] = ("skill_target", "arbiter", "planner_a"),
) -> RolloutFn:
    """Build a rollout that runs the target agent (an LLM) with both documents in context."""
    role = _pick_role(router.role_config, preferred_roles)

    async def rollout(docs: dict[str, str], task: dict[str, Any]) -> str:
        system = (
            "You are a coding agent. Follow this guidance and engineering skill exactly "
            "when responding.\n\n" + _compose_context(docs)
        )
        messages = [
            {"role": "system", "content": system},
            {"role": "user", "content": _task_prompt(task) or "Complete the task."},
        ]
        result = await router.complete_structured(
            role, messages, _AgentAnswer, fallback=_AgentAnswer(answer="")
        )
        return result.answer

    return rollout


def make_llm_optimizer(
    router: Any,
    *,
    objective: str = DEFAULT_OBJECTIVE,
    preferred_roles: tuple[str, ...] = ("skill_optimizer", "arbiter", "critic_a"),
) -> OptimizerFn:
    """Build an optimizer that proposes bounded edits across guidance + skill at once."""
    role = _pick_role(router.role_config, preferred_roles)

    async def optimizer(
        docs: dict[str, str], feedback: list[dict[str, Any]], rejected: list[str], skill_name: str
    ) -> SkillEditProposal:
        system = (
            "You optimize the documents that steer a coding agent. "
            f"{objective}\n\n"
            "You may edit two documents simultaneously: 'guidance' (the agent preamble) and "
            "'skill' (the engineering skill). Set each edit's 'target' accordingly. Propose a "
            "SMALL set of bounded edits. Each edit anchors on existing text via 'find'. Use "
            "op=replace to swap 'find' for 'text', op=add to insert 'text' after 'find' (empty "
            "'find' appends), op=delete to remove 'find'. Do not rewrite a whole document."
        )
        user = json.dumps(
            {
                "skill_name": skill_name,
                "documents": {name: docs.get(name, "") for name in DOC_TARGETS if name in docs},
                "failing_cases": feedback,
                "previously_rejected_edit_ids": rejected,
            },
            indent=2,
        )
        messages = [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ]
        return await router.complete_structured(
            role, messages, SkillEditProposal, fallback=SkillEditProposal()
        )

    return optimizer


# ---------------------------------------------------------------------------
# Artifact persistence.
# ---------------------------------------------------------------------------


def default_artifact_path(project_root: Path, skill_name: str) -> Path:
    timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    safe = skill_name.replace("/", "-").replace("\\", "-")
    return project_root / ".devcouncil" / "optimizations" / f"{timestamp}-{safe}-skillopt.json"


def write_result_artifact(path: Path, result: SkillOptResult, *, objective: str, dataset_path: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "optimizer": "devcouncil.skillopt",
        "skill": result.skill_name,
        "objective": objective,
        "dataset_path": dataset_path,
        "train_size": result.train_size,
        "val_size": result.val_size,
        "seed_val_score": result.seed_val_score,
        "best_val_score": result.best_val_score,
        "improved": result.improved,
        "accepted_edit_count": result.accepted_edit_count,
        "rejected_edit_count": result.rejected_edit_count,
        "epochs": [asdict(record) for record in result.epochs],
        "seed_docs": result.seed_docs,
        "best_docs": result.best_docs,
        "applied": result.applied,
        "created_at": datetime.now(timezone.utc).isoformat(),
    }
    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
