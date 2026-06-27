"""Knowledge/design context injection into the task prompt — and its absence is a no-op."""

import subprocess

from devcouncil.cli.commands.init import initialize_project
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution.prompt_builder import PromptBuilder


def _task_and_req():
    task = Task(
        id="TASK-001", title="Add billing summary", description="Show a billing summary widget.",
        requirement_ids=["REQ-001"],
        planned_files=[PlannedFile(path="ui.py", reason="widget", allowed_change="create")],
    )
    req = Requirement(
        id="REQ-001", title="Billing summary", description="x", priority="high", source="user",
        acceptance_criteria=[AcceptanceCriterion(id="AC-1", description="renders", verification_method="unit_test")],
    )
    return task, req


def _init(tmp_path):
    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    initialize_project(tmp_path, quiet=True)


def test_design_and_okf_sections_injected(tmp_path):
    _init(tmp_path)
    kdir = tmp_path / ".devcouncil" / "knowledge"
    (kdir / "design").mkdir(parents=True)
    (kdir / "okf").mkdir(parents=True)
    (kdir / "design" / "design.md").write_text(
        '---\nname: Acme\ncolors:\n  primary: "#1a1a1a"\n---\n# Overview\nUse Acme tokens.', encoding="utf-8"
    )
    (kdir / "okf" / "billing.md").write_text(
        "---\ntype: Table\ntitle: Invoices\ntags: [billing]\n---\nInvoices are immutable once issued.",
        encoding="utf-8",
    )
    task, req = _task_and_req()
    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [req])
    assert "Design system" in prompt
    assert "Use Acme tokens" in prompt          # design always-on
    assert "Project knowledge" in prompt
    assert "immutable once issued" in prompt     # OKF matched on its 'billing' tag


def test_no_knowledge_is_a_no_op(tmp_path):
    _init(tmp_path)  # no knowledge dir created
    task, req = _task_and_req()
    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [req])
    assert "Design system" not in prompt
    assert "Project knowledge (Open Knowledge Format)" not in prompt


def test_disabled_via_config_skips_injection(tmp_path):
    _init(tmp_path)
    kdir = tmp_path / ".devcouncil" / "knowledge" / "design"
    kdir.mkdir(parents=True)
    (kdir / "design.md").write_text("---\nname: Acme\n---\nUse Acme tokens.", encoding="utf-8")
    # Turn the feature off in config.
    cfg_path = tmp_path / ".devcouncil" / "config.yaml"
    cfg_path.write_text(
        cfg_path.read_text(encoding="utf-8") + "\nknowledge:\n  enabled: false\n", encoding="utf-8"
    )
    task, req = _task_and_req()
    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [req])
    assert "Design system" not in prompt
