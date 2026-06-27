"""design.md (google-labs-code, alpha) — model, lint, and export.

A design.md file pairs machine-readable design *tokens* (YAML frontmatter: colors,
typography, rounded, spacing, components) with human-readable rationale (a markdown body
of canonical sections). DevCouncil parses it so the design system can be (a) injected as
agent context and (b) validated/converted, mirroring the upstream ``@google/design.md``
CLI's ``lint`` and ``export`` subcommands.
"""

from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any, Literal

from pydantic import BaseModel, Field

from devcouncil.knowledge.frontmatter import split_frontmatter
# Cycle-safe: knowledge.okf does not import this module (or the skills package).
from devcouncil.knowledge.okf import OKFDocument

# Canonical section order from the design.md spec; sections that ARE present must appear
# in this relative order. Lowercased for comparison.
CANONICAL_SECTIONS = [
    "overview",
    "colors",
    "typography",
    "layout",
    "elevation & depth",
    "shapes",
    "components",
    "do's and don'ts",
]
# O(1) membership companion to the ordered list above.
_CANONICAL_SET = frozenset(CANONICAL_SECTIONS)

# Token categories a component property may reference (e.g. "colors.primary").
_TOKEN_CATEGORIES = ("colors", "typography", "rounded", "spacing")

# A token reference is either dotted (colors.primary) or brace-wrapped ({colors.primary}).
_REF_RE = re.compile(r"^\{?\s*(?P<cat>colors|typography|rounded|spacing)\.(?P<name>[\w-]+)\s*\}?$")
_HEX_RE = re.compile(r"^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$")
_HEADING_RE = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")

Severity = Literal["error", "warning", "info"]


class Finding(BaseModel):
    """A single lint finding."""

    rule: str
    severity: Severity
    message: str

    def format(self) -> str:
        return f"[{self.severity}] {self.rule}: {self.message}"


class DesignSystem(BaseModel):
    """Parsed design.md: tokens (frontmatter) plus ordered markdown sections."""

    name: str = ""
    colors: dict[str, Any] = Field(default_factory=dict)
    typography: dict[str, Any] = Field(default_factory=dict)
    rounded: dict[str, Any] = Field(default_factory=dict)
    spacing: dict[str, Any] = Field(default_factory=dict)
    components: dict[str, Any] = Field(default_factory=dict)
    # (heading text, body) pairs in document order.
    sections: list[tuple[str, str]] = Field(default_factory=list)
    body: str = ""

    def category(self, name: str) -> dict[str, Any]:
        return {
            "colors": self.colors,
            "typography": self.typography,
            "rounded": self.rounded,
            "spacing": self.spacing,
        }.get(name, {})


def _parse_sections(body: str) -> list[tuple[str, str]]:
    """Split a markdown body into (heading, section-body) pairs at ATX headings."""
    sections: list[tuple[str, str]] = []
    current_heading: str | None = None
    buf: list[str] = []
    for line in body.splitlines():
        m = _HEADING_RE.match(line)
        if m:
            if current_heading is not None:
                sections.append((current_heading, "\n".join(buf).strip()))
            current_heading = m.group(2).strip()
            buf = []
        else:
            buf.append(line)
    if current_heading is not None:
        sections.append((current_heading, "\n".join(buf).strip()))
    return sections


def parse_design_md(source: str | Path) -> DesignSystem:
    """Parse a design.md document from a path or raw text."""
    if isinstance(source, Path):
        text = source.read_text(encoding="utf-8")
    else:
        text = source
    meta, body = split_frontmatter(text)

    def _as_dict(value: Any) -> dict[str, Any]:
        return value if isinstance(value, dict) else {}

    return DesignSystem(
        name=str(meta.get("name") or ""),
        colors=_as_dict(meta.get("colors")),
        typography=_as_dict(meta.get("typography")),
        rounded=_as_dict(meta.get("rounded")),
        spacing=_as_dict(meta.get("spacing")),
        components=_as_dict(meta.get("components")),
        sections=_parse_sections(body),
        body=body.strip(),
    )


