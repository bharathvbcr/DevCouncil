from __future__ import annotations

from pathlib import Path

from devcouncil.live.cards import review_turn
from devcouncil.live.models import AgentTurn, CritiqueCard
from devcouncil.llm.router import ModelRouter


class LiveReviewService:
    """Reviews coding-agent responses with deterministic or model-backed critique cards."""

    def __init__(self, router: ModelRouter | None = None, role: str = "live_reviewer"):
        self.router = router
        self.role = role

    async def review(
        self,
        turn: AgentTurn,
        project_root: Path,
        client: str = "generic",
        use_llm: bool = False,
    ) -> CritiqueCard:
        fallback = review_turn(turn, project_root, client=client)
        if not use_llm or self.router is None:
            return fallback

        prompt = f"""
You are DevCouncil's live coding-agent reviewer.
Review the latest assistant response before the developer follows it.

Return a critique card with:
- verdict: Approved, Concerns, or Critical Issues.
- concerns: concrete risks in the response, reasoning, plan, or proof.
- alternatives: safer approaches or architectures.
- evidence_requests: exact proof the agent should provide before claiming done.
- message_for_agent: a concise ready-to-paste instruction for the coding agent.

Do not praise. Do not review formatting. Focus on correctness, missing requirements, architectural drift,
unsafe commands, weak evidence, and premature completion claims.

Client: {client}
Session: {turn.session_id}
Turn: {turn.turn_id}

Assistant response:
{turn.content}
"""
        try:
            reviewed = await self.router.complete_structured(
                role=self.role,
                messages=[{"role": "user", "content": prompt}],
                schema=CritiqueCard,
            )
        except ValueError:
            reviewed = await self.router.complete_structured(
                role="implementation_reviewer",
                messages=[{"role": "user", "content": prompt}],
                schema=CritiqueCard,
            )
        except Exception:
            return fallback

        return reviewed.model_copy(update={
            "id": fallback.id,
            "session_id": turn.session_id,
            "turn_id": turn.turn_id,
            "client": client,
            "source_path": fallback.source_path,
        })
