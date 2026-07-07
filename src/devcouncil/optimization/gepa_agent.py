from __future__ import annotations

import importlib
import json
import sys
from collections.abc import Iterable
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable

import yaml  # type: ignore[import-untyped]

from devcouncil.executors.agent_registry import (
    load_agent_profiles,
    load_cli_agent_specs,
    normalize_agent_name,
)
from devcouncil.utils.fsio import atomic_write_text
from devcouncil.utils.json_persist import read_json, write_json


DEFAULT_OBJECTIVE = (
    "Optimize the DevCouncil CLI-agent profile prompt preamble so coding agents stay inside "
    "planned scope, run the expected verification commands, preserve evidence, avoid destructive "
    "or out-of-policy changes, and address the observed failures in the evaluation examples."
)


class GepaUnavailableError(RuntimeError):
    """Raised when the optional GEPA package is not installed."""


@dataclass(frozen=True)
class AgentPromptOptimizationResult:
    agent: str
    profile_name: str
    seed_preamble: str
    best_preamble: str
    best_score: float | None
    artifact_path: Path
    applied: bool


def optimize_agent_profile(
    *,
    project_root: Path,
    agent: str,
    profile_name: str,
    evals_path: Path,
    max_metric_calls: int = 40,
    objective: str | None = None,
    apply: bool = False,
    output_path: Path | None = None,
) -> AgentPromptOptimizationResult:
    """Run GEPA over a CLI-agent profile prompt preamble.

    GEPA needs an evaluator. DevCouncil's evaluator consumes an offline JSON/JSONL
    dataset of observed agent failures and desired prompt behavior, then feeds that
    context back to GEPA as actionable side information.
    """

    root = project_root.expanduser().resolve()
    normalized_agent = normalize_agent_name(agent)
    if normalized_agent not in load_cli_agent_specs(root):
        raise ValueError(f"Agent '{agent}' is not registered or supported.")

    profiles = load_agent_profiles(root)
    profile = profiles.get(profile_name)
    if profile is None:
        raise ValueError(f"Profile '{profile_name}' is not configured.")

    resolved_evals_path = evals_path.expanduser()
    if not resolved_evals_path.is_absolute():
        resolved_evals_path = root / resolved_evals_path
    dataset = load_agent_eval_dataset(resolved_evals_path)
    gepa_module = _load_gepa_optimize_anything()
    seed_preamble = profile.prompt_preamble or ""
    effective_objective = objective or DEFAULT_OBJECTIVE

    def evaluator(candidate: Any, example: dict[str, Any] | None = None) -> float:
        if example is None:
            scores = [
                _score_candidate(
                    candidate,
                    item,
                    log=getattr(gepa_module, "log", lambda message: None),
                )
                for item in dataset
            ]
            return sum(scores) / len(scores)
        return _score_candidate(
            candidate,
            example,
            log=getattr(gepa_module, "log", lambda message: None),
        )

    engine = gepa_module.EngineConfig(max_metric_calls=max_metric_calls)
    config = gepa_module.GEPAConfig(engine=engine)
    gepa_result = gepa_module.optimize_anything(
        seed_candidate=seed_preamble,
        evaluator=evaluator,
        dataset=dataset,
        objective=effective_objective,
        config=config,
    )

    best_preamble = _candidate_to_text(_best_candidate(gepa_result)).strip()
    if not best_preamble:
        best_preamble = seed_preamble
    best_score = _best_score(gepa_result)

    artifact_path = output_path or _default_artifact_path(root, normalized_agent, profile_name)
    artifact_path = artifact_path.expanduser()
    if not artifact_path.is_absolute():
        artifact_path = root / artifact_path
    _write_result_artifact(
        artifact_path,
        {
            "optimizer": "gepa.optimize_anything",
            "agent": normalized_agent,
            "profile": profile_name,
            "objective": effective_objective,
            "evals_path": str(resolved_evals_path),
            "max_metric_calls": max_metric_calls,
            "example_count": len(dataset),
            "seed_preamble": seed_preamble,
            "best_preamble": best_preamble,
            "best_score": best_score,
            "applied": apply,
            "created_at": datetime.now(timezone.utc).isoformat(),
        },
    )

    if apply:
        _apply_profile_preamble(root, profile_name, best_preamble)

    return AgentPromptOptimizationResult(
        agent=normalized_agent,
        profile_name=profile_name,
        seed_preamble=seed_preamble,
        best_preamble=best_preamble,
        best_score=best_score,
        artifact_path=artifact_path,
        applied=apply,
    )


def load_agent_eval_dataset(path: Path) -> list[dict[str, Any]]:
    evals_path = path.expanduser()
    if not evals_path.exists():
        raise ValueError(f"GEPA eval dataset not found: {evals_path}")

    if evals_path.suffix.lower() == ".jsonl":
        examples = [
            json.loads(line)
            for line in evals_path.read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]
    else:
        raw = read_json(evals_path)
        if isinstance(raw, dict):
            if isinstance(raw.get("examples"), list):
                examples = raw["examples"]
            elif isinstance(raw.get("dataset"), list):
                examples = raw["dataset"]
            else:
                examples = [raw]
        elif isinstance(raw, list):
            examples = raw
        else:
            raise ValueError("GEPA eval dataset must be a JSON object, JSON array, or JSONL file.")

    normalized: list[dict[str, Any]] = []
    for index, item in enumerate(examples, start=1):
        if not isinstance(item, dict):
            raise ValueError("Every GEPA eval example must be a JSON object.")
        example = dict(item)
        example.setdefault("id", f"example-{index}")
        normalized.append(example)

    if not normalized:
        raise ValueError("GEPA eval dataset must contain at least one example.")
    return normalized


