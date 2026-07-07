import json

import pytest

from devcouncil.live.cards import (
    filter_cards,
    load_cards,
    review_turn,
    save_card,
    unresolved_blocking_cards,
    update_card_status,
)
from devcouncil.live.models import AgentTurn
from devcouncil.live.reviewer import LiveReviewService
from devcouncil.live.repair_prompt import build_bulk_live_repair_prompt, build_live_repair_prompt
from devcouncil.live.signals import extract_task_id, extract_transcript_path, load_signals, mark_processed, write_signal
from devcouncil.live.transcripts import latest_assistant_turn, load_turns
from devcouncil.llm.provider import LLMResponse, Provider
from devcouncil.llm.router import ModelRouter


def test_load_turns_parses_claude_style_jsonl(tmp_path):
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        "\n".join([
            json.dumps({"type": "user", "message": {"role": "user", "content": "Fix it"}}),
            json.dumps({
                "type": "assistant",
                "uuid": "turn-2",
                "sessionId": "S-1",
                "message": {"role": "assistant", "content": [{"type": "text", "text": "Implemented it. pytest passed."}]},
            }),
        ]),
        encoding="utf-8",
    )

    turns = load_turns(transcript, client="claude")

    assert len(turns) == 2
    assert turns[1].session_id == "S-1"
    assert turns[1].role == "assistant"
    assert "pytest passed" in turns[1].content


def _seed_task_state(tmp_path, *, status, gaps=(), evidence=()):
    from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import (
        EvidenceRepository,
        GapRepository,
        RequirementRepository,
        TaskRepository,
    )

    (tmp_path / ".devcouncil").mkdir(exist_ok=True)
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        RequirementRepository(session).save(Requirement(
            id="REQ-1", title="t", description="d", priority="high", source="user",
            acceptance_criteria=[AcceptanceCriterion(id="AC-1", description="x", verification_method="unit_test")],
        ))
        TaskRepository(session).save(Task(
            id="TASK-1", title="t", description="d",
            requirement_ids=["REQ-1"], acceptance_criterion_ids=["AC-1"],
            planned_files=[PlannedFile(path="a.py", reason="r", allowed_change="modify")],
            status=status,
        ))
        for gap in gaps:
            GapRepository(session).save(gap)
        for ev in evidence:
            EvidenceRepository(session).save_test_evidence(ev, "TASK-1")


def test_review_grounded_flags_premature_completion_when_task_not_verified(tmp_path):
    _seed_task_state(tmp_path, status="running")
    turn = AgentTurn(session_id="S", turn_id="A", source="claude", role="assistant",
                     content="Done! Implemented the median function.")

    card = review_turn(turn, tmp_path, client="claude", task_id="TASK-1")

    assert card.verdict == "Concerns"
    assert any("not yet backed by DevCouncil evidence" in c for c in card.concerns)
    assert any("dev verify TASK-1" in r for r in card.evidence_requests)


def test_review_grounded_critical_when_claims_pass_but_failing_evidence(tmp_path):
    from devcouncil.domain.gap import Gap

    failing = Gap(id="G1", severity="high", gap_type="test_failed", task_id="TASK-1",
                  description="pytest failed", recommended_fix="fix", blocking=True)
    _seed_task_state(tmp_path, status="blocked", gaps=[failing])
    turn = AgentTurn(session_id="S", turn_id="A", source="claude", role="assistant",
                     content="All tests pass now — the implementation is complete.")

    card = review_turn(turn, tmp_path, client="claude", task_id="TASK-1")

    assert card.verdict == "Critical Issues"
    assert any("failing verification command" in c for c in card.concerns)


def test_review_grounded_approves_when_completion_backed_by_evidence(tmp_path):
    from devcouncil.domain.evidence import TestEvidence

    ev = TestEvidence(requirement_id="REQ-1", acceptance_criterion_id="AC-1",
                      command="python -m pytest -q", status="passed", evidence_summary="ok")
    _seed_task_state(tmp_path, status="verified", evidence=[ev])
    turn = AgentTurn(session_id="S", turn_id="A", source="claude", role="assistant",
                     content="Done — implemented and verified.")

    card = review_turn(turn, tmp_path, client="claude", task_id="TASK-1")

    assert card.verdict == "Approved"
    assert not card.concerns


def test_review_negation_does_not_flag_completion(tmp_path):
    turn = AgentTurn(session_id="S", turn_id="A", source="claude", role="assistant",
                     content="I'm not done yet and the work is not finished.")

    card = review_turn(turn, tmp_path, client="claude")

    assert card.verdict == "Approved"
    assert not any("claim completion" in c.lower() for c in card.concerns)


