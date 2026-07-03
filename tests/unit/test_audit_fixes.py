"""Regression tests for the audit-driven fixes:

- Ollama configurable timeout (OLLAMA_TIMEOUT)
- LLM cache key includes provider knobs (num_ctx / base_url)
- doctor verifies the configured Ollama model is actually pulled
- hardware sizing is VRAM-aware and cross-platform
- patch engine: whitespace/3-way fallback ladder + path-traversal validation
- prompt budget is context-window aware on local Ollama
- verifier: don't let an empty acceptance compiler silently demote a real test
  failure, and surface coarse acceptance proof as a first-class advisory
"""

import asyncio
import subprocess

import pytest

from devcouncil.domain.evidence import CommandResult
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task


# ---------------------------------------------------------------- Ollama timeout

def test_ollama_timeout_resolution(monkeypatch):
    from devcouncil.llm.provider import OllamaProvider

    monkeypatch.delenv("OLLAMA_TIMEOUT", raising=False)
    assert OllamaProvider._resolve_timeout() == OllamaProvider.DEFAULT_TIMEOUT
    monkeypatch.setenv("OLLAMA_TIMEOUT", "900")
    assert OllamaProvider._resolve_timeout() == 900.0
    for disabling in ("0", "none", "off", ""):
        monkeypatch.setenv("OLLAMA_TIMEOUT", disabling)
        assert OllamaProvider._resolve_timeout() is None
    monkeypatch.setenv("OLLAMA_TIMEOUT", "garbage")
    assert OllamaProvider._resolve_timeout() == OllamaProvider.DEFAULT_TIMEOUT


# ---------------------------------------------------------------- cache fingerprint

def test_cache_key_includes_provider_fingerprint(tmp_path):
    from devcouncil.llm.cache import LLMCache

    cache = LLMCache(tmp_path)
    msgs = [{"role": "user", "content": "hi"}]
    small = cache._get_key("m", msgs, 0.0, True, "ollama:num_ctx=4096;base_url=x")
    large = cache._get_key("m", msgs, 0.0, True, "ollama:num_ctx=16384;base_url=x")
    assert small != large  # raising num_ctx must invalidate the cache
    # An empty fingerprint (cloud providers) is stable.
    assert cache._get_key("m", msgs, 0.0, True) == cache._get_key("m", msgs, 0.0, True, "")


def test_ollama_provider_exposes_fingerprint():
    from devcouncil.llm.provider import OllamaProvider

    p = OllamaProvider(base_url="http://localhost:11434/v1", num_ctx=8192)
    fp = p.cache_fingerprint()
    assert "num_ctx=8192" in fp and "endpoint=http://localhost:11434/api/chat" in fp
    # The /v1 and non-/v1 spellings of the same server collapse to one fingerprint.
    assert p.cache_fingerprint() == OllamaProvider(base_url="http://localhost:11434", num_ctx=8192).cache_fingerprint()


# ---------------------------------------------------------------- doctor model presence

def test_ollama_model_present_matching():
    from devcouncil.cli.commands.doctor import _ollama_model_present

    pulled = {"qwen2.5-coder:7b", "llama3:latest"}
    assert _ollama_model_present("qwen2.5-coder:7b", pulled)
    assert _ollama_model_present("llama3", pulled)            # implicit :latest
    assert not _ollama_model_present("qwen2.5-coder:32b", pulled)
    assert not _ollama_model_present("mistral", set())


# ---------------------------------------------------------------- hardware sizing

def test_recommend_ollama_model_prefers_vram_ceiling():
    from devcouncil import hardware

    # 64 GB RAM but only 8 GB VRAM -> the GPU is the ceiling, not system RAM.
    assert hardware.recommend_ollama_model(ram_gb=64, vram_gb=8) == "qwen2.5-coder:7b"
    # No discrete GPU -> size by RAM (unified memory / CPU host).
    assert hardware.recommend_ollama_model(ram_gb=64, vram_gb=None) == "qwen2.5-coder:32b"
    assert hardware.recommend_ollama_model(ram_gb=24, vram_gb=None) == "qwen2.5-coder:14b"


