"""Local-LLM performance + verdict-calibration fixes.

Covers the benchmark-surfaced failure modes:
  * under-credited ``incomplete`` verdicts when a weak local monitor compiles
    unrunnable checks (nothing decisive ran -> coarse fallback must apply);
  * false ``blocked`` verdicts from agents adding unplanned NEW test files
    (advisory, not scope drift) and from planner-emitted command-only tasks
    with no planned files;
  * local-aware auto-defaults for acceptance/reviewer check sampling;
  * Ollama provider: raised default num_ctx, keep_alive, and schema-constrained
    structured output (with graceful fallback on older servers).
"""

import asyncio
import json

import pytest

from devcouncil.app.config import (
    AcceptanceCheckConfig,
    DevCouncilConfig,
    ReviewerCheckConfig,
    role_runs_on_local_provider,
)
from devcouncil.domain.evidence import CommandResult, TestEvidence
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.gating.checks.planned_files_check import PlannedFilesCheck
from devcouncil.llm.provider import OllamaProvider
from devcouncil.verification.verifier import Verifier


@pytest.fixture
def anyio_backend():
    return "asyncio"


def _requirement() -> Requirement:
    return Requirement(
        id="REQ-001",
        title="Median",
        description="median() computes the statistical median",
        priority="high",
        source="user",
        acceptance_criteria=[
            AcceptanceCriterion(
                id="AC-001",
                description="median([]) raises ValueError",
                verification_method="unit_test",
            )
        ],
    )


def _task() -> Task:
    return Task(
        id="TASK-001",
        title="Implement median",
        description="Implement median()",
        requirement_ids=["REQ-001"],
        acceptance_criterion_ids=["AC-001"],
        planned_files=[
            PlannedFile(path="stats.py", reason="median logic", allowed_change="modify"),
        ],
        expected_tests=["pytest tests/test_stats.py"],
    )


# --- verifier: unrunnable-only compiled checks fall back to coarse proof -----


def test_unrunnable_only_compiled_checks_fall_back_to_coarse(tmp_path):
    # A compiled check that never RAN proves nothing either way. When every
    # candidate is unrunnable (and repair cannot fix it), the criterion must fall
    # back to the coarse signal (a passing acceptance-capable command) instead of
    # being recorded "not proven" — the top source of under-credited `incomplete`
    # verdicts on weak local monitor models.
    class FakeCompiler:
        async def compile_candidates(self, task, requirements, code_context, samples=1, **kwargs):
            return {"AC-001": ['python -c "import missing_module"']}

        async def repair(self, ac_id, ac_description, failing_command, error_summary, code_context):
            return None  # cannot fix the command

    task = _task()
    verifier = Verifier(tmp_path)
    verifier.acceptance_compiler = FakeCompiler()
    verifier.get_changed_files = lambda: ["stats.py"]
    verifier.get_diff = lambda: "diff --git a/stats.py b/stats.py\n+def median(...)"

    def fake_run(command, task_id="verify"):
        if "missing_module" in command:  # the unrunnable compiled check
            return CommandResult(
                command=command, exit_code=1, stdout_path="", stderr_path="",
                summary="Exit code 1. stderr: No module named missing_module",
            )
        # The task's expected test genuinely passes (coarse signal).
        return CommandResult(command=command, exit_code=0, stdout_path="", stderr_path="", summary="ok")

    verifier._run_command = fake_run

    gaps, evidence = asyncio.run(verifier.verify_task(task, [_requirement()]))

    proven = [
        ev for ev in evidence
        if isinstance(ev, TestEvidence)
        and ev.acceptance_criterion_id == "AC-001"
        and ev.status == "passed"
    ]
    assert proven and proven[0].mode == "coarse"
    assert not any(g.gap_type == "acceptance_criteria_unproven" for g in gaps)
    # The weak proof is still surfaced honestly, as the advisory coarse-proof gap.
    assert any(g.gap_type == "coarse_acceptance_proof" and not g.blocking for g in gaps)


