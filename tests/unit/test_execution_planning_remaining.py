import json
from contextlib import contextmanager
from types import SimpleNamespace

import pytest

from devcouncil.app.errors import GatingError
from devcouncil.domain.evidence import CommandResult
from devcouncil.domain.gap import Gap
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution.context_builder import ContextBuilder
from devcouncil.execution.fs_watcher import FilesystemWatcher
from devcouncil.execution.permissions import PermissionManager, PermissionPolicy
from devcouncil.execution.prompt_builder import PromptBuilder
from devcouncil.planning import correction_manifest as cm
from devcouncil.planning.prompt_enhancer_service import (
    PromptEnhancement,
    PromptEnhancerService,
    _clean_items,
    _compact_brief,
    _full_intake,
    _knowledge_brief,
    _knowledge_intake,
    load_latest_prompt_enhancement,
    save_active_prompt_enhancement,
)
from devcouncil.planning.repair_service import RepairOutput, RepairService


def _task() -> Task:
    return Task(
        id="TASK-1",
        title="Implement feature",
        description="Touch code and tests",
        requirement_ids=["REQ-1"],
        planned_files=[
            PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
            PlannedFile(path="src/new.py", reason="new", allowed_change="create"),
        ],
        expected_tests=["pytest tests/test_app.py"],
        allowed_commands=["python -m pytest"],
        forbidden_changes=["secret.txt"],
    )


def _gap(
    gap_id: str,
    *,
    severity: str = "high",
    gap_type: str = "test_failed",
    blocking: bool = True,
    method: str | None = None,
) -> Gap:
    return Gap(
        id=gap_id,
        severity=severity,  # type: ignore[arg-type]
        gap_type=gap_type,  # type: ignore[arg-type]
        task_id="TASK-1",
        description=f"{severity} {gap_type}",
        recommended_fix="fix",
        blocking=blocking,
        expected_verification_method=method,
    )


def test_permission_manager_dynamic_ignores_and_validation(tmp_path):
    (tmp_path / ".devcouncilignore").write_text("# comment\nsecrets/*\n", encoding="utf-8")
    manager = PermissionManager(PermissionPolicy(allowed_shell_commands=["pytest -q"]), tmp_path)
    task = _task()

    assert manager.is_file_change_allowed("secrets/token.txt", task) is False
    assert manager.is_file_change_allowed("src/app.py", task, operation="modify") is True
    assert manager.is_command_allowed("pytest -q", task) is True
    manager.validate_action("file_write", "src/app.py", task, operation="modify")
    manager.validate_action("shell", "pytest -q", task)

    with pytest.raises(GatingError):
        manager.validate_action("file_write", "other.py", task)
    with pytest.raises(GatingError):
        manager.validate_action("shell", "rm -rf .", task)


def test_context_builder_builds_redacted_task_context(monkeypatch, tmp_path):
    src = tmp_path / "src"
    src.mkdir()
    (src / "app.py").write_text("API_KEY='abcdef1234567890'\n", encoding="utf-8")
    task = _task()
    requirement = SimpleNamespace(id="REQ-1", model_dump=lambda: {"id": "REQ-1", "title": "Req"})
    monkeypatch.setattr(ContextBuilder, "get_structure_summary", lambda self, task=None: ["src/app.py", "src/new.py"])

    payload = json.loads(ContextBuilder(tmp_path).build_task_context(task, [requirement]))

    assert payload["task"]["id"] == "TASK-1"
    assert payload["relevant_requirements"] == [{"id": "REQ-1", "title": "Req"}]
    assert "abcdef1234567890" not in payload["file_contents"]["src/app.py"]
    assert payload["file_contents"]["src/new.py"] == "[New file - does not exist yet]"
    assert payload["project_structure"] == ["src/app.py", "src/new.py"]


