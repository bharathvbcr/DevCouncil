"""Corpus verification gate facade (re-exports split gate modules)."""

from __future__ import annotations

from devcouncil.verification.checks.acceptance_corpus import detect_acceptance_corpus_gaps
from devcouncil.verification.checks.corpus_stale import detect_corpus_stale_gaps
from devcouncil.verification.checks.doc_code_ref import detect_doc_code_ref_gaps

__all__ = ["detect_acceptance_corpus_gaps", "detect_corpus_stale_gaps", "detect_doc_code_ref_gaps"]