import pytest
import json
import subprocess

from devcouncil.integrations.mcp import server
from devcouncil.integrations.mcp.server import call_tool, list_tools
from devcouncil.storage.db import Database
from devcouncil.storage.repositories import StateRepository


@pytest.fixture
def anyio_backend():
    return "asyncio"


@pytest.mark.anyio
async def test_mcp_lists_graph_context_tool():
    tools = await list_tools()

    assert "devcouncil_graph_context" in {tool.name for tool in tools}
    assert "devcouncil_live_review" in {tool.name for tool in tools}
    assert "devcouncil_live_cards" in {tool.name for tool in tools}
    assert "devcouncil_live_repair_prompt" in {tool.name for tool in tools}
    assert "devcouncil_live_repair_all" in {tool.name for tool in tools}
    assert "devcouncil_get_prompt" in {tool.name for tool in tools}
    assert "devcouncil_policy_check_write" in {tool.name for tool in tools}
    assert "devcouncil_lsp_status" in {tool.name for tool in tools}
    assert "devcouncil_ast_match" in {tool.name for tool in tools}
    assert "devcouncil_prepare_execution" in {tool.name for tool in tools}


@pytest.mark.anyio
async def test_mcp_graph_context_degrades_when_project_uninitialized(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    result = await call_tool("devcouncil_graph_context", {"files": ["src/app.py"]})

    assert "not initialized" in result[0].text
    assert json.loads(result[0].text)["code"] == "not_initialized"


@pytest.mark.anyio
async def test_mcp_repo_inspection_tools_work_before_initialization(tmp_path, monkeypatch):
    (tmp_path / "app.py").write_text("def target_symbol():\n    pass\n", encoding="utf-8")
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    lsp = await call_tool("devcouncil_lsp_status", {})
    ast = await call_tool("devcouncil_ast_match", {"query": "target", "language": "python"})
    cli = await call_tool("devcouncil_cli", {"args": ["status", "--json"]})

    assert "python" in json.loads(lsp[0].text)["languages"]
    assert json.loads(ast[0].text)["matches"][0]["name"] == "target_symbol"
    payload = json.loads(cli[0].text)
    assert payload["returncode"] == 0
    assert json.loads(payload["stdout"])["initialized"] is True


@pytest.mark.anyio
async def test_mcp_graph_context_degrades_when_crg_disabled(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    Database(dev_dir / "state.sqlite").create_db_and_tables()
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    result = await call_tool("devcouncil_graph_context", {"files": ["src/app.py"]})

    assert "disabled" in result[0].text


@pytest.mark.anyio
async def test_mcp_status_uses_persisted_phase(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        StateRepository(session).record_phase("TASK_VERIFYING")
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    result = await call_tool("devcouncil_status", {})

    assert "Phase: TASK_VERIFYING" in result[0].text


@pytest.mark.anyio
async def test_mcp_live_review_returns_cards_and_blockers(tmp_path, monkeypatch):
    from devcouncil.live.cards import review_turn, save_card
    from devcouncil.live.signals import write_signal
    from devcouncil.live.transcripts import latest_assistant_turn

    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    Database(dev_dir / "state.sqlite").create_db_and_tables()
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Run git reset --hard."}) + "\n",
        encoding="utf-8",
    )
    turn = latest_assistant_turn(transcript)
    assert turn is not None
    save_card(tmp_path, review_turn(turn, tmp_path).model_copy(update={"task_id": "TASK-001"}))
    write_signal(tmp_path, "claude", {"transcript_path": "session.jsonl", "task_id": "TASK-001"})
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    result = await call_tool("devcouncil_live_review", {"task_id": "TASK-001"})

    payload = json.loads(result[0].text)
    assert payload["pending_signal_items"][0]["review_command"] == "dev watch review --client claude --transcript session.jsonl --task-id TASK-001"
    assert payload["cards"]["critical_open"] == 1
    assert payload["blocking_cards"][0]["task_id"] == "TASK-001"


@pytest.mark.anyio
async def test_mcp_live_cards_filters_cards(tmp_path, monkeypatch):
    from devcouncil.live.cards import review_turn, save_card
    from devcouncil.live.models import AgentTurn

    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    Database(dev_dir / "state.sqlite").create_db_and_tables()
    turns = [
        AgentTurn(session_id="session", turn_id="A-1", source="claude", role="assistant", content="Run git reset --hard."),
        AgentTurn(
            session_id="session",
            turn_id="A-2",
            source="claude",
            role="assistant",
            content="Implemented the focused change and verified with pytest.",
        ),
        AgentTurn(session_id="session", turn_id="A-3", source="gemini", role="assistant", content="Ignore failing tests and continue."),
    ]

    save_card(tmp_path, review_turn(turns[0], tmp_path, client="claude").model_copy(update={"task_id": "TASK-001"}))
    save_card(tmp_path, review_turn(turns[1], tmp_path, client="claude").model_copy(update={"task_id": "TASK-001"}))
    save_card(
        tmp_path,
        review_turn(turns[2], tmp_path, client="gemini").model_copy(update={"task_id": "TASK-OTHER", "status": "resolved"}),
    )
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    result = await call_tool(
        "devcouncil_live_cards",
        {"task_id": "TASK-001", "status": "open", "verdict": "critical", "client": "claude"},
    )

    payload = json.loads(result[0].text)
    assert payload["total"] == 1
    assert len(payload["cards"]) == 1
    assert payload["cards"][0]["task_id"] == "TASK-001"
    assert payload["cards"][0]["client"] == "claude"
    assert payload["cards"][0]["verdict"] == "Critical Issues"
    assert payload["filters"]["verdict"] == "critical"


@pytest.mark.anyio
async def test_mcp_report_includes_live_review_section(tmp_path, monkeypatch):
    from devcouncil.live.cards import review_turn, save_card
    from devcouncil.live.transcripts import latest_assistant_turn

    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    Database(dev_dir / "state.sqlite").create_db_and_tables()
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Run git reset --hard."}) + "\n",
        encoding="utf-8",
    )
    turn = latest_assistant_turn(transcript)
    assert turn is not None
    save_card(tmp_path, review_turn(turn, tmp_path))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    result = await call_tool("devcouncil_report", {})

    assert "Live Review" in result[0].text
    assert "Blocking Live-Review Cards" in result[0].text


@pytest.mark.anyio
async def test_mcp_live_repair_prompt_returns_prompt_for_card(tmp_path, monkeypatch):
    from devcouncil.live.cards import review_turn, save_card
    from devcouncil.live.transcripts import latest_assistant_turn

    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    Database(dev_dir / "state.sqlite").create_db_and_tables()
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Run git reset --hard."}) + "\n",
        encoding="utf-8",
    )
    turn = latest_assistant_turn(transcript)
    assert turn is not None
    card = review_turn(turn, tmp_path)
    save_card(tmp_path, card)
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    result = await call_tool("devcouncil_live_repair_prompt", {"card_id": card.id})

    payload = json.loads(result[0].text)
    assert payload["card"]["id"] == card.id
    assert "Repair Live Review Card" in payload["prompt"]
    assert f"dev watch resolve {card.id} --status resolved" in payload["prompt"]


