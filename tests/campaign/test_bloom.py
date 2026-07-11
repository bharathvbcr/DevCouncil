"""Bloom classification + rank routing."""

from __future__ import annotations

from devcouncil.campaign.bloom import BloomLevel, classify_bloom, route_rank, route_text, summarize_routing
from devcouncil.campaign.roles import Rank


def test_execution_verbs_route_to_worker():
    for text in ["Implement the login form", "Fix the null check", "Add a config flag", "Write the parser"]:
        level = classify_bloom(text)
        assert level <= BloomLevel.APPLY
        assert route_rank(level) is Rank.WORKER


def test_cognition_verbs_route_to_reviewer():
    for text in [
        "Design the storage architecture",
        "Evaluate the two caching strategies",
        "Analyze why the build is flaky",
        "Root-cause the deadlock",
    ]:
        assert route_text(text) is Rank.REVIEWER


def test_explicit_override_wins():
    assert classify_bloom("implement a button", override=BloomLevel.CREATE) is BloomLevel.CREATE


def test_difficulty_hint_used_when_no_keyword():
    # No Bloom verb in the text — fall back to the difficulty hint.
    assert classify_bloom("the wibble", difficulty="hard") is BloomLevel.ANALYZE
    assert route_text("the wibble", difficulty="hard") is Rank.REVIEWER


def test_default_is_apply():
    assert classify_bloom("") is BloomLevel.APPLY


def test_summarize_routing_counts_both_ranks():
    counts = summarize_routing(["Implement X", "Design Y", "Fix Z"])
    assert counts["worker"] == 2
    assert counts["reviewer"] == 1


def test_implementation_verbs_override_evaluate_keywords():
    assert route_text("Fix login bug and review tests") is Rank.WORKER
    assert route_text("Review architecture trade-offs") is Rank.REVIEWER