def _load_gepa_optimize_anything() -> Any:
    injected_module = sys.modules.get("gepa.optimize_anything")
    if injected_module is not None:
        return injected_module
    try:
        return importlib.import_module("gepa.optimize_anything")
    except ImportError as exc:
        raise GepaUnavailableError(
            "GEPA is not installed in this environment. Reinstall or sync DevCouncil dependencies, "
            "then rerun `dev agents optimize`."
        ) from exc


def _score_candidate(candidate: Any, example: dict[str, Any], *, log: Callable[[str], None]) -> float:
    candidate_text = _candidate_to_text(candidate)
    lower_candidate = candidate_text.lower()
    required_terms = _string_list(example, "required_terms", "expected_prompt_fragments", "must_include")
    forbidden_terms = _string_list(example, "forbidden_terms", "must_avoid")

    log(f"Example {example.get('id')}: {example.get('task', '')}".strip())
    for key in ("observed_failure", "desired_behavior", "feedback", "rubric"):
        value = example.get(key)
        if value:
            log(f"{key.replace('_', ' ').title()}: {value}")
    if required_terms:
        log(f"Required prompt terms: {', '.join(required_terms)}")
    if forbidden_terms:
        log(f"Forbidden prompt terms: {', '.join(forbidden_terms)}")

    if not candidate_text.strip():
        log("Candidate preamble is empty.")
        return 0.0

    required_hits = [term for term in required_terms if term.lower() in lower_candidate]
    forbidden_hits = [term for term in forbidden_terms if term.lower() in lower_candidate]
    missing_terms = [term for term in required_terms if term not in required_hits]
    if missing_terms:
        log(f"Missing required terms: {', '.join(missing_terms)}")
    if forbidden_hits:
        log(f"Forbidden terms present: {', '.join(forbidden_hits)}")

    required_score = len(required_hits) / len(required_terms) if required_terms else 0.5
    forbidden_score = 1.0 - (len(forbidden_hits) / len(forbidden_terms)) if forbidden_terms else 1.0
    length_score = _length_score(candidate_text)
    score = (required_score * 0.65) + (forbidden_score * 0.25) + (length_score * 0.10)
    return max(0.0, min(1.0, score))


def _candidate_to_text(candidate: Any) -> str:
    if candidate is None:
        return ""
    if isinstance(candidate, str):
        return candidate
    if isinstance(candidate, dict):
        for key in ("prompt_preamble", "preamble", "prompt", "system_prompt", "text"):
            value = candidate.get(key)
            if isinstance(value, str):
                return value
        return json.dumps(candidate, sort_keys=True)
    for attr in ("prompt_preamble", "preamble", "prompt", "system_prompt", "text"):
        value = getattr(candidate, attr, None)
        if isinstance(value, str):
            return value
    return str(candidate)


def _best_candidate(result: Any) -> Any:
    if isinstance(result, dict):
        for key in ("best_candidate", "candidate", "best"):
            if key in result:
                return result[key]
        return result
    for attr in ("best_candidate", "candidate", "best"):
        if hasattr(result, attr):
            return getattr(result, attr)
    return result


def _best_score(result: Any) -> float | None:
    raw_score: Any
    if isinstance(result, dict):
        raw_score = result.get("best_score", result.get("score"))
    else:
        raw_score = getattr(result, "best_score", getattr(result, "score", None))
    if raw_score is None:
        return None
    try:
        return float(raw_score)
    except (TypeError, ValueError):
        return None


def _string_list(example: dict[str, Any], *keys: str) -> list[str]:
    values: list[str] = []
    for key in keys:
        raw = example.get(key)
        if raw is None:
            continue
        if isinstance(raw, str):
            values.append(raw)
        elif isinstance(raw, Iterable):
            values.extend(str(item) for item in raw if str(item).strip())
    return [value.strip() for value in values if value.strip()]


def _length_score(candidate_text: str) -> float:
    words = candidate_text.split()
    if 10 <= len(words) <= 180:
        return 1.0
    if len(words) < 10:
        return len(words) / 10
    return max(0.0, 1.0 - ((len(words) - 180) / 180))


def _default_artifact_path(project_root: Path, agent: str, profile_name: str) -> Path:
    timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    safe_profile = profile_name.replace("/", "-").replace("\\", "-")
    return project_root / ".devcouncil" / "optimizations" / f"{timestamp}-{agent}-{safe_profile}-gepa.json"


def _write_result_artifact(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    write_json(path, payload)


def _apply_profile_preamble(project_root: Path, profile_name: str, best_preamble: str) -> None:
    config_path = project_root / ".devcouncil" / "config.yaml"
    raw_config = yaml.safe_load(config_path.read_text(encoding="utf-8")) if config_path.exists() else {}
    raw_config = raw_config or {}
    profiles = raw_config.setdefault("integrations", {}).setdefault("cli_agents", {}).setdefault("profiles", {})
    profile = profiles.setdefault(profile_name, {})
    profile["prompt_preamble"] = best_preamble
    config_path.parent.mkdir(parents=True, exist_ok=True)
    atomic_write_text(config_path, yaml.safe_dump(raw_config, sort_keys=False))