def test_recommend_ollama_model_default_when_unknown(monkeypatch):
    from devcouncil import hardware

    monkeypatch.setattr(hardware, "total_ram_gb", lambda: None)
    assert hardware.recommend_ollama_model(ram_gb=None, vram_gb=None) == hardware.DEFAULT_OLLAMA_MODEL


def test_host_summary_has_cross_platform_labels():
    from devcouncil.hardware import HostSummary

    gpu_host = HostSummary(
        is_macos=False, is_apple_silicon=False, chip=None, ram_gb=64.0,
        recommended_ollama_model="qwen2.5-coder:7b", vram_gb=8.0,
    )
    assert gpu_host.platform_label  # never empty, even off-mac
    assert "VRAM" in gpu_host.memory_label
    cpu_host = HostSummary(
        is_macos=False, is_apple_silicon=False, chip=None, ram_gb=16.0,
        recommended_ollama_model="qwen2.5-coder:7b", vram_gb=None,
    )
    assert cpu_host.memory_label == cpu_host.ram_label


# ---------------------------------------------------------------- patch engine

def _git_repo(tmp_path, contents="line1\nline2\nline3\n"):
    subprocess.run(["git", "init", "-q"], cwd=tmp_path, check=True)
    (tmp_path / "a.txt").write_text(contents)
    subprocess.run(["git", "add", "-A"], cwd=tmp_path, check=True)
    subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "init"],
        cwd=tmp_path, check=True,
    )
    return tmp_path


def test_patch_applies_with_whitespace_drift(tmp_path):
    from devcouncil.execution.patch import PatchEngine

    # Working tree is space-indented; the patch's context/removed line is tab-indented.
    # The strict first rung fails on the whitespace mismatch; the --ignore-whitespace
    # rung in the ladder applies it cleanly (instead of the old single-shot hard fail).
    _git_repo(tmp_path, contents="def f():\n    return 1\n")
    patch = (
        "--- a/a.txt\n"
        "+++ b/a.txt\n"
        "@@ -1,2 +1,2 @@\n"
        " def f():\n"
        "-\treturn 1\n"
        "+\treturn 2\n"
    )
    assert PatchEngine(tmp_path).apply_patch(patch) is True
    assert "return 2" in (tmp_path / "a.txt").read_text()


def test_patch_rejects_path_traversal(tmp_path):
    from devcouncil.execution.patch import PatchEngine
    from devcouncil.app.errors import ExecutionError

    _git_repo(tmp_path)
    evil = (
        "--- a/../../etc/evil\n"
        "+++ b/../../etc/evil\n"
        "@@ -0,0 +1 @@\n"
        "+x\n"
    )
    with pytest.raises(ExecutionError, match="outside the project root"):
        PatchEngine(tmp_path).apply_patch(evil)


def test_patch_failure_names_git_report(tmp_path):
    from devcouncil.execution.patch import PatchEngine
    from devcouncil.app.errors import ExecutionError

    _git_repo(tmp_path)
    junk = (
        "--- a/a.txt\n"
        "+++ b/a.txt\n"
        "@@ -99,3 +99,3 @@\n"
        " nope\n"
        "-zzz\n"
        "+yyy\n"
        " done\n"
    )
    with pytest.raises(ExecutionError, match="git reported"):
        PatchEngine(tmp_path).apply_patch(junk)


# ---------------------------------------------------------------- prompt budget

def _write_ollama_config(tmp_path):
    cfg_dir = tmp_path / ".devcouncil"
    cfg_dir.mkdir(parents=True, exist_ok=True)
    (cfg_dir / "config.yaml").write_text(
        "models:\n  provider: ollama\n  roles: {}\n", encoding="utf-8"
    )


