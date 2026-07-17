"""Unit tests for corpus-related verify gates."""

from __future__ import annotations

from pathlib import Path

import pytest

from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.indexing.wiring import CorpusGraph, CorpusNode, write_corpus_graph
from devcouncil.verification.checks.acceptance_corpus import detect_acceptance_corpus_gaps
from devcouncil.verification.checks.corpus_stale import detect_corpus_stale_gaps
from devcouncil.verification.checks.doc_code_ref import detect_doc_code_ref_gaps


def _gap_id(task_id: str, kind: str) -> str:
    return f"GAP-{task_id}-{kind}"


def _write_corpus(root: Path, *, label: str = "Authentication") -> None:
    graph = CorpusGraph(
        source_roots=["docs"],
        nodes=[
            CorpusNode(
                id="sec-1",
                kind="section",
                label=label,
                path="docs/corpus.md",
                content=f"Section about {label}",
            )
        ],
        edges=[],
    )
    write_corpus_graph(root, graph)


class TestCorpusStaleGate:
    def test_skips_when_disabled(self, tmp_path: Path):
        task = Task(id="T1", title="t", description="d", planned_files=[
            PlannedFile(path="docs/foo.md", reason="r", allowed_change="modify"),
        ])
        gaps = detect_corpus_stale_gaps(
            task=task,
            project_root=tmp_path,
            changed_files=[],
            next_gap_id=_gap_id,
            corpus_stale_enabled=False,
        )
        assert gaps == []

    def test_flags_missing_corpus_for_doc_touch(self, tmp_path: Path):
        (tmp_path / "docs").mkdir()
        (tmp_path / "docs" / "foo.md").write_text("# Doc\n", encoding="utf-8")
        task = Task(id="T1", title="t", description="d", planned_files=[
            PlannedFile(path="docs/foo.md", reason="r", allowed_change="modify"),
        ])
        gaps = detect_corpus_stale_gaps(
            task=task,
            project_root=tmp_path,
            changed_files=["docs/foo.md"],
            next_gap_id=_gap_id,
            corpus_stale_blocking=False,
        )
        assert len(gaps) == 1
        assert gaps[0].gap_type == "corpus_stale"
        assert gaps[0].blocking is False

    def test_no_gap_when_corpus_fresh(self, tmp_path: Path):
        (tmp_path / "docs").mkdir()
        (tmp_path / "docs" / "foo.md").write_text("# Doc\n", encoding="utf-8")
        _write_corpus(tmp_path)
        task = Task(id="T1", title="t", description="d", planned_files=[
            PlannedFile(path="docs/foo.md", reason="r", allowed_change="modify"),
        ])
        gaps = detect_corpus_stale_gaps(
            task=task,
            project_root=tmp_path,
            changed_files=["docs/foo.md"],
            next_gap_id=_gap_id,
        )
        assert gaps == []


class TestDocCodeRefGate:
    def test_flags_broken_code_ref_in_changed_doc(self, tmp_path: Path):
        doc = tmp_path / "docs" / "guide.md"
        doc.parent.mkdir(parents=True)
        doc.write_text("See src/devcouncil/missing_module.py for details.\n", encoding="utf-8")
        task = Task(id="T1", title="t", description="d")
        gaps = detect_doc_code_ref_gaps(
            task=task,
            project_root=tmp_path,
            changed_files=["docs/guide.md"],
            diff_content="",
            next_gap_id=_gap_id,
            doc_code_ref_blocking=False,
        )
        assert len(gaps) == 1
        assert gaps[0].gap_type == "doc_code_ref"
        assert "missing_module.py" in gaps[0].description

    def test_no_gap_for_valid_code_ref(self, tmp_path: Path):
        src = tmp_path / "src" / "devcouncil" / "app"
        src.mkdir(parents=True)
        (src / "config.py").write_text("x = 1\n", encoding="utf-8")
        doc = tmp_path / "docs" / "guide.md"
        doc.parent.mkdir(parents=True)
        doc.write_text("See src/devcouncil/app/config.py.\n", encoding="utf-8")
        task = Task(id="T1", title="t", description="d")
        gaps = detect_doc_code_ref_gaps(
            task=task,
            project_root=tmp_path,
            changed_files=["docs/guide.md"],
            diff_content="",
            next_gap_id=_gap_id,
        )
        assert gaps == []


