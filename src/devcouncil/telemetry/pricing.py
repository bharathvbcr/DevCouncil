from functools import lru_cache
from importlib import resources
from typing import Dict

import yaml

MODEL_PRICING_RESOURCE = "model_pricing.yaml"


@lru_cache(maxsize=1)
def load_model_pricing() -> Dict[str, Dict[str, float]]:
    data = resources.files(__package__).joinpath(MODEL_PRICING_RESOURCE).read_text(encoding="utf-8")
    loaded = yaml.safe_load(data) or {}
    return {
        str(model): {
            "prompt_per_1k": float(pricing.get("prompt_per_1k", 0.0)),
            "completion_per_1k": float(pricing.get("completion_per_1k", 0.0)),
        }
        for model, pricing in loaded.items()
        if isinstance(pricing, dict)
    }


def pricing_for_model(model: str, default: Dict[str, float] | None = None) -> Dict[str, float]:
    pricing = load_model_pricing().get(model)
    if pricing is not None:
        return pricing
    return default or {"prompt_per_1k": 0.0, "completion_per_1k": 0.0}
