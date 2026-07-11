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
    assert "new file (no importers yet)" in prompt


def test_no_impact_block_without_repo_map(tmp_path):
    src = tmp_path / "src"
    src.mkdir()
    (src / "a.py").write_text("x = 1\n", encoding="utf-8")
    task = _task([PlannedFile(path="src/a.py", reason="x", allowed_change="modify")])

    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])

    assert "Impact (changing X touches Y)" not in prompt
