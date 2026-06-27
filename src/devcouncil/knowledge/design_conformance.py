"""Design-system conformance: prove code honored the design tokens.

The lint/export side of :mod:`devcouncil.knowledge.design` validates the *tokens*; this
module checks the *consumers*. It scans source / stylesheet text for hardcoded style
literals (hex colors, ``px`` font-size / spacing values) that bypass the design system's
tokens, so a project can fail CI / a pre-commit hook when an agent (or human) hand-rolls a
color instead of referencing ``colors.primary``.

Heuristics are deliberately conservative — the goal is high-signal, low-noise, because a
false positive that blocks CI is worse than a missed literal:

* We only inspect *declarations* whose property name looks like styling (``color:``,
  ``background:``, ``font-size:``, ``margin:``, ``padding:``, …, plus the camelCase JS/TS
  style-object spellings like ``backgroundColor``). Arbitrary hex/px elsewhere is ignored.
* A literal that exactly matches a defined token value is allowed (that's the token's
  value, just written out).
* We only flag a *kind* when the design system actually defines tokens of that kind — you
  cannot "bypass" a scale that doesn't exist, and judging it would only add noise.
* Comments (``/* … */`` and ``//``) are stripped before scanning.
"""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any, Iterable

from pydantic import BaseModel

from devcouncil.knowledge.design import DesignSystem

# File extensions worth scanning for style literals.
STYLE_EXTENSIONS = frozenset(
    {".css", ".scss", ".sass", ".less", ".js", ".jsx", ".ts", ".tsx", ".vue", ".svelte"}
)

# Property names (normalized to lowercase letters-only, so "background-color" and
# "backgroundColor" both collapse to "backgroundcolor") that carry a *color* value.
_COLOR_PROPS = frozenset({
    "color", "background", "backgroundcolor", "border", "bordercolor",
    "bordertopcolor", "borderrightcolor", "borderbottomcolor", "borderleftcolor",
    "outline", "outlinecolor", "fill", "stroke", "boxshadow", "textshadow",
    "caretcolor", "accentcolor", "columnrulecolor", "textdecorationcolor",
})
# Property names that carry a font-size value.
_FONT_SIZE_PROPS = frozenset({"fontsize"})
# Property names that carry a spacing (length) value.
_SPACING_PROPS = frozenset({
    "margin", "margintop", "marginright", "marginbottom", "marginleft",
    "padding", "paddingtop", "paddingright", "paddingbottom", "paddingleft",
    "gap", "rowgap", "columngap", "gridgap",
})

# A single property:value declaration. The value stops at a comma so JS style objects
# ({ fontSize: '20px', color: '#fff' }) and CSS rgba()/gradients don't swallow the next
# declaration; this can under-report multi-literal CSS values, which is the safe direction.
_DECL_RE = re.compile(r"(?P<prop>[A-Za-z][A-Za-z-]*)\s*:\s*(?P<value>[^;{}\n,]*)")
# Hex colors: #rgb / #rgba / #rrggbb / #rrggbbaa.
_HEX_RE = re.compile(r"#(?:[0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{4}|[0-9a-fA-F]{3})\b")
# A px length literal (not preceded by a word char / dot, so "12.5px" is one token).
_PX_RE = re.compile(r"(?<![\w.])(\d+(?:\.\d+)?)px\b")
# A token value that is a bare or px length.
_PX_TOKEN_RE = re.compile(r"^(\d+(?:\.\d+)?)px$")
_NUM_TOKEN_RE = re.compile(r"^\d+(?:\.\d+)?$")


class Violation(BaseModel):
    """A hardcoded style literal that bypasses a design token."""

    file: str
    line: int
    kind: str  # 'color' | 'font-size' | 'spacing'
    snippet: str
    message: str

    def format(self) -> str:
        loc = f"{self.file}:{self.line}" if self.file else f"line {self.line}"
        return f"{loc} [{self.kind}] {self.message}"


def _normalize_prop(prop: str) -> str:
    """Collapse a CSS/JS property name to lowercase letters only for set membership."""
    return re.sub(r"[^a-z]", "", prop.lower())


def _normalize_hex(value: str) -> str:
    """Lowercase a hex color and expand 3/4-digit shorthand to 6/8 digits."""
    h = value[1:].lower()
    if len(h) in (3, 4):
        h = "".join(ch * 2 for ch in h)
    return "#" + h


def _color_token_values(ds: DesignSystem) -> set[str]:
    """Normalized hex values declared in the design system's color tokens."""
    out: set[str] = set()
    for value in ds.colors.values():
        if isinstance(value, str) and _HEX_RE.fullmatch(value.strip()):
            out.add(_normalize_hex(value.strip()))
    return out


def _px_values(values: Iterable[Any]) -> set[float]:
    """Numeric px-equivalents from token values (``"8px"`` or bare ``8``)."""
    out: set[float] = set()
    for value in values:
        if isinstance(value, (int, float)) and not isinstance(value, bool):
            out.add(float(value))
            continue
        if not isinstance(value, str):
            continue
        s = value.strip()
        m = _PX_TOKEN_RE.match(s) or _NUM_TOKEN_RE.match(s)
        if m:
            out.add(float(m.group(1) if m.re is _PX_TOKEN_RE else s))
    return out


