"""Knowledge sources: discover and select OKF / design.md context for prompts.

A :class:`KnowledgeSource` is the prompt-facing view of an ingested knowledge file. It
mirrors :class:`devcouncil.skills.registry.Skill` — same frontmatter contract, same
trigger-based selection and relevance ranking — so OKF bundles and a project design system
flow into planning/council/task prompts through the existing budget-aware machinery.

On-disk layout (under the project root)::

    .devcouncil/knowledge/
        design/design.md        # one design system, always selected
        okf/*.md                # ingested OKF documents, selected by trigger/keyword
"""

from __future__ import annotations

from pathlib import Path
from typing import Literal

from pydantic import BaseModel, Field

from devcouncil.knowledge.frontmatter import split_frontmatter
from devcouncil.skills.registry import SkillTriggers, _keyword_in_text

KNOWLEDGE_DIR = ".devcouncil/knowledge"

Kind = Literal["okf", "design"]


class KnowledgeSource(BaseModel):
    name: str
    kind: Kind
    description: str = ""
    always: bool = False
    triggers: SkillTriggers = Field(default_factory=SkillTriggers)
    body: str = ""
    priority: int = 50
    source_path: Path | None = None

    def _match_and_score(self, goal_lower: str) -> tuple[bool, int]:
        """Whether this source applies to ``goal_lower`` and its relevance rank, in one
        keyword scan. Unlike a Skill, ``matches`` is NOT equivalent to ``score > 0`` here:
        OKF sources have a nonzero ``priority`` floor, so the two must be returned together."""
        if self.always:
            return True, 1_000_000 + self.priority
        hits = sum(1 for kw in self.triggers.keywords if _keyword_in_text(kw, goal_lower))
        return hits > 0, self.priority + 5 * hits

    def matches(self, goal: str) -> bool:
        """True if this source applies to the given goal text.

        Design systems are always-on (a coding agent should always honor them); OKF
        knowledge is matched on goal keywords like a domain skill.
        """
        return self._match_and_score(goal.lower())[0]

    def relevance_score(self, goal: str) -> int:
        return self._match_and_score(goal.lower())[1]

    def render(self) -> str:
        """A titled markdown block for inclusion in a prompt preamble."""
        header = f"## {self.description or self.name}".rstrip()
        return f"{header}\n\n{self.body.strip()}".strip()


# Parsed-source cache keyed on (resolved path, mtime_ns, kind, always, priority). Knowledge
# discovery runs once per task during planning (and repeatedly via MCP), so without this an
# N-task plan re-reads + re-parses every knowledge file N times. Keyed on mtime so an edited
# or freshly-ingested file is re-parsed; the args are in the key because they shape the
# resulting source. Cached sources are read-only (callers only match/score/render them).
_source_cache: dict[tuple[str, int, str, bool, int], KnowledgeSource] = {}
_SOURCE_CACHE_MAX = 512


def clear_knowledge_caches() -> None:
    """Drop the cached parsed knowledge sources (useful in long-running processes/tests)."""
    _source_cache.clear()


def _source_from_file(path: Path, kind: Kind, always: bool, priority: int) -> KnowledgeSource:
    try:
        key: tuple[str, int, str, bool, int] | None = (
            str(path.resolve()), path.stat().st_mtime_ns, kind, always, priority,
        )
    except OSError:
        key = None
    if key is not None:
        cached = _source_cache.get(key)
        if cached is not None:
            return cached
    source = _parse_source_file(path, kind, always, priority)
    if key is not None:
        if len(_source_cache) >= _SOURCE_CACHE_MAX:
            _source_cache.clear()
        _source_cache[key] = source
    return source


def _parse_source_file(path: Path, kind: Kind, always: bool, priority: int) -> KnowledgeSource:
    meta, body = split_frontmatter(path.read_text(encoding="utf-8"))
    triggers = meta.get("triggers") or {}
    # Derive keywords from explicit triggers and, for OKF, the document's tags — so an
    # OKF doc tagged [sales, revenue] fires on goals mentioning those domains for free.
    keywords = list(triggers.get("keywords") or [])
    if kind == "okf":
        tags = meta.get("tags") or []
        if isinstance(tags, str):
            tags = [tags]
        keywords.extend(str(t) for t in tags)
    description = str(meta.get("description") or meta.get("title") or meta.get("type") or path.stem)
    return KnowledgeSource(
        name=str(meta.get("name") or path.stem),
        kind=kind,
        description=description,
        always=always,
        triggers=SkillTriggers(keywords=keywords, globs=list(triggers.get("globs") or [])),
        body=body.strip(),
        priority=priority,
        source_path=path,
    )


def discover_knowledge_sources(
    project_root: Path,
    directory: str = KNOWLEDGE_DIR,
    design_always: bool = True,
    design_priority: int = 80,
    okf_priority: int = 50,
) -> list[KnowledgeSource]:
    """Find ingested knowledge under ``<project_root>/<directory>/{design,okf}``."""
    base = project_root / directory
    sources: list[KnowledgeSource] = []
    design_dir = base / "design"
    if design_dir.exists():
        for path in sorted(design_dir.glob("*.md")):
            sources.append(_source_from_file(path, "design", design_always, design_priority))
    okf_dir = base / "okf"
    if okf_dir.exists():
        for path in sorted(okf_dir.rglob("*.md")):
            # Skip OKF index files: they are navigation, not knowledge worth injecting.
            if path.name.lower() == "index.md":
                continue
            sources.append(_source_from_file(path, "okf", False, okf_priority))
    return sources


def select_knowledge_sources(
    goal: str = "",
    project_root: Path | None = None,
    directory: str = KNOWLEDGE_DIR,
    design_always: bool = True,
) -> list[KnowledgeSource]:
    """Select and rank the knowledge sources that apply to ``goal``.

    Always-on design sources sort first; OKF sources follow, ranked by relevance. Returns
    an empty list when nothing is ingested.
    """
    if project_root is None:
        return []
    sources = discover_knowledge_sources(project_root, directory, design_always=design_always)
    # One goal.lower() + one keyword scan per source (match and rank computed together).
    goal_lower = goal.lower()
    scored: list[tuple[KnowledgeSource, int]] = []
    for source in sources:
        ok, score = source._match_and_score(goal_lower)
        if ok:
            scored.append((source, score))
    scored.sort(key=lambda item: (not item[0].always, -item[1], item[0].name))
    return [source for source, _ in scored]


def render_knowledge_preamble(
    sources: list[KnowledgeSource],
    max_chars: int = 6000,
    kind: Kind | None = None,
) -> str:
    """Concatenate source bodies (optionally filtered to one ``kind``) into a single
    preamble, bounded to ``max_chars`` total. Sources are taken in the order given (so the
    most relevant survive the budget); the first source is always included."""
    separator = "\n\n---\n\n"
    chosen = [s for s in sources if kind is None or s.kind == kind]
    blocks: list[str] = []
    total = 0
    for source in chosen:
        block = source.render()
        if not block:
            continue
        # Count the separator that will join this block to the previous one, so the budget
        # bounds the *rendered* preamble length, not just the sum of block bodies.
        extra = len(separator) if blocks else 0
        if blocks and total + extra + len(block) > max_chars:
            break
        blocks.append(block)
        total += extra + len(block)
    return separator.join(blocks).strip()
