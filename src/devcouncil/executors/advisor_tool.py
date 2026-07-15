"""Claude Code advisor-tool helpers (CLI ``--advisor`` / SDK ``extra_args`` / settings).

Kept out of :mod:`devcouncil.execution.prompt_builder` so Codex/Cursor/native never
see advisor steering. Pairing preflight is soft: clear bad pairs skip attaching
``--advisor`` so Claude does not hard-exit and burn the go-loop repair budget.

DevCouncil only soft-filters obvious mismatches (haiku advisor, weaker family,
fable+non-fable). Claude Code validates the full versioned pairing matrix at launch.
"""

from __future__ import annotations

import logging
import os
import re
import shutil
import subprocess
from functools import lru_cache
from typing import Literal, Mapping

logger = logging.getLogger(__name__)

# Claude Code floor for the advisor server tool (Anthropic API only).
MIN_ADVISOR_CLAUDE_VERSION = (2, 1, 98)
# Fable as main or advisor requires a newer Claude Code floor.
MIN_FABLE_ADVISOR_CLAUDE_VERSION = (2, 1, 170)
DISABLE_ADVISOR_ENV = "CLAUDE_CODE_DISABLE_ADVISOR_TOOL"

# Cloud providers where Claude Code does not offer the Anthropic advisor tool.
NON_ANTHROPIC_ADVISOR_ENVS = (
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
)

# Short interactive nudge shared by MCP prompts, slash commands, and subagents.
ADVISOR_STEERING_NUDGE = (
    "When the advisor tool is available, consult it before committing to an approach, "
    "when stuck on a recurring error, and before declaring the task complete."
)

# Anthropic "Suggested system prompt for coding tasks" — timing block.
ADVISOR_CODING_TIMING = (
    "You have access to an `advisor` tool backed by a stronger reviewer model. "
    "It takes NO parameters — when you call advisor(), your entire conversation "
    "history is automatically forwarded. They see the task, every tool call you've "
    "made, every result you've seen.\n\n"
    "Call advisor BEFORE substantive work — before writing, before committing to an "
    "interpretation, before building on an assumption. If the task requires orientation "
    "first (finding files, fetching a source, seeing what's there), do that, then call "
    "advisor. Orientation is not substantive work. Writing, editing, and declaring an "
    "answer are.\n\n"
    "Also call advisor:\n"
    "- When you believe the task is complete. BEFORE this call, make your deliverable "
    "durable: write the file, save the result, commit the change. The advisor call takes "
    "time; if the session ends during it, a durable result persists and an unwritten "
    "one doesn't.\n"
    "- When stuck — errors recurring, approach not converging, results that don't fit.\n"
    "- When considering a change of approach.\n\n"
    "On tasks longer than a few steps, call advisor at least once before committing to "
    "an approach and once before declaring done. On short reactive tasks where the next "
    "action is dictated by tool output you just read, you don't need to keep calling — "
    "the advisor adds most of its value on the first call, before the approach crystallizes."
)

# Anthropic treat-advice block (place directly after timing).
ADVISOR_TREAT_ADVICE = (
    "Give the advice serious weight. If you follow a step and it fails empirically, or "
    "you have primary-source evidence that contradicts a specific claim (the file says X, "
    "the paper states Y), adapt. A passing self-test is not evidence the advice is wrong — "
    "it's evidence your test doesn't check what the advice is checking.\n\n"
    "If you've already retrieved data pointing one way and the advisor points another: "
    "don't silently switch. Surface the conflict in one more advisor call — "
    '"I found X, you suggest Y, which constraint breaks the tie?" The advisor saw your '
    "evidence but may have underweighted it; a reconcile call is cheaper than committing "
    "to the wrong branch."
)

REPAIR_ADVISOR_AUTHORITY = (
    "A correction_manifest is present for this repair. Treat it as authoritative over "
    "prior session history and any prior advisor advice; change approach on identical gaps."
)

