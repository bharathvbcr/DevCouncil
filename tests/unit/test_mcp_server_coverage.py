import json
import subprocess
from types import SimpleNamespace

import pytest
from pydantic import AnyUrl

from devcouncil.domain.evidence import CommandResult, DiffCoverageEvidence, DiffEvidence, TestEvidence
from devcouncil.domain.gap import Gap
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.integrations.mcp import server
from devcouncil.integrations.mcp.server import call_tool, get_prompt, list_prompts, read_resource
from devcouncil.storage.db import Database
from devcouncil.storage.repositories import EvidenceRepository, GapRepository, TaskRepository
from devcouncil.verification.verifier import VerificationOutcome


@pytest.fixture
def anyio_backend():
    return "asyncio"


def _init_db(tmp_path, tasks=()):
    dev = tmp_path / ".devcouncil"
    dev.mkdir(exist_ok=True)
    db = Database(dev / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        repo = TaskRepository(session)
        for task in tasks:
            repo.save(task)
    return db


def _task(task_id="TASK-001", **kwargs):
    data = {"id": task_id, "title": task_id, "description": "desc"}
    data.update(kwargs)
    return Task(**data)


def _planned(path="src/a.py"):
    return [PlannedFile(path=path, reason="logic", allowed_change="modify")]


def _gap(gap_id="GAP-1", *, task_id="TASK-001", blocking=True, **kwargs):
    data = {
        "id": gap_id,
        "severity": "high",
        "gap_type": "test_failed",
        "task_id": task_id,
        "description": "failed",
        "recommended_fix": "fix it",
        "blocking": blocking,
    }
    data.update(kwargs)
    return Gap(**data)


async def _payload(tool, args):
    return json.loads((await call_tool(tool, args))[0].text)


async def _checkout(task_id="TASK-001", client_id="client"):
    return await _payload("devcouncil_checkout_task", {"task_id": task_id, "client_id": client_id})


def _git(root, *args):
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True)


def _init_git(root):
    _git(root, "init")
    _git(root, "-c", "user.email=t@example.com", "-c", "user.name=T", "commit", "--allow-empty", "-m", "init")


@pytest.mark.anyio
async def test_helper_branches_and_cached_builders(tmp_path, monkeypatch):
    assert server._truncate_text(None) == ("", False)
    assert server._truncate_text(b"hello") == ("hello", False)
    truncated, was_truncated = server._truncate_text("abcdef", limit=3)
    assert truncated.startswith("abc")
    assert was_truncated is True

    assert server._forbidden_cli_flags(["status", "--project-root=elsewhere", "--github-pr-comment"]) == [
        "--github-pr-comment",
        "--project-root",
    ]
    assert server._read_log_file(None) == ""
    assert server._read_log_file(str(tmp_path / "missing.log")) == ""

    monkeypatch.setattr(server.subprocess, "run", lambda *a, **k: (_ for _ in ()).throw(OSError("git missing")))
    assert server._is_git_repo(tmp_path) is False

    assert server._diff_target_paths(
        'diff --git "a/src/old name.py" "b/src/new name.py"\n'
        "rename from src/old name.py\n"
        "rename to src/new name.py\n"
        "copy from src/copy.py\n"
        "copy to src/copy 2.py\n"
        "--- /dev/null\n"
        "+++ b/src/created.py\n"
    ) == [
        "src/old name.py",
        "src/new name.py",
        "src/copy.py",
        "src/copy 2.py",
        "src/created.py",
    ]

    assert server._allowed_next_tools("verified", False) == ["devcouncil_release_task"]
    assert server._allowed_next_tools("done", False) == []
    assert "devcouncil_verify_task" in server._allowed_next_tools("blocked", False)
    assert "devcouncil_checkout_task" in server._allowed_next_tools("planned", False)

    server._reset_caches()
    ast_1 = server._get_ast_matcher(tmp_path)
    ast_2 = server._get_ast_matcher(tmp_path)
    lsp_1 = server._get_lsp_inspector(tmp_path)
    lsp_2 = server._get_lsp_inspector(tmp_path)
    graph_1 = server._get_graph_adapter(tmp_path)
    graph_2 = server._get_graph_adapter(tmp_path)
    assert ast_1 is ast_2
    assert lsp_1 is lsp_2
    assert graph_1 is graph_2

    monkeypatch.setattr(server, "_build_router", lambda root: {"root": str(root)})
    first = server._load_router(tmp_path)
    second = server._load_router(tmp_path)
    assert first is second
    (tmp_path / ".devcouncil").mkdir(exist_ok=True)
    (tmp_path / ".devcouncil" / "config.yaml").write_text("models:\n  provider: nope\n", encoding="utf-8")
    refreshed = server._load_router(tmp_path)
    assert refreshed == {"root": str(tmp_path)}


