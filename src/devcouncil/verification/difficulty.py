"""Deterministic task-difficulty estimation and the rigor policy derived from it.

DevCouncil's anti-laziness gates (stub detection, effort heuristics, coverage
enforcement) are *advisory everywhere, blocking on hard tasks* by default. This
module supplies the two pieces that policy needs:

- :func:`estimate_difficulty` — a cheap, deterministic (no LLM) classifier of a
  task as ``easy`` / ``normal`` / ``hard`` from its declared scope. A manual
  ``Task.difficulty`` value always wins, so planners and humans can override.
- :func:`resolve_rigor_policy` — folds the difficulty together with
  ``verification.rigor`` config into a :class:`RigorPolicy` the verifier, prompt
  builder, and repair loop can branch on without re-deriving anything.

Everything here must stay side-effect free and never raise: rigor is a layer on
top of verification, and a bug in it must degrade to "no extra enforcement",
never to a crashed verify run.
"""

from __future__ import annotations

import logging
import re
from dataclasses import dataclass, field
from typing import List, Literal, Optional, cast

from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task

logger = logging.getLogger(__name__)

Difficulty = Literal["easy", "normal", "hard"]

# Keywords whose presence in the task/requirement text signals intrinsically hard
# work (cross-cutting, stateful, or correctness-critical). Word-ish boundaries so
# "author" does not match "auth". The whole keyword bucket contributes at most
# _KEYWORD_CAP points — a wordy description must not outweigh structural signals.
_HARD_KEYWORD_RE = re.compile(
    r"\b(refactor\w*|migrat\w*|concurren\w*|async\w*|race|deadlock|protocol|parser|"
    r"cache|caching|transaction\w*|auth(?:n|z|entication|orization)?|crypto\w*|"
    r"distributed|backward[- ]compat\w*|thread\w*|lock(?:ing|s)?|schema)\b",
    re.IGNORECASE,
)
_KEYWORD_CAP = 1

_HARD_THRESHOLD = 4
_NORMAL_THRESHOLD = 2


def _linked_requirements(task: Task, requirements: Optional[List[Requirement]]) -> List[Requirement]:
    if not requirements:
        return []
    wanted = set(task.requirement_ids)
    return [r for r in requirements if r.id in wanted]


def difficulty_score(task: Task, requirements: Optional[List[Requirement]] = None) -> int:
    """The raw additive score behind :func:`estimate_difficulty` (exposed for tests
    and for surfacing "why was this hard" in logs)."""
    score = 0
    writable = [pf for pf in task.planned_files if pf.allowed_change != "read_only"]
    if len(writable) >= 5:
        score += 2
    elif len(writable) >= 3:
        score += 1

    ac_count = len(task.acceptance_criterion_ids)
    if ac_count >= 5:
        score += 2
    elif ac_count >= 3:
        score += 1

    changes = {pf.allowed_change for pf in writable}
    if "create" in changes and "modify" in changes:
        score += 1

    if len(task.depends_on) >= 2:
        score += 1

    linked = _linked_requirements(task, requirements)
    text = " ".join(
        [task.title, task.description]
        + [r.title for r in linked]
        + [r.description for r in linked]
    )
    if _HARD_KEYWORD_RE.search(text):
        score += _KEYWORD_CAP

    if any(r.priority in ("high", "critical") for r in linked):
        score += 1

    return score


def estimate_difficulty(task: Task, requirements: Optional[List[Requirement]] = None) -> Difficulty:
    """Classify a task's difficulty deterministically.

    A manual ``Task.difficulty`` (set by a planner or a human) always wins. The
    estimator never raises; any unexpected error degrades to ``"normal"``.
    """
    manual = getattr(task, "difficulty", None)
    if manual in ("easy", "normal", "hard"):
        return cast(Literal["easy", "normal", "hard"], manual)
    try:
        score = difficulty_score(task, requirements)
    except Exception:  # pragma: no cover - defensive
        logger.debug("difficulty_score failed for %s; defaulting to normal", getattr(task, "id", "?"), exc_info=True)
        return "normal"
    if score >= _HARD_THRESHOLD:
        return "hard"
    if score >= _NORMAL_THRESHOLD:
        return "normal"
    return "easy"


@dataclass
class RigorPolicy:
    """Resolved enforcement decisions for one task's verification run.

    ``*_enabled`` says whether a gate runs at all; ``*_blocking`` says whether its
    findings block. ``applied`` lists the escalations that actually took effect so
    the verification outcome can record "passed under strict gates" vs "passed".
    """

    difficulty: Difficulty = "normal"
    stub_enabled: bool = True
    stub_blocking: bool = False
    effort_enabled: bool = True
    effort_blocking: bool = False
    coarse_acceptance_enabled: bool = True
    coarse_acceptance_blocking: bool = False
    unwired_enabled: bool = True
    unwired_blocking: bool = False
    dead_symbol_enabled: bool = True
    dead_symbol_blocking: bool = False
    liveness_ratchet_enabled: bool = True
    liveness_ratchet_blocking: bool = False
    stale_map_enabled: bool = True
    stale_map_blocking: bool = False
    enforce_coverage: bool = False
    reviewer_required: bool = False
    extra_repair_attempts: int = 0
    min_added_lines_per_planned_file: int = 5
    min_acceptance_samples: int = 0
    applied: List[str] = field(default_factory=list)