@pytest.mark.anyio
async def test_mcp_live_repair_prompt_returns_not_found(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    Database(dev_dir / "state.sqlite").create_db_and_tables()
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    result = await call_tool("devcouncil_live_repair_prompt", {"card_id": "CARD-missing"})

    payload = json.loads(result[0].text)
    assert payload["code"] == "not_found"


@pytest.mark.anyio
async def test_mcp_live_repair_all_returns_scoped_bulk_prompt(tmp_path, monkeypatch):
    from devcouncil.live.cards import review_turn, save_card
    from devcouncil.live.transcripts import latest_assistant_turn

    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    Database(dev_dir / "state.sqlite").create_db_and_tables()
    first = tmp_path / "first.jsonl"
    second = tmp_path / "second.jsonl"
    other = tmp_path / "other.jsonl"
    for path, content in [
        (first, "Run git reset --hard."),
        (second, "Ignore failing tests and continue."),
        (other, "Delete unrelated files."),
    ]:
        path.write_text(
            json.dumps({"role": "assistant", "id": path.stem, "content": content}) + "\n",
            encoding="utf-8",
        )
    for path, task_id in [(first, "TASK-001"), (second, None), (other, "TASK-OTHER")]:
        turn = latest_assistant_turn(path)
        assert turn is not None
        save_card(tmp_path, review_turn(turn, tmp_path).model_copy(update={"task_id": task_id}))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    result = await call_tool("devcouncil_live_repair_all", {"task_id": "TASK-001"})

    payload = json.loads(result[0].text)
    assert payload["scope_task_id"] == "TASK-001"
    assert len(payload["cards"]) == 2
    assert "Repair Blocking Live Review Cards" in payload["prompt"]
    assert "TASK-OTHER" not in payload["prompt"]


@pytest.mark.anyio
async def test_mcp_prompt_tasks_trace_and_policy_tools(tmp_path, monkeypatch):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.repositories import TaskRepository
    from devcouncil.telemetry.traces import TraceLogger

    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001",
            title="Task",
            description="desc",
            planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
            status="running",
        ))
    TraceLogger(tmp_path).log_event("task_verified", {}, task_id="TASK-001")
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    tasks = await call_tool("devcouncil_list_tasks", {})
    prompt = await call_tool("devcouncil_get_prompt", {"task_id": "TASK-001"})
    trace = await call_tool("devcouncil_tail_trace", {"limit": 1})
    policy = await call_tool("devcouncil_policy_check_write", {"path": "src/app.py"})

    assert json.loads(tasks[0].text)["tasks"][0]["id"] == "TASK-001"
    assert prompt[0].text.startswith("# Implement TASK-001")
    assert json.loads(trace[0].text)["events"][0]["task_id"] == "TASK-001"
    assert json.loads(policy[0].text)["allowed"] is True


