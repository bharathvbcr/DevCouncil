"""Tests for bounding how much skill text rides inline in prompts."""

import time

from devcouncil.domain.task import Task
from devcouncil.execution.prompt_builder import PromptBuilder
from devcouncil.skills import registry as R
from devcouncil.skills.registry import bound_skills, load_skills, select_skills


def test_bound_skills_caps_inline_and_defers_rest():
    skills = load_skills()
    inline, deferred = bound_skills(skills, max_skills=3, max_chars=100_000)
    assert len(inline) == 3
    # The always-on core skill is always kept inline (it sorts first).
    assert inline[0].name == "core-engineering"
    # Every selected skill is accounted for as either inline or deferred (no loss).
    assert len(inline) + len(deferred) == len(skills)


def test_bound_skills_keeps_core_even_when_over_char_budget():
    inline, _ = bound_skills(load_skills(), max_chars=1)
    # A tiny char budget still yields the first (core) skill inline, never empty.
    assert inline and inline[0].name == "core-engineering"


def test_every_packaged_skill_is_well_formed():
    """A contributor adding a broken skill (no description/body, or no way to be
    selected) should fail here rather than silently never applying."""
    for skill in load_skills():
        assert skill.description, f"{skill.name}: missing description"
        assert skill.body.strip(), f"{skill.name}: empty body"
        selectable = skill.always or skill.triggers.keywords or skill.triggers.globs
        assert selectable, f"{skill.name}: not always-on and has no triggers -> never selects"


def test_repo_basename_scan_is_cached_and_invalidates(tmp_path, monkeypatch):
    (tmp_path / "build.gradle").write_text("x", encoding="utf-8")
    R.clear_skill_caches()

    walks = {"n": 0}
    original = R._walk_repo_basenames

    def counting(root):
        walks["n"] += 1
        return original(root)

    monkeypatch.setattr(R, "_walk_repo_basenames", counting)

    first = {s.name for s in select_skills(goal="", project_root=tmp_path)}
    second = {s.name for s in select_skills(goal="", project_root=tmp_path)}
    assert "android" in first and first == second
    assert walks["n"] == 1  # second call served from cache

    # Adding a top-level marker bumps the root mtime and invalidates the cache.
    time.sleep(0.01)
    (tmp_path / "package.json").write_text("{}", encoding="utf-8")
    third = {s.name for s in select_skills(goal="", project_root=tmp_path)}
    assert "web" in third
    assert walks["n"] == 2
    R.clear_skill_caches()


def test_prompt_builder_defers_skills_beyond_cap():
    task = Task(
        id="T",
        title="api server with android client, web dashboard, ios app, docker, kafka",
        description="grpc rest postgres redis jetpack compose swiftui react vite",
        planned_files=[],
        expected_tests=[],
        allowed_commands=[],
    )
    prompt = PromptBuilder().build_task_prompt(task, [])
    assert "## Engineering skills" in prompt
    # When more than the cap apply, the overflow is pointed at the scaffolded files.
    assert "Also applicable" in prompt
