import logging
from typing import Dict

logger = logging.getLogger(__name__)

class CostEstimator:
    """Estimates LLM usage cost based on provider pricing."""
    
    # Rough estimates, update for production
    PRICING = {
        "anthropic/claude-3.5-sonnet": {"input": 0.000003, "output": 0.000015},
        "anthropic/claude-3-opus": {"input": 0.000015, "output": 0.000075},
        "anthropic/claude-sonnet-4": {"input": 0.000003, "output": 0.000015},
        "google/gemini-pro-1.5": {"input": 0.00000125, "output": 0.000005},
        "google/gemini-2.5-pro": {"input": 0.00000125, "output": 0.00001},
        "openai/gpt-4o": {"input": 0.000005, "output": 0.000015},
        "openai/o3-mini": {"input": 0.0000011, "output": 0.0000044},
    }

    # Conservative default for unknown models
    DEFAULT_PRICING = {"input": 0.000005, "output": 0.000015}

    @classmethod
    def estimate_cost(cls, model: str, usage: Dict[str, int]) -> float:
        prices = cls.PRICING.get(model)
        if not prices:
            logger.debug("Unknown model for cost estimation: %s — using default pricing", model)
            prices = cls.DEFAULT_PRICING
            
        prompt_tokens = usage.get("prompt_tokens", 0)
        completion_tokens = usage.get("completion_tokens", 0)
        
        cost = (prompt_tokens * prices["input"]) + (completion_tokens * prices["output"])
        return cost