def test_prompt_budget_caps_to_local_window(tmp_path, monkeypatch):
    from devcouncil.execution import prompt_builder as pb

    _write_ollama_config(tmp_path)
    monkeypatch.setenv("OLLAMA_NUM_CTX", "4096")
    budget = pb._local_context_window_budget(tmp_path)
    # (4096 - reserved) * chars-per-token, floored at the minimum.
    expected = max(pb._MIN_PROMPT_CHARS, (4096 - pb._RESERVED_COMPLETION_TOKENS) * pb._CHARS_PER_TOKEN)
    assert budget == expected
    assert budget < pb.MAX_PROMPT_CHARS  # genuinely tighter than the char-only default


def test_prompt_budget_none_for_cloud_or_unset(tmp_path, monkeypatch):
    from devcouncil.execution import prompt_builder as pb
    from devcouncil.llm.provider import OllamaProvider

    # Ollama with no explicit window: the provider now sends DEFAULT_NUM_CTX (the
    # server default would silently truncate planning prompts), so the prompt
    # budget must cap to that same window — the provider's actual truncation point.
    _write_ollama_config(tmp_path)
    monkeypatch.delenv("OLLAMA_NUM_CTX", raising=False)
    expected = max(
        pb._MIN_PROMPT_CHARS,
        (OllamaProvider.DEFAULT_NUM_CTX - pb._RESERVED_COMPLETION_TOKENS) * pb._CHARS_PER_TOKEN,
    )
    assert pb._local_context_window_budget(tmp_path) == expected

    # Explicit opt-out (OLLAMA_NUM_CTX=0 -> server default): window unknown, don't guess.
    monkeypatch.setenv("OLLAMA_NUM_CTX", "0")
    assert pb._local_context_window_budget(tmp_path) is None

    # Cloud provider -> never capped by OLLAMA_NUM_CTX even if it leaks into the env.
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        "models:\n  provider: openrouter\n  roles: {}\n", encoding="utf-8"
    )
    monkeypatch.setenv("OLLAMA_NUM_CTX", "4096")
    assert pb._local_context_window_budget(tmp_path) is None


# ---------------------------------------------------------------- verifier authority

def _req():
    return Requirement(
        id="REQ-001", title="R", description="d", priority="high", source="user",
        acceptance_criteria=[
            AcceptanceCriterion(id="AC-001", description="t", verification_method="unit_test")
        ],
    )


def _task_with_test():
    return Task(
        id="TASK-001", title="T", description="d",
        requirement_ids=["REQ-001"], acceptance_criterion_ids=["AC-001"],
        planned_files=[PlannedFile(path="src/auth.py", reason="x", allowed_change="modify")],
        allowed_commands=["pytest tests/test_auth.py"],
    )


def _wire(verifier):
    verifier.get_changed_files = lambda: ["src/auth.py"]
    verifier.get_diff = lambda: "diff --git a/src/auth.py b/src/auth.py\n+token logic"


def test_failing_test_repromoted_when_compiler_yields_no_checks(tmp_path):
    """A real test failure must stay blocking when the acceptance compiler produces no
    usable per-criterion checks (empty/exception-swallowed) — it has no authority to demote."""
    from devcouncil.verification.verifier import Verifier

    class EmptyCompiler:
        async def compile(self, task, requirements, code_context):
            return {}  # produced nothing

    verifier = Verifier(tmp_path)
    verifier.acceptance_compiler = EmptyCompiler()
    _wire(verifier)
    verifier._commands_for_task = lambda task: {"test": ["pytest tests/test_auth.py"]}
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=1, stdout_path="", stderr_path="", summary="AssertionError",
    )

    gaps, _ = asyncio.run(verifier.verify_task(_task_with_test(), [_req()]))
    assert any(g.gap_type == "test_failed" and g.blocking for g in gaps)