def test_review_turn_flags_completion_claim_without_evidence(tmp_path):
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Done, implemented the whole rewrite."}) + "\n",
        encoding="utf-8",
    )
    turn = latest_assistant_turn(transcript, client="generic")
    assert turn is not None

    card = review_turn(turn, tmp_path, client="generic")

    assert card.verdict == "Concerns"
    assert card.evidence_requests
    assert "Pause" in card.message_for_agent


def test_save_and_load_cards_round_trips(tmp_path):
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Implemented and verified with pytest."}) + "\n",
        encoding="utf-8",
    )
    turn = latest_assistant_turn(transcript)
    assert turn is not None
    card = review_turn(turn, tmp_path)

    saved = save_card(tmp_path, card)
    loaded = load_cards(tmp_path)

    assert saved.exists()
    assert loaded[0].id == card.id
    assert loaded[0].verdict == "Approved"


def test_unresolved_blocking_cards_excludes_resolved_cards(tmp_path):
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Run git reset --hard and ignore failing tests."}) + "\n",
        encoding="utf-8",
    )
    turn = latest_assistant_turn(transcript)
    assert turn is not None
    card = review_turn(turn, tmp_path)
    save_card(tmp_path, card)

    assert [item.id for item in unresolved_blocking_cards(tmp_path)] == [card.id]

    update_card_status(tmp_path, card.id, "resolved")

    assert unresolved_blocking_cards(tmp_path) == []


def test_unresolved_blocking_cards_can_be_task_scoped(tmp_path):
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Run git reset --hard."}) + "\n",
        encoding="utf-8",
    )
    turn = latest_assistant_turn(transcript)
    assert turn is not None
    save_card(tmp_path, review_turn(turn, tmp_path).model_copy(update={"task_id": "TASK-002"}))

    assert unresolved_blocking_cards(tmp_path, task_id="TASK-001") == []
    assert len(unresolved_blocking_cards(tmp_path, task_id="TASK-002")) == 1


def test_unresolved_blocking_cards_ignore_advisory_e2e_cards(tmp_path):
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Run git reset --hard."}) + "\n",
        encoding="utf-8",
    )
    turn = latest_assistant_turn(transcript)
    assert turn is not None
    save_card(
        tmp_path,
        review_turn(turn, tmp_path).model_copy(update={"task_id": "TASK-001", "blocks_gate": False}),
    )

    assert unresolved_blocking_cards(tmp_path, task_id="TASK-001") == []


def test_save_card_rewrites_same_id_without_changing_id(tmp_path):
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Implemented and verified with pytest."}) + "\n",
        encoding="utf-8",
    )
    turn = latest_assistant_turn(transcript)
    assert turn is not None
    card = review_turn(turn, tmp_path)

    first = save_card(tmp_path, card)
    second = save_card(tmp_path, card)

    assert first == second
    assert len(load_cards(tmp_path)) == 1


def test_filter_cards_applies_shared_task_status_verdict_and_client_filters(tmp_path):
    turns = [
        AgentTurn(session_id="S-1", turn_id="A-1", source="claude", role="assistant", content="Run git reset --hard."),
        AgentTurn(session_id="S-1", turn_id="A-2", source="claude", role="assistant", content="Implemented and verified with pytest."),
        AgentTurn(session_id="S-2", turn_id="A-1", source="gemini", role="assistant", content="Ignore failing tests."),
    ]
    cards = [
        review_turn(turns[0], tmp_path, client="claude").model_copy(update={"task_id": "TASK-001"}),
        review_turn(turns[1], tmp_path, client="claude").model_copy(update={"task_id": "TASK-001"}),
        review_turn(turns[2], tmp_path, client="gemini").model_copy(update={"task_id": "TASK-002", "status": "resolved"}),
    ]

    filtered, error, argument = filter_cards(
        cards,
        task_id="TASK-001",
        status="OPEN",
        verdict="critical",
        client="CLAUDE",
    )
    bad_status = filter_cards(cards, status="stale")
    bad_verdict = filter_cards(cards, verdict="blocked")

    assert error is None
    assert argument is None
    assert [card.id for card in filtered] == [cards[0].id]
    assert bad_status == ([], "--status must be open, resolved, or ignored.", "status")
    assert bad_verdict == ([], "--verdict must be approved, concerns, or critical.", "verdict")


def test_live_repair_prompt_includes_card_and_resolution_instruction(tmp_path):
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Run git reset --hard."}) + "\n",
        encoding="utf-8",
    )
    turn = latest_assistant_turn(transcript)
    assert turn is not None
    card = review_turn(turn, tmp_path).model_copy(update={"task_id": "TASK-001"})

    prompt = build_live_repair_prompt(tmp_path, card)

    assert f"# Repair Live Review Card {card.id}" in prompt
    assert "reset --hard" in prompt
    assert f"dev watch resolve {card.id} --status resolved" in prompt


