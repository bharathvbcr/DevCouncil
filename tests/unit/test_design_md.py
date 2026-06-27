"""design.md parsing, linting, and export."""

import json

from devcouncil.knowledge.design import (
    contrast_ratio,
    export,
    lint,
    parse_design_md,
)

GOOD = """---
name: Acme
colors:
  primary: "#1a1a1a"
  surface: "#ffffff"
typography:
  body:
    fontFamily: Inter
    fontSize: 16px
rounded:
  md: 8px
components:
  button:
    backgroundColor: colors.primary
    textColor: colors.surface
    rounded: rounded.md
---
# Overview
Acme system.
# Colors
Primary is near-black.
"""


def test_parse_extracts_tokens_and_sections():
    ds = parse_design_md(GOOD)
    assert ds.name == "Acme"
    assert ds.colors["primary"] == "#1a1a1a"
    assert ds.components["button"]["backgroundColor"] == "colors.primary"
    assert [h for h, _ in ds.sections] == ["Overview", "Colors"]


def test_lint_clean_on_good_system():
    assert lint(parse_design_md(GOOD)) == []


def test_lint_flags_broken_token_reference():
    bad = GOOD.replace("rounded: rounded.md", "rounded: rounded.nope")
    rules = {f.rule for f in lint(parse_design_md(bad))}
    assert "broken-token-reference" in rules


def test_lint_flags_low_contrast():
    bad = GOOD.replace('surface: "#ffffff"', 'surface: "#1b1b1b"')
    findings = lint(parse_design_md(bad))
    assert any(f.rule == "contrast-ratio" for f in findings)


def test_lint_flags_missing_primary_and_bad_order():
    src = """---
colors:
  brand: "#123456"
components:
  card:
    backgroundColor: colors.brand
---
# Colors
c
# Overview
o
"""
    rules = {f.rule for f in lint(parse_design_md(src))}
    assert "missing-primary-color" in rules
    assert "section-ordering" in rules


def test_contrast_ratio_black_on_white_is_21():
    assert round(contrast_ratio("#000000", "#ffffff"), 1) == 21.0
    assert contrast_ratio("oklch(0.6 0.1 20)", "#fff") is None  # non-hex → unknown


def test_export_css_tailwind_w3c():
    ds = parse_design_md(GOOD)
    css = export(ds, "css")
    assert "--color-primary: #1a1a1a;" in css

    tailwind = export(ds, "tailwind")
    assert "module.exports" in tailwind
    # The config body is valid JSON after the assignment.
    body = tailwind.split("module.exports =", 1)[1].rsplit(";", 1)[0]
    assert json.loads(body)["theme"]["extend"]["colors"]["primary"] == "#1a1a1a"

    w3c = json.loads(export(ds, "w3c"))
    assert w3c["color"]["primary"] == {"$value": "#1a1a1a", "$type": "color"}