def test_unrunnable_checks_without_coarse_signal_stay_unproven_nonblocking(tmp_path):
    # No coarse signal available -> the criterion must remain unproven (honest
    # `incomplete`), surfaced NON-blocking ("could not verify"), never `blocked`.
    class FakeCompiler:
        async def compile_candidates(self, task, requirements, code_context, samples=1, **kwargs):
            return {"AC-001": ['python -c "import missing_module"']}

        async def repair(self, *args, **kwargs):
            return None

    task = _task()
    task.expected_tests = []  # nothing coarse to fall back to
    verifier = Verifier(tmp_path)
    verifier.acceptance_compiler = FakeCompiler()
    verifier.get_changed_files = lambda: ["stats.py"]
    verifier.get_diff = lambda: "diff --git a/stats.py b/stats.py\n+def median(...)"
    verifier._commands_for_task = lambda task: {}
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=1, stdout_path="", stderr_path="",
        summary="Exit code 1. stderr: No module named missing_module",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    unproven = [g for g in gaps if g.gap_type == "acceptance_criteria_unproven"]
    assert unproven and all(not g.blocking for g in unproven)


def test_keyword_less_behavioral_command_still_coarse_proves(tmp_path):
    # `make check` has no test keyword, but as a declared expected_test it is trusted
    # evidence (deny-list, not allowlist): a passing run must coarse-prove the AC
    # instead of producing a false `incomplete`.
    task = _task()
    task.expected_tests = ["make check"]
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["stats.py"]
    verifier.get_diff = lambda: "diff --git a/stats.py b/stats.py\n+def median(...)"
    verifier._command_applicable = lambda cmd: (True, "")
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="ok",
    )

    gaps, evidence = asyncio.run(verifier.verify_task(task, [_requirement()]))

    proven = [
        ev for ev in evidence
        if isinstance(ev, TestEvidence) and ev.acceptance_criterion_id == "AC-001" and ev.status == "passed"
    ]
    assert proven and proven[0].mode == "coarse"
    assert not any(g.gap_type == "acceptance_criteria_unproven" for g in gaps)


def test_trivial_expected_test_does_not_coarse_prove(tmp_path):
    # `python --version` exits 0 but proves nothing; the AC must stay unproven
    # (honest incomplete) rather than being coarse-proven by trivia.
    task = _task()
    task.expected_tests = ["python --version"]
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["stats.py"]
    verifier.get_diff = lambda: "diff --git a/stats.py b/stats.py\n+def median(...)"
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="ok",
    )

    gaps, evidence = asyncio.run(verifier.verify_task(task, [_requirement()]))

    assert not [
        ev for ev in evidence
        if isinstance(ev, TestEvidence) and ev.acceptance_criterion_id == "AC-001" and ev.status == "passed"
    ]
    assert any(g.gap_type == "acceptance_criteria_unproven" for g in gaps)


# --- verifier: new unplanned test files are advisory orphans -----------------


def test_new_unplanned_test_file_is_advisory_orphan(tmp_path):
    task = _task()
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["stats.py", "tests/test_stats_extra.py"]
    verifier.get_diff = lambda: "diff --git a/stats.py b/stats.py\n+def median(...)"
    verifier._classify_change_paths = lambda changed: (["tests/test_stats_extra.py"], [])
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="ok",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    orphans = {g.file: g for g in gaps if g.gap_type == "orphan_diff"}
    assert set(orphans) == {"tests/test_stats_extra.py"}
    assert not orphans["tests/test_stats_extra.py"].blocking


def test_unplanned_source_file_still_blocks_as_orphan(tmp_path):
    task = _task()
    verifier = Verifier(tmp_path)
    verifier.get_changed_files = lambda: ["stats.py", "src/rogue.py"]
    verifier.get_diff = lambda: "diff --git a/stats.py b/stats.py\n+def median(...)"
    # rogue.py is a MODIFIED existing source file, not an added test.
    verifier._classify_change_paths = lambda changed: ([], [])
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="ok",
    )

    gaps, _ = asyncio.run(verifier.verify_task(task, [_requirement()]))

    orphans = {g.file: g for g in gaps if g.gap_type == "orphan_diff"}
    assert orphans["src/rogue.py"].blocking