class TestAcceptanceCorpusGate:
    def _requirements(self) -> list[Requirement]:
        return [
            Requirement(
                id="REQ-1",
                title="Docs",
                description="Doc alignment",
                priority="high",
                acceptance_criteria=[
                    AcceptanceCriterion(
                        id="AC-DOC",
                        description="Behavior matches docs/corpus.md verification gates section.",
                        verification_method="manual",
                    ),
                ],
            )
        ]

    def test_skips_non_doc_acceptance_criteria(self, tmp_path: Path):
        reqs = [
            Requirement(
                id="REQ-1",
                title="Code",
                description="d",
                priority="high",
                acceptance_criteria=[
                    AcceptanceCriterion(
                        id="AC-CODE",
                        description="Function returns 42.",
                        verification_method="unit_test",
                    ),
                ],
            )
        ]
        task = Task(
            id="T1",
            title="t",
            description="d",
            acceptance_criterion_ids=["AC-CODE"],
        )
        gaps = detect_acceptance_corpus_gaps(
            task=task,
            requirements=reqs,
            project_root=tmp_path,
            changed_files=[],
            diff_content="",
            next_gap_id=_gap_id,
        )
        assert gaps == []

    def test_flags_missing_corpus_and_evidence(self, tmp_path: Path):
        task = Task(
            id="T1",
            title="t",
            description="d",
            acceptance_criterion_ids=["AC-DOC"],
        )
        gaps = detect_acceptance_corpus_gaps(
            task=task,
            requirements=self._requirements(),
            project_root=tmp_path,
            changed_files=[],
            diff_content="",
            next_gap_id=_gap_id,
            acceptance_corpus_blocking=False,
        )
        assert len(gaps) == 1
        assert gaps[0].gap_type == "acceptance_corpus"
        assert gaps[0].acceptance_criterion_id == "AC-DOC"
        assert gaps[0].blocking is False

    def test_passes_with_corpus_hit(self, tmp_path: Path):
        (tmp_path / "docs").mkdir()
        (tmp_path / "docs" / "corpus.md").write_text("# Corpus\n", encoding="utf-8")
        _write_corpus(tmp_path, label="verification gates")
        task = Task(
            id="T1",
            title="t",
            description="d",
            acceptance_criterion_ids=["AC-DOC"],
        )
        gaps = detect_acceptance_corpus_gaps(
            task=task,
            requirements=self._requirements(),
            project_root=tmp_path,
            changed_files=[],
            diff_content="",
            next_gap_id=_gap_id,
        )
        assert gaps == []

    def test_passes_with_explicit_evidence_path(self, tmp_path: Path):
        doc = tmp_path / "docs" / "corpus.md"
        doc.parent.mkdir(parents=True)
        doc.write_text("# Corpus\n", encoding="utf-8")
        task = Task(
            id="T1",
            title="t",
            description="d",
            acceptance_criterion_ids=["AC-DOC"],
            planned_files=[
                PlannedFile(path="docs/corpus.md", reason="evidence", allowed_change="modify"),
            ],
        )
        gaps = detect_acceptance_corpus_gaps(
            task=task,
            requirements=self._requirements(),
            project_root=tmp_path,
            changed_files=["docs/corpus.md"],
            diff_content="docs/corpus.md",
            next_gap_id=_gap_id,
        )
        assert gaps == []


@pytest.mark.parametrize(
    "mode,is_hard,expected_blocking",
    [
        ("never", True, False),
        ("soft", False, False),
        ("soft", True, True),
        ("hard", True, True),
        ("always", False, True),
    ],
)
def test_acceptance_corpus_rigor_modes(mode, is_hard, expected_blocking):
    from devcouncil.verification.difficulty import _soft_mode_flags

    enabled, blocking = _soft_mode_flags(mode, is_hard)
    if mode == "never":
        assert enabled is False
    else:
        assert enabled is True
    assert blocking is expected_blocking
