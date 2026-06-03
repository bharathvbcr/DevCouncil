import json
import sys
from pathlib import Path
from types import SimpleNamespace

import yaml


def _write_agent_config(project_root: Path, preamble: str) -> Path:
    config_path = project_root / ".devcouncil" / "config.yaml"
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text(
        yaml.safe_dump(
            {
                "integrations": {
                    "cli_agents": {
                        "profiles": {
                            "yolo": {
                                "description": "YOLO profile",
                                "prompt_preamble": preamble,
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
    return config_path


def _write_eval_dataset(path: Path) -> None:
    path.write_text(
        "\n".join(
            [
                json.dumps(
                    {
                        "id": "missing-verification",
                        "task": "Implement checkout validation",
                        "observed_failure": "The agent claimed success without running tests.",
                        "desired_behavior": "Run the allowed verification command before final response.",
                        "required_terms": ["verification", "evidence"],
                        "forbidden_terms": ["skip tests"],
                    }
                )
            ]
        )
        + "\n",
        encoding="utf-8",
    )


def test_agent_prompt_optimizer_invokes_gepa_with_actionable_feedback(tmp_path, monkeypatch):
    from devcouncil.optimization.gepa_agent import optimize_agent_profile

    config_path = _write_agent_config(tmp_path, "Move quickly, but stay in scope.")
    evals_path = tmp_path / "agent-evals.jsonl"
    _write_eval_dataset(evals_path)
    artifact_path = tmp_path / "gepa-result.json"

    calls = []
    logged = []

    class FakeEngineConfig:
        def __init__(self, **kwargs):
            self.kwargs = kwargs

    class FakeGEPAConfig:
        def __init__(self, **kwargs):
            self.kwargs = kwargs

    def fake_optimize_anything(**kwargs):
        calls.append(kwargs)
        score = kwargs["evaluator"](
            "Always run verification and include evidence before reporting success.",
            kwargs["dataset"][0],
        )
        return SimpleNamespace(
            best_candidate="Always run verification and include evidence before reporting success.",
            best_score=score,
        )

    fake_module = SimpleNamespace(
        optimize_anything=fake_optimize_anything,
        GEPAConfig=FakeGEPAConfig,
        EngineConfig=FakeEngineConfig,
        log=logged.append,
    )
    monkeypatch.setitem(sys.modules, "gepa.optimize_anything", fake_module)

    result = optimize_agent_profile(
        project_root=tmp_path,
        agent="codex",
        profile_name="yolo",
        evals_path=evals_path,
        max_metric_calls=7,
        apply=False,
        output_path=artifact_path,
    )

    assert calls, "GEPA optimize_anything should be invoked"
    assert calls[0]["seed_candidate"] == "Move quickly, but stay in scope."
    assert calls[0]["dataset"][0]["id"] == "missing-verification"
    assert calls[0]["config"].kwargs["engine"].kwargs["max_metric_calls"] == 7
    assert "verification" in "\n".join(logged)
    assert "claimed success without running tests" in "\n".join(logged)
    assert result.best_preamble == "Always run verification and include evidence before reporting success."
    assert result.applied is False
    assert artifact_path.exists()

    raw_config = yaml.safe_load(config_path.read_text(encoding="utf-8"))
    assert raw_config["integrations"]["cli_agents"]["profiles"]["yolo"]["prompt_preamble"] == (
        "Move quickly, but stay in scope."
    )


def test_agent_prompt_optimizer_can_apply_best_preamble(tmp_path, monkeypatch):
    from devcouncil.optimization.gepa_agent import optimize_agent_profile

    config_path = _write_agent_config(tmp_path, "Old profile text.")
    evals_path = tmp_path / "agent-evals.json"
    evals_path.write_text(
        json.dumps(
            {
                "examples": [
                    {
                        "id": "scope",
                        "desired_behavior": "Do not edit files outside the planned scope.",
                        "required_terms": ["planned scope"],
                    }
                ]
            }
        ),
        encoding="utf-8",
    )

    class FakeEngineConfig:
        def __init__(self, **kwargs):
            self.kwargs = kwargs

    class FakeGEPAConfig:
        def __init__(self, **kwargs):
            self.kwargs = kwargs

    fake_module = SimpleNamespace(
        optimize_anything=lambda **kwargs: SimpleNamespace(
            best_candidate="Stay inside the planned scope and report evidence.",
            best_score=1.0,
        ),
        GEPAConfig=FakeGEPAConfig,
        EngineConfig=FakeEngineConfig,
        log=lambda message: None,
    )
    monkeypatch.setitem(sys.modules, "gepa.optimize_anything", fake_module)

    result = optimize_agent_profile(
        project_root=tmp_path,
        agent="codex",
        profile_name="yolo",
        evals_path=evals_path,
        max_metric_calls=3,
        apply=True,
    )

    raw_config = yaml.safe_load(config_path.read_text(encoding="utf-8"))
    assert raw_config["integrations"]["cli_agents"]["profiles"]["yolo"]["prompt_preamble"] == (
        "Stay inside the planned scope and report evidence."
    )
    assert result.applied is True
