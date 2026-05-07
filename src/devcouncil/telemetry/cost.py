import logging
from typing import Dict

from devcouncil.telemetry.pricing import pricing_for_model

logger = logging.getLogger(__name__)

class CostEstimator:
    """Estimates LLM usage cost based on provider pricing."""

    # Conservative default for unknown models
    DEFAULT_PRICING = {"prompt_per_1k": 0.005, "completion_per_1k": 0.015}

    @classmethod
    def estimate_cost(cls, model: str, usage: Dict[str, int]) -> float:
        prices = pricing_for_model(model, cls.DEFAULT_PRICING)
        if prices == cls.DEFAULT_PRICING:
            logger.debug("Unknown model for cost estimation: %s — using default pricing", model)
            
        prompt_tokens = usage.get("prompt_tokens", 0)
        completion_tokens = usage.get("completion_tokens", 0)
        
        cost = ((prompt_tokens / 1000.0) * prices["prompt_per_1k"]) + (
            (completion_tokens / 1000.0) * prices["completion_per_1k"]
        )
        return cost