def test_failing_test_stays_demoted_when_compiler_has_checks(tmp_path):
    """When the compiler DOES produce per-criterion checks (which pass), a bogus planner
    test failure remains demoted/non-blocking — the original false-block protection."""
    from devcouncil.verification.verifier import Verifier

    class GoodCompiler:
        async def compile(self, task, requirements, code_context):
            return {"AC-001": ['python -c "assert True"']}

    verifier = Verifier(tmp_path)
    verifier.acceptance_compiler = GoodCompiler()
    _wire(verifier)
    verifier._commands_for_task = lambda task: {"test": ["pytest tests/test_auth.py"]}

    def run(command, task_id="verify"):
        # The compiled check passes (exit 0); the planner test "fails" (exit 1).
        code = 0 if "assert True" in command else 1
        return CommandResult(
            command=command, exit_code=code, stdout_path="", stderr_path="",
            summary="ok" if code == 0 else "AssertionError",
        )

    verifier._run_command = run
    gaps, _ = asyncio.run(verifier.verify_task(_task_with_test(), [_req()]))
    # The planner test failure is recorded but not blocking; nothing blocks the gate.
    assert not any(g.blocking for g in gaps)


def _task_two_acs():
    return Task(
        id="TASK-001", title="T", description="d",
        requirement_ids=["REQ-001"], acceptance_criterion_ids=["AC-001", "AC-002"],
        planned_files=[PlannedFile(path="src/auth.py", reason="x", allowed_change="modify")],
        allowed_commands=["pytest tests/test_auth.py"],
    )


def _req_two_acs():
    return Requirement(
        id="REQ-001", title="R", description="d", priority="high", source="user",
        acceptance_criteria=[
            AcceptanceCriterion(id="AC-001", description="a", verification_method="unit_test"),
            AcceptanceCriterion(id="AC-002", description="b", verification_method="unit_test"),
        ],
    )


def test_partial_compiler_coverage_repromotes_failure(tmp_path):
    """A compiler that covers only SOME ACs must not be allowed to demote a real test
    failure while the uncovered ACs ride the coarse fallback — that was a false PASS."""
    from devcouncil.verification.verifier import Verifier

    class PartialCompiler:
        async def compile(self, task, requirements, code_context):
            return {"AC-001": ['python -c "assert True"']}  # AC-002 uncovered

    verifier = Verifier(tmp_path)
    verifier.acceptance_compiler = PartialCompiler()
    _wire(verifier)
    verifier._commands_for_task = lambda task: {"test": ["pytest tests/test_auth.py"]}

    def run(command, task_id="verify"):
        code = 0 if "assert True" in command else 1  # compiled AC-001 passes; planner test fails
        return CommandResult(
            command=command, exit_code=code, stdout_path="", stderr_path="",
            summary="ok" if code == 0 else "AssertionError",
        )

    verifier._run_command = run
    gaps, _ = asyncio.run(verifier.verify_task(_task_two_acs(), [_req_two_acs()]))
    assert any(g.gap_type == "test_failed" and g.blocking for g in gaps)


def test_quality_only_command_does_not_coarse_prove(tmp_path):
    """A passing type-checker/linter must not coarse-prove a behavioral AC."""
    from devcouncil.verification.verifier import Verifier

    verifier = Verifier(tmp_path)
    verifier.acceptance_compiler = None
    _wire(verifier)
    verifier._commands_for_task = lambda task: {"typecheck": ["mypy ."]}
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="ok",
    )

    gaps, _ = asyncio.run(verifier.verify_task(_task_with_test(), [_req()]))
    # mypy passing proves nothing behavioral: the AC stays unproven and is NOT coarse-proven.
    assert any(g.gap_type == "acceptance_criteria_unproven" for g in gaps)
    assert not any(g.gap_type == "coarse_acceptance_proof" for g in gaps)


def test_shell_session_denies_chained_injection(tmp_path):
    """The guarded shell must allowlist each segment of a chained command, so an allowed
    prefix can't smuggle an arbitrary command past a wildcard allowlist."""
    from devcouncil.execution.shell_session import GuardedShellSession

    task = Task(
        id="T", title="t", description="d",
        planned_files=[], allowed_commands=["git *"],
    )
    session = GuardedShellSession(tmp_path, task, shell="auto")
    assert session.policy.evaluate_command("git status && rm -rf important", task).action == "deny"
    assert session.policy.evaluate_command("git status", task).action in {"allow", "warn"}