# Soft cost trim for the user prompt (CC-passable; not Messages-API-only).
ADVISOR_COST_TRIM_USER = (
    "(Advisor: keep guidance under ~80 words — I need a focused starting point, "
    "not a comprehensive plan.)"
)

# Capability tiers for soft pairing. Higher = stronger. Unknown models are not
# rejected here — Claude Code validates those at launch.
_FAMILY_TIER: dict[str, int] = {
    "haiku": 1,
    "sonnet": 2,
    "opus": 3,
    "fable": 4,
}

# Markers that mean the CLI died on advisor/pairing/infra — not a code gap to repair.
ADVISOR_INFRA_FAILURE_MARKERS = (
    "does not support the advisor",
    "advisor is not supported",
    "advisor model",
    "--advisor",
    "advisor tool",
    "availablemodels",
    "available models",
)


def advisor_steering_text(*, repair: bool = False) -> str:
    """Attach-gated Claude-only system steering (timing + treat-advice + optional repair)."""
    parts = [ADVISOR_CODING_TIMING, ADVISOR_TREAT_ADVICE]
    if repair:
        parts.append(REPAIR_ADVISOR_AUTHORITY)
    return "\n\n".join(parts)


def advisor_user_cost_trim() -> str:
    """Optional soft ~80-word ask for the advisor, placed in the user prompt when attached."""
    return ADVISOR_COST_TRIM_USER


def normalize_model_family(model: str | None) -> str | None:
    """Map a model alias or id to a coarse family name, or None if unrecognized."""
    if not model:
        return None
    text = model.strip().lower()
    if not text:
        return None
    # Prefer explicit family tokens (claude-opus-4-7, opus, sonnet-4-6, …).
    for family in ("fable", "opus", "sonnet", "haiku"):
        if family in text:
            return family
    return None


def advisor_pairing_ok(main_model: str | None, advisor_model: str | None) -> tuple[bool, str | None]:
    """Soft check that *advisor* is not clearly weaker / invalid for *main*.

    Returns ``(ok, skip_reason)``. Unknown families are allowed through (Claude
    validates at launch). Clear mismatches return ``ok=False`` with a reason so
    callers can skip ``--advisor`` instead of hard-exiting.

    This is intentionally coarse — not Claude Code's full versioned matrix
    (e.g. Sonnet 5 vs 4.6, Opus 4.6 vs 4.7+). Claude Code enforces those at launch.
    """
    advisor = (advisor_model or "").strip()
    if not advisor:
        return False, "no advisor_model configured"

    adv_family = normalize_model_family(advisor)
    if adv_family == "haiku":
        return False, "haiku cannot act as an advisor"

    main_family = normalize_model_family(main_model)
    if main_family is None or adv_family is None:
        # Unknown / unset main: attach and let Claude validate.
        return True, None

    main_tier = _FAMILY_TIER.get(main_family)
    adv_tier = _FAMILY_TIER.get(adv_family)
    if main_tier is None or adv_tier is None:
        return True, None

    # Fable main only accepts Fable advisor (docs).
    if main_family == "fable" and adv_family != "fable":
        return False, f"fable main rejects {adv_family} advisor"

    if adv_tier < main_tier:
        return False, f"{adv_family} advisor is weaker than {main_family} main"

    return True, None


def _env_flag_set(source: Mapping[str, str], key: str) -> bool:
    value = str(source.get(key) or "").strip().lower()
    return value in {"1", "true", "yes", "on"}


def advisor_provider_unsupported(env: dict[str, str] | None = None) -> str | None:
    """Return the cloud-provider env key if advisor cannot run (Bedrock/Vertex/Foundry)."""
    source = env if env is not None else os.environ
    for key in NON_ANTHROPIC_ADVISOR_ENVS:
        if _env_flag_set(source, key):
            return key
    return None


