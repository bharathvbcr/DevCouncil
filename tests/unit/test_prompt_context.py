"""Batch C — the task prompt carries real context, not just file paths."""

import json

from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution.prompt_builder import PromptBuilder


def _task(planned):
    return Task(id="TASK-001", title="T", description="D", planned_files=planned)


def test_prompt_injects_existing_file_contents_and_symbols(tmp_path):
    src = tmp_path / "src"
    src.mkdir()
    (src / "auth.py").write_text(
        "def login(user, password):\n    return True\n\n\nclass Session:\n    def open(self):\n        ...\n",
        encoding="utf-8",
    )
    task = _task([PlannedFile(path="src/auth.py", reason="logic", allowed_change="modify")])

    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])

    assert "Current file contents" in prompt
    assert "def login(user, password):" in prompt          # real body
    assert "def login(user, password) L1" in prompt          # symbol outline
    assert "class Session L5" in prompt
    assert "```python" in prompt


def test_prompt_marks_new_files_without_body(tmp_path):
    task = _task([PlannedFile(path="src/new_module.py", reason="create it", allowed_change="create")])

    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])

    assert "new file (does not exist yet)" in prompt
    assert "```python" not in prompt  # nothing to fence for a non-existent file


def test_prompt_file_contents_respect_per_file_budget(tmp_path):
    src = tmp_path / "src"
    src.mkdir()
    big = "x = 1  # padding line\n" * 5000  # well over the per-file cap
    (src / "big.py").write_text(big, encoding="utf-8")
    task = _task([PlannedFile(path="src/big.py", reason="logic", allowed_change="modify")])

    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])

    assert "truncated to" in prompt
    # The injected body must not exceed the per-file cap (plus small framing overhead).
    assert len(prompt) < len(big)


def test_prompt_redacts_secrets_in_injected_contents(tmp_path):
    src = tmp_path / "src"
    src.mkdir()
    (src / "config.py").write_text(
        'api_key="abcd1234efgh5678ijkl9012mnop3456"\n',
        encoding="utf-8",
    )
    task = _task([PlannedFile(path="src/config.py", reason="logic", allowed_change="modify")])

    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])

    assert "abcd1234efgh5678ijkl9012mnop3456" not in prompt
    assert "REDACTED" in prompt


def test_repo_map_fallback_adds_structural_context(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    repo_map = {
        "files": [
            {"path": "src/payments/gateway.py", "summary": "charge cards"},
            {"path": "src/payments/models.py", "summary": "payment models"},
        ],
        "subsystems": [
            {
                "area": "src/payments",
                "summary": "Payment processing and gateways.",
                "critical_files": ["src/payments/gateway.py", "src/payments/models.py"],
                "neighbors": ["src/billing"],
                "handoff_paths": ["payments/gateway.py -> billing/invoice.py"],
            }
        ],
    }
    (tmp_path / ".devcouncil" / "repo_map.json").write_text(json.dumps(repo_map), encoding="utf-8")
    task = _task([PlannedFile(path="src/payments/gateway.py", reason="x", allowed_change="modify")])

    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])

    assert "Repo map (structural context)" in prompt
    assert "Payment processing and gateways." in prompt
    assert "src/payments/models.py" in prompt          # nearby key file
    assert "payment models" in prompt                   # its summary
    assert "src/billing" in prompt                      # neighbor


def test_repo_map_fallback_absent_without_map(tmp_path):
    task = _task([PlannedFile(path="src/x.py", reason="x", allowed_change="modify")])
    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])
    assert "Repo map (structural context)" not in prompt


def test_dependents_section_lists_importers(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    repo_map = {
        "dependents": {
            "src/core/models.py": ["src/api/handlers.py", "src/main.py"],
        }
    }
    (tmp_path / ".devcouncil" / "repo_map.json").write_text(json.dumps(repo_map), encoding="utf-8")
    task = _task([PlannedFile(path="src/core/models.py", reason="x", allowed_change="modify")])

    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])

    assert "Dependents (blast radius)" in prompt
    assert "is imported by:" in prompt
    assert "src/api/handlers.py" in prompt
    assert "src/main.py" in prompt


def test_dependents_section_skipped_for_created_files(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    repo_map = {"dependents": {"src/new.py": ["src/other.py"]}}
    (tmp_path / ".devcouncil" / "repo_map.json").write_text(json.dumps(repo_map), encoding="utf-8")
    # A 'create' file has no blast radius — even if the (stale) map lists dependents.
    task = _task([PlannedFile(path="src/new.py", reason="x", allowed_change="create")])

    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])

    assert "Dependents (blast radius)" not in prompt


def test_prompt_warns_when_repo_map_is_stale(tmp_path, monkeypatch):
    (tmp_path / ".devcouncil").mkdir()
    repo_map = {
        "dependents": {"src/core/models.py": ["src/api/handlers.py"]},
        "generated_head": "deadbeef",
        "indexed_hash": "oldhash",
    }
    (tmp_path / ".devcouncil" / "repo_map.json").write_text(json.dumps(repo_map), encoding="utf-8")
    # Force the staleness check to report stale regardless of git state.
    monkeypatch.setattr(
        "devcouncil.indexing.repo_mapper.RepoMapper.map_is_stale", lambda self, data: True
    )
    task = _task([PlannedFile(path="src/core/models.py", reason="x", allowed_change="modify")])

    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])

    assert "repo map is behind" in prompt.lower()
    # And the note appears exactly once even though dependents are present.
    assert prompt.lower().count("repo map is behind") == 1


