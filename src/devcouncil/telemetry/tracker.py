import logging
from pathlib import Path
from typing import Dict, Any

from devcouncil.telemetry.pricing import pricing_for_model
from devcouncil.utils.json_persist import read_json, write_json

logger = logging.getLogger(__name__)


class TelemetryTracker:
    def __init__(self, project_root: Path):
        self.project_root = project_root
        self.log_file = project_root / ".devcouncil" / "logs" / "telemetry.json"
        # log_usage() reloads the ledger immediately before saving (for concurrent-write
        # safety), so any value read here would always be overwritten before use. Start
        # from the same default shape _load() returns for a missing file instead of
        # doing a dead disk read at construction.
        self.stats: Dict[str, Any] = {"total_cost": 0.0, "total_prompt_tokens": 0, "total_completion_tokens": 0, "models": {}}

    def _load(self) -> Dict[str, Any]:
        if self.log_file.exists():
            try:
                return read_json(self.log_file)
            except Exception as e:
                # A corrupt ledger silently resets accumulated cost stats on the next save.
                logger.warning("Failed to load telemetry ledger %s, starting fresh: %s", self.log_file, e)
        return {"total_cost": 0.0, "total_prompt_tokens": 0, "total_completion_tokens": 0, "models": {}}

    def _save(self):
        self.log_file.parent.mkdir(parents=True, exist_ok=True)
        write_json(self.log_file, self.stats)

    def _warn_if_budget_crossed(self, previous_total: float, new_total: float) -> None:
        """Warn (once, at the crossing) when cumulative spend passes the configured budget.

        WARN-ONLY by design — this never raises and never blocks a run. The budget is
        ``telemetry.cost_budget_usd`` in .devcouncil/config.yaml (``dev cost budget --set X``);
        a missing/invalid config or an unset budget silently disables the check. Warning at
        the crossing (previous < budget <= new) rather than on every over-budget call keeps
        one model-call-per-warning noise out of long runs.
        """
        try:
            from devcouncil.app.config import load_config

            budget = load_config(self.project_root).telemetry.cost_budget_usd
        except Exception:
            return
        if budget is None or budget <= 0:
            return
        if previous_total < budget <= new_total:
            logger.warning(
                "Cost budget crossed: cumulative model spend $%.4f now exceeds the "
                "configured budget of $%.2f (telemetry.cost_budget_usd). Warn-only — "
                "runs continue. Review with 'dev cost budget' or 'dev cost show'.",
                new_total,
                budget,
            )

    def log_usage(self, model: str, usage: Dict[str, int], *, local: bool = False):
        # Re-read the ledger immediately before mutating so the whole load->mutate->save
        # runs as one synchronous (await-free) step. The router constructs a fresh tracker
        # per call and only calls log_usage *after* the LLM await, so snapshotting the
        # baseline at construction would let concurrent calls (e.g. plan.py's gather, or a
        # parallelized SkillOpt _evaluate) each load the same baseline and clobber each
        # other's entries on save. Reloading here makes the last writer additive, not lossy.
        self.stats = self._load()
        previous_total = float(self.stats.get("total_cost", 0.0) or 0.0)
        prompt_tokens = usage.get("prompt_tokens", 0)
        completion_tokens = usage.get("completion_tokens", 0)

        if local:
            # On-device providers (Ollama) incur no per-token cost. Zero by provider, not
            # by model-id matching — consistent with telemetry/cost.py — so a local tag
            # that happens to collide with a priced entry is never billed.
            cost = 0.0
        else:
            rates = pricing_for_model(model)
            cost = (prompt_tokens / 1000.0) * rates["prompt_per_1k"] + (
                completion_tokens / 1000.0
            ) * rates["completion_per_1k"]

        # .get with defaults: a hand-edited or partially-written telemetry.json must
        # not crash usage logging — missing keys just restart their counters.
        self.stats["total_cost"] = float(self.stats.get("total_cost", 0.0) or 0.0) + cost
        self.stats["total_prompt_tokens"] = int(self.stats.get("total_prompt_tokens", 0) or 0) + prompt_tokens
        self.stats["total_completion_tokens"] = (
            int(self.stats.get("total_completion_tokens", 0) or 0) + completion_tokens
        )

        models = self.stats.setdefault("models", {})
        if model not in models:
            models[model] = {"cost": 0.0, "prompt_tokens": 0, "completion_tokens": 0}

        self.stats["models"][model]["cost"] += cost
        self.stats["models"][model]["prompt_tokens"] += prompt_tokens
        self.stats["models"][model]["completion_tokens"] += completion_tokens

        self._save()
        self._warn_if_budget_crossed(previous_total, float(self.stats.get("total_cost", 0.0) or 0.0))