def test_is_test_path_conventions():
    assert Verifier._is_test_path("tests/test_stats.py")
    assert Verifier._is_test_path("test_config.py")
    assert Verifier._is_test_path("pkg/module_test.go")
    assert Verifier._is_test_path("src/__tests__/widget.test.tsx")
    assert Verifier._is_test_path("conftest.py")
    assert not Verifier._is_test_path("stats.py")
    assert not Verifier._is_test_path("src/testing_utils.py")
    assert not Verifier._is_test_path("contest.py")


# --- readiness gate: command-only tasks proceed with an advisory -------------


def test_command_only_task_without_planned_files_is_advisory():
    task = Task(
        id="TASK-LINT",
        title="Run linter checks",
        description="Run the linter over the repo",
        requirement_ids=["REQ-001"],
        acceptance_criterion_ids=[],
        planned_files=[],
        allowed_commands=["ruff check ."],
    )
    gaps = PlannedFilesCheck().check(task)
    assert gaps and all(not g.blocking for g in gaps)


def test_task_with_nothing_actionable_still_blocks():
    task = Task(
        id="TASK-EMPTY",
        title="Do something",
        description="No files, no commands, no tests",
        requirement_ids=["REQ-001"],
        acceptance_criterion_ids=[],
        planned_files=[],
    )
    gaps = PlannedFilesCheck().check(task)
    assert any(g.blocking for g in gaps)


def test_read_only_task_is_advisory_at_readiness():
    task = Task(
        id="TASK-ANALYZE",
        title="Analyze module",
        description="Read-only analysis",
        requirement_ids=["REQ-001"],
        acceptance_criterion_ids=[],
        planned_files=[PlannedFile(path="stats.py", reason="analyze", allowed_change="read_only")],
    )
    gaps = PlannedFilesCheck().check(task)
    assert gaps and all(not g.blocking for g in gaps)


# --- local-aware auto defaults ------------------------------------------------


def test_acceptance_check_auto_defaults_resolve_by_monitor_locality():
    auto = AcceptanceCheckConfig()
    assert auto.resolved(local_monitor=False) == (1, 1, False)
    assert auto.resolved(local_monitor=True) == (3, 2, True)


def test_acceptance_check_explicit_values_beat_auto():
    explicit = AcceptanceCheckConfig(samples=5, repair_attempts=0, per_criterion=False)
    assert explicit.resolved(local_monitor=True) == (5, 0, False)
    assert explicit.resolved(local_monitor=False) == (5, 0, False)


def test_reviewer_check_auto_defaults_resolve_by_reviewer_locality():
    auto = ReviewerCheckConfig()
    assert auto.resolved(local_reviewer=False) == 1
    assert auto.resolved(local_reviewer=True) == 3
    assert ReviewerCheckConfig(samples=2).resolved(local_reviewer=True) == 2


def test_acceptance_check_unsafe_overrides_warn_only_on_local_monitor():
    # Explicit single-shot + batched compilation on a LOCAL monitor: both flagged
    # (calibration probes showed samples=1 rubber-stamping a real defect).
    unsafe = AcceptanceCheckConfig(samples=1, per_criterion=False)
    warnings = unsafe.unsafe_override_warnings(local_monitor=True)
    assert len(warnings) == 2
    assert any("samples" in w for w in warnings)
    assert any("per_criterion" in w for w in warnings)
    # Same explicit config on a cloud monitor is the intended default: no warnings.
    assert unsafe.unsafe_override_warnings(local_monitor=False) == []
    # Auto defaults (no explicit overrides) never warn — auto already picks safe values.
    assert AcceptanceCheckConfig().unsafe_override_warnings(local_monitor=True) == []
    # Explicit but safe values don't warn either.
    safe = AcceptanceCheckConfig(samples=3, per_criterion=True)
    assert safe.unsafe_override_warnings(local_monitor=True) == []


