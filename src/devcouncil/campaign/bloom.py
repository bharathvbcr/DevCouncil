"""Bloom's-taxonomy routing — decide whether a task needs a worker or a thinker.

The Coordinator tags every subtask with a Bloom level and routes it: the lower three
levels (Remember / Understand / Apply) are execution and go to an **Worker**;
the upper three (Analyze / Evaluate / Create) are cognition and go to the
**Reviewer**. The classifier is a deterministic keyword heuristic — intentionally
simple, cheap and testable — with an explicit override honoured first so a
planner or human can pin a level via ``Task.difficulty`` or a tag.
"""

from __future__ import annotations

import re
from enum import IntEnum
from typing import Dict, Iterable, List, Optional

from devcouncil.campaign.roles import Rank


class BloomLevel(IntEnum):
    REMEMBER = 1
    UNDERSTAND = 2
    APPLY = 3
    ANALYZE = 4
    EVALUATE = 5
    CREATE = 6

    @property
    def label(self) -> str:
        return self.name.capitalize()


# Ordered high→low so a strong verb ("architect", L6) wins over an incidental
# weak one ("list", L1) appearing in the same sentence.
_KEYWORDS: Dict[BloomLevel, List[str]] = {
    BloomLevel.CREATE: [
        "design", "architect", "architecture", "invent", "compose", "author",
        "scaffold new", "green-field", "greenfield", "propose", "strategy",
        "strategize", "plan the", "new subsystem", "from scratch",
    ],
    BloomLevel.EVALUATE: [
        "evaluate", "assess", "review", "audit", "critique", "judge", "compare",
        "trade-off", "tradeoff", "recommend", "decide", "prioritize", "quality",
        "verify design", "root cause", "root-cause",
    ],
    BloomLevel.ANALYZE: [
        "analyze", "analyse", "investigate", "diagnose", "debug", "profile",
        "why", "reverse-engineer", "break down", "correlate", "trace",
        "distinguish", "differentiate",
    ],
    BloomLevel.APPLY: [
        "implement", "build", "write", "add", "create ", "fix", "refactor",
        "wire", "integrate", "migrate", "port", "configure", "hook up",
        "apply", "use", "run", "execute",
    ],
    BloomLevel.UNDERSTAND: [
        "summarize", "summarise", "explain", "describe", "document", "clarify",
        "interpret", "outline", "paraphrase", "restate",
    ],
    BloomLevel.REMEMBER: [
        "list", "find", "locate", "look up", "fetch", "collect", "gather",
        "identify", "name", "retrieve", "read",
    ],
}

# Strong execution verbs — when present, cap classification at APPLY even if
# evaluate/analyze keywords ("review", "quality", …) also appear in the title.
_IMPLEMENTATION_OVERRIDES: List[str] = [
    "implement", "fix", "add", "write", "build", "refactor", "wire", "integrate",
    "migrate", "port", "configure", "hook up",
]


_DIFFICULTY_HINT: Dict[str, BloomLevel] = {
    "easy": BloomLevel.APPLY,
    "normal": BloomLevel.APPLY,
    "hard": BloomLevel.ANALYZE,
}


def _match(text: str, phrase: str) -> bool:
    if phrase.endswith(" ") or " " in phrase:
        return phrase.strip() in text
    return re.search(rf"\b{re.escape(phrase)}\b", text) is not None


def classify_bloom(
    text: str,
    *,
    override: Optional[BloomLevel] = None,
    difficulty: Optional[str] = None,
) -> BloomLevel:
    """Classify free text into a :class:`BloomLevel`.

    Precedence: explicit ``override`` > keyword match > ``difficulty`` hint >
    default (:attr:`BloomLevel.APPLY`, the most common execution level).
    """
    if override is not None:
        return override
    haystack = (text or "").lower()
    for level in sorted(_KEYWORDS, reverse=True):  # CREATE(6) → REMEMBER(1)
        if level is BloomLevel.APPLY:
            continue
        if any(_match(haystack, kw) for kw in _KEYWORDS[level]):
            if level >= BloomLevel.EVALUATE and any(
                _match(haystack, kw) for kw in _IMPLEMENTATION_OVERRIDES
            ):
                return BloomLevel.APPLY
            return level
    if any(_match(haystack, kw) for kw in _KEYWORDS[BloomLevel.APPLY]):
        return BloomLevel.APPLY
    if difficulty and difficulty.lower() in _DIFFICULTY_HINT:
        return _DIFFICULTY_HINT[difficulty.lower()]
    return BloomLevel.APPLY


def route_rank(level: BloomLevel) -> Rank:
    """Map a Bloom level to the rank that should own it."""
    return Rank.REVIEWER if level >= BloomLevel.ANALYZE else Rank.WORKER


def route_text(
    text: str,
    *,
    override: Optional[BloomLevel] = None,
    difficulty: Optional[str] = None,
) -> Rank:
    """Convenience: classify ``text`` and return the owning rank."""
    return route_rank(classify_bloom(text, override=override, difficulty=difficulty))


def summarize_routing(items: Iterable[str]) -> Dict[str, int]:
    """Count how a batch of task descriptions would route (for the dashboard)."""
    counts = {Rank.WORKER.value: 0, Rank.REVIEWER.value: 0}
    for item in items:
        counts[route_text(item).value] += 1
    return counts
