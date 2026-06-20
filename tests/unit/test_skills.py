from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.skills.registry import (
    load_skills,
    render_preamble,
    scaffold_skills,
    select_skills,
)

runner = CliRunner()


def test_library_loads_core_and_domains():
    names = {s.name for s in load_skills()}
    assert {"core-engineering", "android", "ios", "windows", "web", "ai-training"} <= names
    always = [s.name for s in load_skills() if s.always]
    assert always == ["core-engineering"]
    # The contributor README has no skill frontmatter and must not load as a skill.
    assert "README" not in names and "readme" not in {n.lower() for n in names}


def test_core_skill_merges_both_sources():
    core = next(s for s in load_skills() if s.name == "core-engineering")
    body = core.body.lower()
    # Karpathy principles
    assert "think before coding" in body
    assert "simplicity first" in body
    assert "surgical changes" in body
    assert "goal-driven execution" in body
    # Fable 5 communication/honesty themes
    assert "lead with the outcome" in body
    assert "evidence" in body


def test_select_always_includes_core_for_empty_goal():
    selected = select_skills(goal="")
    assert [s.name for s in selected] == ["core-engineering"]


def test_select_matches_domain_by_keyword():
    selected = {s.name for s in select_skills(goal="Build an Android app with Jetpack Compose")}
    assert "android" in selected
    assert "core-engineering" in selected
    assert "ios" not in selected


def test_select_matches_domain_by_repo_file(tmp_path):
    (tmp_path / "Package.swift").write_text("// swift package", encoding="utf-8")
    selected = {s.name for s in select_skills(goal="", project_root=tmp_path)}
    assert "ios" in selected
    assert "android" not in selected


def test_scaffold_writes_and_is_idempotent(tmp_path):
    chosen = select_skills(goal="react website")
    written = scaffold_skills(tmp_path, chosen)
    paths = {p.relative_to(tmp_path).as_posix() for p in written}
    assert ".claude/skills/core-engineering/SKILL.md" in paths
    assert ".claude/skills/web/SKILL.md" in paths
    # Re-running writes nothing new.
    assert scaffold_skills(tmp_path, chosen) == []
    content = (tmp_path / ".claude" / "skills" / "web" / "SKILL.md").read_text(encoding="utf-8")
    assert content.startswith("---\nname: web\n")


def test_render_preamble_concatenates_bodies():
    preamble = render_preamble(select_skills(goal="android"))
    assert "Core Engineering Discipline" in preamble
    assert "Android App Development Intake" in preamble


def test_cli_skills_list_runs():
    result = runner.invoke(app, ["skills"])
    assert result.exit_code == 0
    assert "core-engineering" in result.output


def test_cli_skills_show_unknown_errors():
    result = runner.invoke(app, ["skills", "show", "does-not-exist"])
    assert result.exit_code == 1


def test_prompt_builder_injects_applicable_skills():
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.execution.prompt_builder import PromptBuilder

    android = Task(
        id="T1",
        title="Build Android login screen",
        description="Use Jetpack Compose",
        planned_files=[PlannedFile(path="app/Main.kt", reason="ui", allowed_change="modify")],
        expected_tests=["gradlew test"],
        allowed_commands=["gradlew test"],
    )
    prompt = PromptBuilder().build_task_prompt(android, [])
    assert "## Engineering skills" in prompt
    # The "Applicable skills:" header names exactly the selected skills...
    applicable = next(line for line in prompt.splitlines() if "Applicable skills:" in line)
    assert "core-engineering" in applicable
    assert "android" in applicable
    assert "ios" not in applicable
    # ...and the full android intake body is injected inline (not just the name).
    assert "Establish current state first" in prompt

    generic = Task(id="T2", title="Fix a typo", description="spelling", planned_files=[], expected_tests=[], allowed_commands=[])
    generic_prompt = PromptBuilder().build_task_prompt(generic, [])
    generic_applicable = next(line for line in generic_prompt.splitlines() if "Applicable skills:" in line)
    assert "core-engineering" in generic_applicable
    assert "android" not in generic_applicable


