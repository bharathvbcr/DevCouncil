from __future__ import annotations

import logging
from pathlib import Path

from devcouncil.live.cards import review_turn
from devcouncil.live.models import AgentTurn, CritiqueCard
from devcouncil.llm.router import ModelRouter

logger = logging.getLogger(__name__)


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
            logger.debug("Live review (deterministic) for %s turn=%s: %s", client, turn.turn_id, fallback.verdict)
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
        samples = self._samples(project_root)
        cards: list[CritiqueCard] = []
        for attempt in range(samples):
            # Vary temperature so independent samples actually differ (and so the router
            # cache returns distinct generations). Attempt 0 stays deterministic.
            temperature = 0.0 if attempt == 0 else min(0.8, 0.3 + 0.2 * attempt)
            try:
                cards.append(await self.router.complete_structured(
                    role=self.role,
                    messages=[{"role": "user", "content": prompt}],
                    schema=CritiqueCard,
                    temperature=temperature,
                ))
            except ValueError:
                try:
                    cards.append(await self.router.complete_structured(
                        role="implementation_reviewer",
                        messages=[{"role": "user", "content": prompt}],
                        schema=CritiqueCard,
                        temperature=temperature,
                    ))
                except Exception:
                    continue
            except Exception:
                continue

        if not cards:
            logger.warning("Live review produced no cards for %s turn=%s; using deterministic fallback", client, turn.turn_id)
            return fallback

        reviewed = self._vote(cards)
        logger.info(
            "Live review (LLM, %d sample(s)) for %s turn=%s: %s",
            len(cards), client, turn.turn_id, reviewed.verdict,
        )
        return reviewed.model_copy(update={
            "id": fallback.id,
            "session_id": turn.session_id,
            "turn_id": turn.turn_id,
            "client": client,
            "source_path": fallback.source_path,
        })

    def _samples(self, project_root: Path) -> int:
        try:
            from devcouncil.app.config import load_config, role_runs_on_local_provider

            cfg = load_config(project_root)
            # Auto-tune by reviewer locality: a cost-free local (Ollama) reviewer votes
            # over 3 independent samples (outvoting a lone mis-calibrated verdict); a
            # paid cloud reviewer stays single-shot. Explicit config always wins.
            return cfg.verification.reviewer_checks.resolved(
                role_runs_on_local_provider(cfg, "live_reviewer")
            )
        except Exception:
            return 1

    @staticmethod
    def _vote(cards: list[CritiqueCard]) -> CritiqueCard:
        """Majority-vote the verdict across independent reviews, then return a card whose
        verdict matches the vote (so its concerns/evidence are consistent).

        A single review is returned as-is. With several, the BLOCKING verdict
        ("Critical Issues") is chosen only on a strict majority, and "Approved" likewise;
        anything else de-escalates to the non-blocking "Concerns". This prevents a lone
        mis-calibrated reviewer from blocking, without ever auto-approving a real concern."""
        if len(cards) == 1:
            return cards[0]
        from collections import Counter

        counts = Counter(card.verdict for card in cards)
        threshold = len(cards) / 2
        if counts.get("Critical Issues", 0) > threshold:
            verdict = "Critical Issues"
        elif counts.get("Approved", 0) > threshold:
            verdict = "Approved"
        else:
            verdict = "Concerns"
        # Return a representative card with the voted verdict so concerns/evidence align;
        # fall back to the first card if none matches (then override just the verdict).
        for card in cards:
            if card.verdict == verdict:
                return card
        return cards[0].model_copy(update={"verdict": verdict})