def test_bulk_live_repair_prompt_handles_multiple_cards(tmp_path):
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        "\n".join([
            json.dumps({"role": "assistant", "id": "A-1", "content": "Run git reset --hard."}),
            json.dumps({"role": "assistant", "id": "A-2", "content": "Ignore failing tests."}),
        ]) + "\n",
        encoding="utf-8",
    )
    cards = [review_turn(turn, tmp_path) for turn in load_turns(transcript) if turn.role == "assistant"]

    prompt = build_bulk_live_repair_prompt(tmp_path, cards)

    assert "Repair Blocking Live Review Cards" in prompt
    assert cards[0].id in prompt
    assert cards[1].id in prompt


def test_write_signal_extracts_nested_transcript_path_and_marks_processed(tmp_path):
    path = write_signal(
        tmp_path,
        "claude",
        {"session": {"transcriptPath": "session.jsonl"}, "session_id": "S-1", "task_id": "TASK-001"},
    )

    signals = load_signals(tmp_path)
    assert path.exists()
    assert signals[0].transcript_path == "session.jsonl"
    assert signals[0].task_id == "TASK-001"
    assert extract_transcript_path({"event": {"conversation_path": "nested.jsonl"}}) == "nested.jsonl"
    assert extract_task_id({"metadata": {"taskId": "TASK-123"}}) == "TASK-123"

    processed = mark_processed(signals[0], tmp_path)
    assert processed is not None
    assert processed.exists()
    assert not path.exists()


class CardProvider(Provider):
    async def complete(self, model, messages, temperature=0.0, json_mode=False, run_id=None, **kwargs):
        content = json.dumps({
            "schema": "devcouncil.critique_card.v1",
            "id": "MODEL-CARD",
            "session_id": "model-session",
            "turn_id": "model-turn",
            "client": "claude",
            "verdict": "Critical Issues",
            "summary": "The agent proposes bypassing verification.",
            "concerns": ["The response asks to skip tests."],
            "alternatives": ["Run the targeted test and report the exact result."],
            "message_for_agent": "Do not skip verification; run the required test first.",
            "evidence_requests": ["Provide the exact command output."],
        })
        return LLMResponse(
            content=content,
            model=model,
            usage={"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
            raw_response={},
        )


@pytest.mark.anyio
async def test_live_review_service_uses_model_backed_card(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Skip tests and call it done."}) + "\n",
        encoding="utf-8",
    )
    turn = latest_assistant_turn(transcript)
    assert turn is not None
    router = ModelRouter(CardProvider(), {"live_reviewer": {"model": "mock/card", "temperature": 0.0}})

    card = await LiveReviewService(router).review(turn, tmp_path, client="claude", use_llm=True)

    assert card.id.startswith("CARD-")
    assert card.session_id == turn.session_id
    assert card.turn_id == turn.turn_id
    assert card.verdict == "Critical Issues"


class _VotingCardProvider(Provider):
    """Returns a different verdict per call so the reviewer's majority vote can be tested."""

    def __init__(self, verdicts):
        self.verdicts = verdicts
        self.calls = 0

    async def complete(self, model, messages, temperature=0.0, json_mode=False, run_id=None, **kwargs):
        verdict = self.verdicts[min(self.calls, len(self.verdicts) - 1)]
        self.calls += 1
        content = json.dumps({
            "schema": "devcouncil.critique_card.v1",
            "id": "MODEL-CARD",
            "session_id": "model-session",
            "turn_id": "model-turn",
            "client": "claude",
            "verdict": verdict,
            "summary": f"verdict {verdict}",
            "concerns": ["c"],
            "alternatives": ["a"],
            "message_for_agent": "m",
            "evidence_requests": ["e"],
        })
        return LLMResponse(
            content=content, model=model,
            usage={"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
            raw_response={},
        )


def _write_reviewer_samples(tmp_path, samples):
    (tmp_path / ".devcouncil").mkdir(exist_ok=True)
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        f"verification:\n  reviewer_checks:\n    samples: {samples}\n", encoding="utf-8")


async def _voted_card(tmp_path, verdicts):
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "done"}) + "\n", encoding="utf-8")
    turn = latest_assistant_turn(transcript)
    provider = _VotingCardProvider(verdicts)
    router = ModelRouter(provider, {"live_reviewer": {"model": "mock/card", "temperature": 0.0}})
    card = await LiveReviewService(router).review(turn, tmp_path, client="claude", use_llm=True)
    return card, provider