def _repo_with_file_and_deps(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    src = tmp_path / "src"
    src.mkdir()
    (src / "a.py").write_text("def f():\n    return 1\n" * 10, encoding="utf-8")
    repo_map = {"dependents": {"src/a.py": ["src/b.py", "src/c.py", "src/d.py"]}}
    (tmp_path / ".devcouncil" / "repo_map.json").write_text(json.dumps(repo_map), encoding="utf-8")
    return _task([PlannedFile(path="src/a.py", reason="logic", allowed_change="modify")])


def test_central_budget_drops_lowest_priority_first(tmp_path):
    task = _repo_with_file_and_deps(tmp_path)
    pb = PromptBuilder(tmp_path)
    files_text = pb._planned_files_section(task)
    deps_text = pb._dependents_section(task, pb._load_repo_map())
    skills_text = pb._skills_section(task)  # always-on core skill is also a segment
    full = pb.build_task_prompt(task, [], max_chars=10 ** 9)
    overhead = len(full) - len(files_text) - len(deps_text) - len(skills_text)  # core + instructions

    # Budget fits only the higher-priority file contents, not dependents or skills.
    budget = overhead + len(files_text) + 5
    out = pb.build_task_prompt(task, [], max_chars=budget)

    assert "Current file contents" in out          # priority 1 kept
    assert "Dependents (blast radius)" not in out   # lower priority dropped
    assert "omitted: dependents" in out             # explicit, not silent


def test_central_budget_keeps_core_and_marks_omissions(tmp_path):
    task = _repo_with_file_and_deps(tmp_path)
    pb = PromptBuilder(tmp_path)
    out = pb.build_task_prompt(task, [], max_chars=1)  # tiny: all optional dropped

    assert "# Implement TASK-001" in out            # core always kept
    assert "## Instructions" in out                 # instructions always kept
    assert "## Allowed files" in out
    assert "Current file contents" not in out
    assert "Context budget reached" in out


def test_prompt_carries_enhancer_constraints_to_executor(tmp_path):
    # The planning prompt-enhancer's codebase-specific constraints must reach the executor
    # prompt, not just the planning debate.
    run_dir = tmp_path / ".devcouncil" / "runs" / "run-1"
    run_dir.mkdir(parents=True)
    (run_dir / "prompt_enhancement.json").write_text(json.dumps({
        "original_goal": "rpn eval",
        "enhanced_goal": "implement an RPN evaluator",
        "constraints": ["division truncates toward zero", "must not use eval()"],
        "applied_skills": ["python-numerics"],
    }), encoding="utf-8")

    task = _task([PlannedFile(path="rpn.py", reason="impl", allowed_change="create")])
    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])

    assert "Codebase-specific constraints" in prompt
    assert "division truncates toward zero" in prompt
    assert "must not use eval()" in prompt
    assert "python-numerics" in prompt


def test_prompt_has_no_constraints_section_without_enhancement(tmp_path):
    task = _task([PlannedFile(path="rpn.py", reason="impl", allowed_change="create")])
    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])
    assert "Codebase-specific constraints" not in prompt


def test_prompt_enhancement_prefers_active_plan_over_latest_run(tmp_path):
    from devcouncil.planning.prompt_enhancer_service import load_latest_prompt_enhancement
    dc = tmp_path / ".devcouncil"
    run = dc / "runs" / "run-1"
    run.mkdir(parents=True)
    (run / "prompt_enhancement.json").write_text(
        json.dumps({"original_goal": "g", "enhanced_goal": "g", "constraints": ["OLD run constraint"]}),
        encoding="utf-8")
    (dc / "active_prompt_enhancement.json").write_text(
        json.dumps({"original_goal": "g", "enhanced_goal": "g", "constraints": ["ACTIVE constraint"]}),
        encoding="utf-8")
    enh = load_latest_prompt_enhancement(tmp_path)
    assert enh is not None and enh.constraints == ["ACTIVE constraint"]


def test_prompt_enhancement_falls_back_to_run_artifact(tmp_path):
    from devcouncil.planning.prompt_enhancer_service import load_latest_prompt_enhancement
    run = tmp_path / ".devcouncil" / "runs" / "run-1"
    run.mkdir(parents=True)
    (run / "prompt_enhancement.json").write_text(
        json.dumps({"original_goal": "g", "enhanced_goal": "g", "constraints": ["RUN constraint"]}),
        encoding="utf-8")
    enh = load_latest_prompt_enhancement(tmp_path)
    assert enh is not None and enh.constraints == ["RUN constraint"]


def test_prompt_enhancement_loader_safe_on_malformed_json(tmp_path):
    from devcouncil.planning.prompt_enhancer_service import load_latest_prompt_enhancement
    dc = tmp_path / ".devcouncil"
    dc.mkdir(parents=True)
    (dc / "active_prompt_enhancement.json").write_text("{not json", encoding="utf-8")
    assert load_latest_prompt_enhancement(tmp_path) is None  # malformed -> None, never raises


def test_prompt_injects_rigor_section_for_hard_tasks(tmp_path):
    task = Task(
        id="TASK-HARD",
        title="Refactor auth",
        description="Cross-cutting migration",
        difficulty="hard",
        planned_files=[PlannedFile(path="src/auth.py", reason="edit", allowed_change="modify")],
    )
    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])
    assert "classified HARD" in prompt
    assert "devcouncil: allow-stub" in prompt


def test_prompt_omits_rigor_section_for_easy_tasks(tmp_path):
    task = Task(
        id="TASK-EASY",
        title="Small fix",
        description="One-line change",
        difficulty="easy",
        planned_files=[PlannedFile(path="src/a.py", reason="edit", allowed_change="modify")],
    )
    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])
    assert "classified HARD" not in prompt