def test_reviewer_check_unsafe_overrides_warn_only_on_local_reviewer():
    assert len(ReviewerCheckConfig(samples=1).unsafe_override_warnings(local_reviewer=True)) == 1
    assert ReviewerCheckConfig(samples=1).unsafe_override_warnings(local_reviewer=False) == []
    assert ReviewerCheckConfig().unsafe_override_warnings(local_reviewer=True) == []
    assert ReviewerCheckConfig(samples=3).unsafe_override_warnings(local_reviewer=True) == []


def test_role_runs_on_local_provider_honors_role_override():
    cfg = DevCouncilConfig.model_validate({
        "models": {
            "provider": "openrouter",
            "roles": {
                "implementation_reviewer": {"model": "qwen2.5-coder:7b", "provider": "ollama"},
                "planner_a": {"model": "some/cloud-model"},
            },
        }
    })
    assert role_runs_on_local_provider(cfg, "implementation_reviewer") is True
    assert role_runs_on_local_provider(cfg, "planner_a") is False

    local_cfg = DevCouncilConfig.model_validate({"models": {"provider": "ollama"}})
    assert role_runs_on_local_provider(local_cfg, "implementation_reviewer") is True


# --- Ollama provider: keep_alive + schema-constrained structured output ------


class _FakeResponse:
    def __init__(self, status_code, data=None, text=""):
        self.status_code = status_code
        self._data = data or {}
        self.text = text

    def json(self):
        return self._data


def _native_response(content='{"ok": true}'):
    return _FakeResponse(200, {
        "message": {"role": "assistant", "content": content},
        "model": "m",
        "prompt_eval_count": 1,
        "eval_count": 1,
    })


def _make_fake_client(calls, responses):
    """responses: list consumed per call (last one repeats)."""
    import copy as _copy

    class FakeClient:
        def __init__(self, timeout):
            self.timeout = timeout
            self.is_closed = False  # provider reuses one client across calls

        async def post(self, url, headers, json):
            # Snapshot the payload: the provider legitimately mutates it between
            # retries (e.g. dropping a rejected ``think`` field), and the recorded
            # calls must reflect what was actually SENT each time.
            calls.append({"url": url, "headers": headers, "json": _copy.deepcopy(json)})
            index = min(len(calls) - 1, len(responses) - 1)
            return responses[index]

    return FakeClient


@pytest.mark.anyio
async def test_ollama_sends_keep_alive_by_default(tmp_path, monkeypatch):
    monkeypatch.delenv("OLLAMA_KEEP_ALIVE", raising=False)
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", _make_fake_client(calls, [_native_response()])
    )
    provider = OllamaProvider(project_root=tmp_path)
    await provider.complete("m", [{"role": "user", "content": "hi"}])
    assert calls[0]["json"]["keep_alive"] == OllamaProvider.DEFAULT_KEEP_ALIVE


@pytest.mark.anyio
async def test_ollama_keep_alive_env_override_and_server_default(tmp_path, monkeypatch):
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", _make_fake_client(calls, [_native_response()])
    )
    monkeypatch.setenv("OLLAMA_KEEP_ALIVE", "-1")
    await OllamaProvider(project_root=tmp_path).complete("m", [{"role": "user", "content": "hi"}])
    assert calls[-1]["json"]["keep_alive"] == "-1"

    monkeypatch.setenv("OLLAMA_KEEP_ALIVE", "default")
    await OllamaProvider(project_root=tmp_path).complete("m", [{"role": "user", "content": "hi"}])
    assert "keep_alive" not in calls[-1]["json"]


@pytest.mark.anyio
async def test_ollama_uses_schema_constrained_format_when_schema_given(tmp_path, monkeypatch):
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", _make_fake_client(calls, [_native_response()])
    )
    provider = OllamaProvider(project_root=tmp_path)
    schema = {"type": "object", "properties": {"ok": {"type": "boolean"}}, "required": ["ok"]}
    await provider.complete(
        "m", [{"role": "user", "content": "hi"}], json_mode=True, json_schema=schema
    )
    assert calls[0]["json"]["format"] == schema


