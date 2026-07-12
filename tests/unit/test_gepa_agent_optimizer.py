import json
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest
import yaml

from devcouncil.optimization import gepa_agent as ga


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


# ---- validation errors --------------------------------------------------------


def test_optimize_unknown_agent_raises(tmp_path):
    _write_agent_config(tmp_path, "seed")
    evals = tmp_path / "e.jsonl"
    _write_eval_dataset(evals)
    with pytest.raises(ValueError, match="not registered"):
        ga.optimize_agent_profile(
            project_root=tmp_path, agent="not-a-real-agent", profile_name="yolo", evals_path=evals
        )


def test_optimize_unknown_profile_raises(tmp_path):
    _write_agent_config(tmp_path, "seed")
    evals = tmp_path / "e.jsonl"
    _write_eval_dataset(evals)
    with pytest.raises(ValueError, match="not configured"):
        ga.optimize_agent_profile(
            project_root=tmp_path, agent="codex", profile_name="no-such-profile", evals_path=evals
        )


def test_optimize_aggregate_evaluator_and_empty_best(tmp_path, monkeypatch):
    _write_agent_config(tmp_path, "seed preamble")
    evals = tmp_path / "e.jsonl"
    _write_eval_dataset(evals)

    captured = {}

    def fake_optimize_anything(**kwargs):
        # Call the evaluator with example=None to exercise the aggregate path.
        captured["aggregate"] = kwargs["evaluator"]("some candidate text here")
        return SimpleNamespace(best_candidate="   ", best_score="not-a-number")

    fake_module = SimpleNamespace(
        optimize_anything=fake_optimize_anything,
        GEPAConfig=lambda **k: SimpleNamespace(kwargs=k),
        EngineConfig=lambda **k: SimpleNamespace(kwargs=k),
    )
    monkeypatch.setitem(sys.modules, "gepa.optimize_anything", fake_module)

    # Use a relative evals path + relative output path to exercise resolution.
    result = ga.optimize_agent_profile(
        project_root=tmp_path,
        agent="codex",
        profile_name="yolo",
        evals_path=Path("e.jsonl"),
        output_path=Path("out/result.json"),
    )
    assert "aggregate" in captured
    # Blank best candidate falls back to the seed; unparseable score -> None.
    assert result.best_preamble == "seed preamble"
    assert result.best_score is None
    assert (tmp_path / "out" / "result.json").exists()


# ---- load_agent_eval_dataset --------------------------------------------------

def test_load_dataset_missing_file(tmp_path):
    with pytest.raises(ValueError, match="not found"):
        ga.load_agent_eval_dataset(tmp_path / "nope.json")


def test_load_dataset_json_dataset_key(tmp_path):
    path = tmp_path / "d.json"
    path.write_text(json.dumps({"dataset": [{"id": "a"}]}), encoding="utf-8")
    out = ga.load_agent_eval_dataset(path)
    assert out[0]["id"] == "a"


def test_load_dataset_single_object(tmp_path):
    path = tmp_path / "d.json"
    path.write_text(json.dumps({"task": "solo"}), encoding="utf-8")
    out = ga.load_agent_eval_dataset(path)
    assert out[0]["task"] == "solo"
    assert out[0]["id"] == "example-1"


def test_load_dataset_list(tmp_path):
    path = tmp_path / "d.json"
    path.write_text(json.dumps([{"id": "x"}, {"id": "y"}]), encoding="utf-8")
    assert len(ga.load_agent_eval_dataset(path)) == 2


def test_load_dataset_invalid_top_type(tmp_path):
    path = tmp_path / "d.json"
    path.write_text(json.dumps("just a string"), encoding="utf-8")
    with pytest.raises(ValueError, match="JSON object, JSON array, or JSONL"):
        ga.load_agent_eval_dataset(path)


def test_load_dataset_non_dict_item(tmp_path):
    path = tmp_path / "d.json"
    path.write_text(json.dumps([1, 2, 3]), encoding="utf-8")
    with pytest.raises(ValueError, match="must be a JSON object"):
        ga.load_agent_eval_dataset(path)


def test_load_dataset_empty(tmp_path):
    path = tmp_path / "d.json"
    path.write_text(json.dumps([]), encoding="utf-8")
    with pytest.raises(ValueError, match="at least one example"):
        ga.load_agent_eval_dataset(path)


