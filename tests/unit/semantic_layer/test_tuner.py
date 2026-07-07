"""Tests for ThresholdAutoTuner."""

from __future__ import annotations

from semantic_layer.config import CacheConfig
from semantic_layer.tuner import ThresholdAutoTuner


def test_suggest_threshold_with_no_data_returns_max():
    config = CacheConfig(similarity_threshold=0.92)
    tuner = ThresholdAutoTuner(config)
    assert tuner.suggest_threshold() == ThresholdAutoTuner.TAU_MAX


def test_apply_lowers_threshold_when_false_positives_detected():
    config = CacheConfig(similarity_threshold=0.92)
    tuner = ThresholdAutoTuner(config)

    for _ in range(25):
        tuner.record(similarity=0.97, was_hit=True, was_correct=False)

    new_tau = tuner.apply()
    assert new_tau < 0.97
    assert config.similarity_threshold == new_tau


def test_fpr_estimate_with_insufficient_labels_is_zero():
    config = CacheConfig()
    tuner = ThresholdAutoTuner(config)
    tuner.record(similarity=0.95, was_hit=True, was_correct=None)
    assert tuner._estimate_fpr(0.90) == 0.0
