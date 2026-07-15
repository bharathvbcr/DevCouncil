"""The always-on 'changing X touches Y' impact block in the task prompt."""

import json

from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution.prompt_builder import PromptBuilder


def _task(planned):
    return Task(id="TASK-IMP", title="T", description="D", planned_files=planned)


def _write_map(tmp_path, data):
    (tmp_path / ".devcouncil").mkdir(exist_ok=True)
    (tmp_path / ".devcouncil" / "repo_map.json").write_text(json.dumps(data), encoding="utf-8")


def test_impact_block_lists_dependents_and_neighbors(tmp_path):
    src = tmp_path / "src" / "payments"
    src.mkdir(parents=True)
    (src / "gateway.py").write_text("def charge():\n    return 1\n", encoding="utf-8")
    _write_map(tmp_path, {
        "files": [{"path": "src/payments/gateway.py", "area": "src/payments"}],
        "subsystems": [
            {"area": "src/payments", "summary": "payments", "neighbors": ["src/billing"]},
        ],
        "dependents": {
            "src/payments/gateway.py": ["src/api/checkout.py", "src/api/refund.py"],
        },
    })
    task = _task([PlannedFile(path="src/payments/gateway.py", reason="x", allowed_change="modify")])

    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])

    assert "Impact (changing X touches Y)" in prompt
    assert "imported by 2 file(s)" in prompt
    assert "src/api/checkout.py" in prompt
    assert "subsystem `src/payments`" in prompt
    assert "neighbors: `src/billing`" in prompt


def test_impact_block_present_even_without_code_review_graph(tmp_path):
    # No graph CLI configured (default). The impact block must still appear from the map.
    src = tmp_path / "src" / "core"
    src.mkdir(parents=True)
    (src / "engine.py").write_text("def run():\n    return 1\n", encoding="utf-8")
    _write_map(tmp_path, {
        "files": [{"path": "src/core/engine.py", "area": "src/core"}],
        "subsystems": [{"area": "src/core", "neighbors": []}],
        "dependents": {"src/core/engine.py": ["src/cli/main.py"]},
    })
    task = _task([PlannedFile(path="src/core/engine.py", reason="x", allowed_change="modify")])

    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])

    assert "Impact (changing X touches Y)" in prompt
    assert "src/cli/main.py" in prompt


def test_impact_block_marks_new_file(tmp_path):
    _write_map(tmp_path, {
        "files": [],
        "subsystems": [{"area": "src/newpkg", "neighbors": ["src/core"]}],
        "dependents": {},
    })
    task = _task([PlannedFile(path="src/newpkg/thing.py", reason="x", allowed_change="create")])

    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])

    assert "Impact (changing X touches Y)" in prompt
    assert "new file — plan its importer before finishing" in prompt


def test_no_impact_block_without_repo_map(tmp_path):
    src = tmp_path / "src"
    src.mkdir()
    (src / "a.py").write_text("x = 1\n", encoding="utf-8")
    task = _task([PlannedFile(path="src/a.py", reason="x", allowed_change="modify")])

    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])

    assert "Impact (changing X touches Y)" not in prompt


# ---- local context window budget ----------------------------------------------

def test_local_context_window_budget_ollama(tmp_path):
    from types import SimpleNamespace
    from unittest.mock import patch

    from devcouncil.execution import prompt_builder as pb

    cfg = SimpleNamespace(models=SimpleNamespace(provider="ollama"))
    with patch("devcouncil.llm.provider.OllamaProvider._resolve_num_ctx", return_value=8192):
        budget = pb._local_context_window_budget(tmp_path, cfg=cfg)
    assert budget == max(pb._MIN_PROMPT_CHARS, (8192 - pb._RESERVED_COMPLETION_TOKENS) * pb._CHARS_PER_TOKEN)


def test_local_context_window_budget_tiny_window_clamps(tmp_path):
    from types import SimpleNamespace
    from unittest.mock import patch

    from devcouncil.execution import prompt_builder as pb

    cfg = SimpleNamespace(models=SimpleNamespace(provider="ollama-local"))
    with patch("devcouncil.llm.provider.OllamaProvider._resolve_num_ctx", return_value=100):
        budget = pb._local_context_window_budget(tmp_path, cfg=cfg)
    assert budget == pb._MIN_PROMPT_CHARS


