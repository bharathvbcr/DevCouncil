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

    def matches(self, goal: str) -> bool:
        """True if this source applies to the given goal text.

        Design systems are always-on (a coding agent should always honor them); OKF
        knowledge is matched on goal keywords like a domain skill.
        """
        if self.always:
            return True
        goal_lower = goal.lower()
        return any(_keyword_in_text(kw, goal_lower) for kw in self.triggers.keywords)

    def relevance_score(self, goal: str) -> int:
        if self.always:
            return 1_000_000 + self.priority
        goal_lower = goal.lower()
        hits = sum(1 for kw in self.triggers.keywords if _keyword_in_text(kw, goal_lower))
        return self.priority + 5 * hits

    def render(self) -> str:
        """A titled markdown block for inclusion in a prompt preamble."""
        header = f"## {self.description or self.name}".rstrip()
        return f"{header}\n\n{self.body.strip()}".strip()


def _source_from_file(path: Path, kind: Kind, always: bool, priority: int) -> KnowledgeSource:
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
    matched = [s for s in sources if s.matches(goal)]
    matched.sort(key=lambda s: (not s.always, -s.relevance_score(goal), s.name))
    return matched


def render_knowledge_preamble(
    sources: list[KnowledgeSource],
    max_chars: int = 6000,
    kind: Kind | None = None,
) -> str:
    """Concatenate source bodies (optionally filtered to one ``kind``) into a single
    preamble, bounded to ``max_chars`` total. Sources are taken in the order given (so the
    most relevant survive the budget); the first source is always included."""
    chosen = [s for s in sources if kind is None or s.kind == kind]
    blocks: list[str] = []
    total = 0
    for source in chosen:
        block = source.render()
        if not block:
            continue
        if blocks and total + len(block) > max_chars:
            break
        blocks.append(block)
        total += len(block)
    return "\n\n---\n\n".join(blocks).strip()