@pytest.mark.anyio
async def test_ollama_falls_back_to_plain_json_when_server_rejects_schema(tmp_path, monkeypatch):
    # An older Ollama that rejects schema-constrained ``format`` (HTTP 400) must be
    # retried once with format="json" — the optimization can never fail the run.
    calls = []
    responses = [_FakeResponse(400, text="unknown format"), _native_response()]
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", _make_fake_client(calls, responses)
    )
    provider = OllamaProvider(project_root=tmp_path)
    schema = {"type": "object", "properties": {"ok": {"type": "boolean"}}}
    resp = await provider.complete(
        "m", [{"role": "user", "content": "hi"}], json_mode=True, json_schema=schema
    )
    assert len(calls) == 2
    assert calls[1]["json"]["format"] == "json"
    assert json.loads(resp.content) == {"ok": True}


@pytest.mark.anyio
async def test_router_passes_schema_to_supporting_provider(tmp_path):
    # complete_structured must hand the pydantic schema to providers that accept
    # ``json_schema`` (grammar-constrained decoding) and silently skip those that don't.
    from pydantic import BaseModel

    from devcouncil.llm.provider import LLMResponse
    from devcouncil.llm.router import ModelRouter

    class Out(BaseModel):
        value: str

    seen = {}

    class SchemaAwareProvider:
        def cache_fingerprint(self):
            return "fake"

        def is_local_cost_free(self):
            return True

        async def complete(self, model, messages, temperature=0.0, json_mode=False,
                           task_id=None, run_id=None, json_schema=None):
            seen["schema"] = json_schema
            return LLMResponse(content='{"value": "ok"}', model=model, usage={}, raw_response={})

    router = ModelRouter(SchemaAwareProvider(), {"r": {"model": "m"}}, project_root=tmp_path)
    result = await router.complete_structured("r", [{"role": "user", "content": "go"}], Out)
    assert result.value == "ok"
    assert seen["schema"] == Out.model_json_schema()


@pytest.mark.anyio
async def test_ollama_plain_json_mode_unchanged_without_schema(tmp_path, monkeypatch):
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", _make_fake_client(calls, [_native_response()])
    )
    provider = OllamaProvider(project_root=tmp_path)
    await provider.complete("m", [{"role": "user", "content": "hi"}], json_mode=True)
    assert calls[0]["json"]["format"] == "json"


# --- Ollama provider: thinking-mode control (OLLAMA_THINK) -------------------


@pytest.mark.anyio
async def test_ollama_think_env_false_sent_in_payload(tmp_path, monkeypatch):
    # Thinking dominates local latency on reasoning models (measured ~65x on one
    # acceptance-compile call); OLLAMA_THINK=false must request it off explicitly.
    monkeypatch.setenv("OLLAMA_THINK", "false")
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", _make_fake_client(calls, [_native_response()])
    )
    await OllamaProvider(project_root=tmp_path).complete("m", [{"role": "user", "content": "hi"}])
    assert calls[0]["json"]["think"] is False


@pytest.mark.anyio
async def test_ollama_think_env_true_and_unset(tmp_path, monkeypatch):
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", _make_fake_client(calls, [_native_response()])
    )
    monkeypatch.setenv("OLLAMA_THINK", "true")
    await OllamaProvider(project_root=tmp_path).complete("m", [{"role": "user", "content": "hi"}])
    assert calls[-1]["json"]["think"] is True

    monkeypatch.delenv("OLLAMA_THINK", raising=False)
    await OllamaProvider(project_root=tmp_path).complete("m", [{"role": "user", "content": "hi"}])
    assert "think" not in calls[-1]["json"]


@pytest.mark.anyio
async def test_ollama_think_rejected_by_server_degrades_and_is_remembered(tmp_path, monkeypatch):
    # An older server / non-thinking model rejects the field: retry once without it
    # and never send it again on this provider instance.
    monkeypatch.setenv("OLLAMA_THINK", "false")
    calls = []
    responses = [_FakeResponse(400, text="unknown field think"), _native_response(), _native_response()]
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", _make_fake_client(calls, responses)
    )
    provider = OllamaProvider(project_root=tmp_path)
    resp = await provider.complete("m", [{"role": "user", "content": "hi"}])
    assert json.loads(resp.content) == {"ok": True}
    assert len(calls) == 2
    assert "think" in calls[0]["json"] and "think" not in calls[1]["json"]

    await provider.complete("m", [{"role": "user", "content": "again"}])
    assert "think" not in calls[2]["json"]


