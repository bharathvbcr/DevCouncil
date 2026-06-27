"""Tests for the Skill <-> OKF document bridge (single source of truth for interop)."""

from __future__ import annotations

from devcouncil.knowledge.okf import OKFDocument
from devcouncil.knowledge.skill_bridge import (
    SKILL_OKF_TYPE,
    is_skill_document,
    okf_document_to_skill,
    skill_to_okf_document,
)
from devcouncil.skills.registry import Skill, SkillTriggers


def _make_skill() -> Skill:
    return Skill(
        name="android",
        title="Android Engineering",
        description="Guidance for Android app work.",
        always=False,
        triggers=SkillTriggers(keywords=["kotlin", "gradle", "android"], globs=["*.kt"]),
        body="Use Jetpack Compose for new UI.",
        source_path=None,
    )


def test_round_trip_preserves_core_fields() -> None:
    skill = _make_skill()
    doc = skill_to_okf_document(skill)
    back = okf_document_to_skill(doc)
    assert back is not None
    assert back.name == skill.name
    assert back.description == skill.description
    assert back.body == skill.body
    # Keywords survive (order-independent); globs are not representable in OKF tags.
    assert set(back.triggers.keywords) == set(skill.triggers.keywords)


def test_type_field_is_skill_okf_type() -> None:
    doc = skill_to_okf_document(_make_skill())
    assert doc.type == SKILL_OKF_TYPE
    assert is_skill_document(doc)


def test_tags_are_sorted_and_deduped() -> None:
    skill = Skill(
        name="web",
        description="",
        triggers=SkillTriggers(keywords=["react", "react", "css", "api"]),
        body="x",
    )
    doc = skill_to_okf_document(skill)
    assert doc.tags == ["api", "css", "react"]


def test_non_skill_document_returns_none() -> None:
    doc = OKFDocument(
        type="BigQuery Table",
        title="events",
        description="event log table",
        rel_path="tables/events.md",
    )
    assert is_skill_document(doc) is False
    assert okf_document_to_skill(doc) is None


def test_rel_path_uses_skills_dir_and_name() -> None:
    doc = skill_to_okf_document(_make_skill())
    assert doc.rel_path == "skills/android.md"


def test_rel_path_stem_drives_name_on_ingest() -> None:
    doc = OKFDocument(
        type=SKILL_OKF_TYPE,
        title="Some Title",
        description="d",
        tags=["go"],
        body="b",
        rel_path="skills/golang.md",
    )
    skill = okf_document_to_skill(doc)
    assert skill is not None
    assert skill.name == "golang"


def test_name_falls_back_to_title_slug_without_rel_path() -> None:
    doc = OKFDocument(
        type=SKILL_OKF_TYPE,
        title="iOS & SwiftUI",
        body="b",
    )
    skill = okf_document_to_skill(doc)
    assert skill is not None
    assert skill.name == "ios-swiftui"


def test_is_skill_document_is_case_insensitive() -> None:
    doc = OKFDocument(type="  engineering skill  ", title="t")
    assert is_skill_document(doc) is True