def design_system_to_okf_document(
    ds: DesignSystem, rel_path: str = "design/design.md"
) -> OKFDocument:
    """Render a :class:`DesignSystem` as an OKF document for inclusion in a bundle.

    Mirrors :func:`skill_bridge.skill_to_okf_document` so design knowledge travels in an OKF
    bundle alongside skills and the artifact graph. The body is a deterministic, readable
    rendering of the design tokens (in fixed category order, preserving each category's own
    key order) followed by the human-readable rationale (``ds.body``). ``tags`` are left empty
    and ``timestamp`` is left to the caller (a design system is library content, not a
    timestamped artifact).
    """
    title = ds.name or "Design System"
    lines: list[str] = []

    def _emit(label: str, mapping: dict[str, Any]) -> None:
        if not mapping:
            return
        lines.append(f"### {label}")
        for name, value in mapping.items():
            if isinstance(value, dict):
                inner = ", ".join(f"{k}: {v}" for k, v in value.items())
                lines.append(f"- **{name}**: {inner}")
            else:
                lines.append(f"- **{name}**: {value}")
        lines.append("")

    _emit("Colors", ds.colors)
    _emit("Typography", ds.typography)
    _emit("Rounded", ds.rounded)
    _emit("Spacing", ds.spacing)
    _emit("Components", ds.components)

    if ds.body:
        lines.append("## Rationale")
        lines.append("")
        lines.append(ds.body)

    return OKFDocument(
        type="Design System",
        title=title,
        description=f"Design system tokens and guidance for {title}."[:280],
        tags=[],
        timestamp="",
        body="\n".join(lines).strip(),
        rel_path=rel_path,
    )


def _iter_component_refs(ds: DesignSystem):
    """Yield (component, prop, value) for every component property that is a string."""
    for comp_name, props in ds.components.items():
        if not isinstance(props, dict):
            continue
        for prop, value in props.items():
            if isinstance(value, str):
                yield comp_name, prop, value


def _hex_to_rgb(value: str) -> tuple[int, int, int] | None:
    if not _HEX_RE.match(value):
        return None
    h = value.lstrip("#")
    if len(h) == 3:
        h = "".join(ch * 2 for ch in h)
    return int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16)


def _relative_luminance(rgb: tuple[int, int, int]) -> float:
    def chan(c: int) -> float:
        s = c / 255.0
        return s / 12.92 if s <= 0.03928 else ((s + 0.055) / 1.055) ** 2.4

    r, g, b = (chan(c) for c in rgb)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast_ratio(fg: str, bg: str) -> float | None:
    """WCAG contrast ratio between two hex colors, or ``None`` if either isn't hex."""
    frgb, brgb = _hex_to_rgb(fg), _hex_to_rgb(bg)
    if frgb is None or brgb is None:
        return None
    lf, lb = _relative_luminance(frgb), _relative_luminance(brgb)
    lighter, darker = max(lf, lb), min(lf, lb)
    return (lighter + 0.05) / (darker + 0.05)


def _resolve_color(ds: DesignSystem, value: str) -> str | None:
    """Resolve a component color value to a hex string (follows one token reference)."""
    m = _REF_RE.match(value.strip())
    if m and m.group("cat") == "colors":
        resolved = ds.colors.get(m.group("name"))
        return resolved if isinstance(resolved, str) else None
    return value if _HEX_RE.match(value.strip()) else None