def test_ollama_cache_fingerprint_includes_think_and_ctx_cap(tmp_path, monkeypatch):
    monkeypatch.setenv("OLLAMA_THINK", "false")
    monkeypatch.setenv("OLLAMA_MAX_NUM_CTX", "32768")
    fp = OllamaProvider(project_root=tmp_path).cache_fingerprint()
    assert "think=False" in fp and "max_num_ctx=32768" in fp


# --- Ollama provider: adaptive context window --------------------------------


@pytest.mark.anyio
async def test_ollama_num_ctx_grows_to_fit_large_prompt(tmp_path, monkeypatch):
    # A prompt that would overflow the configured window must raise num_ctx for the
    # request (Ollama otherwise silently TRUNCATES the prompt — the model reviews
    # half a diff and the verdict is garbage).
    monkeypatch.delenv("OLLAMA_NUM_CTX", raising=False)
    monkeypatch.delenv("OLLAMA_MAX_NUM_CTX", raising=False)
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", _make_fake_client(calls, [_native_response()])
    )
    provider = OllamaProvider(project_root=tmp_path)
    big_prompt = "x" * (OllamaProvider.DEFAULT_NUM_CTX * 4)  # ~4x the default window in tokens
    await provider.complete("m", [{"role": "user", "content": big_prompt}])
    sent = calls[0]["json"]["options"]["num_ctx"]
    assert sent > OllamaProvider.DEFAULT_NUM_CTX
    assert sent <= OllamaProvider.DEFAULT_MAX_NUM_CTX


@pytest.mark.anyio
async def test_ollama_num_ctx_adaptive_growth_is_capped(tmp_path, monkeypatch):
    monkeypatch.delenv("OLLAMA_NUM_CTX", raising=False)
    monkeypatch.setenv("OLLAMA_MAX_NUM_CTX", "20000")
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", _make_fake_client(calls, [_native_response()])
    )
    provider = OllamaProvider(project_root=tmp_path)
    huge_prompt = "x" * 1_000_000
    await provider.complete("m", [{"role": "user", "content": huge_prompt}])
    assert calls[0]["json"]["options"]["num_ctx"] == 20000


@pytest.mark.anyio
async def test_ollama_num_ctx_unchanged_for_small_prompt(tmp_path, monkeypatch):
    monkeypatch.delenv("OLLAMA_NUM_CTX", raising=False)
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", _make_fake_client(calls, [_native_response()])
    )
    await OllamaProvider(project_root=tmp_path).complete("m", [{"role": "user", "content": "hi"}])
    assert calls[0]["json"]["options"]["num_ctx"] == OllamaProvider.DEFAULT_NUM_CTX


# --- Ollama provider: thinking-channel content fallback ----------------------


@pytest.mark.anyio
async def test_ollama_empty_content_falls_back_to_thinking_channel(tmp_path, monkeypatch):
    # A thinking model that answered inside the reasoning channel must not surface
    # an empty response (which fails parsing outright); the thinking text at least
    # gives the router's extraction/healing path something to work with.
    payload = {
        "message": {"role": "assistant", "content": "", "thinking": 'reasoning... {"ok": true}'},
        "model": "m", "prompt_eval_count": 1, "eval_count": 1,
    }
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient",
        _make_fake_client(calls, [_FakeResponse(200, payload)]),
    )
    resp = await OllamaProvider(project_root=tmp_path).complete("m", [{"role": "user", "content": "hi"}])
    assert '{"ok": true}' in resp.content


# --- router: JSON extraction for thinking/local models -----------------------


def test_extract_json_strips_inline_think_block():
    from devcouncil.llm.router import ModelRouter

    content = '<think>\nmaybe {"draft": "no"} hmm\n</think>\n{"value": "x"}'
    assert ModelRouter._extract_json(content) == '{"value": "x"}'