def test_backend_skill_selects_for_servers_without_over_matching(tmp_path):
    def names(goal, files):
        for f in files:
            (tmp_path / f).write_text("x", encoding="utf-8")
        return {s.name for s in select_skills(goal=goal, project_root=tmp_path)}

    # Go service, Dockerized Python API, and a REST goal all pull the backend skill.
    assert "backend" in names("add a handler", ["go.mod", "main.go"])

    # ...but it must NOT fire for a pure Android or ML repo (no generic pyproject glob).
    android = names("add a screen", [])  # tmp_path now has go.mod; use a fresh dir below
    _ = android
    other = tmp_path.parent / (tmp_path.name + "_android")
    other.mkdir()
    (other / "build.gradle.kts").write_text("x", encoding="utf-8")
    (other / "Main.kt").write_text("x", encoding="utf-8")
    android_skills = {s.name for s in select_skills(goal="add a screen", project_root=other)}
    assert "android" in android_skills and "backend" not in android_skills

    ml = tmp_path.parent / (tmp_path.name + "_ml")
    ml.mkdir()
    (ml / "train.py").write_text("x", encoding="utf-8")
    (ml / "requirements.txt").write_text("torch\n", encoding="utf-8")
    ml_skills = {s.name for s in select_skills(goal="train a model", project_root=ml)}
    assert "ai-training" in ml_skills and "backend" not in ml_skills


def test_scaffolded_repo_skills_do_not_break_library_selection(tmp_path):
    # Regression: a scaffolded .claude/skills/<name>/SKILL.md keeps only name+description
    # frontmatter (no always/triggers). When merged it must NOT strip the library skill's
    # selection metadata, or selection silently returns nothing in any scaffolded repo.
    from devcouncil.skills.registry import scaffold_skills

    # An Android repo with the android + core skills already scaffolded locally.
    (tmp_path / "build.gradle.kts").write_text("plugins {}\n", encoding="utf-8")
    scaffold_skills(tmp_path, [get_skill_or_skip("core-engineering"), get_skill_or_skip("android")])

    selected = {s.name for s in select_skills(goal="", project_root=tmp_path)}
    # core-engineering stays always-on; android still matches via its inherited globs.
    assert "core-engineering" in selected
    assert "android" in selected


def get_skill_or_skip(name):
    from devcouncil.skills.registry import get_skill

    skill = get_skill(name)
    assert skill is not None
    return skill


def test_new_domain_skills_select_precisely(tmp_path):
    def names(goal, files):
        d = tmp_path / f"d{abs(hash((goal, tuple(files)))) % 10000}"
        d.mkdir()
        for f in files:
            (d / f).write_text("x", encoding="utf-8")
        return {s.name for s in select_skills(goal=goal, project_root=d)}

    # devops: IaC files / tooling keywords.
    assert "devops" in names("add a vpc", ["main.tf"])
    assert "devops" in names("update the helm chart", ["Chart.yaml"])
    assert "devops" in names("provision infrastructure with terraform", [])

    # data-engineering: pipeline/warehouse tooling.
    assert "data-engineering" in names("add a dbt model", ["dbt_project.yml"])
    assert "data-engineering" in names("build an ETL data pipeline with airflow", [])

    # systems: native/embedded toolchain.
    assert "systems" in names("add a class", ["CMakeLists.txt", "foo.cpp"])
    assert "systems" in names("flash firmware to an stm32", [])

    # None of the three over-match a plain backend/android/web repo.
    for files, goal in (
        (["go.mod", "main.go"], "add a handler"),
        (["build.gradle.kts", "A.kt"], "add a screen"),
        (["package.json", "App.tsx"], "add a page"),
    ):
        selected = names(goal, files)
        assert {"devops", "data-engineering", "systems"} & selected == set()


def test_mobile_game_security_desktop_skills_select(tmp_path):
    def names(goal, files):
        d = tmp_path / f"d{abs(hash((goal, tuple(files)))) % 100000}"
        d.mkdir()
        for f in files:
            (d / f).write_text("x", encoding="utf-8")
        return {s.name for s in select_skills(goal=goal, project_root=d)}

    assert "mobile-cross-platform" in names("add a screen", ["pubspec.yaml", "main.dart"])
    assert "mobile-cross-platform" in names("build a flutter app", [])
    assert "game-dev" in names("add a weapon", ["Player.unity"])
    assert "game-dev" in names("godot scene with gdscript", ["project.godot"])
    assert "security" in names("fix the sql injection vulnerability", [])
    assert "desktop" in names("add a window", ["tauri.conf.json"])
    assert "desktop" in names("electron main process ipc bridge", [])