def test_filesystem_watcher_records_events_debounce_and_cache(monkeypatch, tmp_path):
    task = _task()
    watcher = FilesystemWatcher(tmp_path, "TASK-1", on_event=lambda event: notified.append(event))
    notified = []
    decisions = []

    class FakePolicy:
        def evaluate_file_change(self, path, task_arg, operation="modify", internal=False):
            decisions.append((path, task_arg.id if task_arg else None, operation))
            return SimpleNamespace(action="allow" if path == "src/app.py" else "deny", reason=f"reason:{path}")

    monkeypatch.setattr(watcher, "policy", FakePolicy())
    monkeypatch.setattr(watcher, "_load_task", lambda: task)
    monkeypatch.setattr("devcouncil.execution.fs_watcher.get_db", lambda root: None)

    assert watcher.should_ignore(".git/config") is True
    assert watcher.should_ignore("src/app.py") is False
    assert watcher.handle_event(str(tmp_path.parent / "outside.py")) is None
    assert watcher.handle_event(str(tmp_path / ".devcouncil" / "state.sqlite")) is None

    event = watcher.handle_event(str(tmp_path / "src" / "app.py"), operation="modify")
    assert event == {"path": "src/app.py", "operation": "modify", "allowed": True, "reason": "reason:src/app.py"}
    assert notified == [event]
    assert watcher.handle_event(str(tmp_path / "src" / "app.py"), operation="modify") is None

    watcher._seen.clear()
    denied = watcher.handle_event(str(tmp_path / "other.py"), operation="delete")
    assert denied["allowed"] is False
    assert decisions[-1] == ("other.py", "TASK-1", "delete")
    assert watcher._task_cached() is task


def test_filesystem_watcher_scan_once_filters_ignored_paths(monkeypatch, tmp_path):
    task = _task()
    watcher = FilesystemWatcher(tmp_path, "TASK-1")
    monkeypatch.setattr(watcher, "_task_cached", lambda: task)
    monkeypatch.setattr(watcher, "_changed_files", lambda: [".git/config", "src/app.py", "build/out"])
    monkeypatch.setattr(watcher, "_record_path", lambda path, task_arg, operation: {"path": path, "operation": operation})

    assert watcher.scan_once() == [{"path": "src/app.py", "operation": "modify"}]


def test_prompt_builder_symbol_sections_repo_map_and_call_sites(monkeypatch, tmp_path):
    src = tmp_path / "src"
    tests = tmp_path / "tests"
    src.mkdir()
    tests.mkdir()
    (src / "app.py").write_text(
        "class Service:\n"
        "    @staticmethod\n"
        "    def make(x):\n"
        "        return x\n"
        "\n"
        "async def fetch(url):\n"
        "    return url\n",
        encoding="utf-8",
    )
    (src / "new.py").write_text("export function greet() {}\nexport const value = 1\n", encoding="utf-8")
    (tests / "test_app.py").write_text("from src.app import Service\nassert Service.make(1)\n", encoding="utf-8")
    builder = PromptBuilder(tmp_path)
    task = _task()

    py_outline = builder._symbol_outline("src/app.py", (src / "app.py").read_text(encoding="utf-8"))
    ts_outline = builder._symbol_outline("src/new.ts", "export class Widget {}\nexport const thing = 1\n")
    assert "class Service L1" in py_outline
    assert any("def make" in row and "@staticmethod" in row for row in py_outline)
    assert ts_outline == ["export class Widget L1", "export const thing L2"]
    assert builder._symbol_outline("broken.py", "def nope(:") == []
    assert builder._lang_for("file.unknown") == ""

    planned = builder._planned_files_section(task)
    assert "Current file contents" in planned
    assert "Symbols:" in planned
    assert "src/new.py" in planned

    repo_map = {
        "subsystems": [
            {
                "area": "src",
                "summary": "core",
                "critical_files": ["src/helpers.py", "src/app.py"],
                "neighbors": ["tests"],
                "handoff_paths": ["src -> tests"],
            }
        ],
        "files": [{"path": "src/helpers.py", "summary": "helper summary"}],
        "dependents": {"src/app.py": ["tests/test_app.py", *[f"tests/t{i}.py" for i in range(10)]]},
        "dependency_risks": [
            {"package": "pkg", "severity": "critical", "advisory_id": "CVE-1", "summary": "bad"},
            "bad-shape",
        ],
    }
    assert "helper summary" in builder._repo_map_section(["src/app.py"], repo_map)
    assert "imported by" in builder._dependents_section(task, repo_map)
    assert "Service.make" in builder._call_sites_section(task, repo_map)
    assert "CVE-1" in builder._dependency_risks_section(repo_map)
    assert builder._references_symbol("Service.make()", "Service") is True
    assert builder._references_symbol("MyService", "Service") is False

    monkeypatch.setattr(builder, "_load_repo_map", lambda: repo_map)
    monkeypatch.setattr("devcouncil.indexing.repo_mapper.RepoMapper", lambda root: SimpleNamespace(map_is_stale=lambda data: True))
    assert builder._repo_map_stale(repo_map) is True
    assert builder._repo_map_stale(None) is False