def lint(ds: DesignSystem) -> list[Finding]:
    """Validate a design system. Mirrors a high-value subset of the upstream rules:
    broken token references, missing primary color, low text/background contrast,
    orphaned tokens, and canonical section ordering.
    """
    findings: list[Finding] = []
    referenced: set[str] = set()

    # broken-token-reference
    for comp, prop, value in _iter_component_refs(ds):
        m = _REF_RE.match(value.strip())
        if not m:
            continue
        cat, name = m.group("cat"), m.group("name")
        referenced.add(f"{cat}.{name}")
        if name not in ds.category(cat):
            findings.append(Finding(
                rule="broken-token-reference",
                severity="error",
                message=f"components.{comp}.{prop} references '{cat}.{name}' which is not defined",
            ))

    # missing-primary-color
    if ds.colors and "primary" not in ds.colors:
        findings.append(Finding(
            rule="missing-primary-color",
            severity="warning",
            message="no 'primary' color token is defined",
        ))

    # contrast: any component declaring both a text and background color
    for comp, props in ds.components.items():
        if not isinstance(props, dict):
            continue
        fg_raw = props.get("textColor") or props.get("color")
        bg_raw = props.get("backgroundColor")
        if not (isinstance(fg_raw, str) and isinstance(bg_raw, str)):
            continue
        fg, bg = _resolve_color(ds, fg_raw), _resolve_color(ds, bg_raw)
        if fg and bg:
            ratio = contrast_ratio(fg, bg)
            if ratio is not None and ratio < 4.5:
                findings.append(Finding(
                    rule="contrast-ratio",
                    severity="warning",
                    message=f"components.{comp} text/background contrast is {ratio:.2f}:1 (WCAG AA needs 4.5:1)",
                ))

    # orphaned-token: color tokens defined but never referenced by a component
    for name in ds.colors:
        if ds.components and f"colors.{name}" not in referenced:
            findings.append(Finding(
                rule="orphaned-token",
                severity="info",
                message=f"color token 'colors.{name}' is never referenced by a component",
            ))

    # section-ordering (+ duplicate-section)
    present = [low for h, _ in ds.sections if (low := h.lower()) in _CANONICAL_SET]
    # A duplicated canonical section is its own problem; report it and de-duplicate before
    # the ordering check, so a duplicated-but-correctly-ordered doc isn't mislabeled as
    # "out of canonical order" (the duplicate alone made actual != expected).
    seen: set[str] = set()
    actual: list[str] = []
    duplicates: list[str] = []
    for name in present:
        if name in seen:
            if name not in duplicates:
                duplicates.append(name)
        else:
            seen.add(name)
            actual.append(name)
    for name in duplicates:
        findings.append(Finding(
            rule="duplicate-section",
            severity="warning",
            message=f"section '{name}' appears more than once",
        ))
    expected = [name for name in CANONICAL_SECTIONS if name in seen]
    if actual != expected:
        findings.append(Finding(
            rule="section-ordering",
            severity="warning",
            message=f"sections are out of canonical order: {actual} (expected {expected})",
        ))

    return findings


def _flatten_typography(value: Any) -> str:
    """Render a typography token (dict of fontFamily/fontSize/...) as a CSS-ish summary."""
    if isinstance(value, dict):
        return "; ".join(f"{k}: {v}" for k, v in value.items())
    return str(value)


def export(ds: DesignSystem, fmt: Literal["css", "tailwind", "w3c"]) -> str:
    """Convert a design system to ``css`` custom properties, a ``tailwind`` theme-extend
    config, or a ``w3c`` Design Tokens JSON document."""
    if fmt == "css":
        return _export_css(ds)
    if fmt == "tailwind":
        return _export_tailwind(ds)
    if fmt == "w3c":
        return _export_w3c(ds)
    raise ValueError(f"unknown export format: {fmt!r}")


def _export_css(ds: DesignSystem) -> str:
    lines = [":root {"]
    for name, value in ds.colors.items():
        lines.append(f"  --color-{name}: {value};")
    for name, value in ds.rounded.items():
        lines.append(f"  --rounded-{name}: {value};")
    for name, value in ds.spacing.items():
        lines.append(f"  --spacing-{name}: {value};")
    for name, value in ds.typography.items():
        if isinstance(value, dict):
            for prop, pval in value.items():
                lines.append(f"  --typography-{name}-{prop}: {pval};")
        else:
            lines.append(f"  --typography-{name}: {value};")
    lines.append("}")
    return "\n".join(lines) + "\n"


def _export_tailwind(ds: DesignSystem) -> str:
    theme: dict[str, Any] = {}
    if ds.colors:
        theme["colors"] = dict(ds.colors)
    if ds.rounded:
        theme["borderRadius"] = dict(ds.rounded)
    if ds.spacing:
        theme["spacing"] = dict(ds.spacing)
    config = {"theme": {"extend": theme}}
    return "/** @type {import('tailwindcss').Config} */\nmodule.exports = " + json.dumps(config, indent=2) + ";\n"


def _export_w3c(ds: DesignSystem) -> str:
    """W3C Design Tokens Community Group format (``$value``/``$type`` groups)."""
    out: dict[str, Any] = {}
    if ds.colors:
        out["color"] = {name: {"$value": value, "$type": "color"} for name, value in ds.colors.items()}
    if ds.spacing:
        out["spacing"] = {name: {"$value": value, "$type": "dimension"} for name, value in ds.spacing.items()}
    if ds.rounded:
        out["rounded"] = {name: {"$value": value, "$type": "dimension"} for name, value in ds.rounded.items()}
    if ds.typography:
        out["typography"] = {
            name: {"$value": value if isinstance(value, dict) else {"value": value}, "$type": "typography"}
            for name, value in ds.typography.items()
        }
    return json.dumps(out, indent=2) + "\n"
