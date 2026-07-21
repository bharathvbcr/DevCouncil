import json
from types import SimpleNamespace

import pytest
from typer.testing import CliRunner

import devcouncil.cli.commands.watch as watch_cmd
from devcouncil.cli.main import app
from devcouncil.live.models import AgentSession, AgentTurn, CritiqueCard
from devcouncil.live.signals import ReviewSignal


runner = CliRunner()


def _card(cid="CARD-1", verdict="Concerns", status="open", task_id=None):
    return CritiqueCard(
        id=cid,
        session_id="S1",
        turn_id="T1",
        client="claude",
        verdict=verdict,
        summary="A short summary.",
        concerns=["c1"],
        alternatives=["a1"],
        evidence_requests=["run pytest"],
        message_for_agent="Please add a test.",
        task_id=task_id,
        status=status,
    )


@pytest.fixture
def no_trace(monkeypatch):
    monkeypatch.setattr(
        watch_cmd, "TraceLogger",
        lambda root: SimpleNamespace(log_event=lambda *a, **k: None),
    )


# --- sessions (human) -------------------------------------------------------------


def test_watch_sessions_human_table(tmp_path, monkeypatch):
    monkeypatch.setattr(
        watch_cmd, "discover_sessions",
        lambda root, client="claude": [
            AgentSession(id="s1", client="claude", transcript_path="/t/s1.jsonl", turns=3)
        ],
    )
    result = runner.invoke(app, ["watch", "sessions", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "s1" in result.output


# --- review -----------------------------------------------------------------------


def test_watch_review_no_transcript_selected(tmp_path, monkeypatch):
    monkeypatch.setattr(watch_cmd, "discover_sessions", lambda root, client="generic": [])
    result = runner.invoke(app, ["watch", "review", "--project-root", str(tmp_path)])
    assert result.exit_code == 2
    assert "No transcript selected" in result.output


def test_watch_review_no_transcript_json(tmp_path, monkeypatch):
    monkeypatch.setattr(watch_cmd, "discover_sessions", lambda root, client="generic": [])
    result = runner.invoke(app, ["watch", "review", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 2
    assert json.loads(result.stdout)["ok"] is False


def test_watch_review_no_assistant_turn(tmp_path, monkeypatch):
    transcript = tmp_path / "t.jsonl"
    transcript.write_text("{}\n", encoding="utf-8")
    monkeypatch.setattr(watch_cmd, "latest_assistant_turn", lambda path, client="generic": None)
    result = runner.invoke(
        app, ["watch", "review", "--transcript", str(transcript), "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 1
    assert "No assistant turn" in result.output


def test_watch_review_success_human(tmp_path, monkeypatch, no_trace):
    transcript = tmp_path / "t.jsonl"
    transcript.write_text("{}\n", encoding="utf-8")
    turn = AgentTurn(session_id="S1", turn_id="T1", role="assistant", content="did work")
    monkeypatch.setattr(watch_cmd, "latest_assistant_turn", lambda path, client="generic": turn)
    monkeypatch.setattr(watch_cmd, "active_task_id", lambda root: None)
    monkeypatch.setattr(watch_cmd, "review_turn", lambda turn, root, client="generic", task_id=None: _card())
    monkeypatch.setattr(watch_cmd, "card_path", lambda root, cid: tmp_path / f"{cid}.json")
    monkeypatch.setattr(watch_cmd, "save_card", lambda root, card: tmp_path / f"{card.id}.json")

    result = runner.invoke(
        app, ["watch", "review", "--transcript", str(transcript), "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    assert "Critique Card" in result.output
    assert "Saved critique card" in result.output


def test_watch_review_success_json(tmp_path, monkeypatch, no_trace):
    transcript = tmp_path / "t.jsonl"
    transcript.write_text("{}\n", encoding="utf-8")
    turn = AgentTurn(session_id="S1", turn_id="T1", role="assistant", content="did work")
    monkeypatch.setattr(watch_cmd, "latest_assistant_turn", lambda path, client="generic": turn)
    monkeypatch.setattr(watch_cmd, "active_task_id", lambda root: None)
    monkeypatch.setattr(watch_cmd, "review_turn", lambda turn, root, client="generic", task_id=None: _card())
    monkeypatch.setattr(watch_cmd, "card_path", lambda root, cid: tmp_path / f"{cid}.json")
    monkeypatch.setattr(watch_cmd, "save_card", lambda root, card: tmp_path / f"{card.id}.json")

    result = runner.invoke(
        app, ["watch", "review", "--transcript", str(transcript), "--json", "--no-persist", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    assert json.loads(result.stdout)["id"] == "CARD-1"


# --- cards ------------------------------------------------------------------------


def test_watch_cards_bad_limit(tmp_path):
    result = runner.invoke(app, ["watch", "cards", "--limit", "0", "--project-root", str(tmp_path)])
    assert result.exit_code == 2
    assert "--limit must be greater than 0" in result.output


def test_watch_cards_filter_error(tmp_path, monkeypatch):
    monkeypatch.setattr(watch_cmd, "load_cards", lambda root: [])
    monkeypatch.setattr(
        watch_cmd, "filter_cards",
        lambda cards, **k: ([], "bad filter", "status"),
    )
    result = runner.invoke(app, ["watch", "cards", "--status", "bogus", "--project-root", str(tmp_path)])
    assert result.exit_code == 2
    assert "bad filter" in result.output


def test_watch_cards_human_prints(tmp_path, monkeypatch):
    monkeypatch.setattr(watch_cmd, "load_cards", lambda root: [_card()])
    monkeypatch.setattr(watch_cmd, "filter_cards", lambda cards, **k: ([_card()], None, None))
    result = runner.invoke(app, ["watch", "cards", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Critique Card" in result.output


# --- status (human, with blockers + pending) --------------------------------------


def test_watch_status_human_with_blockers(tmp_path, monkeypatch):
    monkeypatch.setattr(
        watch_cmd, "live_review_summary",
        lambda root, task_id=None: {
            "active_task_id": "TASK-1",
            "scope_task_id": "TASK-1",
            "pending_signals": 1,
            "pending_signal_items": [
                {"id": "claude-abc.json", "client": "claude", "task_id": "TASK-1"}
            ],
            "blocking_cards": [{"id": "CARD-9", "task_id": "TASK-1", "summary": "blocker"}],
            "cards": {"total": 2, "open": 1, "critical_open": 1},
        },
    )
    result = runner.invoke(app, ["watch", "status", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Blocking" in result.output
    assert "claude" in result.output
    assert "dev watch signals" in result.output


# --- resolve ----------------------------------------------------------------------


def test_watch_resolve_bad_status(tmp_path):
    result = runner.invoke(app, ["watch", "resolve", "CARD-1", "--status", "bogus", "--project-root", str(tmp_path)])
    assert result.exit_code == 2
    assert "--status must be" in result.output


def test_watch_resolve_not_found(tmp_path, monkeypatch):
    monkeypatch.setattr(watch_cmd, "update_card_status", lambda root, cid, status: None)
    result = runner.invoke(app, ["watch", "resolve", "CARD-X", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "not found" in result.output


def test_watch_resolve_success(tmp_path, monkeypatch, no_trace):
    monkeypatch.setattr(watch_cmd, "update_card_status", lambda root, cid, status: _card(status=status))
    result = runner.invoke(app, ["watch", "resolve", "CARD-1", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Updated CARD-1" in result.output


def test_watch_resolve_success_json(tmp_path, monkeypatch, no_trace):
    monkeypatch.setattr(watch_cmd, "update_card_status", lambda root, cid, status: _card(status=status))
    result = runner.invoke(app, ["watch", "resolve", "CARD-1", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert json.loads(result.stdout)["ok"] is True


# --- repair -----------------------------------------------------------------------


def test_watch_repair_not_found(tmp_path, monkeypatch):
    monkeypatch.setattr(watch_cmd, "get_card", lambda root, cid: None)
    result = runner.invoke(app, ["watch", "repair", "CARD-X", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "not found" in result.output


def test_watch_repair_prints_prompt(tmp_path, monkeypatch):
    monkeypatch.setattr(watch_cmd, "get_card", lambda root, cid: _card())
    monkeypatch.setattr(watch_cmd, "build_live_repair_prompt", lambda root, card: "REPAIR PROMPT")
    result = runner.invoke(app, ["watch", "repair", "CARD-1", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "REPAIR PROMPT" in result.output


def test_watch_repair_json(tmp_path, monkeypatch):
    monkeypatch.setattr(watch_cmd, "get_card", lambda root, cid: _card())
    monkeypatch.setattr(watch_cmd, "build_live_repair_prompt", lambda root, card: "REPAIR PROMPT")
    result = runner.invoke(app, ["watch", "repair", "CARD-1", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert json.loads(result.stdout)["prompt"] == "REPAIR PROMPT"


# --- repair-all -------------------------------------------------------------------


def test_watch_repair_all_prints_prompt(tmp_path, monkeypatch):
    monkeypatch.setattr(
        watch_cmd, "live_review_summary",
        lambda root, task_id=None: {"scope_task_id": "TASK-1", "blocking_cards": [{"id": "CARD-1"}]},
    )
    monkeypatch.setattr(watch_cmd, "load_cards", lambda root: [_card()])
    monkeypatch.setattr(watch_cmd, "build_bulk_live_repair_prompt", lambda root, cards: "BULK PROMPT")
    result = runner.invoke(app, ["watch", "repair-all", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "BULK PROMPT" in result.output


def test_watch_repair_all_json(tmp_path, monkeypatch):
    monkeypatch.setattr(
        watch_cmd, "live_review_summary",
        lambda root, task_id=None: {"scope_task_id": None, "blocking_cards": []},
    )
    monkeypatch.setattr(watch_cmd, "load_cards", lambda root: [])
    monkeypatch.setattr(watch_cmd, "build_bulk_live_repair_prompt", lambda root, cards: "")
    result = runner.invoke(app, ["watch", "repair-all", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert json.loads(result.stdout)["ok"] is True


# --- signals ----------------------------------------------------------------------


def test_watch_signals_human(tmp_path, monkeypatch):
    sig = ReviewSignal(client="claude", transcript_path="/t.jsonl", review_command="dev watch review", path="/s.json")
    monkeypatch.setattr(watch_cmd, "load_signals", lambda root: [sig])
    result = runner.invoke(app, ["watch", "signals", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "dev watch review" in result.output


def test_watch_signals_json_filtered(tmp_path, monkeypatch):
    sigs = [ReviewSignal(client="claude"), ReviewSignal(client="codex")]
    monkeypatch.setattr(watch_cmd, "load_signals", lambda root: sigs)
    result = runner.invoke(app, ["watch", "signals", "--client", "codex", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert len(data["signals"]) == 1


# --- pending ----------------------------------------------------------------------


def test_watch_pending_reviews_and_processes(tmp_path, monkeypatch, no_trace):
    transcript = tmp_path / "t.jsonl"
    transcript.write_text("{}\n", encoding="utf-8")
    sig = ReviewSignal(client="claude", transcript_path=str(transcript), task_id="TASK-1")
    monkeypatch.setattr(watch_cmd, "load_signals", lambda root: [sig])
    monkeypatch.setattr(watch_cmd, "active_task_id", lambda root: None)
    turn = AgentTurn(session_id="S1", turn_id="T1", role="assistant", content="work")
    monkeypatch.setattr(watch_cmd, "latest_assistant_turn", lambda path, client="claude": turn)
    monkeypatch.setattr(watch_cmd, "review_turn", lambda turn, root, client="claude", task_id=None: _card())
    monkeypatch.setattr(watch_cmd, "card_path", lambda root, cid: tmp_path / f"{cid}.json")
    monkeypatch.setattr(watch_cmd, "save_card", lambda root, card: tmp_path / f"{card.id}.json")
    monkeypatch.setattr(watch_cmd, "mark_processed", lambda sig, root: tmp_path / "processed.json")

    result = runner.invoke(app, ["watch", "pending", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Saved critique card" in result.output


def test_watch_pending_skips_signal_without_transcript(tmp_path, monkeypatch):
    sig = ReviewSignal(client="claude", transcript_path=None)
    monkeypatch.setattr(watch_cmd, "load_signals", lambda root: [sig])
    monkeypatch.setattr(watch_cmd, "active_task_id", lambda root: None)
    result = runner.invoke(app, ["watch", "pending", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert len(data["skipped"]) == 1


# --- follow -----------------------------------------------------------------------


def test_watch_follow_no_transcript(tmp_path, monkeypatch):
    monkeypatch.setattr(watch_cmd, "discover_sessions", lambda root, client="generic": [])
    result = runner.invoke(app, ["watch", "follow", "--project-root", str(tmp_path)])
    assert result.exit_code == 2
    assert "No transcript selected" in result.output


def test_watch_follow_once_reviews_new_turn(tmp_path, monkeypatch, no_trace):
    transcript = tmp_path / "t.jsonl"
    transcript.write_text("{}\n", encoding="utf-8")
    turn = AgentTurn(session_id="S1", turn_id="T1", role="assistant", content="work")
    monkeypatch.setattr(watch_cmd, "latest_assistant_turn", lambda path, client="generic": turn)
    monkeypatch.setattr(watch_cmd, "active_task_id", lambda root: None)
    monkeypatch.setattr(watch_cmd, "review_turn", lambda turn, root, client="generic", task_id=None: _card())
    monkeypatch.setattr(watch_cmd, "card_path", lambda root, cid: tmp_path / f"{cid}.json")
    monkeypatch.setattr(watch_cmd, "save_card", lambda root, card: tmp_path / f"{card.id}.json")

    result = runner.invoke(
        app, ["watch", "follow", "--transcript", str(transcript), "--once", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    assert "Saved critique card" in result.output


def test_watch_follow_once_no_new_turn(tmp_path, monkeypatch):
    transcript = tmp_path / "t.jsonl"
    transcript.write_text("{}\n", encoding="utf-8")
    monkeypatch.setattr(watch_cmd, "latest_assistant_turn", lambda path, client="generic": None)
    result = runner.invoke(
        app, ["watch", "follow", "--transcript", str(transcript), "--once", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 1
    assert "No new assistant turn" in result.output


# --- import -----------------------------------------------------------------------


def test_watch_import_human(tmp_path, monkeypatch):
    transcript = tmp_path / "t.jsonl"
    transcript.write_text("{}\n", encoding="utf-8")
    turn = AgentTurn(session_id="S1", turn_id="T1", role="assistant", content="work")
    monkeypatch.setattr(watch_cmd, "load_turns", lambda path, client="generic": [turn])
    result = runner.invoke(app, ["watch", "import", str(transcript)])
    assert result.exit_code == 0
    assert "Loaded 1 turns" in result.output


def test_watch_import_json(tmp_path, monkeypatch):
    transcript = tmp_path / "t.jsonl"
    transcript.write_text("{}\n", encoding="utf-8")
    turn = AgentTurn(session_id="S1", turn_id="T1", role="assistant", content="work")
    monkeypatch.setattr(watch_cmd, "load_turns", lambda path, client="generic": [turn])
    result = runner.invoke(app, ["watch", "import", str(transcript), "--json"])
    if result.exit_code != 0:
        print("EXIT CODE:", result.exit_code)
        print("STDOUT:", result.stdout)
        print("STDERR:", result.stderr)
        print("EXCEPTION:", result.exception)
    assert result.exit_code == 0
    assert len(json.loads(result.stdout)["turns"]) == 1


# --- helpers ----------------------------------------------------------------------


def test_review_turn_deterministic(tmp_path, monkeypatch):
    turn = AgentTurn(session_id="S1", turn_id="T1", role="assistant", content="x")
    monkeypatch.setattr(watch_cmd, "review_turn", lambda turn, root, client="generic", task_id=None: _card())
    import asyncio
    card = asyncio.run(watch_cmd._review_turn(turn, tmp_path, "generic", False))
    assert card.id == "CARD-1"


def test_review_turn_llm_build_failure_falls_back(tmp_path, monkeypatch):
    turn = AgentTurn(session_id="S1", turn_id="T1", role="assistant", content="x")

    def boom(root):
        raise RuntimeError("no config")

    monkeypatch.setattr(watch_cmd, "load_config", boom)
    monkeypatch.setattr(watch_cmd, "review_turn", lambda turn, root, client="generic", task_id=None: _card())
    import asyncio
    card = asyncio.run(watch_cmd._review_turn(turn, tmp_path, "generic", True))
    assert card.id == "CARD-1"


def test_review_turn_llm_success(tmp_path, monkeypatch):
    turn = AgentTurn(session_id="S1", turn_id="T1", role="assistant", content="x")
    cfg = SimpleNamespace(
        models=SimpleNamespace(provider="openrouter", roles={}),
        provider=SimpleNamespace(),
    )
    monkeypatch.setattr(watch_cmd, "load_config", lambda root: cfg)
    monkeypatch.setattr(watch_cmd, "validate_model_provider", lambda provider: None)
    monkeypatch.setattr(watch_cmd, "get_api_key", lambda provider, root: "key")
    monkeypatch.setattr(watch_cmd, "create_provider", lambda *a, **k: object())
    monkeypatch.setattr(watch_cmd, "ModelRouter", lambda *a, **k: object())

    class _Service:
        def __init__(self, router):
            pass

        async def review(self, turn, root, client="generic", use_llm=True, task_id=None):
            return _card()

    monkeypatch.setattr(watch_cmd, "LiveReviewService", _Service)
    import asyncio
    card = asyncio.run(watch_cmd._review_turn(turn, tmp_path, "claude", True, task_id="TASK-1"))
    assert card.task_id == "TASK-1"


def test_save_card_once_no_persist(tmp_path):
    card = _card()
    path, dup = watch_cmd._save_card_once(tmp_path, card, persist=False, force=False)
    assert path is None
    assert dup is False


def test_save_card_once_duplicate(tmp_path, monkeypatch):
    card = _card()
    existing = tmp_path / "CARD-1.json"
    existing.write_text("{}", encoding="utf-8")
    monkeypatch.setattr(watch_cmd, "card_path", lambda root, cid: existing)
    path, dup = watch_cmd._save_card_once(tmp_path, card, persist=True, force=False)
    assert dup is True


def test_resolve_signal_transcript_relative(tmp_path):
    sig = ReviewSignal(client="claude", transcript_path="sub/t.jsonl")
    resolved = watch_cmd._resolve_signal_transcript(tmp_path, sig)
    assert resolved == (tmp_path / "sub" / "t.jsonl").resolve()


def test_resolve_signal_transcript_none():
    sig = ReviewSignal(client="claude", transcript_path=None)
    assert watch_cmd._resolve_signal_transcript(__import__("pathlib").Path("."), sig) is None


# --- _resolve_transcript via discovered sessions ----------------------------------


def test_review_no_turn_json(tmp_path, monkeypatch):
    transcript = tmp_path / "t.jsonl"
    transcript.write_text("{}\n", encoding="utf-8")
    monkeypatch.setattr(watch_cmd, "latest_assistant_turn", lambda path, client="generic": None)
    result = runner.invoke(
        app, ["watch", "review", "--transcript", str(transcript), "--json", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 1
    assert json.loads(result.stdout)["ok"] is False


def test_review_latest_uses_discovered_session(tmp_path, monkeypatch, no_trace):
    transcript = tmp_path / "sess.jsonl"
    transcript.write_text("{}\n", encoding="utf-8")
    monkeypatch.setattr(
        watch_cmd, "discover_sessions",
        lambda root, client="generic": [
            AgentSession(id="s1", client="generic", transcript_path=str(transcript), turns=1)
        ],
    )
    turn = AgentTurn(session_id="s1", turn_id="T1", role="assistant", content="x")
    monkeypatch.setattr(watch_cmd, "latest_assistant_turn", lambda path, client="generic": turn)
    monkeypatch.setattr(watch_cmd, "active_task_id", lambda root: None)
    monkeypatch.setattr(watch_cmd, "review_turn", lambda turn, root, client="generic", task_id=None: _card())
    monkeypatch.setattr(watch_cmd, "card_path", lambda root, cid: tmp_path / f"{cid}.json")
    monkeypatch.setattr(watch_cmd, "save_card", lambda root, card: tmp_path / f"{card.id}.json")

    result = runner.invoke(app, ["watch", "review", "--latest", "--project-root", str(tmp_path)])
    assert result.exit_code == 0


def test_review_session_by_id(tmp_path, monkeypatch, no_trace):
    transcript = tmp_path / "sess.jsonl"
    transcript.write_text("{}\n", encoding="utf-8")
    monkeypatch.setattr(
        watch_cmd, "discover_sessions",
        lambda root, client="generic": [
            AgentSession(id="target", client="generic", transcript_path=str(transcript), turns=1)
        ],
    )
    turn = AgentTurn(session_id="target", turn_id="T1", role="assistant", content="x")
    monkeypatch.setattr(watch_cmd, "latest_assistant_turn", lambda path, client="generic": turn)
    monkeypatch.setattr(watch_cmd, "active_task_id", lambda root: None)
    monkeypatch.setattr(watch_cmd, "review_turn", lambda turn, root, client="generic", task_id=None: _card())
    monkeypatch.setattr(watch_cmd, "card_path", lambda root, cid: tmp_path / f"{cid}.json")
    monkeypatch.setattr(watch_cmd, "save_card", lambda root, card: tmp_path / f"{card.id}.json")

    result = runner.invoke(app, ["watch", "review", "--session", "target", "--project-root", str(tmp_path)])
    assert result.exit_code == 0


def test_resolve_transcript_claude_task_pinned(tmp_path, monkeypatch):
    import devcouncil.live.transcripts as transcripts
    pinned = tmp_path / "pinned.jsonl"
    monkeypatch.setattr(transcripts, "claude_transcript_for_task", lambda root, task_id: pinned)
    result = watch_cmd._resolve_transcript(tmp_path, "claude", task_id="TASK-1")
    assert result == pinned


# --- pending: skipped turn + json summary -----------------------------------------


def test_watch_pending_skips_when_no_turn(tmp_path, monkeypatch, no_trace):
    transcript = tmp_path / "t.jsonl"
    transcript.write_text("{}\n", encoding="utf-8")
    sig = ReviewSignal(client="claude", transcript_path=str(transcript))
    monkeypatch.setattr(watch_cmd, "load_signals", lambda root: [sig])
    monkeypatch.setattr(watch_cmd, "active_task_id", lambda root: None)
    monkeypatch.setattr(watch_cmd, "latest_assistant_turn", lambda path, client="claude": None)
    result = runner.invoke(app, ["watch", "pending", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert len(json.loads(result.stdout)["skipped"]) == 1


def test_watch_pending_keep_and_json(tmp_path, monkeypatch, no_trace):
    transcript = tmp_path / "t.jsonl"
    transcript.write_text("{}\n", encoding="utf-8")
    sig = ReviewSignal(client="claude", transcript_path=str(transcript))
    monkeypatch.setattr(watch_cmd, "load_signals", lambda root: [sig])
    monkeypatch.setattr(watch_cmd, "active_task_id", lambda root: None)
    turn = AgentTurn(session_id="S1", turn_id="T1", role="assistant", content="work")
    monkeypatch.setattr(watch_cmd, "latest_assistant_turn", lambda path, client="claude": turn)
    monkeypatch.setattr(watch_cmd, "review_turn", lambda turn, root, client="claude", task_id=None: _card())
    monkeypatch.setattr(watch_cmd, "card_path", lambda root, cid: tmp_path / f"{cid}.json")
    monkeypatch.setattr(watch_cmd, "save_card", lambda root, card: tmp_path / f"{card.id}.json")
    result = runner.invoke(app, ["watch", "pending", "--keep", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert len(json.loads(result.stdout)["reviewed"]) == 1


# --- follow: duplicate card path --------------------------------------------------


def test_watch_follow_once_duplicate(tmp_path, monkeypatch, no_trace):
    transcript = tmp_path / "t.jsonl"
    transcript.write_text("{}\n", encoding="utf-8")
    turn = AgentTurn(session_id="S1", turn_id="T1", role="assistant", content="work")
    monkeypatch.setattr(watch_cmd, "latest_assistant_turn", lambda path, client="generic": turn)
    monkeypatch.setattr(watch_cmd, "active_task_id", lambda root: None)
    monkeypatch.setattr(watch_cmd, "review_turn", lambda turn, root, client="generic", task_id=None: _card())
    existing = tmp_path / "CARD-1.json"
    existing.write_text("{}", encoding="utf-8")
    monkeypatch.setattr(watch_cmd, "card_path", lambda root, cid: existing)

    result = runner.invoke(
        app, ["watch", "follow", "--transcript", str(transcript), "--once", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    assert "Already reviewed" in result.output


def test_watch_sessions_empty_state_json(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    import devcouncil.cli.commands.watch as watch_cmd

    monkeypatch.setattr(watch_cmd, "discover_sessions", lambda root, client="claude": [])

    result = runner.invoke(app, ["watch", "sessions", "--json"])

    assert result.exit_code == 0
    payload = json.loads(result.stdout)
    assert payload["sessions"] == []


def test_watch_cards_empty_state_json(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    import devcouncil.cli.commands.watch as watch_cmd

    monkeypatch.setattr(watch_cmd, "load_cards", lambda root: [])

    result = runner.invoke(app, ["watch", "cards", "--json"])

    assert result.exit_code == 0
    payload = json.loads(result.stdout)
    assert payload["cards"] == []
    assert payload["total"] == 0


def test_watch_status_empty_review_state_json(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    import devcouncil.cli.commands.watch as watch_cmd

    monkeypatch.setattr(
        watch_cmd,
        "live_review_summary",
        lambda root, task_id=None: {
            "active_task_id": None,
            "scope_task_id": None,
            "pending_signals": 0,
            "pending_signal_items": [],
            "blocking_cards": [],
            "cards": {"total": 0, "open": 0, "critical_open": 0},
        },
    )

    result = runner.invoke(app, ["watch", "status", "--json"])

    assert result.exit_code == 0
    payload = json.loads(result.stdout)
    assert payload["blocking_cards"] == []
    assert payload["cards"]["total"] == 0
