"""Bloom classification + rank routing."""

from __future__ import annotations

from devcouncil.shogun.bloom import BloomLevel, classify_bloom, route_rank, route_text, summarize_routing
from devcouncil.shogun.roles import Rank


def test_execution_verbs_route_to_ashigaru():
    for text in ["Implement the login form", "Fix the null check", "Add a config flag", "Write the parser"]:
        level = classify_bloom(text)
        assert level <= BloomLevel.APPLY
        assert route_rank(level) is Rank.ASHIGARU


def test_cognition_verbs_route_to_gunshi():
    for text in [
        "Design the storage architecture",
        "Evaluate the two caching strategies",
        "Analyze why the build is flaky",
        "Root-cause the deadlock",
    ]:
        assert route_text(text) is Rank.GUNSHI


def test_explicit_override_wins():
    assert classify_bloom("implement a button", override=BloomLevel.CREATE) is BloomLevel.CREATE


def test_difficulty_hint_used_when_no_keyword():
    # No Bloom verb in the text — fall back to the difficulty hint.
    assert classify_bloom("the wibble", difficulty="hard") is BloomLevel.ANALYZE
    assert route_text("the wibble", difficulty="hard") is Rank.GUNSHI


def test_default_is_apply():
    assert classify_bloom("") is BloomLevel.APPLY


def test_summarize_routing_counts_both_ranks():
    counts = summarize_routing(["Implement X", "Design Y", "Fix Z"])
    assert counts["ashigaru"] == 2
    assert counts["gunshi"] == 1