def test_prompt_builder_budget_and_optional_sections(monkeypatch, tmp_path):
    builder = PromptBuilder(tmp_path)
    task = _task()

    monkeypatch.setattr("devcouncil.execution.prompt_builder.load_latest_prompt_enhancement", lambda root: (_ for _ in ()).throw(RuntimeError("boom")), raising=False)
    assert builder._load_prompt_enhancement() is None
    assert builder._load_repo_map() is None
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "repo_map.json").write_text("{bad json", encoding="utf-8")
    assert builder._load_repo_map() is None

    monkeypatch.setattr(
        "devcouncil.skills.registry.select_skills",
        lambda goal, project_root=None: [SimpleNamespace(name="web", description="desc", title="Web", body="body")],
    )
    monkeypatch.setattr(
        "devcouncil.skills.registry.bound_skills",
        lambda selected: (selected[:1], [SimpleNamespace(name="ios", description="mobile", title="iOS")]),
    )
    monkeypatch.setattr("devcouncil.skills.registry.render_preamble", lambda inline: "skill body")
    assert "web" in builder._skills_section(task)
    assert "ios" in builder._skills_section(task)

    cfg = SimpleNamespace(
        knowledge=SimpleNamespace(
            enabled=True,
            directory=".devcouncil/knowledge",
            design_always=True,
            design_max_chars=100,
            okf_max_chars=100,
        )
    )
    monkeypatch.setattr(
        "devcouncil.knowledge.sources.select_knowledge_sources",
        lambda **kwargs: [
            SimpleNamespace(kind="design", render=lambda: "design body"),
            SimpleNamespace(kind="okf", render=lambda: "okf body"),
        ],
    )
    monkeypatch.setattr(
        "devcouncil.knowledge.sources.render_knowledge_preamble",
        lambda sources, max_chars, kind=None: f"{kind} body",
    )
    design, knowledge = builder._knowledge_sections(task, cfg=cfg)
    assert "Design system" in design
    assert "Project knowledge" in knowledge

    monkeypatch.setattr(
        "devcouncil.app.config.load_config",
        lambda root: SimpleNamespace(models=SimpleNamespace(provider="openrouter")),
    )
    assert cm is not None  # keep import used alongside monkeypatch target
    from devcouncil.execution.prompt_builder import _local_context_window_budget

    assert _local_context_window_budget(tmp_path) is None
    monkeypatch.setattr(
        "devcouncil.app.config.load_config",
        lambda root: SimpleNamespace(models=SimpleNamespace(provider="ollama")),
    )
    monkeypatch.setattr("devcouncil.llm.provider.OllamaProvider._resolve_num_ctx", lambda: 3000)
    assert _local_context_window_budget(tmp_path) == max(8000, (3000 - 1536) * 4)
    monkeypatch.setattr("devcouncil.llm.provider.OllamaProvider._resolve_num_ctx", lambda: 100)
    assert _local_context_window_budget(tmp_path) == 8000