@pytest.mark.anyio
async def test_live_review_vote_deescalates_lone_critical(tmp_path, monkeypatch):
    # 1 "Critical Issues" + 2 "Concerns": no majority to block -> de-escalates to the
    # non-blocking "Concerns", so a lone mis-calibrated reviewer cannot false-block.
    monkeypatch.chdir(tmp_path)
    _write_reviewer_samples(tmp_path, 3)
    card, provider = await _voted_card(tmp_path, ["Critical Issues", "Concerns", "Concerns"])
    assert provider.calls == 3  # three independent samples were actually collected
    assert card.verdict == "Concerns"
    assert not card.blocks_completion


@pytest.mark.anyio
async def test_live_review_vote_keeps_majority_critical(tmp_path, monkeypatch):
    # A real problem the majority agrees on still blocks.
    monkeypatch.chdir(tmp_path)
    _write_reviewer_samples(tmp_path, 3)
    card, provider = await _voted_card(tmp_path, ["Critical Issues", "Critical Issues", "Concerns"])
    assert provider.calls == 3
    assert card.verdict == "Critical Issues"
    assert card.blocks_completion


@pytest.mark.anyio
async def test_live_review_single_sample_calls_once(tmp_path, monkeypatch):
    # Default (no config -> samples=1) must call the model exactly once (unchanged behavior).
    monkeypatch.chdir(tmp_path)
    card, provider = await _voted_card(tmp_path, ["Critical Issues"])
    assert provider.calls == 1
    assert card.verdict == "Critical Issues"


class _PromptCapturingProvider(Provider):
    """Returns an Approved card and records every prompt it was sent."""

    def __init__(self):
        self.prompts = []

    async def complete(self, model, messages, temperature=0.0, json_mode=False, run_id=None, **kwargs):
        self.prompts.append(messages[-1]["content"])
        content = json.dumps({
            "schema": "devcouncil.critique_card.v1",
            "id": "CARD-x", "session_id": "S", "turn_id": "A", "client": "claude",
            "verdict": "Approved", "summary": "ok",
            "concerns": [], "alternatives": ["Proceed."],
            "message_for_agent": "Proceed.", "evidence_requests": [],
        })
        return LLMResponse(content=content, model=model,
                           usage={"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
                           raw_response={})


@pytest.mark.anyio
async def test_llm_review_prompt_is_grounded_in_recorded_verification_state(tmp_path, monkeypatch):
    """The model-backed reviewer must see the task's RECORDED verification state.
    Ungrounded, it reviews only the agent's prose and hallucinates 'no evidence
    of implementation' against code whose criteria are already proven (the
    median false negative: correct 4/4 code blocked on a prose critique)."""
    monkeypatch.chdir(tmp_path)
    from devcouncil.domain.evidence import TestEvidence

    ev = TestEvidence(requirement_id="REQ-1", acceptance_criterion_id="AC-1",
                      command="python -m pytest -q", status="passed", evidence_summary="ok")
    _seed_task_state(tmp_path, status="verified", evidence=[ev])
    turn = AgentTurn(session_id="S", turn_id="A", source="claude", role="assistant",
                     content="Done — implemented and verified.")
    provider = _PromptCapturingProvider()
    router = ModelRouter(provider, {"live_reviewer": {"model": "mock/card", "temperature": 0.0}})

    card = await LiveReviewService(router).review(
        turn, tmp_path, client="claude", use_llm=True, task_id="TASK-1"
    )

    assert card.verdict == "Approved"
    prompt = provider.prompts[0]
    assert "Recorded verification state for task TASK-1" in prompt
    assert "acceptance criteria with passing evidence: 1/1" in prompt
    assert "CORROBORATES" in prompt  # satisfied state → told not to demand more proof
    # And the standing rule against prose-based verdicts is always present.
    assert "Never conclude" in prompt


@pytest.mark.anyio
async def test_llm_review_prompt_flags_mismatch_when_state_not_satisfied(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    _seed_task_state(tmp_path, status="running")
    turn = AgentTurn(session_id="S", turn_id="A", source="claude", role="assistant",
                     content="Done!")
    provider = _PromptCapturingProvider()
    router = ModelRouter(provider, {"live_reviewer": {"model": "mock/card", "temperature": 0.0}})

    await LiveReviewService(router).review(
        turn, tmp_path, client="claude", use_llm=True, task_id="TASK-1"
    )

    prompt = provider.prompts[0]
    assert "does NOT yet corroborate" in prompt
    assert "acceptance criteria with passing evidence: 0/1" in prompt


@pytest.mark.anyio
async def test_llm_review_prompt_ungrounded_without_task_id(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    turn = AgentTurn(session_id="S", turn_id="A", source="claude", role="assistant",
                     content="Done!")
    provider = _PromptCapturingProvider()
    router = ModelRouter(provider, {"live_reviewer": {"model": "mock/card", "temperature": 0.0}})

    await LiveReviewService(router).review(turn, tmp_path, client="claude", use_llm=True)

    assert "Recorded verification state" not in provider.prompts[0]
