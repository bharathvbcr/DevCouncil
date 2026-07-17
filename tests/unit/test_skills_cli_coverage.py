from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from types import SimpleNamespace

from typer.testing import CliRunner

from devcouncil.cli.commands import skills as skills_cmd
from devcouncil.optimization.skillopt import (
    GUIDANCE,
    SKILL,
    EpochRecord,
    SkillOptResult,
)
from devcouncil.skills import registry
from devcouncil.skills.registry import Skill, SkillTriggers


runner = CliRunner()


def _write_skill(path: Path, name: str, body: str = "Body", extra: str = "") -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        f"---\nname: {name}\ndescription: {name} description\n{extra}---\n# {name}\n{body}\n",
        encoding="utf-8",
    )
    return path


def test_skills_list_reports_empty_library(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setattr(skills_cmd, "load_skills", lambda project_root: [])

    result = runner.invoke(skills_cmd.app, ["--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert "No skills found" in result.output


def test_skills_list_marks_repo_and_library_sources(monkeypatch, tmp_path: Path) -> None:
    repo_skill = Skill(
        name="repo-skill",
        description="repo",
        always=True,
        body="repo body",
        source_path=tmp_path / ".devcouncil" / "skills" / "repo-skill.md",
    )
    library_skill = Skill(
        name="library-skill",
        description="library",
        triggers=SkillTriggers(keywords=["library"]),
        body="library body",
        source_path=tmp_path.parent / "library-skill.md",
    )
    monkeypatch.setattr(skills_cmd, "load_skills", lambda project_root: [repo_skill, library_skill])
    monkeypatch.setattr(skills_cmd, "select_skills", lambda goal, root: [library_skill])

    result = runner.invoke(
        skills_cmd.app,
        ["--goal", "library task", "--project-root", str(tmp_path)],
    )

    assert result.exit_code == 0
    assert "repo-skill" in result.output
    assert "library-skill" in result.output
    assert "repo" in result.output
    assert "library" in result.output
    assert "Scaffold the applicable skills" in result.output


def test_is_repo_skill_handles_missing_source_and_relative_paths(tmp_path: Path) -> None:
    assert skills_cmd._is_repo_skill(Skill(name="none"), tmp_path) is False
    assert skills_cmd._is_repo_skill(Skill(name="local", source_path=tmp_path / "skill.md"), tmp_path) is True
    assert skills_cmd._is_repo_skill(Skill(name="external", source_path=tmp_path.parent / "skill.md"), tmp_path) is False


def test_skills_show_success_and_missing(monkeypatch, tmp_path: Path) -> None:
    skill = Skill(name="demo", description="Demo skill", body="# Demo\nUse it.")
    monkeypatch.setattr(
        skills_cmd,
        "get_skill",
        lambda name, project_root: skill if name == "demo" else None,
    )

    shown = runner.invoke(skills_cmd.app, ["show", "demo", "--project-root", str(tmp_path)])
    missing = runner.invoke(skills_cmd.app, ["show", "missing", "--project-root", str(tmp_path)])

    assert shown.exit_code == 0
    assert "Demo skill" in shown.output
    assert "Use it." in shown.output
    assert missing.exit_code == 1
    assert "No skill named 'missing'" in missing.output


def test_skills_scaffold_reports_written_and_noop(monkeypatch, tmp_path: Path) -> None:
    chosen = [Skill(name="demo", description="Demo", body="Body")]
    written = [tmp_path / ".claude" / "skills" / "demo" / "SKILL.md"]
    calls: list[tuple[Path, list[Skill]]] = []

    monkeypatch.setattr(skills_cmd, "select_skills", lambda goal, root: chosen)

    def fake_scaffold(root: Path, skills: list[Skill]) -> list[Path]:
        calls.append((root, skills))
        return written if len(calls) == 1 else []

    monkeypatch.setattr(skills_cmd, "scaffold_skills", fake_scaffold)

    first = runner.invoke(skills_cmd.app, ["scaffold", "demo goal", "--project-root", str(tmp_path)])
    second = runner.invoke(skills_cmd.app, ["scaffold", "demo goal", "--project-root", str(tmp_path)])

    assert first.exit_code == 0
    assert "Wrote 1 skill file" in first.output
    assert ".claude/skills/demo/SKILL.md" in first.output
    assert second.exit_code == 0
    assert "Skills already up to date" in second.output
    assert calls == [(tmp_path.resolve(), chosen), (tmp_path.resolve(), chosen)]


def test_skills_scaffold_all_loads_every_skill(monkeypatch, tmp_path: Path) -> None:
    all_skills = [Skill(name="one", body="1"), Skill(name="two", body="2")]
    captured: dict[str, object] = {}

    monkeypatch.setattr(skills_cmd, "load_skills", lambda project_root: all_skills)
    monkeypatch.setattr(skills_cmd, "select_skills", lambda goal, root: [])

    def fake_scaffold(root: Path, skills: list[Skill]) -> list[Path]:
        captured["skills"] = skills
        return []

    monkeypatch.setattr(skills_cmd, "scaffold_skills", fake_scaffold)

    result = runner.invoke(skills_cmd.app, ["scaffold", "--all", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert captured["skills"] == all_skills


def test_skill_markdown_write_updates_repo_skill_and_materializes_library_skill(tmp_path: Path) -> None:
    repo_path = _write_skill(tmp_path / ".devcouncil" / "skills" / "repo.md", "repo")
    repo_skill = Skill(
        name="repo",
        title="Repo Title",
        description="Repo description",
        always=True,
        triggers=SkillTriggers(keywords=["repo"], globs=["repo.toml"]),
        body="old",
        source_path=repo_path,
    )
    library_skill = Skill(
        name="library",
        description="Library description",
        triggers=SkillTriggers(keywords=["lib"]),
        body="old",
        source_path=tmp_path.parent / "library.md",
    )

    updated_repo = skills_cmd._write_skill_body(tmp_path, repo_skill, "new repo body")
    materialized = skills_cmd._write_skill_body(tmp_path, library_skill, "new library body")

    assert updated_repo == repo_path
    assert "always: true" in repo_path.read_text(encoding="utf-8")
    assert "new repo body" in repo_path.read_text(encoding="utf-8")
    assert materialized == tmp_path / ".devcouncil" / "skills" / "library.md"
    assert "new library body" in materialized.read_text(encoding="utf-8")


def test_optimize_handles_validation_errors_before_network(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setattr(skills_cmd, "get_skill", lambda name, project_root: None)
    missing_skill = runner.invoke(
        skills_cmd.app,
        ["optimize", "missing", "--evals", "evals.json", "--project-root", str(tmp_path)],
    )
    assert missing_skill.exit_code == 1
    assert "No skill named 'missing'" in missing_skill.output

    skill = Skill(name="demo", description="Demo", body="seed skill")
    monkeypatch.setattr(skills_cmd, "get_skill", lambda name, project_root: skill)

    import devcouncil.optimization.gepa_agent as gepa_agent

    monkeypatch.setattr(
        gepa_agent,
        "load_agent_eval_dataset",
        lambda path: (_ for _ in ()).throw(ValueError("bad evals")),
    )
    bad_evals = runner.invoke(
        skills_cmd.app,
        ["optimize", "demo", "--evals", "evals.json", "--project-root", str(tmp_path)],
    )
    assert bad_evals.exit_code == 2
    assert "bad evals" in bad_evals.output

    monkeypatch.setattr(gepa_agent, "load_agent_eval_dataset", lambda path: [{"id": "1"}])
    import devcouncil.executors.agent_registry as agent_registry

    monkeypatch.setattr(agent_registry, "load_agent_profiles", lambda root: {})
    missing_profile = runner.invoke(
        skills_cmd.app,
        [
            "optimize",
            "demo",
            "--evals",
            "evals.json",
            "--profile",
            "prod",
            "--project-root",
            str(tmp_path),
        ],
    )
    assert missing_profile.exit_code == 2
    assert "Known profiles" in missing_profile.output


@dataclass
class _Profile:
    prompt_preamble: str = "seed guidance"


def test_optimize_dry_run_writes_artifact_without_applying(monkeypatch, tmp_path: Path) -> None:
    skill = Skill(name="demo", description="Demo", body="seed skill")
    monkeypatch.setattr(skills_cmd, "get_skill", lambda name, project_root: skill)
    monkeypatch.setattr(skills_cmd, "_build_router", lambda root: object())

    import devcouncil.executors.agent_registry as agent_registry
    import devcouncil.optimization.gepa_agent as gepa_agent
    import devcouncil.optimization.skillopt as skillopt

    monkeypatch.setattr(gepa_agent, "load_agent_eval_dataset", lambda path: [{"id": "1"}])
    monkeypatch.setattr(agent_registry, "load_agent_profiles", lambda root: {"default": _Profile()})
    monkeypatch.setattr(skillopt, "make_llm_rollout", lambda router: object())
    monkeypatch.setattr(skillopt, "make_llm_optimizer", lambda router: object())
    monkeypatch.setattr(skillopt, "default_artifact_path", lambda root, name: root / "artifact.json")

    async def fake_optimize_skill(**kwargs):
        return SkillOptResult(
            skill_name=kwargs["skill_name"],
            seed_docs=kwargs["docs"],
            best_docs={GUIDANCE: "seed guidance", SKILL: "seed skill"},
            seed_val_score=0.5,
            best_val_score=0.5,
            epochs=[EpochRecord(1, 0.5, 0.5, 0.5, 0, 0, [], False, "noop")],
        )

    written: dict[str, object] = {}
    monkeypatch.setattr(skillopt, "optimize_skill", fake_optimize_skill)
    monkeypatch.setattr(
        skillopt,
        "write_result_artifact",
        lambda path, result, objective, dataset_path: written.update(
            {"path": path, "applied": result.applied, "dataset": dataset_path}
        ),
    )

    result = runner.invoke(
        skills_cmd.app,
        ["optimize", "demo", "--evals", "evals.json", "--project-root", str(tmp_path)],
    )

    assert result.exit_code == 0
    assert "SkillOpt complete (dry-run)" in result.output
    assert "No validated improvement" not in result.output
    assert written["path"] == tmp_path.resolve() / "artifact.json"
    assert written["applied"] is False


def test_optimize_apply_updates_changed_skill_and_guidance(monkeypatch, tmp_path: Path) -> None:
    skill = Skill(name="demo", description="Demo", body="seed skill")
    monkeypatch.setattr(skills_cmd, "get_skill", lambda name, project_root: skill)
    monkeypatch.setattr(skills_cmd, "_build_router", lambda root: object())

    import devcouncil.executors.agent_registry as agent_registry
    import devcouncil.optimization.gepa_agent as gepa_agent
    import devcouncil.optimization.skillopt as skillopt

    monkeypatch.setattr(gepa_agent, "load_agent_eval_dataset", lambda path: [{"id": "1"}])
    monkeypatch.setattr(agent_registry, "load_agent_profiles", lambda root: {"default": _Profile()})
    monkeypatch.setattr(skillopt, "make_llm_rollout", lambda router: object())
    monkeypatch.setattr(skillopt, "make_llm_optimizer", lambda router: object())
    monkeypatch.setattr(skillopt, "write_result_artifact", lambda *args, **kwargs: None)

    async def fake_optimize_skill(**kwargs):
        return SkillOptResult(
            skill_name="demo",
            seed_docs=kwargs["docs"],
            best_docs={GUIDANCE: "better guidance", SKILL: "better skill"},
            seed_val_score=0.2,
            best_val_score=0.8,
            epochs=[EpochRecord(1, 0.5, 0.2, 0.8, 1, 1, [SKILL], True, "accepted")],
            accepted_edit_count=1,
        )

    applied: dict[str, object] = {}
    monkeypatch.setattr(skillopt, "optimize_skill", fake_optimize_skill)
    def fake_write_skill_body(root: Path, selected: Skill, body: str) -> Path:
        applied["skill_body"] = body
        return root / "skill.md"

    monkeypatch.setattr(skills_cmd, "_write_skill_body", fake_write_skill_body)
    monkeypatch.setattr(
        gepa_agent,
        "_apply_profile_preamble",
        lambda root, profile, body: applied.update({"profile": profile, "guidance": body}),
    )

    result = runner.invoke(
        skills_cmd.app,
        ["optimize", "demo", "--evals", "evals.json", "--apply", "--project-root", str(tmp_path)],
    )

    assert result.exit_code == 0
    assert "SkillOpt complete (applied)" in result.output
    assert applied["skill_body"] == "better skill"
    assert applied["profile"] == "default"
    assert applied["guidance"] == "better guidance"


def test_optimize_reports_router_configuration_error(monkeypatch, tmp_path: Path) -> None:
    skill = Skill(name="demo", description="Demo", body="seed skill")
    monkeypatch.setattr(skills_cmd, "get_skill", lambda name, project_root: skill)
    monkeypatch.setattr(skills_cmd, "_build_router", lambda root: (_ for _ in ()).throw(RuntimeError("no roles")))

    import devcouncil.executors.agent_registry as agent_registry
    import devcouncil.optimization.gepa_agent as gepa_agent

    monkeypatch.setattr(gepa_agent, "load_agent_eval_dataset", lambda path: [{"id": "1"}])
    monkeypatch.setattr(agent_registry, "load_agent_profiles", lambda root: {"default": _Profile()})

    result = runner.invoke(
        skills_cmd.app,
        ["optimize", "demo", "--evals", "evals.json", "--project-root", str(tmp_path)],
    )

    assert result.exit_code == 1
    assert "no roles" in result.output


def test_build_router_adds_skill_roles_and_rejects_empty_role_config(monkeypatch, tmp_path: Path) -> None:
    class Role:
        def __init__(self, model: str):
            self.model = model

        def model_dump(self):
            return {"model": self.model}

    config = SimpleNamespace(
        models=SimpleNamespace(provider="openrouter", roles={"arbiter": Role("model-a")}),
        provider=SimpleNamespace(),
    )
    captured: dict[str, object] = {}

    import devcouncil.app.config as config_mod
    import devcouncil.llm.provider as provider_mod
    import devcouncil.llm.router as router_mod

    monkeypatch.setattr(config_mod, "load_config", lambda root: config)
    monkeypatch.setattr(config_mod, "get_api_key", lambda provider, root: "key")
    monkeypatch.setattr(provider_mod, "create_provider", lambda *args, **kwargs: "provider")

    class FakeRouter:
        def __init__(self, provider, role_config, project_root):
            captured["provider"] = provider
            captured["role_config"] = role_config
            captured["project_root"] = project_root

    monkeypatch.setattr(router_mod, "ModelRouter", FakeRouter)

    router = skills_cmd._build_router(tmp_path)

    assert isinstance(router, FakeRouter)
    role_config = captured["role_config"]
    assert role_config["skill_target"] == {"model": "model-a"}
    assert role_config["skill_optimizer"] == {"model": "model-a"}
    assert role_config["skill_target"] is not role_config["skill_optimizer"]

    config.models.roles = {}
    try:
        skills_cmd._build_router(tmp_path)
    except RuntimeError as exc:
        assert "No model roles configured" in str(exc)
    else:
        raise AssertionError("expected missing role config to fail")


def test_registry_parses_repo_skills_and_ignores_plain_or_unreadable_files(tmp_path: Path) -> None:
    registry.clear_skill_caches()
    _write_skill(
        tmp_path / ".claude" / "skills" / "custom" / "SKILL.md",
        "custom",
        extra="triggers:\n  keywords: [grpc]\n",
    )
    (tmp_path / ".devcouncil" / "skills").mkdir(parents=True)
    (tmp_path / ".devcouncil" / "skills" / "notes.md").write_text("# Not a skill\n", encoding="utf-8")
    broken = tmp_path / ".devcouncil" / "skills" / "broken.md"
    try:
        broken.symlink_to(tmp_path / "missing.md")
    except OSError:
        broken.write_text("", encoding="utf-8")
        broken.chmod(0)

    found = registry.discover_repo_skills(tmp_path)

    assert [skill.name for skill in found] == ["custom"]
    registry.clear_skill_caches()


def test_registry_load_skills_honors_custom_knowledge_dir_and_precedence(monkeypatch, tmp_path: Path) -> None:
    registry.clear_skill_caches()
    library = tmp_path / "library"
    _write_skill(
        library / "base.md",
        "base",
        body="library body",
        extra="always: true\ntriggers:\n  keywords: [base]\n",
    )
    _write_skill(tmp_path / ".claude" / "skills" / "base" / "SKILL.md", "base", body="local body")

    class _Knowledge:
        directory = "custom-knowledge"

    class _Config:
        knowledge = _Knowledge()

    monkeypatch.setattr("devcouncil.app.config.load_config", lambda root: _Config())
    monkeypatch.setattr(
        registry,
        "load_okf_skills",
        lambda root, directory: [
            Skill(name="base", description="conflict", body="must not win"),
            Skill(name=f"okf-{directory}", description="okf", body="okf"),
        ],
    )

    loaded = {skill.name: skill for skill in registry.load_skills(library, tmp_path)}

    assert loaded["base"].body == "# base\nlocal body"
    assert loaded["base"].always is True
    assert loaded["base"].triggers.keywords == ["base"]
    assert "okf-custom-knowledge" in loaded
    registry.clear_skill_caches()


def test_registry_selection_cache_and_walk_edge_cases(monkeypatch, tmp_path: Path) -> None:
    registry.clear_skill_caches()
    seen_roots: list[Path] = []

    def fake_walk(root: Path):
        seen_roots.append(root)
        dirnames = [".git", "src"]
        yield (str(root), dirnames, ["Package.swift"])
        if ".git" in dirnames:
            yield (str(root / ".git"), [], ["ignored"])
        if "src" in dirnames:
            yield (str(root / "src"), [], ["App.swift"])

    monkeypatch.setattr(registry.os, "walk", fake_walk)
    names = registry._walk_repo_basenames(tmp_path)
    assert "package.swift" in names
    assert "ignored" not in names

    first = registry._collect_repo_basenames(tmp_path)
    second = registry._collect_repo_basenames(tmp_path)
    assert first == second
    assert len(seen_roots) == 2  # one direct walk plus one cached collector fill

    registry._basename_cache.clear()
    monkeypatch.setattr(registry, "_BASENAME_CACHE_MAX", 1)
    registry._basename_cache[("old", 1)] = {"old"}
    registry._collect_repo_basenames(tmp_path)
    assert ("old", 1) not in registry._basename_cache
    registry.clear_skill_caches()


def test_registry_collect_repo_basenames_tolerates_stat_failure(monkeypatch, tmp_path: Path) -> None:
    class BadPath(type(tmp_path)):
        def resolve(self):  # type: ignore[override]
            raise OSError("nope")

    bad = BadPath(tmp_path)
    monkeypatch.setattr(registry, "_walk_repo_basenames", lambda root: {"pyproject.toml"})

    assert registry._collect_repo_basenames(bad) == {"pyproject.toml"}


def test_registry_load_okf_skills_skips_index_and_non_skills(tmp_path: Path) -> None:
    from devcouncil.knowledge.frontmatter import build_frontmatter_markdown
    from devcouncil.knowledge.skill_bridge import SKILL_OKF_TYPE

    okf = tmp_path / ".devcouncil" / "knowledge" / "okf"
    okf.mkdir(parents=True)
    (okf / "index.md").write_text("---\ntype: OKF Index\n---\n# Index\n", encoding="utf-8")
    (okf / "plain.md").write_text("---\ntype: Note\n---\n# Note\n", encoding="utf-8")
    (okf / "skill.md").write_text(
        build_frontmatter_markdown(
            {"type": SKILL_OKF_TYPE, "title": "GraphQL", "description": "GraphQL", "tags": ["graphql"]},
            "Use schemas.",
        ),
        encoding="utf-8",
    )

    skills = registry.load_okf_skills(tmp_path)

    assert [skill.name for skill in skills] == ["skill"]
    assert skills[0].triggers.keywords == ["graphql"]


def test_registry_render_and_scaffold_edge_cases(tmp_path: Path) -> None:
    assert registry.render_preamble([]) == ""
    empty = Skill(name="empty", body="")
    full = Skill(name="full", description="Full", body="Full body")
    inline, deferred = registry.bound_skills([empty, full], max_skills=5, max_chars=100)
    assert inline == [full]
    assert deferred == []

    local_path = tmp_path / ".devcouncil" / "skills" / "local.md"
    local_path.parent.mkdir(parents=True)
    local_path.write_text("local", encoding="utf-8")
    local = Skill(name="local", body="local", source_path=local_path)
    external = Skill(name="external", description="External", body="external", source_path=tmp_path.parent / "x.md")

    written = registry.scaffold_skills(tmp_path, [local, external])

    assert [path.relative_to(tmp_path).as_posix() for path in written] == [
        ".claude/skills/external/SKILL.md"
    ]
