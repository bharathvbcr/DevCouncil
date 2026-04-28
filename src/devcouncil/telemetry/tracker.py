import json
from pathlib import Path
from typing import Dict, Any

COST_PER_1K_TOKENS = {
    "anthropic/claude-3-opus": {"prompt": 0.015, "completion": 0.075},
    "anthropic/claude-3.5-sonnet": {"prompt": 0.003, "completion": 0.015},
    "openai/gpt-4o": {"prompt": 0.005, "completion": 0.015},
    "google/gemini-pro-1.5": {"prompt": 0.00125, "completion": 0.00375},
}

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

    def log_usage(self, model: str, usage: Dict[str, int]):
        prompt_tokens = usage.get("prompt_tokens", 0)
        completion_tokens = usage.get("completion_tokens", 0)

        rates = COST_PER_1K_TOKENS.get(model, {"prompt": 0.0, "completion": 0.0})
        cost = (prompt_tokens / 1000.0) * rates["prompt"] + (completion_tokens / 1000.0) * rates["completion"]

        self.stats["total_cost"] += cost
        self.stats["total_prompt_tokens"] += prompt_tokens
        self.stats["total_completion_tokens"] += completion_tokens

        if model not in self.stats["models"]:
            self.stats["models"][model] = {"cost": 0.0, "prompt_tokens": 0, "completion_tokens": 0}

        self.stats["models"][model]["cost"] += cost
        self.stats["models"][model]["prompt_tokens"] += prompt_tokens
        self.stats["models"][model]["completion_tokens"] += completion_tokens

        self._save()
