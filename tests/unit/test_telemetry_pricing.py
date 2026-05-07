import json

from devcouncil.telemetry.cost import CostEstimator
from devcouncil.telemetry.pricing import load_model_pricing
from devcouncil.telemetry.tracker import TelemetryTracker


def test_model_pricing_is_loaded_from_resource_file():
    pricing = load_model_pricing()

    assert "openai/gpt-4o" in pricing
    assert pricing["openai/gpt-4o"]["prompt_per_1k"] > 0


def test_cost_estimator_uses_resource_pricing():
    cost = CostEstimator.estimate_cost(
        "openai/gpt-4o",
        {"prompt_tokens": 1000, "completion_tokens": 1000},
    )

    assert cost == 0.02


def test_telemetry_tracker_uses_resource_pricing(tmp_path):
    tracker = TelemetryTracker(tmp_path)

    tracker.log_usage("openai/gpt-4o", {"prompt_tokens": 1000, "completion_tokens": 1000})

    telemetry = json.loads((tmp_path / ".devcouncil" / "logs" / "telemetry.json").read_text(encoding="utf-8"))
    assert telemetry["total_cost"] == 0.02