def test_local_context_window_budget_zero_optout(tmp_path):
    from types import SimpleNamespace
    from unittest.mock import patch

    from devcouncil.execution import prompt_builder as pb

    cfg = SimpleNamespace(models=SimpleNamespace(provider="ollama"))
    with patch("devcouncil.llm.provider.OllamaProvider._resolve_num_ctx", return_value=0):
        assert pb._local_context_window_budget(tmp_path, cfg=cfg) is None


def test_local_context_window_budget_non_ollama_and_error(tmp_path):
    from types import SimpleNamespace

    from devcouncil.execution import prompt_builder as pb

    cloud = SimpleNamespace(models=SimpleNamespace(provider="openai"))
    assert pb._local_context_window_budget(tmp_path, cfg=cloud) is None
    # A cfg that raises on attribute access degrades to None.
    assert pb._local_context_window_budget(tmp_path, cfg=object()) is None


def test_build_task_prompt_caps_to_window_budget(tmp_path):
    from unittest.mock import patch

    task = _task([PlannedFile(path="src/a.py", reason="x", allowed_change="modify")])
    with patch(
        "devcouncil.execution.prompt_builder._local_context_window_budget", return_value=8000
    ):
        prompt = PromptBuilder(tmp_path).build_task_prompt(task, [], max_chars=60000)
    assert isinstance(prompt, str) and prompt


# ---- python symbol outline ----------------------------------------------------

def test_python_symbol_outline_captures_decorators_and_async():
    pb = PromptBuilder()
    text = (
        "import os\n"
        "async def top():\n    pass\n"
        "class Widget:\n"
        "    @property\n    def name(self):\n        return 1\n"
        "    @staticmethod\n    def make():\n        return 2\n"
        "    @classmethod\n    def build(cls):\n        return 3\n"
        "    async def load(self, x):\n        return x\n"
    )
    out = pb._python_symbol_outline(text)
    joined = "\n".join(out)
    assert "async def top()" in joined
    assert "class Widget" in joined
    assert "@property" in joined and "@staticmethod" in joined and "@classmethod" in joined
    assert "async def load" in joined


def test_python_symbol_outline_syntax_error_returns_empty():
    pb = PromptBuilder()
    assert pb._python_symbol_outline("def broken(:\n") == []


# ---- planned files section edge cases -----------------------------------------

def test_planned_files_section_new_and_error(tmp_path, monkeypatch):
    pb = PromptBuilder(tmp_path)
    pb._outline_cache = {}
    real = tmp_path / "exists.py"
    real.write_text("def f():\n    return 1\n", encoding="utf-8")
    task = _task(
        [
            PlannedFile(path="missing.py", reason="x", allowed_change="create"),
            PlannedFile(path="exists.py", reason="x", allowed_change="modify"),
        ]
    )
    # Force a read error on the existing file.
    orig_read = type(real).read_text

    def flaky(self, *a, **k):
        if self.name == "exists.py":
            raise OSError("cannot read")
        return orig_read(self, *a, **k)

    monkeypatch.setattr("pathlib.Path.read_text", flaky)
    section = pb._planned_files_section(task)
    assert "new file (does not exist yet)" in section
    assert "[error reading file]" in section


def test_planned_files_section_budget_omits(tmp_path, monkeypatch):
    import devcouncil.execution.prompt_builder as pbmod

    monkeypatch.setattr(pbmod, "MAX_FILE_CONTEXT_CHARS", 5)
    pb = PromptBuilder(tmp_path)
    pb._outline_cache = {}
    for i in range(3):
        (tmp_path / f"f{i}.py").write_text("x" * 50, encoding="utf-8")
    task = _task([PlannedFile(path=f"f{i}.py", reason="x", allowed_change="modify") for i in range(3)])
    section = pb._planned_files_section(task)
    assert "omitted to fit the context budget" in section


# ---- best-effort loaders degrade gracefully -----------------------------------

