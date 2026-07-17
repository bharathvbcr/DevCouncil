"""Claim lie-detector: map completion text → assertions → independent checks."""

from __future__ import annotations

from devcouncil.verification.claims.checks import execute_checks
from devcouncil.verification.claims.mapper import map_claims
from devcouncil.verification.claims.models import Assertion, CheckResult, Kind, Status
from devcouncil.verification.claims.transcript import last_assistant_text, last_assistant_sentence
from devcouncil.verification.claims.verdict import ClaimVerdict, decide_claims, summary_line

__all__ = [
    "Assertion",
    "CheckResult",
    "ClaimVerdict",
    "Kind",
    "Status",
    "decide_claims",
    "execute_checks",
    "last_assistant_sentence",
    "last_assistant_text",
    "map_claims",
    "summary_line",
]