def _font_size_scale(ds: DesignSystem) -> set[float]:
    """px font sizes declared across the typography tokens."""
    candidates: list[Any] = []
    for value in ds.typography.values():
        if isinstance(value, dict):
            for key, inner in value.items():
                if "size" in key.lower():
                    candidates.append(inner)
        else:
            candidates.append(value)
    return _px_values(candidates)


def _spacing_scale(ds: DesignSystem) -> set[float]:
    """px lengths declared in the spacing token scale."""
    return _px_values(ds.spacing.values())


def _strip_comments(text: str) -> list[str]:
    """Return per-line text with ``/* … */`` and ``//`` comments blanked out.

    Line count is preserved so reported line numbers stay accurate. String literals are
    tracked so a ``//`` *inside* a string (e.g. ``url('http://x')`` or ``"http://x"``) is
    NOT mistaken for a line comment — otherwise a stray ``color: #f00`` after a URL on the
    same line would be silently dropped. A bare ``scheme://`` (``//`` preceded by ``:``) is
    likewise treated as a URL, not a comment. ``/* … */`` blocks still span lines.
    """
    out: list[str] = []
    in_block = False
    for line in text.splitlines():
        res: list[str] = []
        i, n = 0, len(line)
        quote: str | None = None  # active string delimiter within this line
        while i < n:
            ch = line[i]
            two = line[i:i + 2]
            if in_block:
                if two == "*/":
                    in_block = False
                    i += 2
                else:
                    i += 1
                continue
            if quote is not None:
                res.append(ch)
                if ch == "\\" and i + 1 < n:  # keep an escaped char verbatim
                    res.append(line[i + 1])
                    i += 2
                    continue
                if ch == quote:
                    quote = None
                i += 1
                continue
            if ch in ("'", '"', "`"):
                quote = ch
                res.append(ch)
                i += 1
            elif two == "/*":
                in_block = True
                i += 2
            elif two == "//" and (not res or res[-1] != ":"):
                break  # a real line comment (not a scheme:// URL)
            else:
                res.append(ch)
                i += 1
        out.append("".join(res))
    return out


def scan_text(text: str, ds: DesignSystem, filename: str = "") -> list[Violation]:
    """Scan source/style ``text`` for hardcoded literals that bypass ``ds``'s tokens.

    Returns one :class:`Violation` per offending literal, with 1-based line numbers. Only
    declarations whose property name looks like styling are considered, literals matching a
    token value are allowed, and a kind is only judged when the design system defines tokens
    of that kind (see module docstring).
    """
    color_tokens = _color_token_values(ds)
    font_scale = _font_size_scale(ds)
    spacing_scale = _spacing_scale(ds)

    violations: list[Violation] = []
    for lineno, line in enumerate(_strip_comments(text), start=1):
        for m in _DECL_RE.finditer(line):
            prop = _normalize_prop(m.group("prop"))
            value = m.group("value")
            snippet = m.group(0).strip()

            if color_tokens and prop in _COLOR_PROPS:
                for hm in _HEX_RE.finditer(value):
                    norm = _normalize_hex(hm.group(0))
                    if norm not in color_tokens:
                        violations.append(Violation(
                            file=filename, line=lineno, kind="color", snippet=snippet,
                            message=(
                                f"hardcoded color '{hm.group(0)}' bypasses design tokens; "
                                "use a colors.* token"
                            ),
                        ))

            if font_scale and prop in _FONT_SIZE_PROPS:
                for pm in _PX_RE.finditer(value):
                    num = float(pm.group(1))
                    if num != 0 and num not in font_scale:
                        violations.append(Violation(
                            file=filename, line=lineno, kind="font-size", snippet=snippet,
                            message=(
                                f"hardcoded font-size '{pm.group(0)}' is not in the typography "
                                "scale; use a typography token"
                            ),
                        ))

            if spacing_scale and prop in _SPACING_PROPS:
                for pm in _PX_RE.finditer(value):
                    num = float(pm.group(1))
                    if num != 0 and num not in spacing_scale:
                        violations.append(Violation(
                            file=filename, line=lineno, kind="spacing", snippet=snippet,
                            message=(
                                f"hardcoded spacing '{pm.group(0)}' is not in the spacing "
                                "scale; use a spacing token"
                            ),
                        ))

    return violations


def scan_files(paths: list[Path], ds: DesignSystem) -> list[Violation]:
    """Scan style-ish files for token-bypassing literals (best-effort, never raises).

    Non-style extensions are skipped, and unreadable / binary files are silently ignored so
    a single bad file never aborts a conformance check.
    """
    violations: list[Violation] = []
    for path in paths:
        if path.suffix.lower() not in STYLE_EXTENSIONS:
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError, ValueError):
            continue
        violations.extend(scan_text(text, ds, filename=str(path)))
    return violations
