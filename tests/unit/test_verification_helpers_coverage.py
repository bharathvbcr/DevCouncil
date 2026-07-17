import asyncio
import json
import subprocess
from pathlib import Path
from types import SimpleNamespace

from devcouncil.app.config import DevCouncilConfig
from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.live import summary as summary_module
from devcouncil.live.models import AgentTurn, CritiqueCard
from devcouncil.live.repair_prompt import build_bulk_live_repair_prompt, build_live_repair_prompt
from devcouncil.live.reviewer import LiveReviewService
from devcouncil.live.transcripts import (
    _content,
    _read_jsonl,
    _role,
    discover_sessions,
    latest_assistant_turn,
    load_turns,
)
from devcouncil.verification.acceptance_compiler import (
    AcceptanceTestCompiler,
    CompiledCheck,
    CompiledChecks,
)
from devcouncil.verification.implementation_reviewer import ImplementationReviewer, ReviewOutput
from devcouncil.verification.sandbox import DockerSandbox, NixSandbox, _environment_metadata
from devcouncil.verification.test_resolver import TestResolver


def _task(**updates):
    data = {"id": "T", "title": "Task", "description": "Desc", "requirement_ids": ["REQ"]}
    data.update(updates)
    return Task(**data)


def _req(req_id="REQ", ac_id="AC"):
    return Requirement(
        id=req_id,
        title="Req",
        description="Desc",
        priority="high",
        source="user",
        acceptance_criteria=[
            AcceptanceCriterion(id=ac_id, description="does thing", verification_method="unit_test")
        ],
    )


def _card(**updates):
    data = {
        "id": "CARD-1",
        "session_id": "S",
        "turn_id": "A",
        "client": "generic",
        "verdict": "Concerns",
        "summary": "summary",
    }
    data.update(updates)
    return CritiqueCard(**data)


def test_docker_sandbox_setup_and_command_failures_are_saved_as_failed(tmp_path, monkeypatch):
    config = DevCouncilConfig()
    config.verification.sandbox.docker_image = "image:test"
    config.verification.sandbox.docker_setup_commands = ["setup"]
    monkeypatch.setattr("devcouncil.verification.sandbox.shutil_which", lambda name: "/usr/bin/docker")

    monkeypatch.setattr(subprocess, "run", lambda *a, **k: SimpleNamespace(returncode=9))
    setup_failed = DockerSandbox(tmp_path, config).run(_task(), ["pytest"], [])
    assert setup_failed.status == "failed"
    assert setup_failed.commands == [{"command": "setup", "exit_code": 9}]

    calls = []

    def fake_run(*args, **kwargs):
        calls.append(args[0])
        return SimpleNamespace(returncode=0 if len(calls) == 1 else 2)

    monkeypatch.setattr(subprocess, "run", fake_run)
    command_failed = DockerSandbox(tmp_path, config).run(_task(), ["pytest"], [])
    assert command_failed.status == "failed"
    assert command_failed.commands[-1] == {"command": "pytest", "exit_code": 2}


def test_nix_sandbox_passes_and_fails_commands(tmp_path, monkeypatch):
    config = DevCouncilConfig()
    config.verification.sandbox.nix_flake_attr = "devShells.ci"
    (tmp_path / "flake.nix").write_text("{}", encoding="utf-8")
    monkeypatch.setattr("devcouncil.verification.sandbox.shutil_which", lambda name: "/usr/bin/nix")

    monkeypatch.setattr(subprocess, "run", lambda *a, **k: SimpleNamespace(returncode=0))
    passed = NixSandbox(tmp_path, config).run(_task(), ["pytest"], [])
    assert passed.status == "passed"
    assert passed.environment == {"attr": "devShells.ci"}

    monkeypatch.setattr(subprocess, "run", lambda *a, **k: SimpleNamespace(returncode=1))
    failed = NixSandbox(tmp_path, config).run(_task(), ["pytest"], [])
    assert failed.status == "failed"


def test_environment_metadata_handles_missing_uv(tmp_path, monkeypatch):
    monkeypatch.setattr(
        subprocess,
        "check_output",
        lambda *a, **k: (_ for _ in ()).throw(FileNotFoundError("uv")),
    )
    env = _environment_metadata(tmp_path)
    assert "python" in env
    assert "platform" in env
    assert "uv" not in env


class _Router:
    def __init__(self, responses):
        self.responses = list(responses)
        self.calls = []

    async def complete_structured(self, **kwargs):
        self.calls.append(kwargs)
        response = self.responses.pop(0)
        if isinstance(response, BaseException):
            raise response
        return response


