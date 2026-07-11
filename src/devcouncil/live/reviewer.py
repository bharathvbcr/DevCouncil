from __future__ import annotations

import asyncio
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
        task_id: str | None = None,
    ) -> CritiqueCard:
        fallback = review_turn(turn, project_root, client=client, task_id=task_id)
        if not use_llm or self.router is None:
            logger.debug("Live review (deterministic) for %s turn=%s: %s", client, turn.turn_id, fallback.verdict)
            return fallback

        # Ground the model in the task's RECORDED verification state. Without this
        # the reviewer sees only the agent's prose and reliably hallucinates
        # "provides no evidence of implementation" against code whose acceptance
        # criteria DevCouncil has already proven with executed checks (observed:
        # a benchmark task blocked at 4/4 hidden-test correctness because the
        # ungrounded reviewer critiqued the agent's summary message).
        grounding_block = self._grounding_block(project_root, task_id)

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

Rules for the verdict:
- The response text is a SUMMARY, not the work itself. Never conclude "no code change /
  no evidence" from the response alone — the recorded verification state below (when
  present) is the authoritative account of the work.
- "Critical Issues" requires a concrete, checkable problem: a claim contradicted by the
  recorded state, an unsafe command, or a missing requirement — never vagueness, brevity,
  style, or your estimate of the agent's effort.
{grounding_block}
Client: {client}
Session: {turn.session_id}
Turn: {turn.turn_id}

Assistant response:
{turn.content}
"""
        samples = self._samples(project_root)

        async def _one_sample(attempt: int) -> CritiqueCard | None:
            if self.router is None:
                return None
            # Vary temperature so independent samples actually differ (and so the router
            # cache returns distinct generations). Attempt 0 stays deterministic.
            temperature = 0.0 if attempt == 0 else min(0.8, 0.3 + 0.2 * attempt)
            try:
                return await self.router.complete_structured(
                    role=self.role,
                    messages=[{"role": "user", "content": prompt}],
                    schema=CritiqueCard,
                    temperature=temperature,
                )
            except ValueError:
                try:
                    return await self.router.complete_structured(
                        role="implementation_reviewer",
                        messages=[{"role": "user", "content": prompt}],
                        schema=CritiqueCard,
                        temperature=temperature,
                    )
                except Exception:
                    return None
            except Exception:
                return None

        # The samples are independent by construction (no sample reads another's
        # output), so gather them concurrently instead of awaiting one at a time.
        # Order is preserved (attempt 0's deterministic card stays first) and each
        # sample keeps its own error handling above; return_exceptions only shields
        # siblings from anything that still escapes, which the filter then drops.
        results = await asyncio.gather(
            *(_one_sample(attempt) for attempt in range(samples)),
            return_exceptions=True,
        )
        cards: list[CritiqueCard] = [card for card in results if isinstance(card, CritiqueCard)]

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

    @staticmethod
    def _grounding_block(project_root: Path, task_id: str | None) -> str:
        """Render the task's recorded verification state for the review prompt.

        Best-effort: no task id / no DB / unknown task yields an empty block and
        the model reviews ungrounded (the deterministic fallback stays grounded
        via ``review_turn``)."""
        try:
            from devcouncil.live.cards import _load_task_grounding

            grounding = _load_task_grounding(project_root, task_id)
        except Exception:
            grounding = None
        if grounding is None:
            return ""
        if grounding.is_satisfied:
            corroboration = (
                "This state CORROBORATES a completion claim: do not demand further "
                "evidence and do not raise Critical Issues about proof — the "
                "executable evidence already exists in DevCouncil's records."
            )
        else:
            corroboration = (
                "This state does NOT yet corroborate a completion claim: if the "
                "response claims success, that mismatch is the concern to raise "
                "(cite the numbers above)."
            )
        return f"""
Recorded verification state for task {grounding.task_id} (authoritative — measured from
executed checks and the artifact graph, not taken from the agent's words):
- task status: {grounding.status}
- blocking gaps: {grounding.blocking_gaps}
- failing verification commands: {grounding.failing_commands}
- acceptance criteria with passing evidence: {grounding.acs_passing}/{grounding.acs_total}
{corroboration}
"""

    def _samples(self, project_root: Path) -> int:
        try:
            from devcouncil.app.config import load_config, role_runs_on_local_provider

            cfg = load_config(project_root)
            # Auto-tune by reviewer locality: a cost-free local (Ollama) reviewer votes
            # over 3 independent samples (outvoting a lone mis-calibrated verdict); a
            # paid cloud reviewer stays single-shot. Explicit config always wins —
            # but an explicitly single-shot LOCAL reviewer is flagged, never silent.
            local_reviewer = role_runs_on_local_provider(cfg, "live_reviewer")
            from devcouncil.telemetry.logging_setup import warn_once

            for warning in cfg.verification.reviewer_checks.unsafe_override_warnings(local_reviewer):
                warn_once(logger, warning)  # _samples() runs per review; warn once
            return cfg.verification.reviewer_checks.resolved(local_reviewer)
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