def test_router_healing_failure_routes_to_fallback(tmp_path, monkeypatch):
    """A transient error during the HEALING completion must route into the fallback path
    (after retries) rather than escaping as a raw provider error."""
    import devcouncil.llm.router as router_mod
    from devcouncil.llm.provider import LLMResponse, Provider
    from devcouncil.llm.router import ModelRouter
    from pydantic import BaseModel

    async def _no_sleep(_seconds):
        return None

    monkeypatch.setattr(router_mod.asyncio, "sleep", _no_sleep)

    class Out(BaseModel):
        value: str

    class FlakyProvider(Provider):
        def __init__(self):
            self.heal_calls = 0

        async def complete(self, model, messages, temperature=0.0, json_mode=False,
                           task_id=None, run_id=None):
            is_heal = any("corrected JSON" in str(m.get("content", "")) for m in messages)
            if is_heal:
                self.heal_calls += 1
                raise RuntimeError("transient 429 during healing")
            return LLMResponse(content="not valid json", model=model, usage={}, raw_response={})

    provider = FlakyProvider()
    router = ModelRouter(provider, {"r": {"model": "m", "temperature": 0.0}}, tmp_path)
    fallback = Out(value="fallback")
    result = asyncio.run(
        router.complete_structured("r", [{"role": "user", "content": "hi"}], Out, fallback=fallback)
    )
    assert result is fallback
    assert provider.heal_calls >= 2  # healing was actually retried, not aborted on first error


def test_tracker_zeroes_local_provider_usage(tmp_path):
    """Local (Ollama) usage must be billed $0 by PROVIDER, even for a model id that
    collides with a priced cloud entry."""
    from devcouncil.telemetry.tracker import TelemetryTracker

    tracker = TelemetryTracker(tmp_path)
    # A model id that exists in the price table — would be billed if not for local=True.
    usage = {"prompt_tokens": 100_000, "completion_tokens": 100_000}
    tracker.log_usage("gpt-4o", usage, local=True)
    assert tracker.stats["total_cost"] == 0.0
    # Tokens are still recorded — only cost is zeroed.
    assert tracker.stats["total_prompt_tokens"] == 100_000


def test_ollama_provider_is_local_cost_free():
    from devcouncil.llm.provider import OllamaProvider, OpenRouterProvider

    assert OllamaProvider(base_url="http://localhost:11434/v1").is_local_cost_free() is True
    assert OpenRouterProvider("key").is_local_cost_free() is False


def test_record_phase_bridges_illegal_repair_reentry(tmp_path):
    """A repair re-run records TASK_EXECUTING straight after TASK_BLOCKED; the persisted
    history must be bridged through TASK_READY so it stays a legal transition sequence."""
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import StateRepository
    from devcouncil.app.state_machine import ProjectPhase, TRANSITIONS

    import subprocess
    subprocess.run(["git", "init", "-q"], cwd=tmp_path, check=True)
    from devcouncil.cli.commands.init import initialize_project
    initialize_project(tmp_path, project_name="t", with_map=False, with_skills=False)

    db = get_db(tmp_path)
    with db.get_session() as session:
        repo = StateRepository(session)
        repo.record_phase(ProjectPhase.TASK_EXECUTING.value)
        repo.record_phase(ProjectPhase.TASK_VERIFYING.value)
        repo.record_phase(ProjectPhase.TASK_BLOCKED.value)
        # Illegal direct jump — must be bridged.
        repo.record_phase(ProjectPhase.TASK_EXECUTING.value)
        import json as _json
        history = _json.loads(repo.get_state().history_json)

    assert ProjectPhase.TASK_READY.value in history
    # Every adjacent pair in the persisted history is a legal transition.
    phases = [ProjectPhase(p) for p in history]
    for a, b in zip(phases, phases[1:]):
        assert b in TRANSITIONS.get(a, set()), f"illegal {a.value} -> {b.value}"