@pytest.mark.anyio
async def test_git_diff_reports_files_errors_and_staged_changes(tmp_path):
    _init_git(tmp_path)
    (tmp_path / "a.txt").write_text("zero\n", encoding="utf-8")
    _git(tmp_path, "add", "a.txt")
    _git(tmp_path, "-c", "user.email=t@example.com", "-c", "user.name=T", "commit", "-m", "track a")
    (tmp_path / "a.txt").write_text("one\n", encoding="utf-8")

    unstaged = await server._git_diff(tmp_path, [], staged=False)
    assert unstaged["ok"] is True
    assert unstaged["files"][0]["path"] == "a.txt"
    assert unstaged["files"][0]["additions"] == 1

    _git(tmp_path, "add", "a.txt")
    staged = await server._git_diff(tmp_path, ["a.txt"], staged=True)
    assert staged["staged"] is True
    assert staged["files"][0]["status"] == "M"

    missing = await server._git_diff(tmp_path / "missing", [], staged=False)
    assert missing["ok"] is False
    assert missing["files"] == []


@pytest.mark.anyio
async def test_resources_cover_uninitialized_cards_missing_task_and_knowledge(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    assert "not initialized" in await read_resource(AnyUrl("devcouncil://report"))
    assert json.loads(await read_resource(AnyUrl("devcouncil://tasks"))) == {"tasks": []}
    assert json.loads(await read_resource(AnyUrl("devcouncil://gaps"))) == {"gaps": []}
    assert "cards" in json.loads(await read_resource(AnyUrl("devcouncil://cards")))

    _init_db(tmp_path, [_task()])
    missing = json.loads(await read_resource(AnyUrl("devcouncil://task/TASK-MISSING")))
    assert missing["ok"] is False

    monkeypatch.setattr(server, "_discover_knowledge_sources", lambda root: [])
    assert "No OKF" in await read_resource(AnyUrl("devcouncil://knowledge"))

    source = SimpleNamespace(
        kind="okf",
        name="source/name",
        description="A Source",
        body="fallback body",
        render=lambda: "rendered body",
    )
    monkeypatch.setattr(server, "_discover_knowledge_sources", lambda root: [source])
    index = await read_resource(AnyUrl("devcouncil://knowledge"))
    assert "A Source" in index
    assert await read_resource(AnyUrl(server._knowledge_source_uri("okf", "source/name"))) == "rendered body"
    assert "not found" in await read_resource(AnyUrl("devcouncil://knowledge/okf/missing"))


@pytest.mark.anyio
async def test_prompts_render_all_specs_and_unknowns(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    prompts = await list_prompts()
    names = {prompt.name for prompt in prompts}
    assert "devcouncil_implement_next_task" in names
    assert "initialized" in server._status_snapshot(tmp_path)

    _init_db(tmp_path, [_task()])
    for name in names:
        args = {"client_id": "cursor", "task_id": "TASK-001", "goal": "ship feature"}
        result = await get_prompt(name, args)
        assert result.description
        assert result.messages[0].content.text

    assert "Unknown DevCouncil prompt" in server._render_prompt_text("devcouncil_unknown", {}, tmp_path)
    with pytest.raises(ValueError):
        await get_prompt("devcouncil_unknown", {})

    class BrokenRepo:
        def load_graph(self):
            raise RuntimeError("boom")

    monkeypatch.setattr(server, "ArtifactGraphRepository", lambda session: BrokenRepo())
    assert server._status_snapshot(tmp_path) == "DevCouncil status unavailable."


@pytest.mark.anyio
async def test_task_gap_next_action_and_prompt_error_paths(tmp_path, monkeypatch):
    _init_db(
        tmp_path,
        [
            _task("TASK-001", status="blocked"),
            _task("TASK-002", status="verified"),
            _task("TASK-003", status="done"),
        ],
    )
    with Database(tmp_path / ".devcouncil" / "state.sqlite").get_session() as session:
        GapRepository(session).save(_gap("GAP-B", blocking=True, file="src/a.py", suggested_command="pytest"))
        GapRepository(session).save(_gap("GAP-A", blocking=False, gap_type="diff_not_exercised"))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    missing_task = await _payload("devcouncil_get_task", {"task_id": "TASK-NOPE"})
    assert missing_task["code"] == "not_found"

    blocking = await _payload("devcouncil_get_gaps", {"task_id": "TASK-001", "blocking_only": True})
    assert blocking["blocking_count"] == 1
    assert len(blocking["gaps"]) == 1

    actions = await _payload("devcouncil_get_next_actions", {"task_id": "TASK-001"})
    assert actions["next_actions"]
    assert actions["advisory_actions"]
    assert "devcouncil_verify_task" in actions["allowed_next_tools"]

    verified_actions = await _payload("devcouncil_get_next_actions", {"task_id": "TASK-002"})
    done_actions = await _payload("devcouncil_get_next_actions", {"task_id": "TASK-003"})
    assert verified_actions["allowed_next_tools"] == ["devcouncil_release_task"]
    assert done_actions["allowed_next_tools"] == []

    missing_prompt = await _payload("devcouncil_get_prompt", {"task_id": "TASK-NOPE"})
    assert missing_prompt["code"] == "not_found"

    bad_list = await _payload("devcouncil_list_tasks", {"status": ["blocked"], "limit": False, "offset": False})
    assert bad_list["code"] == "invalid_arguments"


@pytest.mark.anyio
async def test_checkout_renew_list_release_and_scope_edges(tmp_path, monkeypatch):
    _init_db(tmp_path, [_task(planned_files=_planned(), allowed_commands=["pytest"])])
    semantic_dir = tmp_path / ".devcouncil" / "semantic" / "TASK-001"
    semantic_dir.mkdir(parents=True)
    (semantic_dir / "before.json").write_text('{"symbols": ["x"]}', encoding="utf-8")
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    monkeypatch.setattr(server, "_lease_ttl_seconds", lambda root: 60)

    assert (await _payload("devcouncil_checkout_task", {"task_id": "TASK-001"}))["argument"] == "client_id"
    assert (await _payload("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "a", "agent": []}))[
        "argument"
    ] == "agent"
    assert (
        await _payload("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "a", "force": "yes"})
    )["argument"] == "force"
    assert (await _payload("devcouncil_checkout_task", {"task_id": "TASK-NOPE", "client_id": "a"}))[
        "code"
    ] == "not_found"

    first = await _checkout(client_id="a")
    assert first["semantic_context"] == {"symbols": ["x"]}
    forced = await _payload("devcouncil_checkout_task", {"task_id": "TASK-001", "client_id": "b", "force": True})
    token = forced["lease_token"]
    assert forced["ok"] is True

    bad_ttl = await _payload("devcouncil_renew_lease", {"task_id": "TASK-001", "lease_token": token, "ttl_seconds": True})
    assert bad_ttl["argument"] == "ttl_seconds"
    renewed = await _payload("devcouncil_renew_lease", {"task_id": "TASK-001", "lease_token": token, "ttl_seconds": 5})
    assert renewed["ok"] is True
    assert renewed["ttl_seconds"] == 5
    invalid = await _payload("devcouncil_renew_lease", {"task_id": "TASK-001", "lease_token": "wrong"})
    assert invalid["code"] == "invalid_lease"

    leases = await _payload("devcouncil_list_leases", {"active_only": False})
    assert leases["count"] >= 2
    assert (await _payload("devcouncil_list_leases", {"active_only": "yes"}))["argument"] == "active_only"

    bad_scope = await _payload(
        "devcouncil_update_task_scope",
        {"task_id": "TASK-001", "lease_token": "wrong", "expected_tests": ["pytest"]},
    )
    assert bad_scope["code"] == "invalid_lease"
    scope = await _payload(
        "devcouncil_update_task_scope",
        {
            "task_id": "TASK-001",
            "lease_token": token,
            "expected_tests": ["pytest", "pytest"],
            "allowed_commands": ["pytest", "ruff"],
        },
    )
    assert scope["expected_tests"] == ["pytest"]
    assert scope["allowed_commands"] == ["pytest", "ruff"]

    assert (await _payload("devcouncil_release_task", {"task_id": "TASK-001"}))["argument"] == "lease_token"
    assert (await _payload("devcouncil_release_task", {"task_id": "TASK-001", "lease_token": token}))["ok"] is True


@pytest.mark.anyio
async def test_evidence_recording_and_retrieval_edges(tmp_path, monkeypatch):
    _init_db(tmp_path, [_task()])
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    checkout = await _checkout()
    token = checkout["lease_token"]

    assert (
        await _payload(
            "devcouncil_append_evidence",
            {"task_id": "TASK-001", "lease_token": token, "command": "pytest", "summary": "ok", "exit_code": False},
        )
    )["code"] == "invalid_arguments"
    assert (
        await _payload(
            "devcouncil_record_command",
            {"task_id": "TASK-001", "lease_token": "bad", "command": "pytest", "status": "finished"},
        )
    )["code"] == "invalid_lease"
    assert (
        await _payload(
            "devcouncil_record_command",
            {"task_id": "TASK-001", "lease_token": token, "command": "pytest", "status": "finished", "exit_code": []},
        )
    )["code"] == "invalid_arguments"
    assert (
        await _payload(
            "devcouncil_record_command",
            {
                "task_id": "TASK-001",
                "lease_token": token,
                "command": "pytest -q",
                "status": "failed",
                "exit_code": 1,
                "reason": "tests failed",
            },
        )
    )["ok"] is True

    stdout = tmp_path / ".devcouncil" / "stdout.log"
    stderr = tmp_path / ".devcouncil" / "stderr.log"
    stdout.write_text("o" * (server._CLI_OUTPUT_LIMIT + 5), encoding="utf-8")
    stderr.write_text("err", encoding="utf-8")
    with Database(tmp_path / ".devcouncil" / "state.sqlite").get_session() as session:
        EvidenceRepository(session).save_command_result(
            "TASK-001",
            CommandResult(
                command="pytest -q",
                exit_code=1,
                stdout_path=str(stdout),
                stderr_path=str(stderr),
                summary="failed",
            ),
        )
        EvidenceRepository(session).save_command_result(
            "TASK-001",
            CommandResult(command="ruff", exit_code=0, stdout_path="", stderr_path="", summary="ok"),
        )

    evidence = await _payload("devcouncil_get_evidence", {"task_id": "TASK-001", "command": "pytest", "limit": 1})
    assert len(evidence["evidence"]) == 1
    assert evidence["evidence"][0]["truncated"] is True
    assert (await _payload("devcouncil_get_evidence", {"task_id": "TASK-001", "command": []}))[
        "argument"
    ] == "command"


@pytest.mark.anyio
async def test_read_file_secret_window_and_error_branches(tmp_path, monkeypatch):
    _init_db(tmp_path)
    file_path = tmp_path / "src" / "a.py"
    file_path.parent.mkdir()
    file_path.write_text("one\ntwo\nthree\nfour\n", encoding="utf-8")
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    assert (await _payload("devcouncil_read_file", {"path": ".env"}))["code"] == "secret_path"
    assert (await _payload("devcouncil_read_file", {"path": "../escape"}))["code"] == "path_escape"
    assert (await _payload("devcouncil_read_file", {"path": "missing.txt"}))["code"] == "not_found"
    assert (await _payload("devcouncil_read_file", {"path": "src/a.py", "line_range": []}))["argument"] == "line_range"
    assert (await _payload("devcouncil_read_file", {"path": "src/a.py", "line_range": "bad"}))[
        "argument"
    ] == "line_range"

    ranged = await _payload("devcouncil_read_file", {"path": "src/a.py", "line_range": "2-3"})
    assert ranged["content"] == "two\nthree"
    offset = await _payload("devcouncil_read_file", {"path": "src/a.py", "offset": 2, "limit": False})
    assert offset["content"] == "three\nfour"

    original = server.Path.read_bytes

    def fail_read(path):
        if path.name == "a.py":
            raise OSError("cannot read")
        return original(path)

    monkeypatch.setattr(server.Path, "read_bytes", fail_read)
    assert (await _payload("devcouncil_read_file", {"path": "src/a.py"}))["code"] == "read_failed"


@pytest.mark.anyio
async def test_get_diff_argument_task_and_staged_paths(tmp_path, monkeypatch):
    _init_db(tmp_path, [_task(planned_files=_planned("tracked.txt"))])
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    assert (await _payload("devcouncil_get_diff", {"staged": "yes"}))["code"] == "not_a_git_repo"

    _init_git(tmp_path)
    (tmp_path / "tracked.txt").write_text("base\n", encoding="utf-8")
    _git(tmp_path, "add", "tracked.txt")
    _git(tmp_path, "-c", "user.email=t@example.com", "-c", "user.name=T", "commit", "-m", "track file")
    (tmp_path / "tracked.txt").write_text("change\n", encoding="utf-8")
    assert (await _payload("devcouncil_get_diff", {"task_id": []}))["argument"] == "task_id"
    assert (await _payload("devcouncil_get_diff", {"paths": "tracked.txt"}))["argument"] == "paths"
    assert (await _payload("devcouncil_get_diff", {"staged": "yes"}))["argument"] == "staged"
    assert (await _payload("devcouncil_get_diff", {"task_id": "TASK-NOPE"}))["code"] == "not_found"
    diff = await _payload("devcouncil_get_diff", {"task_id": "TASK-001", "paths": ["tracked.txt"]})
    assert diff["ok"] is True
    assert diff["files"][0]["path"] == "tracked.txt"


@pytest.mark.anyio
async def test_write_file_error_paths(tmp_path, monkeypatch):
    _init_db(tmp_path, [_task(planned_files=_planned("src/a.py"))])
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    checkout = await _checkout()
    token = checkout["lease_token"]

    assert (
        await _payload(
            "devcouncil_write_file",
            {"task_id": "TASK-001", "lease_token": token, "path": "src/a.py", "content": 123},
        )
    )["argument"] == "content"
    assert (
        await _payload(
            "devcouncil_write_file",
            {"task_id": "TASK-NOPE", "lease_token": token, "path": "src/a.py", "content": "x"},
        )
    )["code"] == "invalid_lease"
    assert (
        await _payload(
            "devcouncil_write_file",
            {"task_id": "TASK-001", "lease_token": token, "path": "package.json", "content": "{}\n"},
        )
    )["ok"] is False

    monkeypatch.setattr(server.os, "replace", lambda src, dst: (_ for _ in ()).throw(OSError("disk full")))
    failed = await _payload(
        "devcouncil_write_file",
        {"task_id": "TASK-001", "lease_token": token, "path": "src/a.py", "content": "x = 1\n"},
    )
    assert failed["code"] == "write_failed"


@pytest.mark.anyio
async def test_apply_patch_error_paths(tmp_path, monkeypatch):
    _init_db(tmp_path, [_task(planned_files=_planned("src/a.py"))])
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    checkout = await _checkout()
    token = checkout["lease_token"]

    assert (
        await _payload(
            "devcouncil_apply_patch",
            {"task_id": "TASK-001", "lease_token": token, "unified_diff": ""},
        )
    )["argument"] == "unified_diff"
    assert (
        await _payload(
            "devcouncil_apply_patch",
            {"task_id": "TASK-001", "lease_token": token, "unified_diff": "diff --git a/src/a.py b/src/a.py\n"},
        )
    )["code"] == "not_a_git_repo"

    _init_git(tmp_path)
    assert (
        await _payload(
            "devcouncil_apply_patch",
            {"task_id": "TASK-001", "lease_token": token, "unified_diff": "not a patch"},
        )
    )["code"] == "empty_patch"
    diff = "--- a/src/a.py\n+++ b/src/a.py\n@@ -0,0 +1 @@\n+x\n"
    assert (
        await _payload(
            "devcouncil_apply_patch",
            {"task_id": "TASK-001", "lease_token": "bad", "unified_diff": diff},
        )
    )["code"] == "invalid_lease"

    escape = "--- a/../escape.py\n+++ b/../escape.py\n@@ -0,0 +1 @@\n+x\n"
    rejected = await _payload(
        "devcouncil_apply_patch",
        {"task_id": "TASK-001", "lease_token": token, "unified_diff": escape},
    )
    assert rejected["rejected_files"][0]["reason"] == "path escapes the project root"

    patch_rejected = subprocess.CompletedProcess(["git"], 1, stdout="", stderr="bad patch")
    monkeypatch.setattr(server, "_is_git_repo", lambda root: True)
    monkeypatch.setattr(server.subprocess, "run", lambda *a, **k: patch_rejected)
    failed = await _payload(
        "devcouncil_apply_patch",
        {"task_id": "TASK-001", "lease_token": token, "unified_diff": diff},
    )
    assert failed["code"] == "patch_rejected"

    calls = []

    def fake_run(*args, **kwargs):
        calls.append(args[0])
        return subprocess.CompletedProcess(args[0], 0 if len(calls) == 1 else 1, stdout="", stderr="apply failed")

    monkeypatch.setattr(server.subprocess, "run", fake_run)
    failed_apply = await _payload(
        "devcouncil_apply_patch",
        {"task_id": "TASK-001", "lease_token": token, "unified_diff": diff},
    )
    assert failed_apply["code"] == "patch_failed"


@pytest.mark.anyio
async def test_verify_task_sandbox_invalid_lease_success_and_evidence_types(tmp_path, monkeypatch):
    _init_db(tmp_path, [_task()])
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    checkout = await _checkout()
    token = checkout["lease_token"]

    assert (
        await _payload("devcouncil_verify_task", {"task_id": "TASK-001", "lease_token": token, "sandbox": "docker"})
    )["code"] == "unsupported_sandbox"
    assert (
        await _payload("devcouncil_verify_task", {"task_id": "TASK-001", "lease_token": "bad"})
    )["code"] == "invalid_lease"

    class FakeVerifier:
        def __init__(self, root, router=None):
            self.last_outcome = VerificationOutcome(
                mode="compiled",
                compiler_active=True,
                diff_empty=False,
                coverage_measured=True,
                coverage_skipped_reason=None,
            )

        async def verify_task(self, task, requirements):
            return [], [
                CommandResult(command="pytest", exit_code=0, stdout_path="", stderr_path="", summary="ok"),
                DiffCoverageEvidence(task_id=task.id, tool="coverage", measured=True, summary="covered"),
                DiffEvidence(task_id=task.id, changed_files=["src/a.py"], added_files=[], deleted_files=[], diff_summary="d"),
                TestEvidence(
                    requirement_id="REQ-1",
                    acceptance_criterion_id="AC-1",
                    command="pytest",
                    status="passed",
                    evidence_summary="ok",
                    mode="compiled",
                ),
            ]

    import devcouncil.verification.verifier as verifier_module

    monkeypatch.setattr(server, "_load_router", lambda root: object())
    monkeypatch.setattr(verifier_module, "Verifier", FakeVerifier)
    verified = await _payload("devcouncil_verify_task", {"task_id": "TASK-001", "lease_token": token})
    assert verified["passed"] is True
    assert verified["status"] == "verified"
    assert verified["verification_mode"] == "compiled"


@pytest.mark.anyio
async def test_handoff_agent_success_failure_and_argument_edges(tmp_path, monkeypatch):
    _init_db(tmp_path, [_task()])
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    checkout = await _checkout()
    token = checkout["lease_token"]

    assert (await _payload("devcouncil_handoff_agent", {"task_id": "TASK-001", "lease_token": token}))[
        "argument"
    ] == "from_agent"
    assert (
        await _payload(
            "devcouncil_handoff_agent",
            {"task_id": "TASK-001", "lease_token": "bad", "from_agent": "cursor", "to_agent": "codex"},
        )
    )["code"] == "invalid_lease"

    class FakeManifest:
        def model_dump(self):
            return {"id": "manifest"}

    class FakeHandoffService:
        def __init__(self, root):
            self.root = root

        def create(self, task_id, from_agent, to_agent, instruction=""):
            if to_agent == "broken":
                raise ValueError("cannot hand off")
            return FakeManifest(), self.root / ".devcouncil" / "handoff.json", "RUN-1"

    import devcouncil.execution.handoff as handoff_module

    monkeypatch.setattr(handoff_module, "HandoffService", FakeHandoffService)
    ok = await _payload(
        "devcouncil_handoff_agent",
        {
            "task_id": "TASK-001",
            "lease_token": token,
            "from_agent": "cursor",
            "to_agent": "codex",
            "instruction": "continue",
        },
    )
    assert ok["ok"] is True
    assert ok["manifest"] == {"id": "manifest"}
    failed = await _payload(
        "devcouncil_handoff_agent",
        {"task_id": "TASK-001", "lease_token": token, "from_agent": "cursor", "to_agent": "broken"},
    )
    assert failed["code"] == "handoff_failed"


@pytest.mark.anyio
async def test_run_command_policy_success_timeout_and_failure(tmp_path, monkeypatch):
    _init_db(tmp_path, [_task(allowed_commands=["python --version"])])
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    checkout = await _checkout()
    token = checkout["lease_token"]

    denied = await _payload(
        "devcouncil_run_command",
        {"task_id": "TASK-001", "lease_token": token, "command": "rm -rf build"},
    )
    assert denied["code"] == "command_not_allowed"
    assert (await _payload("devcouncil_run_command", {"task_id": "TASK-001", "lease_token": "bad", "command": "python --version"}))[
        "code"
    ] == "invalid_lease"

    monkeypatch.setattr(
        server.subprocess,
        "run",
        lambda *a, **k: subprocess.CompletedProcess(a[0], 0, stdout="Python 3\n", stderr=""),
    )
    ok = await _payload(
        "devcouncil_run_command",
        {"task_id": "TASK-001", "lease_token": token, "command": "python --version"},
    )
    assert ok["ok"] is True
    assert ok["stdout"] == "Python 3\n"

    monkeypatch.setattr(
        server.subprocess,
        "run",
        lambda *a, **k: (_ for _ in ()).throw(subprocess.TimeoutExpired(a[0], 1, output="partial", stderr="slow")),
    )
    timed_out = await _payload(
        "devcouncil_run_command",
        {"task_id": "TASK-001", "lease_token": token, "command": "python --version"},
    )
    assert timed_out["timed_out"] is True

    monkeypatch.setattr(server.subprocess, "run", lambda *a, **k: (_ for _ in ()).throw(FileNotFoundError("missing")))
    failed = await _payload(
        "devcouncil_run_command",
        {"task_id": "TASK-001", "lease_token": token, "command": "python --version"},
    )
    assert failed["code"] == "run_failed"


@pytest.mark.anyio
async def test_next_task_and_cli_additional_error_branches(tmp_path, monkeypatch):
    _init_db(
        tmp_path,
        [
            _task("TASK-001", status="planned", depends_on=["TASK-000"]),
            _task("TASK-000", status="verified"),
            _task("TASK-002", status="ready"),
            _task("TASK-003", status="done"),
        ],
    )
    with Database(tmp_path / ".devcouncil" / "state.sqlite").get_session() as session:
        GapRepository(session).save(_gap("GAP-2", task_id="TASK-002", blocking=True))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    leased = await _checkout("TASK-002")
    assert leased["ok"] is True

    chosen = await _payload("devcouncil_next_task", {})
    assert chosen["task"]["id"] == "TASK-001"
    assert chosen["ready_to_checkout"] is True
    none_left = await _payload("devcouncil_next_task", {"status": "blocked"})
    assert none_left["task"] is None
    assert (await _payload("devcouncil_next_task", {"client_id": []}))["argument"] == "client_id"

    assert (await _payload("devcouncil_cli", {"args": ["doctor"]}))["code"] == "command_not_allowed"
    monkeypatch.setattr(server, "_run_cli_command", lambda args, root: (_ for _ in ()).throw(RuntimeError("boom")))
    assert (await _payload("devcouncil_cli", {"args": ["status"]}))["code"] == "cli_execution_error"


@pytest.mark.anyio
async def test_agent_run_tools_and_select_knowledge_fallback(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    fake_rows = [{"run_id": "RUN-1", "status": "running"}, {"run_id": "RUN-2", "status": "done"}]
    monkeypatch.setattr("devcouncil.cli.commands.runs._collect_runs", lambda root, orphan_after=None: fake_rows)
    monkeypatch.setattr("devcouncil.cli.commands.runs._orphan_after_seconds", lambda root: 30)

    assert (await _payload("devcouncil_list_agent_runs", {"status": []}))["argument"] == "status"
    runs = await _payload("devcouncil_list_agent_runs", {"status": "running", "limit": 1})
    assert runs["returned"] == 1
    assert runs["runs"][0]["run_id"] == "RUN-1"

    assert (await _payload("devcouncil_get_run", {}))["argument"] == "run_id"
    monkeypatch.setattr("devcouncil.cli.commands.runs._runs_dir", lambda root: tmp_path / ".devcouncil" / "runs")
    monkeypatch.setattr("devcouncil.cli.commands.runs._load_manifest", lambda path: None)
    assert (await _payload("devcouncil_get_run", {"run_id": "RUN-NOPE"}))["code"] == "not_found"

    manifest = {"run_id": "RUN-1", "status": "running"}
    transcript = tmp_path / "transcript.jsonl"
    transcript.write_text("line\n", encoding="utf-8")
    monkeypatch.setattr("devcouncil.cli.commands.runs._load_manifest", lambda path: manifest)
    monkeypatch.setattr("devcouncil.cli.commands.runs._is_orphaned", lambda *a, **k: False)
    monkeypatch.setattr("devcouncil.cli.commands.runs._find_transcript", lambda run_dir, run_manifest: transcript)
    monkeypatch.setattr("devcouncil.cli.commands.runs._transcript_tail", lambda path: "tail")
    run = await _payload("devcouncil_get_run", {"run_id": "RUN-1"})
    assert run["manifest"] == manifest
    assert run["transcript_tail"] == "tail"

    source = SimpleNamespace(name="design.md", kind="design", description="Design")
    monkeypatch.setattr(server, "_knowledge_settings", lambda root: ("knowledge", True))
    monkeypatch.setattr("devcouncil.knowledge.sources.select_knowledge_sources", lambda *a, **k: [source])
    monkeypatch.setattr("devcouncil.knowledge.sources.render_knowledge_preamble", lambda sources: "preamble")
    selected = await _payload("devcouncil_select_knowledge", {"goal": "design"})
    assert selected["sources"] == [{"name": "design.md", "kind": "design", "description": "Design"}]

    monkeypatch.setattr(
        "devcouncil.knowledge.sources.select_knowledge_sources",
        lambda *a, **k: (_ for _ in ()).throw(RuntimeError("offline")),
    )
    fallback = await _payload("devcouncil_select_knowledge", {"goal": "design"})
    assert fallback["sources"] == []
    assert "knowledge unavailable" in fallback["note"]
