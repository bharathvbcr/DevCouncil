from pathlib import Path

from pydantic import BaseModel, Field

from devcouncil.llm.router import ModelRouter

# Cap how much skill text we feed the enhancer so a repo matching many skills
# can't blow up the planning prompt. Domain skills are ~50 lines each.
_MAX_SKILLS_FOR_INTAKE = 4
_MAX_INTAKE_CHARS = 8000


class PromptEnhancement(BaseModel):
    original_goal: str
    enhanced_goal: str
    codebase_context: list[str] = Field(default_factory=list)
    debate_focus: list[str] = Field(default_factory=list)
    constraints: list[str] = Field(default_factory=list)
    # Senior-level domain intake folded in from the skills library (android, ios,
    # web, ...). ``applied_skills`` are the matched skill names; ``skills_brief`` is
    # the compact title+description block the council debates with. Both are set
    # deterministically after the model call — the LLM does not populate them.
    applied_skills: list[str] = Field(default_factory=list)
    skills_brief: str = ""
    # Curated project knowledge (Open Knowledge Format bundles) and the project design
    # system (design.md), selected from ``.devcouncil/knowledge`` for this goal. Like the
    # skills fields, set deterministically after the model call.
    applied_knowledge: list[str] = Field(default_factory=list)
    knowledge_brief: str = ""

    def normalized(self, original_goal: str) -> "PromptEnhancement":
        enhanced_goal = self.enhanced_goal.strip() or original_goal
        return self.model_copy(
            update={
                "original_goal": original_goal,
                "enhanced_goal": enhanced_goal,
                "codebase_context": _clean_items(self.codebase_context),
                "debate_focus": _clean_items(self.debate_focus),
                "constraints": _clean_items(self.constraints),
            }
        )

    def debate_prompt(self) -> str:
        sections = [
            "# Enhanced Planning Prompt",
            "",
            "## Original user goal",
            self.original_goal,
            "",
            "## Codebase-specific goal",
            self.enhanced_goal,
        ]
        if self.codebase_context:
            sections.extend(["", "## Relevant codebase context"])
            sections.extend(f"- {item}" for item in self.codebase_context)
        if self.constraints:
            sections.extend(["", "## Constraints to preserve"])
            sections.extend(f"- {item}" for item in self.constraints)
        if self.debate_focus:
            sections.extend(["", "## Debate focus"])
            sections.extend(f"- {item}" for item in self.debate_focus)
        if self.skills_brief:
            sections.extend([
                "",
                "## Domain engineering intake (apply current senior-level practices)",
                "Plan to the *current* state of these domains — recommended libraries, "
                "deprecations to avoid, and the right build/test CLI commands. The coding "
                "agent receives the full skill text; the plan must already assume it.",
                self.skills_brief,
            ])
        if self.knowledge_brief:
            sections.extend([
                "",
                "## Project knowledge & design system (ground the plan in these)",
                "Curated org/domain knowledge (Open Knowledge Format) and the project's "
                "design.md. Honor design tokens/components and reuse known facts rather than "
                "re-deriving or contradicting them.",
                self.knowledge_brief,
            ])
        return "\n".join(sections)


