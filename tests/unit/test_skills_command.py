"""Additional CLI coverage for `dev skills` — scaffold, show, and optimize error paths."""

import json
from pathlib import Path

from typer.testing import CliRunner

import devcouncil.cli.commands.skills as skills_cmd
from devcouncil.cli.main import app

runner = CliRunner()


def test_skills_scaffold_with_goal(tmp_path):
    result = runner.invoke(app, ["skills", "scaffold", "react website", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    scaffolded = {p.name for p in (tmp_path / ".claude" / "skills").iterdir()}
    assert "core-engineering" in scaffolded
    assert "web" in scaffolded


def test_skills_scaffold_idempotent_reports_up_to_date(tmp_path):
    assert runner.invoke(app, ["skills", "scaffold", "--project-root", str(tmp_path)]).exit_code == 0
    result = runner.invoke(app, ["skills", "scaffold", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "already up to date" in result.output


def test_skills_show_prints_body(tmp_path):
    result = runner.invoke(app, ["skills", "show", "core-engineering", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "core-engineering" in result.output.lower() or "engineering" in result.output.lower()


def test_skills_list_highlights_goal_selection(tmp_path):
    result = runner.invoke(app, ["skills", "--goal", "build an android app", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "android" in result.output


def test_skills_optimize_unknown_skill(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    evals = tmp_path / "evals.json"
    evals.write_text(json.dumps([{"goal": "x"}]), encoding="utf-8")

    result = runner.invoke(app, ["skills", "optimize", "nope", "--evals", str(evals)])
    assert result.exit_code == 1
    assert "No skill named" in result.output


def test_skills_optimize_missing_evals_dataset(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(
        app, ["skills", "optimize", "core-engineering", "--evals", str(tmp_path / "missing.json")]
    )
    assert result.exit_code == 2
    assert "not found" in result.output


def test_skills_optimize_unknown_profile(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    evals = tmp_path / "evals.json"
    evals.write_text(json.dumps([{"goal": "x"}]), encoding="utf-8")

    result = runner.invoke(
        app,
        ["skills", "optimize", "core-engineering", "--evals", str(evals), "--profile", "no-such-profile"],
    )
    assert result.exit_code == 2
    assert "No agent profile" in result.output


def test_skills_optimize_router_build_failure(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    evals = tmp_path / "evals.json"
    evals.write_text(json.dumps([{"goal": "x"}]), encoding="utf-8")

    def boom(root):
        raise RuntimeError("no model roles configured")

    monkeypatch.setattr(skills_cmd, "_build_router", boom)
    result = runner.invoke(app, ["skills", "optimize", "core-engineering", "--evals", str(evals)])
    assert result.exit_code == 1
    assert "no model roles configured" in result.output


# --- helper coverage --------------------------------------------------------------


def test_is_repo_skill_detects_in_repo_source(tmp_path):
    from devcouncil.skills.registry import discover_repo_skills

    skill_dir = tmp_path / ".claude" / "skills" / "custom"
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text(
        "---\nname: custom\ndescription: local\n---\n# Custom\n", encoding="utf-8"
    )
    local = next(s for s in discover_repo_skills(tmp_path) if s.name == "custom")
    assert skills_cmd._is_repo_skill(local, tmp_path) is True

    library = next(s for s in __import__("devcouncil.skills.registry", fromlist=["load_skills"]).load_skills())
    assert skills_cmd._is_repo_skill(library, tmp_path) is False


def test_skills_optimize_dry_run_happy_path(tmp_path, monkeypatch):
    from types import SimpleNamespace

    import devcouncil.optimization.skillopt as skillopt

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    evals = tmp_path / "evals.json"
    evals.write_text(json.dumps([{"goal": "x"}]), encoding="utf-8")

    monkeypatch.setattr(skills_cmd, "_build_router", lambda root: object())
    monkeypatch.setattr(skillopt, "make_llm_rollout", lambda router: object())
    monkeypatch.setattr(skillopt, "make_llm_optimizer", lambda router: object())
    monkeypatch.setattr(skillopt, "write_result_artifact", lambda *a, **k: None)

    result_obj = SimpleNamespace(
        improved=True,
        best_skill_body="body",
        best_guidance_body="guidance",
        seed_val_score=0.5,
        best_val_score=0.8,
        epochs=[1, 2],
        accepted_edit_count=2,
        rejected_edit_count=1,
        artifact_path=None,
        applied=False,
    )

    async def fake_optimize(**kwargs):
        return result_obj

    monkeypatch.setattr(skillopt, "optimize_skill", fake_optimize)

    result = runner.invoke(app, ["skills", "optimize", "core-engineering", "--evals", str(evals)])
    assert result.exit_code == 0
    assert "SkillOpt complete (dry-run)" in result.output
    assert "0.500 -> 0.800" in result.output


def test_skill_to_markdown_roundtrips_frontmatter():
    from devcouncil.skills.registry import get_skill

    skill = get_skill("core-engineering")
    assert skill is not None
    md = skills_cmd._skill_to_markdown(skill, "New body text.")
    assert md.startswith("---")
    assert "name: core-engineering" in md
    assert "New body text." in md


# --- _is_repo_skill: source_path None ---------------------------------------------


def test_is_repo_skill_none_source(tmp_path):
    from devcouncil.skills.registry import Skill

    skill = Skill(name="x", source_path=None)
    assert skills_cmd._is_repo_skill(skill, tmp_path) is False


# --- skills list empty registry ---------------------------------------------------


def test_skills_list_empty_registry(tmp_path, monkeypatch):
    monkeypatch.setattr(skills_cmd, "load_skills", lambda project_root: [])
    result = runner.invoke(app, ["skills", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "No skills found" in result.output


# --- _skill_to_markdown: all optional fields --------------------------------------


def test_skill_to_markdown_includes_triggers_and_always():
    from devcouncil.skills.registry import Skill, SkillTriggers

    skill = Skill(
        name="s",
        title="Title",
        description="Desc",
        always=True,
        triggers=SkillTriggers(keywords=["kw"], globs=["*.py"]),
    )
    md = skills_cmd._skill_to_markdown(skill, "body")
    assert "always: true" in md
    assert "triggers" in md
    assert "kw" in md


# --- _write_skill_body: library + repo-local --------------------------------------


def test_write_skill_body_library_materializes_under_devcouncil(tmp_path):
    from devcouncil.skills.registry import Skill

    lib_skill = Skill(name="libskill", source_path=Path("/opt/pkg/skills/libskill/SKILL.md"))
    out = skills_cmd._write_skill_body(tmp_path, lib_skill, "optimized body")
    assert out == tmp_path / ".devcouncil" / "skills" / "libskill.md"
    assert "optimized body" in out.read_text(encoding="utf-8")


def test_write_skill_body_repo_local_overwrites_in_place(tmp_path):
    from devcouncil.skills.registry import Skill

    src = tmp_path / ".claude" / "skills" / "custom" / "SKILL.md"
    src.parent.mkdir(parents=True)
    src.write_text("old", encoding="utf-8")
    skill = Skill(name="custom", source_path=src)
    out = skills_cmd._write_skill_body(tmp_path, skill, "new body")
    assert out == src
    assert "new body" in src.read_text(encoding="utf-8")


# --- _build_router ----------------------------------------------------------------


def test_build_router_success(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    import devcouncil.app.config as config_mod
    import devcouncil.llm.provider as provider_mod
    import devcouncil.llm.router as router_mod

    monkeypatch.setattr(config_mod, "get_api_key", lambda provider, root: "key")
    monkeypatch.setattr(provider_mod, "create_provider", lambda *a, **k: object())
    monkeypatch.setattr(router_mod, "ModelRouter", lambda *a, **k: "ROUTER")
    assert skills_cmd._build_router(tmp_path) == "ROUTER"


def test_build_router_no_roles_raises(tmp_path, monkeypatch):
    from types import SimpleNamespace

    import devcouncil.app.config as config_mod
    import devcouncil.llm.provider as provider_mod

    cfg = SimpleNamespace(
        models=SimpleNamespace(provider="openrouter", roles={}),
        provider=SimpleNamespace(),
    )
    monkeypatch.setattr(config_mod, "load_config", lambda root: cfg)
    monkeypatch.setattr(config_mod, "get_api_key", lambda provider, root: "key")
    monkeypatch.setattr(provider_mod, "create_provider", lambda *a, **k: object())
    import pytest

    with pytest.raises(RuntimeError, match="No model roles"):
        skills_cmd._build_router(tmp_path)


# --- optimize apply path ----------------------------------------------------------


def _fake_optimize_result(**overrides):
    from types import SimpleNamespace

    base = dict(
        improved=True,
        best_skill_body="OPTIMIZED SKILL BODY",
        best_guidance_body="OPTIMIZED GUIDANCE",
        seed_val_score=0.5,
        best_val_score=0.9,
        epochs=[1, 2, 3],
        accepted_edit_count=3,
        rejected_edit_count=0,
        artifact_path=None,
        applied=False,
    )
    base.update(overrides)
    return SimpleNamespace(**base)


def test_skills_optimize_apply_writes_both_documents(tmp_path, monkeypatch):
    import devcouncil.optimization.skillopt as skillopt
    import devcouncil.optimization.gepa_agent as gepa

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    evals = tmp_path / "evals.json"
    evals.write_text(json.dumps([{"goal": "x"}]), encoding="utf-8")

    monkeypatch.setattr(skills_cmd, "_build_router", lambda root: object())
    monkeypatch.setattr(skillopt, "make_llm_rollout", lambda router: object())
    monkeypatch.setattr(skillopt, "make_llm_optimizer", lambda router: object())
    monkeypatch.setattr(skillopt, "write_result_artifact", lambda *a, **k: None)
    applied = {}
    monkeypatch.setattr(
        gepa, "_apply_profile_preamble",
        lambda root, profile, body: applied.setdefault("body", body),
    )

    async def fake_optimize(**kwargs):
        return _fake_optimize_result()

    monkeypatch.setattr(skillopt, "optimize_skill", fake_optimize)

    # Relative evals + relative output exercise the root-resolution branches.
    result = runner.invoke(
        app, ["skills", "optimize", "core-engineering", "--evals", "evals.json", "--apply", "--output", "artifact.json"]
    )
    assert result.exit_code == 0
    assert "SkillOpt complete (applied)" in result.output
    assert "Updated skill body" in result.output
    assert applied["body"] == "OPTIMIZED GUIDANCE"


def test_skills_optimize_apply_no_improvement(tmp_path, monkeypatch):
    import devcouncil.optimization.skillopt as skillopt

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    evals = tmp_path / "evals.json"
    evals.write_text(json.dumps([{"goal": "x"}]), encoding="utf-8")

    monkeypatch.setattr(skills_cmd, "_build_router", lambda root: object())
    monkeypatch.setattr(skillopt, "make_llm_rollout", lambda router: object())
    monkeypatch.setattr(skillopt, "make_llm_optimizer", lambda router: object())
    monkeypatch.setattr(skillopt, "write_result_artifact", lambda *a, **k: None)

    async def fake_optimize(**kwargs):
        return _fake_optimize_result(improved=False)

    monkeypatch.setattr(skillopt, "optimize_skill", fake_optimize)

    result = runner.invoke(app, ["skills", "optimize", "core-engineering", "--evals", str(evals), "--apply"])
    assert result.exit_code == 0
    assert "No validated improvement" in result.output
