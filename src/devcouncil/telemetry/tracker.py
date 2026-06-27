import json
from pathlib import Path
from typing import Dict, Any

from devcouncil.telemetry.pricing import pricing_for_model

class TelemetryTracker:
    def __init__(self, project_root: Path):
        self.log_file = project_root / ".devcouncil" / "logs" / "telemetry.json"
        self.stats = self._load()

    def _load(self) -> Dict[str, Any]:
        if self.log_file.exists():
            try:
                with open(self.log_file, "r") as f:
                    return json.load(f)
            except Exception:
                pass
        return {"total_cost": 0.0, "total_prompt_tokens": 0, "total_completion_tokens": 0, "models": {}}

    def _save(self):
        self.log_file.parent.mkdir(parents=True, exist_ok=True)
        with open(self.log_file, "w") as f:
            json.dump(self.stats, f, indent=2)

    def log_usage(self, model: str, usage: Dict[str, int], *, local: bool = False):
        # Re-read the ledger immediately before mutating so the whole load->mutate->save
        # runs as one synchronous (await-free) step. The router constructs a fresh tracker
        # per call and only calls log_usage *after* the LLM await, so snapshotting the
        # baseline at construction would let concurrent calls (e.g. plan.py's gather, or a
        # parallelized SkillOpt _evaluate) each load the same baseline and clobber each
        # other's entries on save. Reloading here makes the last writer additive, not lossy.
        self.stats = self._load()
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

        self.stats["total_cost"] += cost
        self.stats["total_prompt_tokens"] += prompt_tokens
        self.stats["total_completion_tokens"] += completion_tokens

        if model not in self.stats["models"]:
            self.stats["models"][model] = {"cost": 0.0, "prompt_tokens": 0, "completion_tokens": 0}

        self.stats["models"][model]["cost"] += cost
        self.stats["models"][model]["prompt_tokens"] += prompt_tokens
        self.stats["models"][model]["completion_tokens"] += completion_tokens

        self._save()