class PromptEnhancerService:
    def __init__(self, router: ModelRouter):
        self.router = router

    async def enhance_prompt(
        self,
        goal: str,
        repo_map_json: str,
        graph_context_json: str | None = None,
        project_root: Path | None = None,
    ) -> PromptEnhancement:
        skills = _select_skills(goal, project_root)
        skills_intake = _full_intake(skills)
        skills_brief = _compact_brief(skills)

        knowledge = _select_knowledge(goal, project_root)
        knowledge_intake = _knowledge_intake(knowledge)
        knowledge_brief = _knowledge_brief(knowledge)

        prompt = f"""
Original user goal:
{goal}

Repository map:
{repo_map_json}

Code review graph context:
{graph_context_json or "{}"}

Applicable engineering skills (senior-level domain intake for this codebase/goal):
{skills_intake or "(no domain skills matched; rely on general engineering judgment)"}

Project knowledge & design system (curated facts and design tokens for this codebase):
{knowledge_intake or "(no project knowledge ingested)"}

You are DevCouncil's codebase-specific prompt enhancer.
Rewrite the user goal into a better planning prompt before it is sent to the council debate.

Requirements:
- Preserve the user's intent exactly; do not add unrelated features.
- Make the goal specific to the mapped repository architecture, languages, tests, and likely ownership boundaries.
- Fold the relevant skill intake into the goal and constraints like a senior engineer who
  just briefed themselves: name the *current* recommended libraries/APIs, the deprecated
  ones to avoid, the platform/SDK/toolchain versions to target, and the exact build/test
  CLI commands that will prove the change. Only include skill points relevant to THIS goal.
- Identify constraints the planners and critics must preserve.
- Identify debate focus areas that should force useful disagreement between pragmatic and production-readiness plans.
- Keep the enhanced_goal concise enough to be used as the goal for spec, planning, critique, and arbitration.
"""
        enhancement = await self.router.complete_structured(
            role="prompt_enhancer",
            messages=[{"role": "user", "content": prompt}],
            schema=PromptEnhancement,
            # If enhancement fails on a weak model, fall back to the raw goal —
            # planning proceeds with the user's original intent unchanged.
            fallback=PromptEnhancement(original_goal=goal, enhanced_goal=goal),
        )
        # Skill provenance is deterministic, not model-decided: stamp it after the call
        # so the artifact/report shows exactly which skills shaped this plan.
        return enhancement.normalized(goal).model_copy(
            update={
                "applied_skills": [skill.name for skill in skills],
                "skills_brief": skills_brief,
                "applied_knowledge": [source.name for source in knowledge],
                "knowledge_brief": knowledge_brief,
            }
        )


def _select_skills(goal: str, project_root: Path | None):
    """Codebase-aware skill selection; never raises (skills are best-effort)."""
    try:
        from devcouncil.skills.registry import select_skills

        return select_skills(goal=goal, project_root=project_root)
    except Exception:
        return []


def _full_intake(skills: list) -> str:
    """Full skill bodies (capped) for the one-shot enhancer call."""
    if not skills:
        return ""
    blocks: list[str] = []
    total = 0
    for skill in skills[:_MAX_SKILLS_FOR_INTAKE]:
        body = (getattr(skill, "body", "") or "").strip()
        if not body:
            continue
        block = f"### Skill: {skill.name}\n{body}"
        total += len(block)
        if total > _MAX_INTAKE_CHARS:
            break
        blocks.append(block)
    return "\n\n".join(blocks).strip()


def _compact_brief(skills: list) -> str:
    """One line per skill (name + description) for the council debate prompt."""
    lines = []
    for skill in skills:
        description = (getattr(skill, "description", "") or "").strip()
        lines.append(f"- **{skill.name}** — {description}" if description else f"- **{skill.name}**")
    return "\n".join(lines).strip()


def _select_knowledge(goal: str, project_root: Path | None):
    """OKF/design knowledge selection for planning; never raises (best-effort)."""
    if project_root is None:
        return []
    try:
        from devcouncil.app.config import load_config
        from devcouncil.knowledge.sources import select_knowledge_sources

        cfg = load_config(project_root).knowledge
        if not cfg.enabled:
            return []
        return select_knowledge_sources(
            goal=goal, project_root=project_root,
            directory=cfg.directory, design_always=cfg.design_always,
        )
    except Exception:
        return []


def _knowledge_intake(sources: list) -> str:
    """Full knowledge bodies (capped) for the one-shot enhancer call."""
    if not sources:
        return ""
    blocks: list[str] = []
    total = 0
    for source in sources[:_MAX_SKILLS_FOR_INTAKE]:
        body = (getattr(source, "body", "") or "").strip()
        if not body:
            continue
        kind = getattr(source, "kind", "knowledge")
        block = f"### {kind}: {getattr(source, 'name', '')}\n{body}"
        total += len(block)
        if total > _MAX_INTAKE_CHARS:
            break
        blocks.append(block)
    return "\n\n".join(blocks).strip()


def _knowledge_brief(sources: list) -> str:
    """One line per knowledge source (kind + name + description) for the debate prompt."""
    lines = []
    for source in sources:
        description = (getattr(source, "description", "") or "").strip()
        kind = getattr(source, "kind", "knowledge")
        head = f"- **{getattr(source, 'name', '')}** ({kind})"
        lines.append(f"{head} — {description}" if description else head)
    return "\n".join(lines).strip()


def _clean_items(items: list[str]) -> list[str]:
    return [item.strip() for item in items if item.strip()]
