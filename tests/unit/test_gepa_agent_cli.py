import json
import subprocess
from types import SimpleNamespace

import yaml
from typer.testing import CliRunner

from devcouncil.cli.main import app


runner = CliRunner()


def test_cli_agents_optimize_dry_run_uses_gepa_without_git_mutation(tmp_path, monkeypatch):
    project = tmp_path / "project"
    config_path = project / ".devcouncil" / "config.yaml"
    config_path.parent.mkdir(parents=True)
    config_path.write_text(
        yaml.safe_dump(
            {
                "integrations": {
                    "cli_agents": {
                        "profiles": {
                            "yolo": {
                                "description": "YOLO profile",
                                "prompt_preamble": "Old preamble",
                                "timeout_seconds": 3600,
                                "require_explicit_confirmation": False,
                            }
                        }
                    }
                }
            },
            sort_keys=False,
        ),
        encoding="utf-8",
    )
    evals_path = project / "agent-evals.jsonl"
    evals_path.write_text(
        json.dumps({"id": "verify", "required_terms": ["verification"], "desired_behavior": "Run tests"})
        + "\n",
        encoding="utf-8",
    )

    captured = {}

    def fake_optimize_agent_profile(**kwargs):
        captured.update(kwargs)
        return SimpleNamespace(
            agent="codex",
            profile_name="yolo",
            seed_preamble="Old preamble",
            best_preamble="Optimized preamble",
            best_score=1.0,
            artifact_path=project / ".devcouncil" / "optimizations" / "fake.json",
            applied=kwargs["apply"],
        )

    def fail_subprocess_run(*args, **kwargs):
        raise AssertionError(f"agents optimize should not mutate git state via subprocess.run: {args}")

    monkeypatch.setattr("devcouncil.cli.commands.agents.optimize_agent_profile", fake_optimize_agent_profile)
    monkeypatch.setattr(subprocess, "run", fail_subprocess_run)

    result = runner.invoke(
        app,
        [
            "agents",
            "optimize",
            "--agent",
            "codex",
            "--profile",
            "yolo",
            "--evals",
            str(evals_path),
            "--dry-run",
            "--project-root",
            str(project),
        ],
    )

    assert result.exit_code == 0, result.output
    assert captured["project_root"] == project.resolve()
    assert captured["agent"] == "codex"
    assert captured["profile_name"] == "yolo"
    assert captured["evals_path"] == evals_path
    assert captured["apply"] is False
    assert "Optimized preamble" in result.output

    raw_config = yaml.safe_load(config_path.read_text(encoding="utf-8"))
    assert raw_config["integrations"]["cli_agents"]["profiles"]["yolo"]["prompt_preamble"] == "Old preamble"