def test_correction_manifest_ordering_truncation_and_build(monkeypatch, tmp_path):
    task = _task()
    checkpoint_dir = tmp_path / ".devcouncil" / "checkpoints"
    checkpoint_dir.mkdir(parents=True)
    (checkpoint_dir / "TASK-1-after.patch").write_text("diff --git a/secret b/secret\napi_key=abcdef1234567890\n" * 500, encoding="utf-8")
    out = tmp_path / "stdout.txt"
    out.write_text("line\n" * 2000 + "api_key=abcdef1234567890\n", encoding="utf-8")

    failed = [CommandResult(command="pytest", exit_code=1, stdout_path="stdout.txt", stderr_path="", summary="failed")]

    class FakeEvidenceRepo:
        def __init__(self, session):
            pass

        def get_command_results_for_task(self, task_id):
            return failed

    monkeypatch.setattr(cm, "get_db", lambda root: SimpleNamespace(get_session=lambda: _session()))
    monkeypatch.setattr(cm, "EvidenceRepository", FakeEvidenceRepo)
    config = SimpleNamespace(execution=SimpleNamespace(max_repair_attempts=4, default_executor="claude"))
    gaps = [
        _gap("orphan", severity="critical", gap_type="orphan_diff"),
        _gap("test", severity="critical", gap_type="test_failed"),
        _gap("low", severity="low", gap_type="test_failed"),
    ]

    manifest = cm.build_correction_manifest(tmp_path, task, gaps, prior_attempts=2, config=config)

    assert manifest.root_cause == "critical test_failed"
    assert manifest.ordered_blocking_gaps[:2] == ["critical test_failed", "critical orphan_diff"]
    assert manifest.prior_failed_attempts == 2
    assert manifest.retry_budget == 4
    assert manifest.executor_recommendation == "claude"
    assert "abcdef1234567890" not in manifest.prior_diff
    assert "output truncated" in manifest.failing_output
    assert "pytest (exit 1)" in manifest.failed_evidence[0]


@contextmanager
def _session():
    yield "session"


def test_correction_manifest_repair_service_and_write_load(monkeypatch, tmp_path):
    task = _task()
    incomplete = _gap(
        "AC",
        blocking=False,
        gap_type="acceptance_criteria_unproven",
        method="unit_test",
    )
    manual = _gap(
        "MANUAL",
        blocking=False,
        gap_type="acceptance_criteria_unproven",
        method="manual",
    )
    assert cm.remediable_incomplete_gaps([incomplete, manual]) == [incomplete]
    assert cm._union(["a", "b"], ["b", "c", ""]) == ["a", "b", "c"]
    assert cm._latest_agent_run(tmp_path, "TASK-1") is None

    run_dir = tmp_path / ".devcouncil" / "runs" / "run-1"
    run_dir.mkdir(parents=True)
    (run_dir / "agent-run.json").write_text('{"task_id": "TASK-1", "status": "failed"}', encoding="utf-8")
    assert cm._latest_agent_run(tmp_path, "TASK-1")["status"] == "failed"

    class FakeTaskRepo:
        def __init__(self, session):
            pass

        def get_by_id(self, task_id):
            return task if task_id == "TASK-1" else None

    class FakeGapRepo:
        def __init__(self, session):
            pass

        def get_blocking_for_task(self, task_id):
            return []

        def get_for_task(self, task_id):
            return [incomplete]

    class FakeRecord:
        attempt = 2
        manifest_path = ""

    saved = []

    class FakeManifestRepo:
        def __init__(self, session):
            pass

        def latest_for_task(self, task_id):
            return FakeRecord() if saved else None

        def save(self, *args, **kwargs):
            saved.append((args, kwargs))

    monkeypatch.setattr(cm, "get_db", lambda root: SimpleNamespace(get_session=lambda: _session()))
    monkeypatch.setattr(cm, "TaskRepository", FakeTaskRepo)
    monkeypatch.setattr(cm, "GapRepository", FakeGapRepo)
    monkeypatch.setattr(cm, "CorrectionManifestRepository", FakeManifestRepo)
    monkeypatch.setattr(cm, "EvidenceRepository", lambda session: SimpleNamespace(get_command_results_for_task=lambda task_id: []))
    monkeypatch.setattr(cm, "load_config", lambda root: SimpleNamespace(execution=SimpleNamespace(max_repair_attempts=3, default_executor="codex")))

    assert cm.write_correction_manifest(tmp_path, "MISSING") is None
    assert cm.write_correction_manifest(tmp_path, "TASK-1") is None
    path = cm.write_correction_manifest(tmp_path, "TASK-1", include_incomplete=True)
    assert path is not None and path.exists()
    assert saved

    class LoadManifestRepo(FakeManifestRepo):
        def latest_for_task(self, task_id):
            return SimpleNamespace(manifest_path=str(path))

    monkeypatch.setattr(cm, "CorrectionManifestRepository", LoadManifestRepo)
    loaded = cm.load_latest_correction_manifest(tmp_path, "TASK-1")
    assert loaded.task_id == "TASK-1"


