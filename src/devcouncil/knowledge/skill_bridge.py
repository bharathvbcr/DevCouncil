"""Single source of truth for Skill <-> OKF document interconversion.

DevCouncil skills (:class:`devcouncil.skills.registry.Skill`) and the Open Knowledge
Format (:class:`devcouncil.knowledge.okf.OKFDocument`) describe the same kind of
artifact from two angles: a skill is "guidance that fires on triggers", an OKF document
is "a typed, portable markdown node". This module is the one place that maps between
them, so exporting skills into an OKF bundle and ingesting an OKF bundle back into skills
stay symmetric and don't drift apart across the codebase.

Skill documents are marked with the OKF ``type`` value :data:`SKILL_OKF_TYPE`; that type
tag is what lets :func:`okf_document_to_skill` tell skill nodes apart from other OKF nodes
(BigQuery tables, tasks, requirements, ...) in a mixed bundle.

Import-cycle note: :mod:`devcouncil.skills.registry` imports this module, so ``Skill`` /
``SkillTriggers`` are imported *lazily* inside :func:`okf_document_to_skill` rather than at
module top. ``OKFDocument`` is safe to import at top because ``knowledge.okf`` does not
import the skills package.
"""

from __future__ import annotations

import re
from pathlib import Path
from typing import TYPE_CHECKING

from devcouncil.knowledge.okf import OKFDocument

if TYPE_CHECKING:
    from devcouncil.skills.registry import Skill

# The OKF `type` frontmatter value carried by every skill document. Used both when
# emitting skills (export) and when filtering a mixed bundle back into skills (ingest).
SKILL_OKF_TYPE = "Engineering Skill"

# Fallback name derivation when a document has no rel_path to take a stem from.
_SLUG_RE = re.compile(r"[^a-z0-9]+")


def _slug(text: str) -> str:
    """Lowercase, hyphen-joined slug of ``text`` (used to name a skill that lacks a path)."""
    return _SLUG_RE.sub("-", text.strip().lower()).strip("-") or "skill"


def skill_to_okf_document(skill: "Skill", rel_dir: str = "skills") -> OKFDocument:
    """Render a :class:`Skill` as an OKF document for inclusion in a bundle.

    Keyword triggers become OKF ``tags`` (sorted + deduped for stable, diff-friendly
    output); ``timestamp`` is left empty because a skill is library content, not a
    timestamped artifact. The document lands at ``<rel_dir>/<skill.name>.md``.
    """
    return OKFDocument(
        type=SKILL_OKF_TYPE,
        title=skill.title or skill.name,
        description=skill.description,
        tags=sorted(set(skill.triggers.keywords)),
        timestamp="",
        body=skill.body,
        rel_path=f"{rel_dir.rstrip('/')}/{skill.name}.md",
    )


def is_skill_document(doc: OKFDocument) -> bool:
    """Whether ``doc`` is a skill node, i.e. its OKF ``type`` is :data:`SKILL_OKF_TYPE`.

    Comparison is case-insensitive and whitespace-trimmed so hand-edited bundles still
    round-trip.
    """
    return doc.type.strip().lower() == SKILL_OKF_TYPE.lower()


def okf_document_to_skill(doc: OKFDocument) -> "Skill | None":
    """Reconstruct a :class:`Skill` from an OKF document, or ``None`` if it isn't a skill.

    Non-skill-typed nodes (BigQuery tables, tasks, ...) return ``None`` so callers can
    map over a mixed bundle and keep only the skill nodes. The skill ``name`` comes from
    the document's ``rel_path`` stem when present, else a slug of its title. The stem is
    slugged too, so a foreign bundle whose file is ``skills/Foo Bar.md`` yields the skill
    name ``foo-bar`` (a clean identifier that scaffolds to a sane ``.claude/skills`` dir),
    not ``Foo Bar``. ``always`` is ``False`` and ``globs`` empty because OKF tags only carry
    keyword triggers; ``source_path`` is ``None`` since the skill originates from a bundle.
    """
    if not is_skill_document(doc):
        return None
    # Lazy import to avoid a registry <-> skill_bridge import cycle (see module docstring).
    from devcouncil.skills.registry import Skill, SkillTriggers

    name = _slug(Path(doc.rel_path).stem) if doc.rel_path else _slug(doc.title)
    return Skill(
        name=name,
        title=doc.title,
        description=doc.description,
        always=False,
        triggers=SkillTriggers(keywords=list(doc.tags), globs=[]),
        body=doc.body,
        source_path=None,
    )
