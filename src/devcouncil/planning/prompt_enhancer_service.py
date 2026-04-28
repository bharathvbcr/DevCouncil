from pydantic import BaseModel, Field

from devcouncil.llm.router import ModelRouter


class PromptEnhancement(BaseModel):
    original_goal: str
    enhanced_goal: str
    codebase_context: list[str] = Field(default_factory=list)
    debate_focus: list[str] = Field(default_factory=list)
    constraints: list[str] = Field(default_factory=list)

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
        return "\n".join(sections)


class PromptEnhancerService:
    def __init__(self, router: ModelRouter):
        self.router = router

    async def enhance_prompt(
        self,
        goal: str,
        repo_map_json: str,
        graph_context_json: str | None = None,
    ) -> PromptEnhancement:
        prompt = f"""
Original user goal:
{goal}

Repository map:
{repo_map_json}

Code review graph context:
{graph_context_json or "{}"}

You are DevCouncil's codebase-specific prompt enhancer.
Rewrite the user goal into a better planning prompt before it is sent to the council debate.

Requirements:
- Preserve the user's intent exactly; do not add unrelated features.
- Make the goal specific to the mapped repository architecture, languages, tests, and likely ownership boundaries.
- Identify constraints the planners and critics must preserve.
- Identify debate focus areas that should force useful disagreement between pragmatic and production-readiness plans.
- Keep the enhanced_goal concise enough to be used as the goal for spec, planning, critique, and arbitration.
"""
        enhancement = await self.router.complete_structured(
            role="prompt_enhancer",
            messages=[{"role": "user", "content": prompt}],
            schema=PromptEnhancement,
        )
        return enhancement.normalized(goal)


def _clean_items(items: list[str]) -> list[str]:
    return [item.strip() for item in items if item.strip()]