def test_extract_json_skips_invalid_candidate_and_finds_real_answer():
    from devcouncil.llm.router import ModelRouter

    # A JSON-LOOKING fragment in leading prose (balanced but invalid) must not mask
    # the valid object that follows — the old extractor gave up after the first
    # balanced candidate.
    content = 'Schema example: {"a": } — here is the result: {"value": "x"}'
    assert ModelRouter._extract_json(content) == '{"value": "x"}'


def test_extract_json_handles_dangling_think_tag():
    from devcouncil.llm.router import ModelRouter

    content = '<think>cut off reasoning {"partial": '
    # Nothing parseable — must degrade to SOME text (healing path), not raise.
    assert isinstance(ModelRouter._extract_json(content), str)

    answered = '{"value": "x"}\n<think>trailing reasoning that got cut'
    assert ModelRouter._extract_json(answered) == '{"value": "x"}'


def test_extract_json_existing_behaviors_unchanged():
    from devcouncil.llm.router import ModelRouter

    assert ModelRouter._extract_json('{"a": 1}') == '{"a": 1}'
    assert ModelRouter._extract_json('```json\n{"a": 1}\n```') == '{"a": 1}'
    assert ModelRouter._extract_json('Here: {"v": "a}b"} thanks') == '{"v": "a}b"}'


# --- acceptance compiler: concurrent sampling -------------------------------


@pytest.mark.anyio
async def test_compiler_samples_run_concurrently(tmp_path):
    # On a slow local monitor the independent samples/criteria dominate wall-clock;
    # they must be issued concurrently (gather), not serially awaited.
    import asyncio as _asyncio

    from devcouncil.verification.acceptance_compiler import AcceptanceTestCompiler

    in_flight = {"now": 0, "max": 0}

    class FakeRouter:
        async def complete_structured(self, role, messages, schema, temperature=0.0, fallback=None, **kw):
            in_flight["now"] += 1
            in_flight["max"] = max(in_flight["max"], in_flight["now"])
            await _asyncio.sleep(0.02)
            in_flight["now"] -= 1
            return schema.model_validate({"checks": [
                {"acceptance_criterion_id": "AC-001", "command": f"python -c 'pass'  # t{temperature}"},
            ]})

    compiler = AcceptanceTestCompiler(FakeRouter())
    req = _requirement()
    out = await compiler.compile_candidates(_task(), [req], "diff", samples=3)
    assert "AC-001" in out
    assert in_flight["max"] >= 2, "samples must overlap in time (concurrent), not run serially"


# --- Ollama provider: thinking budget (OLLAMA_THINK levels, OLLAMA_NUM_PREDICT) ---


@pytest.mark.anyio
async def test_ollama_think_level_passed_through(tmp_path, monkeypatch):
    # "low"/"medium"/"high" are thinking-BUDGET levels (Ollama >= 0.12): sent
    # verbatim so budget-capable models get an explicit budget. The existing
    # 400-degrade path covers servers/models that reject the field.
    monkeypatch.setenv("OLLAMA_THINK", "high")
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", _make_fake_client(calls, [_native_response()])
    )
    await OllamaProvider(project_root=tmp_path).complete("m", [{"role": "user", "content": "hi"}])
    assert calls[0]["json"]["think"] == "high"


@pytest.mark.anyio
async def test_ollama_think_unrecognized_value_omitted(tmp_path, monkeypatch):
    monkeypatch.setenv("OLLAMA_THINK", "maximal")
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", _make_fake_client(calls, [_native_response()])
    )
    await OllamaProvider(project_root=tmp_path).complete("m", [{"role": "user", "content": "hi"}])
    assert "think" not in calls[0]["json"]


@pytest.mark.anyio
async def test_ollama_num_predict_caps_generation(tmp_path, monkeypatch):
    # A runaway thinking spiral otherwise generates until the HTTP timeout
    # (600s), which the router's layered retries stack past external kill
    # timeouts. num_predict turns that into a fast, healable truncation.
    monkeypatch.setenv("OLLAMA_NUM_PREDICT", "2048")
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", _make_fake_client(calls, [_native_response()])
    )
    await OllamaProvider(project_root=tmp_path).complete("m", [{"role": "user", "content": "hi"}])
    assert calls[0]["json"]["options"]["num_predict"] == 2048