def test_acceptance_compiler_compile_no_targets_exceptions_dedup_and_prompt_variants():
    task = _task(acceptance_criterion_ids=["AC"])
    req = _req(ac_id="AC")
    assert asyncio.run(AcceptanceTestCompiler(_Router([])).compile(_task(), [req], "diff")) == {}

    router = _Router([
        RuntimeError("model down"),
        CompiledChecks(checks=[
            CompiledCheck(acceptance_criterion_id="AC", command=" python -c 'assert True' "),
            CompiledCheck(acceptance_criterion_id="AC", command="python -c 'assert True'"),
            CompiledCheck(acceptance_criterion_id="OTHER", command="python -c 'assert False'"),
            CompiledCheck(acceptance_criterion_id="AC", command=""),
        ]),
    ])

    out = asyncio.run(AcceptanceTestCompiler(router).compile_candidates(task, [req], "diff", samples=2))

    assert out == {"AC": ["python -c 'assert True'"]}
    assert router.calls[0]["temperature"] == 0.0
    assert router.calls[1]["temperature"] > 0.0
    assert "Independent attempt #1" in router.calls[1]["messages"][0]["content"]


def test_acceptance_compiler_repair_success_empty_same_and_exception():
    req = _req(ac_id="AC")
    compiler = AcceptanceTestCompiler(_Router([
        CompiledChecks(checks=[CompiledCheck(acceptance_criterion_id="AC", command="fixed")]),
        CompiledChecks(checks=[CompiledCheck(acceptance_criterion_id="AC", command="broken")]),
        CompiledChecks(checks=[CompiledCheck(acceptance_criterion_id="OTHER", command="fixed")]),
        RuntimeError("boom"),
    ]))

    assert asyncio.run(compiler.repair("AC", req.acceptance_criteria[0].description, "broken", "err", "diff")) == "fixed"
    assert asyncio.run(compiler.repair("AC", "desc", "broken", "err", "diff")) is None
    assert asyncio.run(compiler.repair("AC", "desc", "broken", "err", "diff")) is None
    assert asyncio.run(compiler.repair("AC", "desc", "broken", "err", "diff")) is None


def test_test_resolver_maps_nested_flat_cli_policy_auth_and_fallback(tmp_path):
    (tmp_path / "tests" / "devcouncil").mkdir(parents=True)
    (tmp_path / "tests" / "devcouncil" / "test_widget.py").write_text("", encoding="utf-8")
    (tmp_path / "tests" / "test_widget.py").write_text("", encoding="utf-8")
    (tmp_path / "tests" / "unit").mkdir()
    (tmp_path / "tests" / "unit" / "test_cli_commands.py").write_text("", encoding="utf-8")
    (tmp_path / "tests" / "unit" / "test_task_policy_engine.py").write_text("", encoding="utf-8")
    (tmp_path / "tests" / "test_auth.py").write_text("", encoding="utf-8")

    resolver = TestResolver(tmp_path)
    suggestions = resolver.suggest_for_task(
        _task(planned_files=[PlannedFile(path="src/devcouncil/widget.py", reason="r", allowed_change="modify")]),
        [
            "src/devcouncil/widget.py",
            "src/devcouncil/cli/commands/run.py",
            "src/devcouncil/policy_engine.py",
            "src/auth.py",
            "docs/readme.md",
        ],
    )
    commands = [item.command for item in suggestions]

    assert "pytest tests/devcouncil/test_widget.py" in commands
    assert "pytest tests/test_widget.py" in commands
    assert "pytest tests/unit/test_cli_commands.py" in commands
    assert "pytest tests/unit/test_task_policy_engine.py" in commands
    assert "pytest tests/test_auth.py" in commands
    assert "pytest tests/unit" in commands
    assert resolver._confidence_for("src/x.py", "pytest tests/test_x.py")[0] == "high"
    assert resolver._confidence_for("src/x.py", "pytest tests/unit")[0] == "low"


def test_implementation_reviewer_uses_linked_requirements_and_returns_model_output():
    finding = Gap(
        id="G",
        severity="medium",
        gap_type="architecture_drift",
        task_id="T",
        description="d",
        recommended_fix="fix",
        blocking=False,
    )
    router = _Router([ReviewOutput(is_satisfactory=False, findings=[finding])])
    task = _task(requirement_ids=["REQ-2"])
    reqs = [_req("REQ-1", "AC-1"), _req("REQ-2", "AC-2")]

    result = asyncio.run(ImplementationReviewer(router).review_changes(task, reqs, "diff with sk-abcdefghijklmnopqrstuvwxyz"))
    prompt = router.calls[0]["messages"][0]["content"]

    assert result.findings == [finding]
    assert "REQ-2" in prompt
    assert "REQ-1" not in prompt
    assert "sk-abcdefghijklmnopqrstuvwxyz" not in prompt


