"""Tests for loading ingested OKF documents as selectable skills (ingest direction).

These cover the bridge between an ingested OKF bundle (``.devcouncil/knowledge/okf``) and
the skills registry: skill-typed nodes become real Skills that select alongside the
packaged library, while non-skill nodes are ignored and local skills win name conflicts.
"""

from __future__ import annotations

from pathlib import Path

from devcouncil.knowledge.frontmatter import build_frontmatter_markdown
from devcouncil.knowledge.skill_bridge import SKILL_OKF_TYPE
from devcouncil.skills.registry import load_skills, select_skills


def _write_okf_doc(project_root: Path, name: str, meta: dict, body: str) -> Path:
    """Write an OKF markdown doc under ``<root>/.devcouncil/knowledge/okf/<name>.md``."""
    okf_dir = project_root / ".devcouncil" / "knowledge" / "okf"
    okf_dir.mkdir(parents=True, exist_ok=True)
    path = okf_dir / f"{name}.md"
    path.write_text(build_frontmatter_markdown(meta, body), encoding="utf-8")
    return path


def test_ingested_okf_skill_loads_with_name_keywords_and_body(tmp_path: Path) -> None:
    _write_okf_doc(
        tmp_path,
        "graphql-api",
        {
            "type": SKILL_OKF_TYPE,
            "title": "GraphQL API Intake",
            "description": "Guidance for GraphQL work.",
            "tags": ["foo", "bar"],
        },
        "Prefer schema-first design.",
    )
    skills = load_skills(project_root=tmp_path)
    loaded = next((s for s in skills if s.name == "graphql-api"), None)
    assert loaded is not None
    assert set(loaded.triggers.keywords) == {"foo", "bar"}
    assert loaded.body == "Prefer schema-first design."
    # OKF tags carry only keywords; globs are not representable.
    assert loaded.triggers.globs == []


def test_select_skills_picks_ingested_okf_skill_on_keyword(tmp_path: Path) -> None:
    _write_okf_doc(
        tmp_path,
        "graphql-api",
        {
            "type": SKILL_OKF_TYPE,
            "title": "GraphQL API Intake",
            "description": "d",
            "tags": ["foo", "bar"],
        },
        "Prefer schema-first design.",
    )
    selected = select_skills("Build something with foo support", project_root=tmp_path)
    assert any(s.name == "graphql-api" for s in selected)


def test_non_skill_okf_doc_is_not_loaded_as_skill(tmp_path: Path) -> None:
    _write_okf_doc(
        tmp_path,
        "events",
        {
            "type": "BigQuery Table",
            "title": "events",
            "description": "event log table",
            "tags": ["analytics"],
        },
        "Columns: id, ts, payload.",
    )
    names = {s.name for s in load_skills(project_root=tmp_path)}
    assert "events" not in names


def test_library_skill_not_overridden_by_okf_doc(tmp_path: Path) -> None:
    # An ingested OKF doc colliding with a packaged library skill name must NOT win:
    # the local/library definition is authoritative.
    _write_okf_doc(
        tmp_path,
        "android",
        {
            "type": SKILL_OKF_TYPE,
            "title": "Hijacked Android",
            "description": "should not replace the library skill",
            "tags": ["zzz-okf-only"],
        },
        "OKF body that must not appear.",
    )
    android = next(s for s in load_skills(project_root=tmp_path) if s.name == "android")
    assert "zzz-okf-only" not in android.triggers.keywords
    assert android.body != "OKF body that must not appear."