def parse_claude_version(text: str) -> tuple[int, ...] | None:
    """Extract a dotted version tuple from ``claude --version`` output."""
    match = re.search(r"(\d+)\.(\d+)\.(\d+)", text or "")
    if not match:
        return None
    return tuple(int(part) for part in match.groups())


@lru_cache(maxsize=1)
def probe_claude_version() -> tuple[int, ...] | None:
    """Best-effort ``claude --version`` parse; None when CLI missing or unreadable."""
    executable = shutil.which("claude")
    if not executable:
        return None
    try:
        result = subprocess.run(
            [executable, "--version"],
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    return parse_claude_version(f"{result.stdout or ''}\n{result.stderr or ''}")


@lru_cache(maxsize=1)
def claude_supports_append_system_prompt() -> bool:
    """True when ``claude -h`` lists ``--append-system-prompt``."""
    executable = shutil.which("claude")
    if not executable:
        return False
    try:
        result = subprocess.run(
            [executable, "-h"],
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return False
    hay = f"{result.stdout or ''}\n{result.stderr or ''}"
    return "--append-system-prompt" in hay


def advisor_disable_env_set(env: dict[str, str] | None = None) -> bool:
    source = env if env is not None else os.environ
    return _env_flag_set(source, DISABLE_ADVISOR_ENV)


def warn_advisor_preflight(
    *,
    env: dict[str, str] | None = None,
    main_model: str | None = None,
    advisor_model: str | None = None,
) -> list[str]:
    """Soft warnings for version floor / disable-env / Fable floor. Never blocks attaching."""
    warnings: list[str] = []
    if advisor_disable_env_set(env):
        warnings.append(
            f"{DISABLE_ADVISOR_ENV} is set; Claude Code will ignore --advisor / advisorModel."
        )
    version = probe_claude_version()
    if version is not None and version < MIN_ADVISOR_CLAUDE_VERSION:
        floor = ".".join(str(part) for part in MIN_ADVISOR_CLAUDE_VERSION)
        current = ".".join(str(part) for part in version)
        warnings.append(
            f"Claude Code {current} is below the advisor floor ({floor}); "
            "upgrade with `claude update` or advisor calls may fail."
        )
    uses_fable = any(
        normalize_model_family(m) == "fable" for m in (main_model, advisor_model)
    )
    if uses_fable and version is not None and version < MIN_FABLE_ADVISOR_CLAUDE_VERSION:
        floor = ".".join(str(part) for part in MIN_FABLE_ADVISOR_CLAUDE_VERSION)
        current = ".".join(str(part) for part in version)
        warnings.append(
            f"Claude Code {current} is below the Fable advisor floor ({floor}); "
            "Fable main/advisor pairs need Claude Code ≥ 2.1.170."
        )
    return warnings


def strip_duplicate_advisor_args(extra_args: list[str]) -> list[str]:
    """Remove ``--advisor`` / ``--advisor=…`` from profile extras when DevCouncil attached."""
    out: list[str] = []
    skip_next = False
    for part in extra_args:
        if skip_next:
            skip_next = False
            continue
        if part == "--advisor":
            skip_next = True
            continue
        if part.startswith("--advisor="):
            continue
        out.append(part)
    return out


AdvisorAttachDecision = Literal["attach", "skip"]


def decide_advisor_attach(
    *,
    main_model: str | None,
    advisor_model: str | None,
    env: dict[str, str] | None = None,
) -> tuple[AdvisorAttachDecision, str | None, str | None]:
    """Return ``(decision, resolved_advisor, reason)`` for CLI/SDK wiring."""
    resolved = (advisor_model or "").strip() or None
    if not resolved:
        return "skip", None, None
    provider = advisor_provider_unsupported(env)
    if provider:
        return (
            "skip",
            resolved,
            f"advisor requires Anthropic API; {provider} is set (soft-skip)",
        )
    ok, reason = advisor_pairing_ok(main_model, resolved)
    if not ok:
        return "skip", resolved, reason
    return "attach", resolved, None