def test_loaders_swallow_errors(tmp_path, monkeypatch):
    pb = PromptBuilder(tmp_path)
    monkeypatch.setattr(
        "devcouncil.planning.prompt_enhancer_service.load_latest_prompt_enhancement",
        lambda root: (_ for _ in ()).throw(RuntimeError("boom")),
    )
    assert pb._load_prompt_enhancement() is None
    # _load_repo_map: invalid JSON -> None
    (tmp_path / ".devcouncil").mkdir(exist_ok=True)
    (tmp_path / ".devcouncil" / "repo_map.json").write_text("not json", encoding="utf-8")
    assert pb._load_repo_map() is None
    # _repo_map_stale swallows RepoMapper failures.
    monkeypatch.setattr(
        "devcouncil.indexing.repo_mapper.RepoMapper.map_is_stale",
        lambda self, data: (_ for _ in ()).throw(RuntimeError("boom")),
    )
    assert pb._repo_map_stale({"files": []}) is False


def test_skills_section_swallows_errors(tmp_path, monkeypatch):
    pb = PromptBuilder(tmp_path)
    monkeypatch.setattr(
        "devcouncil.skills.registry.select_skills",
        lambda **k: (_ for _ in ()).throw(RuntimeError("boom")),
    )
    assert pb._skills_section(_task([])) == ""


# ---- graph impact lines -------------------------------------------------------

def test_graph_impact_lines_from_code_graph(tmp_path, monkeypatch):
    pb = PromptBuilder(tmp_path)
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.load_code_graph", lambda root: object()
    )
    monkeypatch.setattr(
        "devcouncil.indexing.graph.intel.diff_impact",
        lambda *a, **k: {
            "paths": [
                {
                    "path": "src/a.py",
                    "blast": {"layers": [{"depth": 1, "nodes": ["caller_one", "caller_two"]}]},
                },
                {"path": "src/b.py", "blast": {"layers": [{"depth": 1, "nodes": []}]}},
            ]
        },
    )
    task = _task([PlannedFile(path="src/a.py", reason="x", allowed_change="modify")])
    lines = pb._graph_impact_lines(task)
    assert any("caller_one" in ln for ln in lines)


def test_graph_impact_lines_no_graph(tmp_path, monkeypatch):
    pb = PromptBuilder(tmp_path)
    monkeypatch.setattr("devcouncil.indexing.graph.build.load_code_graph", lambda root: None)
    task = _task([PlannedFile(path="src/a.py", reason="x", allowed_change="modify")])
    assert pb._graph_impact_lines(task) == []


def test_graph_impact_lines_swallows_error(tmp_path, monkeypatch):
    pb = PromptBuilder(tmp_path)
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.load_code_graph",
        lambda root: (_ for _ in ()).throw(RuntimeError("boom")),
    )
    task = _task([PlannedFile(path="src/a.py", reason="x", allowed_change="modify")])
    assert pb._graph_impact_lines(task) == []


# ---- liveness debt section ----------------------------------------------------

def test_liveness_debt_section_lists_all_kinds(tmp_path):
    data = {
        "files": [
            {"path": "src/core/a.py", "area": "src/core"},
        ],
        "subsystems": [{"area": "src/core"}],
        "entry_roots": ["src/core/a.py"],
        "unwired_candidates": ["src/core/orphan.py"],
        "unreachable_files": ["src/core/lost.py"],
        "dead_symbol_candidates": ["src/core/a.py:10 unused_fn"],
    }
    task = _task([PlannedFile(path="src/core/a.py", reason="x", allowed_change="modify")])
    section = PromptBuilder(tmp_path)._liveness_debt_section(task, data)
    assert "Nearby liveness debt" in section
    assert "unwired: `src/core/orphan.py`" in section
    assert "unreachable: `src/core/lost.py`" in section
    assert "dead symbol: `src/core/a.py:10 unused_fn`" in section


def test_liveness_debt_section_no_areas_or_hits(tmp_path):
    pb = PromptBuilder(tmp_path)
    assert pb._liveness_debt_section(_task([]), None) == ""
    # Data present but planned files in an unknown area -> no task_areas.
    data = {"files": [], "subsystems": [], "unwired_candidates": ["x.py"]}
    task = _task([PlannedFile(path="unknown/z.py", reason="x", allowed_change="modify")])
    assert pb._liveness_debt_section(task, data) == ""


