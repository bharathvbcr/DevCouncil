"""live_reviewer must actually fire during dev run/e2e (not only via dev watch),
resolve the client's NATIVE transcript (like dev watch), be advisory (never raise),
and respect the integrations.live_review toggle.
"""

from types import SimpleNamespace

from devcouncil.app.config import DevCouncilConfig
from devcouncil.cli.commands import run as runmod


def _patch_review(monkeypatch, recorder, transcript_path):
    monkeypatch.setattr(
        "devcouncil.cli.commands.watch._resolve_transcript",
        lambda root, client, latest=False, task_id=None: transcript_path,
    )
    monkeypatch.setattr("devcouncil.live.transcripts.latest_assistant_turn",
                        lambda p, client=None: object())

    async def fake_review(turn, root, client, use_llm=True, task_id=None):
        recorder["call"] = {"client": client, "use_llm": use_llm, "task_id": task_id}
        return SimpleNamespace(id="card-1", verdict="Concerns")

    monkeypatch.setattr("devcouncil.cli.commands.watch._review_turn", fake_review)
    monkeypatch.setattr("devcouncil.cli.commands.watch._save_card_once",
                        lambda root, card, persist, force: (root / "card.json", False))
    monkeypatch.setattr("devcouncil.cli.commands.watch._log_card_reviewed", lambda *a, **k: None)


def test_live_review_fires_when_enabled(tmp_path, monkeypatch):
    transcript = tmp_path / "session.jsonl"
    transcript.write_text("{}", encoding="utf-8")
    monkeypatch.setattr(runmod, "load_config", lambda root: DevCouncilConfig())
    rec = {}
    _patch_review(monkeypatch, rec, transcript)
    runmod._run_live_review_after_execution(tmp_path, "claude", "TASK-001")
    assert rec["call"] == {"client": "claude", "use_llm": True, "task_id": "TASK-001"}


def test_live_review_skipped_when_disabled(tmp_path, monkeypatch):
    transcript = tmp_path / "session.jsonl"
    transcript.write_text("{}", encoding="utf-8")
    cfg = DevCouncilConfig()
    cfg.integrations.live_review.enabled = False
    monkeypatch.setattr(runmod, "load_config", lambda root: cfg)
    rec = {}
    _patch_review(monkeypatch, rec, transcript)
    runmod._run_live_review_after_execution(tmp_path, "claude", "TASK-001")
    assert "call" not in rec


def test_live_review_skipped_when_no_transcript_found(tmp_path, monkeypatch):
    monkeypatch.setattr(runmod, "load_config", lambda root: DevCouncilConfig())
    rec = {}
    _patch_review(monkeypatch, rec, None)  # _resolve_transcript → None
    runmod._run_live_review_after_execution(tmp_path, "claude", "TASK-001")
    assert "call" not in rec


def test_live_review_marks_cards_advisory(tmp_path, monkeypatch):
    from devcouncil.live.models import CritiqueCard

    transcript = tmp_path / "session.jsonl"
    transcript.write_text("{}", encoding="utf-8")
    monkeypatch.setattr(runmod, "load_config", lambda root: DevCouncilConfig())
    monkeypatch.setattr(
        "devcouncil.cli.commands.watch._resolve_transcript",
        lambda root, client, latest=False, task_id=None: transcript,
    )
    monkeypatch.setattr("devcouncil.live.transcripts.latest_assistant_turn", lambda p, client=None: object())

    async def fake_review(turn, root, client, use_llm=True, task_id=None):
        return CritiqueCard(
            id="CARD-advisory",
            session_id="S",
            turn_id="T",
            client=client,
            verdict="Critical Issues",
            summary="bad",
        )

    monkeypatch.setattr("devcouncil.cli.commands.watch._review_turn", fake_review)
    monkeypatch.setattr("devcouncil.cli.commands.watch._log_card_reviewed", lambda *a, **k: None)
    runmod._run_live_review_after_execution(tmp_path, "claude", "TASK-001")

    from devcouncil.live.cards import load_cards

    cards = load_cards(tmp_path)
    assert len(cards) == 1
    assert cards[0].blocks_gate is False
    assert cards[0].task_id == "TASK-001"


def test_live_review_never_raises(tmp_path, monkeypatch):
    monkeypatch.setattr(runmod, "load_config", lambda root: DevCouncilConfig())
    monkeypatch.setattr("devcouncil.cli.commands.watch._resolve_transcript",
                        lambda root, client, latest=False: (_ for _ in ()).throw(RuntimeError("boom")))
    runmod._run_live_review_after_execution(tmp_path, "claude", "TASK-001")