# ---- _load_gepa_optimize_anything ---------------------------------------------

def test_load_gepa_uses_injected_module(monkeypatch):
    sentinel = SimpleNamespace(marker=True)
    monkeypatch.setitem(sys.modules, "gepa.optimize_anything", sentinel)
    assert ga._load_gepa_optimize_anything() is sentinel


def test_load_gepa_import_error(monkeypatch):
    monkeypatch.delitem(sys.modules, "gepa.optimize_anything", raising=False)

    def boom(name):
        raise ImportError("no gepa")

    monkeypatch.setattr(ga.importlib, "import_module", boom)
    with pytest.raises(ga.GepaUnavailableError):
        ga._load_gepa_optimize_anything()


# ---- _score_candidate ---------------------------------------------------------

def test_score_candidate_empty_returns_zero():
    logs = []
    score = ga._score_candidate("", {"id": "x"}, log=logs.append)
    assert score == 0.0
    assert any("empty" in m.lower() for m in logs)


def test_score_candidate_full_signal():
    logs = []
    example = {
        "id": "e1",
        "task": "do it",
        "observed_failure": "skipped tests",
        "required_terms": ["verify", "evidence"],
        "forbidden_terms": ["yolo"],
    }
    text = "Always verify and provide evidence for every change you make in scope."
    score = ga._score_candidate(text, example, log=logs.append)
    assert 0.0 <= score <= 1.0
    joined = "\n".join(logs)
    assert "Required prompt terms" in joined


def test_score_candidate_missing_and_forbidden_logged():
    logs = []
    example = {
        "id": "e2",
        "required_terms": ["absent-term"],
        "forbidden_terms": ["danger"],
    }
    ga._score_candidate("this text has danger in it", example, log=logs.append)
    joined = "\n".join(logs)
    assert "Missing required terms" in joined
    assert "Forbidden terms present" in joined


# ---- _candidate_to_text -------------------------------------------------------

def test_candidate_to_text_variants():
    assert ga._candidate_to_text(None) == ""
    assert ga._candidate_to_text("hi") == "hi"
    assert ga._candidate_to_text({"preamble": "P"}) == "P"
    # dict without known key -> json dump
    assert ga._candidate_to_text({"other": 1}) == json.dumps({"other": 1}, sort_keys=True)
    # object attr
    assert ga._candidate_to_text(SimpleNamespace(prompt="via-attr")) == "via-attr"
    # object without known attr -> str()
    assert ga._candidate_to_text(SimpleNamespace(nope=1)).startswith("namespace")


# ---- _best_candidate / _best_score --------------------------------------------

def test_best_candidate_variants():
    assert ga._best_candidate({"best_candidate": "c"}) == "c"
    assert ga._best_candidate({"no_key": 1}) == {"no_key": 1}
    assert ga._best_candidate(SimpleNamespace(candidate="attr-c")) == "attr-c"
    obj = SimpleNamespace(x=1)
    assert ga._best_candidate(obj) is obj


def test_best_score_variants():
    assert ga._best_score({"best_score": "0.5"}) == 0.5
    assert ga._best_score({"score": 1}) == 1.0
    assert ga._best_score({}) is None
    assert ga._best_score(SimpleNamespace(best_score="bad")) is None
    assert ga._best_score(SimpleNamespace(score=0.25)) == 0.25


# ---- _string_list / _length_score ---------------------------------------------

def test_string_list_handles_str_and_iterables():
    example = {"a": "single", "b": ["one", "  ", "two"], "c": None}
    assert ga._string_list(example, "a", "b", "c") == ["single", "one", "two"]


def test_length_score_bands():
    assert ga._length_score(" ".join(["w"] * 50)) == 1.0
    assert ga._length_score("w w w") == 3 / 10  # under 10 words
    long_text = " ".join(["w"] * 360)
    assert ga._length_score(long_text) == 0.0  # far over 180 words


# ---- _default_artifact_path ---------------------------------------------------

def test_default_artifact_path_sanitizes_profile(tmp_path):
    path = ga._default_artifact_path(tmp_path, "codex", "team/prod")
    assert "team-prod" in path.name
    assert path.parent == tmp_path / ".devcouncil" / "optimizations"
