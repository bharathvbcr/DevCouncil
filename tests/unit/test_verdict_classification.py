"""Verdict classification: infra vs quality incomplete."""

from devcouncil.domain.gap import Gap
from devcouncil.reporting.verdict import classify_incomplete_kind, classify_verdict


class _Graph:
    def __init__(self, gaps, summary):
        self._gaps = gaps
        self._summary = summary

    def coverage_summary(self):
        return self._summary

    def blocking_gaps(self):
        return [g for g in self._gaps if g.blocking]

    def all_gaps(self):
        return list(self._gaps)


def test_quality_incomplete_by_default():
    graph = _Graph(
        [],
        {"blocking_gaps": 0, "ac_without_evidence": 1},
    )
    verdict, kind = classify_verdict(graph)
    assert verdict == "incomplete"
    assert kind == "quality"


def test_infra_incomplete_from_agent_run():
    graph = _Graph([], {"blocking_gaps": 0, "ac_without_evidence": 1})
    verdict, kind = classify_verdict(
        graph,
        agent_run={"returncode": 1, "stderr_preview": ["You've hit your session limit"]},
    )
    assert verdict == "incomplete"
    assert kind == "infra"


def test_infra_incomplete_from_gap_description():
    graph = _Graph(
        [Gap(
            id="G1", task_id="T", gap_type="invalid_verification_command",
            description="claude agent sdk is not installed", severity="high", blocking=False,
            recommended_fix="install sdk",
        )],
        {"blocking_gaps": 0, "ac_without_evidence": 1},
    )
    assert classify_incomplete_kind(graph) == "infra"