def test_keyword_matching_is_word_boundary_aware():
    # Regression: short framework keywords must not fire on words they sit inside.
    from devcouncil.skills.registry import _keyword_in_text

    # "gin" (a Go framework) must not match "engine" or "logging".
    assert _keyword_in_text("gin", "unreal engine gameplay") is False
    assert _keyword_in_text("gin", "improve logging output") is False
    assert _keyword_in_text("gin", "add a gin route") is True
    # Phrase / punctuation keywords still match as substrings.
    assert _keyword_in_text("react native", "a react native screen") is True
    assert _keyword_in_text(".net", "update the asp.net app") is True

    # End to end: an "unreal engine" goal selects game-dev, not backend.
    selected = {s.name for s in select_skills(goal="unreal engine gameplay ability")}
    assert "game-dev" in selected
    assert "backend" not in selected


def test_prompt_enhancer_is_codebase_aware_and_stamps_skills(tmp_path):
    import asyncio

    from devcouncil.planning.prompt_enhancer_service import PromptEnhancement, PromptEnhancerService

    # An Android repo (detected via build.gradle.kts) even when the goal text is generic.
    (tmp_path / "build.gradle.kts").write_text("plugins {}\n", encoding="utf-8")

    captured = {}

    class FakeRouter:
        async def complete_structured(self, role, messages, schema, fallback=None):
            captured["prompt"] = messages[0]["content"]
            # The model echoes a base enhancement; provenance is stamped by the service.
            return PromptEnhancement(original_goal="x", enhanced_goal="Enhanced goal")

    service = PromptEnhancerService(FakeRouter())
    result = asyncio.run(service.enhance_prompt("add a settings screen", "{}", None, project_root=tmp_path))

    # Codebase-aware: android selected via the gradle file, plus always-on core-engineering.
    assert "android" in result.applied_skills
    assert "core-engineering" in result.applied_skills
    # The full android intake was fed to the enhancer model...
    assert "Establish current state first" in captured["prompt"]
    # ...and the compact brief rides into the council debate prompt.
    assert "android" in result.skills_brief
    assert "Domain engineering intake" in result.debate_prompt()


def test_prompt_enhancer_tolerates_no_matching_skills(tmp_path):
    import asyncio

    from devcouncil.planning.prompt_enhancer_service import PromptEnhancement, PromptEnhancerService

    class FakeRouter:
        async def complete_structured(self, role, messages, schema, fallback=None):
            return PromptEnhancement(original_goal="x", enhanced_goal="Enhanced")

    service = PromptEnhancerService(FakeRouter())
    # Empty repo + a goal with no domain keywords → only the always-on skill applies.
    result = asyncio.run(service.enhance_prompt("rename a variable", "{}", None, project_root=tmp_path))
    assert result.applied_skills == ["core-engineering"]


def test_repo_local_skills_are_discovered_and_selected(tmp_path):
    from devcouncil.skills.registry import discover_repo_skills, load_skills, select_skills

    # A user-authored skill dropped into the repo's .claude/skills/.
    skill_dir = tmp_path / ".claude" / "skills" / "mobile-backend"
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text(
        "---\nname: mobile-backend\ndescription: custom backend intake\n"
        "triggers:\n  keywords: [grpc]\n---\n# Body\nConfirm protobuf versions.\n",
        encoding="utf-8",
    )

    assert "mobile-backend" in {s.name for s in discover_repo_skills(tmp_path)}
    # Packaged-only load is unchanged (no repo skills leak in without project_root).
    assert "mobile-backend" not in {s.name for s in load_skills()}
    # Merged load + selection include it; keyword trigger fires.
    assert "mobile-backend" in {s.name for s in load_skills(project_root=tmp_path)}
    selected = {s.name for s in select_skills(goal="add a grpc endpoint", project_root=tmp_path)}
    assert "mobile-backend" in selected
    assert "core-engineering" in selected


def test_repo_local_skill_overrides_packaged_by_name(tmp_path):
    from devcouncil.skills.registry import load_skills

    skill_dir = tmp_path / ".claude" / "skills" / "android"
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text(
        "---\nname: android\ndescription: OUR android house rules\n---\n# Body\nlocal\n",
        encoding="utf-8",
    )
    by_name = {s.name: s for s in load_skills(project_root=tmp_path)}
    assert by_name["android"].description == "OUR android house rules"


def test_cli_skills_scaffold_all(tmp_path):
    result = runner.invoke(app, ["skills", "scaffold", "--all", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    scaffolded = {p.name for p in (tmp_path / ".claude" / "skills").iterdir()}
    assert {"core-engineering", "android", "ios", "windows", "web", "ai-training"} <= scaffolded