def test_checkpoint_git_ref_rollback_restores_working_tree(tmp_path):
    """The git-ref rollback path (previously dead) must capture the working tree and
    restore it — including a file the task modified."""
    import subprocess
    from devcouncil.execution.checkpoints import CheckpointService

    subprocess.run(["git", "init", "-q"], cwd=tmp_path, check=True)
    (tmp_path / "app.py").write_text("VALUE = 1\n")
    subprocess.run(["git", "add", "-A"], cwd=tmp_path, check=True)
    subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "base"],
        cwd=tmp_path, check=True,
    )

    svc = CheckpointService(tmp_path)
    svc.create_before("TASK-X")
    (tmp_path / "app.py").write_text("VALUE = 999\n")     # the task's change
    (tmp_path / "new_file.py").write_text("created = True\n")  # an untracked new file
    svc.create_after("TASK-X")

    result = svc.rollback("TASK-X")
    assert result.git_ref_created is True and "git refs" in result.message
    assert (tmp_path / "app.py").read_text() == "VALUE = 1\n"  # restored
    assert not (tmp_path / "new_file.py").exists()            # untracked addition undone


def test_squash_does_not_leave_final_attempt_uncommitted(tmp_path):
    """After squashing intermediate [blocked] commits, the working tree is clean — the
    final attempt's changes are IN the squash commit, not a separate one."""
    import subprocess
    from devcouncil.cli.commands.go import _squash_repair_commits

    def git(*args):
        subprocess.run(["git", *args], cwd=tmp_path, check=True, capture_output=True, text=True)

    git("init", "-q")
    (tmp_path / "f.py").write_text("v0\n")
    git("add", "-A")
    git("-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "base")
    base = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=tmp_path, text=True).strip()

    # One [blocked] intermediate commit, then the final attempt's UNCOMMITTED changes.
    (tmp_path / "f.py").write_text("v1\n")
    git("add", "-A")
    git("-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "blocked")
    (tmp_path / "f.py").write_text("v2-final\n")  # final attempt, not committed

    assert _squash_repair_commits(tmp_path, "TASK-X", base, "verified") is True
    porcelain = subprocess.check_output(["git", "status", "--porcelain"], cwd=tmp_path, text=True)
    assert porcelain.strip() == ""  # clean: final change captured by the squash
    assert (tmp_path / "f.py").read_text() == "v2-final\n"
    # Exactly one commit on top of base.
    count = subprocess.check_output(
        ["git", "rev-list", "--count", f"{base}..HEAD"], cwd=tmp_path, text=True
    ).strip()
    assert count == "1"


def test_coarse_acceptance_proof_is_surfaced(tmp_path):
    """With no compiler, a passing acceptance-capable command proves the AC via the coarse
    fallback — which must be surfaced as a non-blocking advisory, not silently accepted."""
    from devcouncil.verification.verifier import Verifier

    verifier = Verifier(tmp_path)
    verifier.acceptance_compiler = None
    _wire(verifier)
    verifier._load_commands = lambda: {"test": ["pytest tests/test_auth.py"], "lint": [], "typecheck": []}
    verifier._run_command = lambda command, task_id="verify": CommandResult(
        command=command, exit_code=0, stdout_path="", stderr_path="", summary="passed",
    )

    gaps, evidence = asyncio.run(verifier.verify_task(_task_with_test(), [_req()]))
    coarse = [g for g in gaps if g.gap_type == "coarse_acceptance_proof"]
    assert coarse and not coarse[0].blocking
    assert "AC-001" in coarse[0].description
    # The AC is still considered proven (no unproven-AC gap).
    assert not any(g.gap_type == "acceptance_criteria_unproven" for g in gaps)
