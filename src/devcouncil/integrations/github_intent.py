"""Resolve a DevCouncil goal from a GitHub issue or pull-request reference.

A terse goal like ``"#142"`` or a full issue URL carries far more intent than a
one-line argument — the issue body usually *is* the spec. This module detects
such references and expands them into a rich goal string (title + body + a few
comments) by shelling out to the authenticated ``gh`` CLI, so private repos work
without any token plumbing. When ``gh`` is unavailable or the lookup fails, the
caller keeps the original goal text unchanged — expansion is strictly additive.
"""

from __future__ import annotations

import json
import logging
import re
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path

from devcouncil.utils.subprocess_env import clean_subprocess_env

logger = logging.getLogger(__name__)

# Cap each pulled discussion comment so a long thread can't dominate the prompt.
_MAX_COMMENT_CHARS = 600

# "#142", "GH-142", "owner/repo#142", or a full issues/pull URL.
_SHORT_REF = re.compile(r"^\s*(?:GH-|#)(\d+)\s*$", re.IGNORECASE)
_OWNER_REPO_REF = re.compile(r"^\s*([\w.-]+/[\w.-]+)#(\d+)\s*$")
_URL_REF = re.compile(
    r"^\s*https?://github\.com/([\w.-]+/[\w.-]+)/(issues|pull)/(\d+)",
    re.IGNORECASE,
)


@dataclass(frozen=True)
class IntentRef:
    number: int
    kind: str  # "issue" | "pull" | "auto"
    repo: str | None  # "owner/repo" when explicit, else None (current repo)


def parse_intent_ref(goal: str) -> IntentRef | None:
    """Return the GitHub reference a goal points at, or None if it's plain text."""
    text = goal.strip()
    m = _URL_REF.match(text)
    if m:
        kind = "pull" if m.group(2).lower() == "pull" else "issue"
        return IntentRef(number=int(m.group(3)), kind=kind, repo=m.group(1))
    m = _OWNER_REPO_REF.match(text)
    if m:
        return IntentRef(number=int(m.group(2)), kind="auto", repo=m.group(1))
    m = _SHORT_REF.match(text)
    if m:
        return IntentRef(number=int(m.group(1)), kind="auto", repo=None)
    return None


def _gh_view(ref: IntentRef, sub: str, root: Path) -> dict | None:
    """Run ``gh <issue|pr> view`` and return the parsed JSON, or None on failure."""
    gh = shutil.which("gh")
    if not gh:
        logger.debug("gh CLI not on PATH; cannot expand %s #%s", sub, ref.number)
        return None
    cmd = [gh, sub, "view", str(ref.number), "--json", "title,body,comments,url,state"]
    if ref.repo:
        cmd += ["--repo", ref.repo]
    try:
        result = subprocess.run(
            cmd, cwd=root, capture_output=True, text=True,
            encoding="utf-8", errors="replace", timeout=20, env=clean_subprocess_env(),
        )
    except Exception as exc:
        logger.warning("gh %s view %s failed: %s", sub, ref.number, exc)
        return None
    if result.returncode != 0 or not result.stdout.strip():
        logger.warning("gh %s view %s returned %s: %s", sub, ref.number, result.returncode, (result.stderr or "").strip()[:200])
        return None
    try:
        data = json.loads(result.stdout)
    except json.JSONDecodeError:
        logger.warning("gh %s view %s returned unparseable JSON", sub, ref.number)
        return None
    return data if isinstance(data, dict) else None


def _compose_goal(ref: IntentRef, data: dict, source: str) -> str:
    title = str(data.get("title") or "").strip()
    body = str(data.get("body") or "").strip()
    url = str(data.get("url") or "").strip()
    lines = [f"Implement {source} #{ref.number}: {title}".rstrip(": ").rstrip()]
    if url:
        lines.append(f"Source: {url}")
    if body:
        lines += ["", body]
    # Pull in up to three discussion comments — clarifications often live there.
    # Cap each so a long thread can't bloat the planning prompt; the issue body
    # above is the primary spec, comments are secondary context.
    comments = data.get("comments")
    if isinstance(comments, list) and comments:
        snippets = []
        for comment in comments[:3]:
            text = str((comment or {}).get("body") or "").strip()
            if text:
                if len(text) > _MAX_COMMENT_CHARS:
                    text = text[:_MAX_COMMENT_CHARS].rstrip() + " […]"
                snippets.append(text)
        if snippets:
            lines += ["", "Discussion notes:"]
            lines += [f"- {s}" for s in snippets]
    return "\n".join(lines).strip()


def resolve_goal_intent(goal: str, root: Path) -> tuple[str, str | None]:
    """Expand a GitHub issue/PR reference into a full goal.

    Returns ``(goal, note)``. When ``goal`` is a reference and the lookup
    succeeds, the first element is the composed goal and ``note`` describes the
    expansion (for display). Otherwise the original goal is returned with a
    ``note`` explaining why it could not be expanded (or ``None`` when the goal
    was plain text and no expansion was attempted).
    """
    ref = parse_intent_ref(goal)
    if ref is None:
        return goal, None

    if not shutil.which("gh"):
        return goal, (
            f"Goal looks like GitHub reference #{ref.number}, but the `gh` CLI is not on "
            "PATH — using the literal text. Install/auth gh to pull the issue/PR body."
        )

    order = (
        ["pull", "issue"] if ref.kind == "pull"
        else ["issue", "pull"] if ref.kind == "issue"
        else ["issue", "pull"]
    )
    for sub in order:
        data = _gh_view(ref, "pr" if sub == "pull" else "issue", root)
        if data is not None:
            source = "pull request" if sub == "pull" else "issue"
            composed = _compose_goal(ref, data, source)
            return composed, f"Pulled intent from {source} #{ref.number} via gh."

    return goal, (
        f"Could not fetch GitHub #{ref.number} via gh (not found, no access, or not a "
        "git/GitHub repo) — using the literal text."
    )
