"""Shared frontmatter util — round-trip, edge cases, and skills-compat."""

from devcouncil.knowledge.frontmatter import build_frontmatter_markdown, split_frontmatter
from devcouncil.skills.registry import Skill


def test_split_build_round_trip():
    md = build_frontmatter_markdown(
        {"type": "X", "title": "T", "tags": ["a", "b"]}, "Body line one.\n\nBody line two."
    )
    meta, body = split_frontmatter(md)
    assert meta["type"] == "X"
    assert meta["title"] == "T"
    assert meta["tags"] == ["a", "b"]
    assert body.strip() == "Body line one.\n\nBody line two."


def test_build_drops_empty_values_and_preserves_order():
    md = build_frontmatter_markdown({"type": "X", "title": "", "tags": [], "resource": None}, "b")
    meta, _ = split_frontmatter(md)
    assert meta == {"type": "X"}
    # 'type' is the first (and only) key kept.
    assert md.splitlines()[1] == "type: X"


def test_no_frontmatter_returns_text_unchanged():
    meta, body = split_frontmatter("just a body, no fences")
    assert meta == {}
    assert body == "just a body, no fences"


def test_malformed_yaml_degrades_gracefully():
    meta, body = split_frontmatter("---\n: : : bad\n---\nbody")
    assert meta == {}
    assert body.strip() == "body"


def test_skill_to_skill_md_uses_shared_builder():
    skill = Skill(name="demo", description="A demo skill.", body="Do the thing.")
    rendered = skill.to_skill_md()
    meta, body = split_frontmatter(rendered)
    assert meta["name"] == "demo"
    assert meta["description"] == "A demo skill."
    assert body.strip() == "Do the thing."


def test_crlf_body_has_no_leading_carriage_return():
    meta, body = split_frontmatter("---\r\ntype: X\r\n---\r\nBody line.\r\n")
    assert meta["type"] == "X"
    assert not body.startswith("\r")
    assert body.startswith("Body line.")