def test_transcript_discovery_parsing_content_roles_and_latest_cache(tmp_path, monkeypatch):
    live_dir = tmp_path / ".devcouncil" / "live" / "generic"
    live_dir.mkdir(parents=True)
    transcript = live_dir / "session.jsonl"
    transcript.write_text(
        "\n".join([
            "",
            "{bad json",
            json.dumps({"role": "weird", "content": [{"text": "hello"}, {"content": "world"}, 7]}),
            json.dumps({"message": {"role": "assistant", "content": [{"type": "text", "text": "done"}]}, "uuid": "u1"}),
        ]),
        encoding="utf-8",
    )

    sessions = discover_sessions(tmp_path, client="generic")
    assert len(sessions) == 1
    assert sessions[0].turns == 4
    assert list(_read_jsonl(transcript))[0]["role"] == "weird"
    turns = load_turns(transcript, client="generic")
    assert turns[0].role == "unknown"
    assert turns[0].content == "hello\nworld"
    assert turns[1].role == "assistant"
    assert latest_assistant_turn(transcript).turn_id == "u1"
    assert latest_assistant_turn(transcript).turn_id == "u1"
    assert _role({"type": "tool"}) == "tool"
    assert _content({"text": "direct"}) == "direct"

    claude_file = tmp_path / "claude.jsonl"
    claude_file.write_text(json.dumps({"role": "assistant", "content": "x"}), encoding="utf-8")
    monkeypatch.setattr("devcouncil.live.transcripts._claude_transcript_candidates", lambda root: [claude_file])
    assert discover_sessions(tmp_path, client="claude")[0].client == "claude"


def test_live_repair_prompts_without_db_and_empty_bulk(tmp_path):
    card = _card(
        concerns=["risk"],
        alternatives=["safer"],
        evidence_requests=["pytest"],
        message_for_agent="fix it",
        task_id="TASK-404",
    )
    prompt = build_live_repair_prompt(tmp_path, card)

    assert "## Concerns" in prompt
    assert "## Original DevCouncil Task Contract" not in prompt
    assert f"dev watch resolve {card.id}" in prompt
    assert build_bulk_live_repair_prompt(tmp_path, []) == "# Live Review Repair\n\nNo blocking live-review cards found for this scope.\n"


def test_live_review_service_deterministic_llm_fallback_role_empty_and_vote(tmp_path, monkeypatch):
    turn = AgentTurn(session_id="S", turn_id="A", source="generic", role="assistant", content="not done yet")
    deterministic = asyncio.run(LiveReviewService().review(turn, tmp_path, use_llm=False))
    assert deterministic.verdict == "Approved"

    approved = _card(id="LLM-1", verdict="Approved")
    router = _Router([ValueError("role unsupported"), approved])
    reviewed = asyncio.run(LiveReviewService(router).review(turn, tmp_path, client="cursor", use_llm=True))
    assert reviewed.id == deterministic.id
    assert reviewed.client == "cursor"
    assert router.calls[1]["role"] == "implementation_reviewer"

    empty = asyncio.run(LiveReviewService(_Router([RuntimeError("down")])).review(turn, tmp_path, use_llm=True))
    assert empty.id == deterministic.id

    monkeypatch.setattr("devcouncil.live.reviewer.load_config", lambda root: (_ for _ in ()).throw(RuntimeError("bad")), raising=False)
    assert LiveReviewService()._samples(tmp_path) == 1
    assert LiveReviewService._vote([approved]) is approved
    assert LiveReviewService._vote([
        _card(id="A", verdict="Approved"),
        _card(id="B", verdict="Approved"),
        _card(id="C", verdict="Critical Issues"),
    ]).verdict == "Approved"
    assert LiveReviewService._vote([
        _card(id="A", verdict="Approved"),
        _card(id="B", verdict="Critical Issues"),
    ]).verdict == "Concerns"


def test_live_review_summary_counts_statuses_and_scopes(monkeypatch, tmp_path):
    cards = [
        _card(id="open-critical", verdict="Critical Issues", status="open"),
        _card(id="open-concern", verdict="Concerns", status="open"),
        _card(id="resolved", status="resolved"),
        _card(id="ignored", status="ignored"),
    ]
    signals = [SimpleNamespace(model_dump=lambda: {"signal": "one"})]
    blockers = [cards[0]]

    monkeypatch.setattr(summary_module, "load_cards", lambda root: cards)
    monkeypatch.setattr(summary_module, "load_signals", lambda root: signals)
    monkeypatch.setattr(summary_module, "active_task_id", lambda root: "TASK-ACTIVE")
    monkeypatch.setattr(summary_module, "unresolved_blocking_cards", lambda root, task_id=None, cards=None: blockers)

    payload = summary_module.live_review_summary(tmp_path)

    assert payload["active_task_id"] == "TASK-ACTIVE"
    assert payload["scope_task_id"] == "TASK-ACTIVE"
    assert payload["pending_signal_items"] == [{"signal": "one"}]
    assert payload["cards"]["open"] == 2
    assert payload["cards"]["resolved"] == 1
    assert payload["cards"]["ignored"] == 1
    assert payload["cards"]["critical_open"] == 1
    assert payload["blocking_cards"][0]["id"] == "open-critical"