@pytest.mark.anyio
async def test_ollama_num_predict_unset_or_invalid_omitted(tmp_path, monkeypatch):
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient",
        _make_fake_client(calls, [_native_response(), _native_response()]),
    )
    monkeypatch.delenv("OLLAMA_NUM_PREDICT", raising=False)
    await OllamaProvider(project_root=tmp_path).complete("m", [{"role": "user", "content": "hi"}])
    assert "num_predict" not in calls[0]["json"]["options"]

    monkeypatch.setenv("OLLAMA_NUM_PREDICT", "banana")
    await OllamaProvider(project_root=tmp_path).complete("m", [{"role": "user", "content": "hi"}])
    assert "num_predict" not in calls[1]["json"]["options"]


def test_ollama_cache_fingerprint_includes_think_level_and_num_predict(tmp_path, monkeypatch):
    # Both knobs change the output for an identical prompt, so both must
    # invalidate the LLM cache.
    monkeypatch.setenv("OLLAMA_THINK", "low")
    monkeypatch.setenv("OLLAMA_NUM_PREDICT", "4096")
    fp = OllamaProvider(project_root=tmp_path).cache_fingerprint()
    assert "think=low" in fp and "num_predict=4096" in fp


# --- Ollama provider: client-side concurrency cap (OLLAMA_MAX_CONCURRENCY) ----


def _make_tracking_client(native_body_factory, tracker):
    """Fake AsyncClient whose post() yields, so true concurrency is observable."""

    class _Resp:
        status_code = 200
        text = ""

        def json(self):
            return native_body_factory()

    class _C:
        def __init__(self, timeout=None):
            self.is_closed = False

        async def post(self, url, headers, json):
            tracker["in_flight"] += 1
            tracker["max_in_flight"] = max(tracker["max_in_flight"], tracker["in_flight"])
            await asyncio.sleep(0.01)  # hold the slot so overlap is measurable
            tracker["in_flight"] -= 1
            return _Resp()

    return _C


@pytest.mark.anyio
async def test_ollama_concurrency_capped_by_default(tmp_path, monkeypatch):
    # Fan-out callers (per-criterion acceptance compiles x samples) launch 20+
    # concurrent calls; a local server generates serially, so uncapped requests
    # queue server-side while their read timeouts tick — late requests then time
    # out at ANY timeout setting (the observed benchmark failure). The provider
    # must bound in-flight requests client-side.
    monkeypatch.delenv("OLLAMA_MAX_CONCURRENCY", raising=False)
    tracker = {"in_flight": 0, "max_in_flight": 0}
    body = lambda: {"message": {"content": '{"ok": true}'}, "model": "m"}  # noqa: E731
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", _make_tracking_client(body, tracker)
    )
    provider = OllamaProvider(project_root=tmp_path)
    await asyncio.gather(
        *(provider.complete("m", [{"role": "user", "content": f"q{i}"}]) for i in range(6))
    )
    assert tracker["max_in_flight"] <= OllamaProvider.DEFAULT_MAX_CONCURRENCY


@pytest.mark.anyio
async def test_ollama_concurrency_cap_disabled_with_off(tmp_path, monkeypatch):
    monkeypatch.setenv("OLLAMA_MAX_CONCURRENCY", "off")
    tracker = {"in_flight": 0, "max_in_flight": 0}
    body = lambda: {"message": {"content": '{"ok": true}'}, "model": "m"}  # noqa: E731
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", _make_tracking_client(body, tracker)
    )
    provider = OllamaProvider(project_root=tmp_path)
    await asyncio.gather(
        *(provider.complete("m", [{"role": "user", "content": f"q{i}"}]) for i in range(6))
    )
    assert tracker["max_in_flight"] > OllamaProvider.DEFAULT_MAX_CONCURRENCY