def test_liveness_debt_section_swallows_error(tmp_path, monkeypatch):
    pb = PromptBuilder(tmp_path)
    monkeypatch.setattr(
        "devcouncil.indexing.subsystem_map.areas_touched",
        lambda *a, **k: (_ for _ in ()).throw(RuntimeError("boom")),
    )
    task = _task([PlannedFile(path="src/core/a.py", reason="x", allowed_change="modify")])
    assert pb._liveness_debt_section(task, {"files": []}) == ""


# ---- dependency risks section -------------------------------------------------

def test_dependency_risks_section_renders(tmp_path):
    data = {
        "dependency_risks": [
            {
                "package": "requests",
                "installed_version": "2.0.0",
                "severity": "high",
                "advisory_id": "CVE-1",
                "summary": "bad thing",
            },
            "not-a-dict",
        ]
    }
    section = PromptBuilder(tmp_path)._dependency_risks_section(data)
    assert "Dependency risks" in section
    assert "`requests` 2.0.0" in section
    assert "[high] CVE-1 — bad thing" in section


def test_dependency_risks_section_more_marker(tmp_path):
    risks = [{"package": f"p{i}"} for i in range(15)]
    section = PromptBuilder(tmp_path)._dependency_risks_section({"dependency_risks": risks})
    assert "more — see" in section


def test_dependency_risks_section_empty(tmp_path):
    assert PromptBuilder(tmp_path)._dependency_risks_section(None) == ""
    assert PromptBuilder(tmp_path)._dependency_risks_section({"dependency_risks": []}) == ""


# ---- call sites section -------------------------------------------------------

def test_call_sites_section_emits_using_lines(tmp_path):
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "lib.py").write_text("def helper():\n    return 1\n", encoding="utf-8")
    (tmp_path / "src" / "user.py").write_text(
        "from lib import helper\n\nvalue = helper()\n", encoding="utf-8"
    )
    data = {"dependents": {"src/lib.py": ["src/user.py"]}}
    pb = PromptBuilder(tmp_path)
    pb._outline_cache = {}
    task = _task([PlannedFile(path="src/lib.py", reason="x", allowed_change="modify")])
    section = pb._call_sites_section(task, data)
    assert "Call sites (where your symbols are used)" in section
    assert "src/user.py:" in section and "helper" in section


def test_references_symbol_whole_word():
    assert PromptBuilder._references_symbol("x = helper()", "helper") is True
    assert PromptBuilder._references_symbol("x = helperness()", "helper") is False
    assert PromptBuilder._references_symbol("no match here", "helper") is False


# ---- build_task_prompt core branches ------------------------------------------

def test_build_task_prompt_forbidden_and_enhancement_and_rigor(tmp_path, monkeypatch):
    from types import SimpleNamespace

    task = Task(
        id="TASK-HARD",
        title="Hard task",
        description="Do it",
        planned_files=[PlannedFile(path="src/a.py", reason="x", allowed_change="modify")],
        forbidden_changes=["src/secret.py"],
        expected_tests=["pytest tests/"],
        allowed_commands=["pytest"],
    )
    pb = PromptBuilder(tmp_path)
    monkeypatch.setattr(
        pb,
        "_load_prompt_enhancement",
        lambda: SimpleNamespace(constraints=["no eval"], applied_skills=["python"]),
    )
    monkeypatch.setattr(
        "devcouncil.verification.difficulty.estimate_difficulty", lambda t, r: "hard"
    )
    prompt = pb.build_task_prompt(task, [])
    assert "Forbidden changes" in prompt and "src/secret.py" in prompt
    assert "Codebase-specific constraints" in prompt and "no eval" in prompt
    assert "task is classified HARD" in prompt


def test_build_task_prompt_rigor_section_swallows_error(tmp_path, monkeypatch):
    task = _task([PlannedFile(path="src/a.py", reason="x", allowed_change="modify")])
    monkeypatch.setattr(
        "devcouncil.verification.difficulty.estimate_difficulty",
        lambda t, r: (_ for _ in ()).throw(RuntimeError("boom")),
    )
    # Should not raise even though difficulty estimation blows up.
    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])
    assert isinstance(prompt, str) and prompt
