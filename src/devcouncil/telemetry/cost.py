import json
import logging
from pathlib import Path
from typing import Any, Dict, List

from devcouncil.telemetry.pricing import pricing_for_model

logger = logging.getLogger(__name__)

UNATTRIBUTED = "(unattributed)"


class CostEstimator:
    """Estimates LLM usage cost based on provider pricing."""

    # Conservative default for unknown models
    DEFAULT_PRICING = {"prompt_per_1k": 0.005, "completion_per_1k": 0.015}

    # Local providers run on-device and incur no per-token cost. Ollama model ids
    # are open-ended (e.g. ``qwen2.5-coder:7b`` or an ``ollama/<name>`` form), so
    # match the conventional prefixes rather than relying on the open-ended yaml.
    LOCAL_MODEL_PREFIXES = ("ollama/", "ollama:")

    @classmethod
    def _is_local_model(cls, model: str) -> bool:
        return model.startswith(cls.LOCAL_MODEL_PREFIXES)

    @classmethod
    def estimate_cost(cls, model: str, usage: Dict[str, int]) -> float:
        # Local/Ollama models are free regardless of yaml coverage; short-circuit
        # before the conservative DEFAULT_PRICING fallback would bill them.
        if cls._is_local_model(model):
            return 0.0
        prices = pricing_for_model(model, cls.DEFAULT_PRICING)
        if prices == cls.DEFAULT_PRICING:
            logger.debug("Unknown model for cost estimation: %s — using default pricing", model)
            
        prompt_tokens = usage.get("prompt_tokens", 0)
        completion_tokens = usage.get("completion_tokens", 0)
        
        cost = ((prompt_tokens / 1000.0) * prices["prompt_per_1k"]) + (
            (completion_tokens / 1000.0) * prices["completion_per_1k"]
        )
        return cost


def _model_calls_file(project_root: Path) -> Path:
    return project_root / ".devcouncil" / "logs" / "model_calls.jsonl"


def read_cost_records(project_root: Path) -> List[Dict[str, Any]]:
    """Read the model-call ledger and attach an estimated cost to each record.

    Never raises: malformed lines are skipped. Records missing ``task_id`` /
    ``run_id`` (older entries written before per-task attribution) keep those as
    ``None`` so callers can bucket them under ``(unattributed)``.
    """
    log_file = _model_calls_file(project_root)
    records: List[Dict[str, Any]] = []
    if not log_file.exists():
        return records
    try:
        lines = log_file.read_text(encoding="utf-8").splitlines()
    except Exception as exc:
        logger.debug("Failed to read model_calls ledger: %s", exc)
        return records

    for line in lines:
        if not line.strip():
            continue
        try:
            entry = json.loads(line)
        except Exception as exc:
            logger.debug("Skipping invalid model_calls line: %s", exc)
            continue
        model = ""
        response = entry.get("response")
        if isinstance(response, dict):
            model = str(response.get("model", "") or "")
        raw_usage = entry.get("usage")
        usage: Dict[str, Any] = raw_usage if isinstance(raw_usage, dict) else {}
        # Local providers (ollama) are always free, regardless of the open-ended model
        # tag Ollama echoes back (e.g. ``mistral:latest``) — trust the recorded provider
        # over fragile model-id prefix matching.
        provider = str(entry.get("provider") or "")
        try:
            cost = 0.0 if provider == "ollama" else CostEstimator.estimate_cost(model, usage)
        except Exception:
            cost = 0.0
        records.append(
            {
                "task_id": entry.get("task_id"),
                "run_id": entry.get("run_id"),
                "timestamp": entry.get("timestamp"),
                "model": model,
                "usage": usage,
                "cost": cost,
            }
        )
    return records


def _group(records: List[Dict[str, Any]], key: str) -> Dict[str, Dict[str, Any]]:
    groups: Dict[str, Dict[str, Any]] = {}
    for record in records:
        raw = record.get(key)
        bucket = str(raw) if isinstance(raw, str) and raw else UNATTRIBUTED
        group = groups.setdefault(
            bucket,
            {"cost": 0.0, "calls": 0, "prompt_tokens": 0, "completion_tokens": 0},
        )
        raw_usage = record.get("usage")
        usage: Dict[str, Any] = raw_usage if isinstance(raw_usage, dict) else {}
        group["cost"] += float(record.get("cost", 0.0) or 0.0)
        group["calls"] += 1
        group["prompt_tokens"] += int(usage.get("prompt_tokens", 0) or 0)
        group["completion_tokens"] += int(usage.get("completion_tokens", 0) or 0)
    return groups


def group_cost(project_root: Path) -> Dict[str, Any]:
    """Aggregate model-call cost grouped by ``task_id`` and ``run_id``.

    Returns a JSON-friendly summary: a grand total plus per-task and per-run
    breakdowns. Unattributed records (older entries, or calls made without a
    task/run context) are bucketed under ``(unattributed)``. Never raises.
    """
    records = read_cost_records(project_root)
    total_cost = sum(float(record.get("cost", 0.0) or 0.0) for record in records)
    return {
        "total_cost": total_cost,
        "total_calls": len(records),
        "by_task": _group(records, "task_id"),
        "by_run": _group(records, "run_id"),
    }


def cost_by_task(project_root: Path) -> Dict[str, Dict[str, Any]]:
    """Convenience accessor for the per-task cost breakdown (used by ``dev status``)."""
    return group_cost(project_root)["by_task"]