def _mode_flags(mode: str, is_hard: bool) -> tuple[bool, bool]:
    """Map a config mode (``never``/``hard``/``always``) to (enabled, blocking)."""
    normalized = (mode or "hard").strip().lower()
    if normalized == "never":
        return False, False
    if normalized == "always":
        return True, True
    # "hard" (and anything unrecognized, defensively) -> run always, block on hard.
    return True, is_hard


def resolve_rigor_policy(
    task: Task,
    requirements: Optional[List[Requirement]] = None,
    config=None,
) -> RigorPolicy:
    """Fold difficulty + ``verification.rigor`` config into a :class:`RigorPolicy`.

    ``config`` is a loaded ``DevCouncilConfig`` or None (defaults apply). Never
    raises; on any error it returns a policy with no blocking escalations.
    """
    try:
        rigor_cfg = config.verification.rigor if config is not None else None
    except Exception:
        rigor_cfg = None

    difficulty = estimate_difficulty(task, requirements)
    policy = RigorPolicy(difficulty=difficulty)

    try:
        enabled = True if rigor_cfg is None else bool(rigor_cfg.enabled)
        if not enabled:
            policy.stub_enabled = False
            policy.effort_enabled = False
            policy.unwired_enabled = False
            policy.dead_symbol_enabled = False
            policy.liveness_ratchet_enabled = False
            policy.stale_map_enabled = False
            return policy

        is_hard = difficulty == "hard"
        stub_mode = "hard" if rigor_cfg is None else getattr(rigor_cfg, "stub_detection", "hard")
        effort_mode = "hard" if rigor_cfg is None else getattr(rigor_cfg, "effort_heuristics", "hard")
        policy.stub_enabled, policy.stub_blocking = _mode_flags(stub_mode, is_hard)
        policy.effort_enabled, policy.effort_blocking = _mode_flags(effort_mode, is_hard)
        coarse_mode = "hard" if rigor_cfg is None else getattr(
            rigor_cfg, "coarse_acceptance_proof", "hard"
        )
        policy.coarse_acceptance_enabled, policy.coarse_acceptance_blocking = _mode_flags(
            coarse_mode, is_hard
        )
        unwired_mode = "hard" if rigor_cfg is None else getattr(rigor_cfg, "unwired_files", "hard")
        dead_mode = "hard" if rigor_cfg is None else getattr(rigor_cfg, "dead_symbols", "hard")
        ratchet_mode = "hard" if rigor_cfg is None else getattr(
            rigor_cfg, "liveness_ratchet", "hard"
        )
        stale_mode = "hard" if rigor_cfg is None else getattr(rigor_cfg, "stale_map", "hard")
        policy.unwired_enabled, policy.unwired_blocking = _mode_flags(unwired_mode, is_hard)
        policy.dead_symbol_enabled, policy.dead_symbol_blocking = _mode_flags(dead_mode, is_hard)
        policy.liveness_ratchet_enabled, policy.liveness_ratchet_blocking = _mode_flags(
            ratchet_mode, is_hard
        )
        policy.stale_map_enabled, policy.stale_map_blocking = _mode_flags(stale_mode, is_hard)

        enforce_cov_on_hard = True if rigor_cfg is None else bool(
            getattr(rigor_cfg, "enforce_coverage_on_hard", True)
        )
        policy.enforce_coverage = is_hard and enforce_cov_on_hard

        reviewer_on_hard = False if rigor_cfg is None else bool(
            getattr(rigor_cfg, "reviewer_required_on_hard", False)
        )
        policy.reviewer_required = is_hard and reviewer_on_hard

        extra = 1 if rigor_cfg is None else max(
            0, int(getattr(rigor_cfg, "extra_repair_attempts_on_hard", 1))
        )
        policy.extra_repair_attempts = extra if is_hard else 0

        policy.min_added_lines_per_planned_file = max(
            1, int(getattr(rigor_cfg, "min_added_lines_per_planned_file", 5))
        )

        samples_on_hard = 2 if rigor_cfg is None else max(
            1, int(getattr(rigor_cfg, "acceptance_samples_on_hard", 2))
        )
        policy.min_acceptance_samples = samples_on_hard if is_hard else 0

        if policy.stub_blocking:
            policy.applied.append("stub_detection_blocking")
        if policy.effort_blocking:
            policy.applied.append("effort_heuristics_blocking")
        if policy.coarse_acceptance_blocking:
            policy.applied.append("coarse_acceptance_proof_blocking")
        if policy.unwired_blocking:
            policy.applied.append("unwired_files_blocking")
        if policy.dead_symbol_blocking:
            policy.applied.append("dead_symbols_blocking")
        if policy.liveness_ratchet_blocking:
            policy.applied.append("liveness_ratchet_blocking")
        if policy.stale_map_blocking:
            policy.applied.append("stale_map_blocking")
        if policy.enforce_coverage:
            policy.applied.append("coverage_enforced")
        if policy.reviewer_required:
            policy.applied.append("reviewer_required")
        if policy.extra_repair_attempts:
            policy.applied.append(f"extra_repair_attempts:{policy.extra_repair_attempts}")
        if policy.min_acceptance_samples > 1:
            policy.applied.append(f"acceptance_samples:{policy.min_acceptance_samples}")
    except Exception:  # pragma: no cover - defensive
        logger.debug("resolve_rigor_policy failed; degrading to advisory-only", exc_info=True)
        return RigorPolicy(difficulty=difficulty)
    return policy