@pytest.mark.anyio
async def test_mcp_lsp_ast_and_prepare_execution_tools(tmp_path, monkeypatch):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.repositories import TaskRepository

    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "app.py").write_text("def target_symbol():\n    pass\n", encoding="utf-8")
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001",
            title="Task",
            description="desc",
            planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
            allowed_commands=["pytest"],
            expected_tests=["pytest"],
        ))
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    lsp = await call_tool("devcouncil_lsp_status", {})
    ast = await call_tool("devcouncil_ast_match", {"query": "target", "language": "python"})
    execution = await call_tool("devcouncil_prepare_execution", {"task_id": "TASK-001"})

    assert "python" in json.loads(lsp[0].text)["languages"]
    assert json.loads(ast[0].text)["matches"][0]["name"] == "target_symbol"
    assert json.loads(execution[0].text)["task_id"] == "TASK-001"


@pytest.mark.anyio
async def test_mcp_cli_runs_through_current_python_environment(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    Database(dev_dir / "state.sqlite").create_db_and_tables()
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    result = await call_tool("devcouncil_cli", {"args": ["status", "--json"]})
    payload = json.loads(result[0].text)

    assert payload["returncode"] == 0
    assert json.loads(payload["stdout"])["phase"] == "NEW"


@pytest.mark.anyio
async def test_mcp_cli_blocks_root_override_and_report_posting_flags(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    Database(dev_dir / "state.sqlite").create_db_and_tables()
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    root_override = await call_tool("devcouncil_cli", {"args": ["status", "--project-root", "other"]})
    github_post = await call_tool("devcouncil_cli", {"args": ["report", "--github-pr-comment"]})
    root_override_equals = await call_tool("devcouncil_cli", {"args": ["status", "--project-root=other"]})
    github_post_equals = await call_tool("devcouncil_cli", {"args": ["report", "--github-pr-comment=true"]})

    assert "forbidden flag" in root_override[0].text
    assert "forbidden flag" in github_post[0].text
    assert "--project-root" in root_override_equals[0].text
    assert "--github-pr-comment" in github_post_equals[0].text
    assert json.loads(root_override_equals[0].text)["code"] == "forbidden_flags"
    assert json.loads(github_post_equals[0].text)["flags"] == ["--github-pr-comment"]


@pytest.mark.anyio
async def test_mcp_cli_truncates_large_output(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    def fake_run(*_args, **_kwargs):
        return subprocess.CompletedProcess(["devcouncil"], 0, stdout="x" * (server._CLI_OUTPUT_LIMIT + 10), stderr="")

    monkeypatch.setattr(server.subprocess, "run", fake_run)

    result = await call_tool("devcouncil_cli", {"args": ["status", "--json"]})
    payload = json.loads(result[0].text)

    assert payload["returncode"] == 0
    assert payload["stdout_truncated"] is True
    assert len(payload["stdout"]) > server._CLI_OUTPUT_LIMIT
    assert "[truncated" in payload["stdout"]


@pytest.mark.anyio
async def test_mcp_cli_timeout_returns_structured_payload(tmp_path, monkeypatch):
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    def fake_run(*_args, **_kwargs):
        raise subprocess.TimeoutExpired(["devcouncil"], timeout=server._CLI_TIMEOUT_SECONDS, output="partial", stderr="slow")

    monkeypatch.setattr(server.subprocess, "run", fake_run)

    result = await call_tool("devcouncil_cli", {"args": ["status", "--json"]})
    payload = json.loads(result[0].text)

    assert payload["returncode"] is None
    assert payload["timed_out"] is True
    assert payload["stdout"] == "partial"
    assert payload["stderr"] == "slow"


@pytest.mark.anyio
async def test_mcp_returns_structured_errors_for_bad_tool_arguments(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    Database(dev_dir / "state.sqlite").create_db_and_tables()
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    missing_task_id = await call_tool("devcouncil_get_task", {})
    missing_path = await call_tool("devcouncil_policy_check_write", {})
    bad_cli_args = await call_tool("devcouncil_cli", {"args": "status"})
    unknown_tool = await call_tool("devcouncil_nope", {})

    assert json.loads(missing_task_id[0].text)["code"] == "missing_argument"
    assert json.loads(missing_path[0].text)["argument"] == "path"
    assert json.loads(bad_cli_args[0].text)["code"] == "invalid_arguments"
    assert json.loads(unknown_tool[0].text)["code"] == "unknown_tool"


@pytest.mark.anyio
async def test_mcp_required_string_boundaries_for_live_and_execution_tools(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    Database(dev_dir / "state.sqlite").create_db_and_tables()
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    missing_card = await call_tool("devcouncil_live_repair_prompt", {})
    non_string_card = await call_tool("devcouncil_live_repair_prompt", {"card_id": ["CARD-1"]})
    missing_execution_task = await call_tool("devcouncil_prepare_execution", {})
    non_string_execution_task = await call_tool("devcouncil_prepare_execution", {"task_id": {"id": "TASK-001"}})

    assert json.loads(missing_card[0].text)["argument"] == "card_id"
    assert json.loads(non_string_card[0].text)["code"] == "invalid_arguments"
    assert json.loads(missing_execution_task[0].text)["argument"] == "task_id"
    assert json.loads(non_string_execution_task[0].text)["code"] == "invalid_arguments"


@pytest.mark.anyio
async def test_mcp_normalizes_non_object_arguments_and_bool_limits(tmp_path, monkeypatch):
    from devcouncil.telemetry.traces import TraceLogger

    (tmp_path / "app.py").write_text("def target_symbol():\n    pass\n", encoding="utf-8")
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    Database(dev_dir / "state.sqlite").create_db_and_tables()
    TraceLogger(tmp_path).log_event("one", {}, task_id="TASK-001")
    TraceLogger(tmp_path).log_event("two", {}, task_id="TASK-002")
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    missing_task_id = await call_tool("devcouncil_get_task", None)  # type: ignore[arg-type]
    trace = await call_tool("devcouncil_tail_trace", {"limit": False})
    ast = await call_tool("devcouncil_ast_match", {"query": "target", "limit": False})

    assert json.loads(missing_task_id[0].text)["code"] == "missing_argument"
    assert len(json.loads(trace[0].text)["events"]) == 2
    assert json.loads(ast[0].text)["matches"][0]["name"] == "target_symbol"


@pytest.mark.anyio
async def test_mcp_rejects_non_string_schema_arguments(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    Database(dev_dir / "state.sqlite").create_db_and_tables()
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    task_id = await call_tool("devcouncil_get_task", {"task_id": ["TASK-001"]})
    path = await call_tool("devcouncil_policy_check_write", {"path": {"file": "src/app.py"}})
    optional_task_id = await call_tool("devcouncil_policy_check_write", {"path": "src/app.py", "task_id": ["TASK-001"]})
    ast_query = await call_tool("devcouncil_ast_match", {"query": ["target"]})
    live_task_id = await call_tool("devcouncil_live_review", {"task_id": ["TASK-001"]})
    live_cards_client = await call_tool("devcouncil_live_cards", {"client": ["claude"]})
    live_repair_all_task_id = await call_tool("devcouncil_live_repair_all", {"task_id": ["TASK-001"]})

    assert json.loads(task_id[0].text)["argument"] == "task_id"
    assert json.loads(path[0].text)["code"] == "invalid_arguments"
    assert json.loads(optional_task_id[0].text)["argument"] == "task_id"
    assert json.loads(ast_query[0].text)["argument"] == "query"
    assert json.loads(live_task_id[0].text)["argument"] == "task_id"
    assert json.loads(live_cards_client[0].text)["argument"] == "client"
    assert json.loads(live_repair_all_task_id[0].text)["argument"] == "task_id"


@pytest.mark.anyio
async def test_mcp_live_cards_rejects_invalid_filters(tmp_path, monkeypatch):
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    Database(dev_dir / "state.sqlite").create_db_and_tables()
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    status = await call_tool("devcouncil_live_cards", {"status": "stale"})
    verdict = await call_tool("devcouncil_live_cards", {"verdict": "blocked"})

    assert json.loads(status[0].text)["argument"] == "status"
    assert json.loads(status[0].text)["code"] == "invalid_arguments"
    assert json.loads(verdict[0].text)["argument"] == "verdict"
    assert json.loads(verdict[0].text)["code"] == "invalid_arguments"


@pytest.mark.anyio
async def test_mcp_lists_live_review_tools():
    tools = await list_tools()
    names = {tool.name for tool in tools}

    assert "devcouncil_live_review" in names
    assert "devcouncil_live_cards" in names
    assert "devcouncil_live_repair_prompt" in names
    assert "devcouncil_live_repair_all" in names