def test_prompt_enhancement_helpers_and_service(monkeypatch, tmp_path):
    enhancement = PromptEnhancement(
        original_goal="old",
        enhanced_goal=" ",
        codebase_context=[" keep ", ""],
        debate_focus=[" focus "],
        constraints=[" constraint "],
        skills_brief="- skill",
        knowledge_brief="- knowledge",
    ).normalized("new")
    assert enhancement.enhanced_goal == "new"
    prompt = enhancement.debate_prompt()
    assert "Domain engineering intake" in prompt
    assert "Project knowledge" in prompt
    assert _clean_items([" a ", "", "b"]) == ["a", "b"]

    skills = [
        SimpleNamespace(name="s1", description="desc", body="body"),
        SimpleNamespace(name="s2", description="", body="x" * 9000),
    ]
    assert "Skill: s1" in _full_intake(skills)
    assert "s2" not in _full_intake(skills)
    assert _compact_brief(skills).splitlines() == ["- **s1** — desc", "- **s2**"]
    sources = [
        SimpleNamespace(kind="design", name="Design", description="tokens", body="body"),
        SimpleNamespace(kind="okf", name="OKF", description="", body="x" * 9000),
    ]
    assert "design: Design" in _knowledge_intake(sources)
    assert "OKF" not in _knowledge_intake(sources)
    assert _knowledge_brief(sources).splitlines() == ["- **Design** (design) — tokens", "- **OKF** (okf)"]

    save_active_prompt_enhancement(tmp_path, enhancement)
    assert load_latest_prompt_enhancement(tmp_path).enhanced_goal == "new"
    (tmp_path / ".devcouncil" / "active_prompt_enhancement.json").write_text("{bad", encoding="utf-8")
    run = tmp_path / ".devcouncil" / "runs" / "r1"
    run.mkdir(parents=True)
    (run / "prompt_enhancement.json").write_text(enhancement.model_dump_json(), encoding="utf-8")
    assert load_latest_prompt_enhancement(tmp_path).enhanced_goal == "new"

    class FakeRouter:
        async def complete_structured(self, **kwargs):
            assert kwargs["role"] == "prompt_enhancer"
            return PromptEnhancement(
                original_goal="ignored",
                enhanced_goal=" enhanced ",
                codebase_context=["ctx"],
                debate_focus=[],
                constraints=[],
            )

    monkeypatch.setattr("devcouncil.planning.prompt_enhancer_service._select_skills", lambda goal, root: skills[:1])
    monkeypatch.setattr("devcouncil.planning.prompt_enhancer_service._select_knowledge", lambda goal, root: sources[:1])
    result = asyncio_run(PromptEnhancerService(FakeRouter()).enhance_prompt("goal", "{}", project_root=tmp_path))
    assert result.applied_skills == ["s1"]
    assert result.applied_knowledge == ["Design"]


def asyncio_run(coro):
    import asyncio

    return asyncio.run(coro)


def test_prompt_enhancer_service_async(monkeypatch, tmp_path):
    skills = [SimpleNamespace(name="s1", description="desc", body="body")]
    sources = [SimpleNamespace(kind="design", name="Design", description="tokens", body="body")]

    class FakeRouter:
        async def complete_structured(self, **kwargs):
            assert "Applicable engineering skills" in kwargs["messages"][0]["content"]
            return PromptEnhancement(
                original_goal="ignored",
                enhanced_goal=" enhanced ",
                codebase_context=["ctx"],
                debate_focus=["focus"],
                constraints=["constraint"],
            )

    monkeypatch.setattr("devcouncil.planning.prompt_enhancer_service._select_skills", lambda goal, root: skills)
    monkeypatch.setattr("devcouncil.planning.prompt_enhancer_service._select_knowledge", lambda goal, root: sources)
    result = asyncio_run(PromptEnhancerService(FakeRouter()).enhance_prompt("goal", "{}", project_root=tmp_path))
    assert result.enhanced_goal == "enhanced"
    assert result.applied_skills == ["s1"]
    assert result.applied_knowledge == ["Design"]


def test_repair_service_generates_plan():
    task = _task()

    class FakeRouter:
        async def complete_structured(self, **kwargs):
            assert kwargs["role"] == "planner_a"
            assert "blocking gaps" in kwargs["messages"][0]["content"]
            return RepairOutput(suggested_tasks=[task])

    output = asyncio_run(RepairService(FakeRouter()).generate_repair_plan([_gap("G")], "context"))
    assert output.suggested_tasks == [task]

