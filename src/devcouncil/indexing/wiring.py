"""Config-declared entry roots and structural exemptions for file-level liveness.

Single source of truth shared by ``dev map`` liveness fields and the
``unwired_file`` / ``dead_symbol`` verification gates so they never disagree on
what counts as "wired by convention/config".

Also hosts comment strippers and wiring-decorator exemptions used by both the
map's ``dead_symbol_candidates`` and the verify ``dead_symbol`` gate.

Never raises on malformed config — degrades to empty/False.
"""

from __future__ import annotations

import ast
import json
import logging
import re
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Dict, Iterable, Iterator, List, Literal, Optional, Set, Tuple

from pydantic import BaseModel, Field

from devcouncil.indexing.walk import IGNORED_DIR_NAMES, should_skip_path
from devcouncil.utils.json_persist import read_model_json, write_model_json

logger = logging.getLogger(__name__)

# Python + JS/TS always. Go is file-level (all package members). Rust is included
# only when tree-sitter edges are available (see is_liveness_code_file).
_LIVENESS_EXTS = {".py", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".go"}
_RUST_LIVENESS_EXT = ".rs"
_TEST_DIR_NAMES = {"tests", "test", "__tests__", "spec"}
_SCRIPT_DIR_NAMES = {"scripts", "bin", "benchmarks"}
ALLOW_UNWIRED = "devcouncil: allow-unwired"
_IMPORTLIB_RE = re.compile(
    r"""(?:importlib(?:\.import_module)?|__import__)\s*\(\s*['"]([^'"]+)['"]"""
)
_DYNAMIC_IMPORT_RE = re.compile(r"""import\s*\(\s*['"]([^'"]+)['"]\s*\)""")
_HATCH_CUSTOM_HOOK_RE = re.compile(
    r"""(?ms)^\[tool\.hatch\.build\.hooks\.custom\]\s*$.*?^path\s*=\s*['"]([^'"]+)['"]"""
)
_CODE_CONFIG_SUFFIXES = {
    ".py", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs",
    ".toml", ".json", ".yaml", ".yml", ".cfg", ".ini",
}
_ROUTE_DIR_HINTS = (
    "app/",
    "pages/",
    "routes/",
    "src/app/",
    "src/pages/",
    "src/routes/",
    "app/routes/",
)

# Decorators that themselves constitute wiring (framework registration).
_WIRING_DECORATOR_HINTS = (
    "app.", "router.", "typer.", "click.", "pytest.", "celery.",
    "flask", "fastapi", "command", "route", "task", "fixture",
    "register", "hookimpl", "hookable",
)

# Bumped when dead-symbol / token-scan semantics change so ratchet baselines
# skip stale symbol diffs instead of firing false stranded_code regressions.
LIVENESS_SCAN_VERSION = 2

_VENDOR_DIR_NAMES = frozenset({"vendor", "vendored", "node_modules"})


def _norm(path: str) -> str:
    """Normalize to posix and strip leading ``./`` only (never ``lstrip('./')``)."""
    s = str(path).replace("\\", "/")
    while s.startswith("./"):
        s = s[2:]
    return s


def is_test_path(path: str) -> bool:
    """True when path looks like a test file by common conventions."""
    norm = _norm(path).lower()
    name = norm.rsplit("/", 1)[-1]
    parts = norm.split("/")
    in_test_dir = any(p in _TEST_DIR_NAMES for p in parts[:-1])
    looks_like_test = (
        name.startswith("test_")
        or name == "conftest.py"
        or any(
            name.endswith(suffix)
            for suffix in (
                "_test.py",
                "_test.go",
                ".test.js",
                ".test.ts",
                ".test.jsx",
                ".test.tsx",
                ".spec.js",
                ".spec.ts",
                ".spec.jsx",
                ".spec.tsx",
                "_spec.rb",
            )
        )
    )
    return looks_like_test or (in_test_dir and not name.startswith("."))


def is_liveness_code_file(path: str) -> bool:
    """True for languages with reliable file-level import edges.

    Go is included (file-level package member edges). Rust is included only when
    the optional tree-sitter layer can emit ``mod``/``use`` edges; without it,
    Rust files would all look unwired.
    """
    suffix = Path(_norm(path)).suffix.lower()
    if suffix in _LIVENESS_EXTS:
        return True
    if suffix == _RUST_LIVENESS_EXT:
        try:
            from devcouncil.indexing.ts_imports import tree_sitter_available

            return tree_sitter_available()
        except Exception:
            return False
    return False


def is_private_symbol(name: str) -> bool:
    """True for underscore-prefixed names skipped by dead-symbol detection."""
    return bool(name) and name.startswith("_")


def is_dunder_symbol(name: str) -> bool:
    """True for ``__dunder__`` names (methods exempt from dead-code reports)."""
    return bool(name) and len(name) >= 4 and name.startswith("__") and name.endswith("__")


def is_vendored_path(path: str) -> bool:
    """True when ``path`` is a vendored/minified bundle, not first-class source.

    Matches ``vendor`` / ``vendored`` / ``node_modules`` path segments and
    ``.min.js`` / ``.min.css`` basenames — same convention
    :func:`structural_exemptions` already encodes for file-level liveness.
    """
    try:
        norm = _norm(path)
        name = Path(norm).name
        parts = norm.lower().split("/")
        if any(p in _VENDOR_DIR_NAMES for p in parts):
            return True
        if name.endswith(".min.js") or name.endswith(".min.css"):
            return True
        return False
    except Exception:
        logger.debug("is_vendored_path failed for %s", path, exc_info=True)
        return False


# Dynamic getattr(x, "name") keys in :func:`build_dynamic_import_index`.
GETATTR_INDEX_PREFIX = "getattr:"

_GETATTR_NAME_RE = re.compile(
    r"""getattr\s*\(\s*[^,]+,\s*['"]([A-Za-z_][A-Za-z0-9_]*)['"]"""
)

# JS/TS export forms shared by map token-scan and verify dead_symbol gate.
_JS_EXPORT_DECL_RE = re.compile(
    r"(?m)^\s*export\s+(?:async\s+)?(?:function|class|const|let|var)\s+([A-Za-z_][A-Za-z0-9_]*)"
)
_JS_EXPORT_LIST_RE = re.compile(
    r"(?m)^\s*export\s+(?:default\s+)?(?:async\s+)?(?:function|class)\s+([A-Za-z_][A-Za-z0-9_]*)"
    r"|^\s*export\s+default\s+([A-Za-z_][A-Za-z0-9_]*)\s*;"
    r"|^\s*export\s*\{([^}]+)\}"
    r"|^\s*export\s+(?:type\s+)?\{([^}]+)\}\s*from\s*['\"][^'\"]+['\"]"
    r"|^\s*export\s+\*\s+as\s+([A-Za-z_][A-Za-z0-9_]*)\s+from\s*['\"][^'\"]+['\"]"
)


def parse_python_all_exports(source: str) -> Set[str]:
    """Return names listed in a module-level ``__all__`` assignment (best-effort)."""
    out: Set[str] = set()
    try:
        tree = ast.parse(source)
    except (SyntaxError, ValueError):
        return out
    for node in tree.body:
        if not isinstance(node, ast.Assign):
            continue
        for t in node.targets:
            if isinstance(t, ast.Name) and t.id == "__all__":
                if isinstance(node.value, (ast.List, ast.Tuple)):
                    for elt in node.value.elts:
                        if isinstance(elt, ast.Constant) and isinstance(elt.value, str):
                            out.add(elt.value)
    return out


def parse_python_reexport_names(path: str, source: str) -> Set[str]:
    """Names re-exported by a barrel ``__init__.py`` or listed in ``__all__``.

    Non-init modules do not treat every ``from x import y`` as a re-export — only
    names that also appear in ``__all__``.
    """
    all_names = parse_python_all_exports(source)
    is_init = path.replace("\\", "/").endswith("__init__.py")
    out: Set[str] = set()
    try:
        tree = ast.parse(source)
    except (SyntaxError, ValueError):
        return out
    for stmt in tree.body:
        if not isinstance(stmt, ast.ImportFrom):
            continue
        for alias in stmt.names:
            if not alias.name or alias.name == "*":
                continue
            local = alias.asname or alias.name
            if is_init or local in all_names:
                out.add(local)
    return out


def iter_js_export_symbols(source: str) -> List[tuple[int, str]]:
    """Yield ``(line, name)`` for JS/TS export forms (decl, list, default, re-export)."""
    found: List[tuple[int, str]] = []
    seen: Set[tuple[int, str]] = set()

    def _add(line: int, name: str) -> None:
        if not name or is_private_symbol(name):
            return
        key = (line, name)
        if key in seen:
            return
        seen.add(key)
        found.append((line, name))

    for m in _JS_EXPORT_DECL_RE.finditer(source):
        line = source[: m.start()].count("\n") + 1
        _add(line, m.group(1))

    for m in _JS_EXPORT_LIST_RE.finditer(source):
        line = source[: m.start()].count("\n") + 1
        if m.group(1):
            _add(line, m.group(1))
        if m.group(2):
            _add(line, m.group(2))
        for group in (m.group(3), m.group(4)):
            if not group:
                continue
            for part in group.split(","):
                part = part.strip()
                if not part or part == "type":
                    continue
                # `Foo as Bar` / `type Foo` / `default as X`
                part = re.sub(r"^type\s+", "", part)
                if " as " in part:
                    part = part.split(" as ")[-1].strip()
                if part == "default":
                    continue
                name = part.split(":", 1)[0].strip()
                _add(line, name)
        if m.group(5):
            _add(line, m.group(5))
    return found


def strip_py_comments(text: str) -> str:
    """Blank ``#`` comments in place — preserve newlines/line count, never renumber.

    Dead-symbol detection indexes tokens from cleaned text while definition spans
    come from ``ast.parse`` on the raw source; dropping lines would skew them.
    """
    ends_nl = text.endswith("\n")
    lines = []
    for line in text.splitlines():
        if "#" not in line:
            lines.append(line)
            continue
        stripped = line.lstrip()
        if stripped.startswith("#"):
            lines.append("")
            continue
        in_str = False
        quote = ""
        buf: List[str] = []
        i = 0
        while i < len(line):
            ch = line[i]
            if in_str:
                buf.append(ch)
                if ch == quote and (i == 0 or line[i - 1] != "\\"):
                    in_str = False
                i += 1
                continue
            if ch in ("'", '"'):
                in_str = True
                quote = ch
                buf.append(ch)
                i += 1
                continue
            if ch == "#":
                break
            buf.append(ch)
            i += 1
        lines.append("".join(buf))
    out = "\n".join(lines)
    return out + ("\n" if ends_nl and lines else "")


def _blank_js_block_comment(match: re.Match[str]) -> str:
    """Replace block-comment body with spaces, keeping every newline."""
    return re.sub(r"[^\n]", " ", match.group(0))


def strip_js_comments(text: str) -> str:
    """Blank ``/* */`` and ``//`` comments in place — preserve newlines/line count."""
    ends_nl = text.endswith("\n")
    text = re.sub(r"/\*.*?\*/", _blank_js_block_comment, text, flags=re.DOTALL)
    lines = []
    for line in text.splitlines():
        if "//" not in line:
            lines.append(line)
            continue
        in_str = False
        quote = ""
        buf: List[str] = []
        i = 0
        while i < len(line):
            ch = line[i]
            if in_str:
                buf.append(ch)
                if ch == quote and (i == 0 or line[i - 1] != "\\"):
                    in_str = False
                i += 1
                continue
            if ch in ("'", '"', "`"):
                in_str = True
                quote = ch
                buf.append(ch)
                i += 1
                continue
            if ch == "/" and i + 1 < len(line) and line[i + 1] == "/":
                break
            buf.append(ch)
            i += 1
        lines.append("".join(buf))
    out = "\n".join(lines)
    return out + ("\n" if ends_nl and lines else "")


def strip_string_literals(text: str) -> str:
    """Blank string literal bodies in place — preserve newlines/line count.

    Dead-symbol token scans index identifiers from cleaned text; leaving
    ``\"cost_by_task\"`` dict keys (etc.) intact falsely clears real dead
    symbols. Dynamic getattr/importlib strings are indexed separately by
    :func:`build_dynamic_import_index` on the raw source before stripping.
    """
    if not text:
        return text
    ends_nl = text.endswith("\n")
    out: List[str] = []
    i = 0
    n = len(text)
    while i < n:
        ch = text[i]
        if ch in ("'", '"', "`"):
            quote = ch
            triple = (
                quote in ("'", '"')
                and i + 2 < n
                and text[i + 1] == quote
                and text[i + 2] == quote
            )
            if triple:
                out.extend((quote, quote, quote))
                i += 3
                while i < n:
                    if (
                        text[i] == quote
                        and i + 2 < n
                        and text[i + 1] == quote
                        and text[i + 2] == quote
                    ):
                        out.extend((quote, quote, quote))
                        i += 3
                        break
                    out.append("\n" if text[i] == "\n" else " ")
                    i += 1
                continue
            out.append(quote)
            i += 1
            while i < n:
                c = text[i]
                if c == "\\" and i + 1 < n:
                    out.append("  ")
                    i += 2
                    continue
                if c == quote:
                    out.append(quote)
                    i += 1
                    break
                if c == "\n":
                    out.append("\n")
                    i += 1
                    if quote != "`":
                        break
                    continue
                out.append(" ")
                i += 1
            continue
        out.append(ch)
        i += 1
    result = "".join(out)
    if ends_nl and not result.endswith("\n"):
        result += "\n"
    return result


def decorator_names(node: ast.AST) -> List[str]:
    """Unparse decorator expressions on a function/class AST node."""
    out: List[str] = []
    for dec in getattr(node, "decorator_list", []) or []:
        try:
            out.append(ast.unparse(dec))
        except Exception:
            if isinstance(dec, ast.Name):
                out.append(dec.id)
            elif isinstance(dec, ast.Attribute):
                out.append(dec.attr)
    return out


def is_wiring_decorated(decorators: List[str]) -> bool:
    """True when any decorator looks like framework registration (route/cli/fixture).

    Dotted hints (``app.``, ``router.``, …) match as prefixes; bare hints
    (``route``, ``task``, ``register``, …) match whole identifier segments only.
    The old substring-over-joined-string check over-matched (e.g. ``multitask``,
    ``preregister``) and hid real dead code behind unrelated decorators.
    """
    for dec in decorators:
        base = dec.split("(", 1)[0].strip().lower()
        if not base:
            continue
        segments = [s for s in re.split(r"[.\s]+", base) if s]
        for hint in _WIRING_DECORATOR_HINTS:
            if hint.endswith("."):
                if base.startswith(hint):
                    return True
            elif hint in segments:
                return True
    return False


def structural_exemptions(path: str) -> bool:
    """True when ``path`` is wired by convention and should not be flagged unwired.

    Shared by map candidates and verify gates. Basename-only exemptions like
    ``main.py`` are intentionally NOT included — real entry points clear via
    :func:`entry_roots`.
    """
    try:
        norm = _norm(path)
        name = Path(norm).name
        lower = norm.lower()
        parts = lower.split("/")
        suffix = Path(norm).suffix.lower()

        if name in {"__main__.py", "conftest.py", "manage.py"}:
            return True
        if name.endswith(".d.ts"):
            return True
        if ".stories." in name or name.endswith((".stories.ts", ".stories.tsx", ".stories.js", ".stories.jsx")):
            return True
        if is_test_path(norm):
            return True
        # Vendored JS/CSS bundles are loaded as package resources, not imported.
        if is_vendored_path(norm):
            return True
        if any(p in _SCRIPT_DIR_NAMES for p in parts[:-1]):
            return True
        # Migrations / alembic version modules.
        if "migrations" in parts or "alembic" in parts:
            if suffix == ".py":
                return True
        # Next/Remix/app-router style route files.
        if any(lower.startswith(hint) or f"/{hint}" in f"/{lower}" for hint in _ROUTE_DIR_HINTS):
            route_names = {
                "page.tsx", "page.ts", "page.jsx", "page.js",
                "layout.tsx", "layout.ts", "layout.jsx", "layout.js",
                "route.ts", "route.js", "route.tsx", "route.jsx",
                "loading.tsx", "error.tsx", "not-found.tsx",
                "middleware.ts", "middleware.js",
                "+page.svelte", "+layout.svelte", "+page.ts", "+layout.ts",
                "index.tsx", "index.ts", "index.jsx", "index.js",
            }
            if name in route_names or name.startswith("route.") or name.startswith("+"):
                return True
        return False
    except Exception:
        logger.debug("structural_exemptions failed for %s", path, exc_info=True)
        return False


def _read_text(root: Path, rel: str) -> str:
    try:
        return (root / rel).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""


def _pyproject_script_targets(root: Path, file_set: Set[str]) -> Set[str]:
    """Resolve pyproject [project.scripts]/entry-points/gui-scripts module paths."""
    text = _read_text(root, "pyproject.toml")
    if not text:
        return set()
    found: Set[str] = set()
    try:
        # Prefer tomllib when available; fall back to regex for exotic envs.
        try:
            import tomllib
        except ImportError:  # pragma: no cover
            import tomli as tomllib  # type: ignore

        data = tomllib.loads(text)
        project = data.get("project") or {}
        entry_maps = []
        for key in ("scripts", "gui-scripts"):
            val = project.get(key)
            if isinstance(val, dict):
                entry_maps.append(val)
        eps = project.get("entry-points") or {}
        if isinstance(eps, dict):
            for group in eps.values():
                if isinstance(group, dict):
                    entry_maps.append(group)
        # pytest plugins often live under tool.pytest.ini_options / pytest.ini
        tool = data.get("tool") or {}
        pytest_cfg = tool.get("pytest") or {}
        ini = pytest_cfg.get("ini_options") or {}
        plugins = ini.get("pytest_plugins") or ini.get("plugins")
        if isinstance(plugins, list):
            for plug in plugins:
                if isinstance(plug, str):
                    _add_module_file(plug.split(":")[0].strip(), file_set, found)
        elif isinstance(plugins, str):
            for plug in re.split(r"[\s,]+", plugins):
                if plug:
                    _add_module_file(plug.split(":")[0].strip(), file_set, found)

        for mapping in entry_maps:
            for target in mapping.values():
                if not isinstance(target, str):
                    continue
                mod = target.split(":")[0].strip()
                _add_module_file(mod, file_set, found)
    except Exception:
        # Regex fallback for scripts = { name = "pkg.mod:fn" }
        for m in re.finditer(
            r"""['"]([A-Za-z_][\w.]*)\s*:\s*[A-Za-z_]\w*['"]""",
            text,
        ):
            _add_module_file(m.group(1), file_set, found)
    return found


def _add_module_file(module: str, file_set: Set[str], out: Set[str]) -> None:
    if not module or module.startswith("."):
        return
    parts = module.replace(".", "/")
    candidates = [
        f"{parts}.py",
        f"{parts}/__init__.py",
        f"src/{parts}.py",
        f"src/{parts}/__init__.py",
    ]
    for cand in candidates:
        if cand in file_set:
            out.add(cand)
            return
    # Soft match: any file whose path ends with the module path (sorted for determinism).
    suffix = f"/{parts}.py"
    suffix_init = f"/{parts}/__init__.py"
    for f in sorted(file_set):
        if f.endswith(suffix) or f.endswith(suffix_init) or f == f"{parts}.py":
            out.add(f)
            return


def _package_json_entry_targets(root: Path, file_set: Set[str]) -> Set[str]:
    text = _read_text(root, "package.json")
    if not text:
        return set()
    found: Set[str] = set()
    try:
        data = json.loads(text)
    except Exception:
        return found

    def _add(candidate: object) -> None:
        if not isinstance(candidate, str):
            return
        rel = _norm(candidate)
        if rel in file_set:
            found.add(rel)
            return
        # Strip leading ./ and try common extensions.
        base = rel
        for ext in ("", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"):
            cand = f"{base}{ext}" if ext and not base.endswith(ext) else base
            if cand in file_set:
                found.add(cand)
                return
            idx = f"{base}/index{ext}" if ext else f"{base}/index.js"
            if idx in file_set:
                found.add(idx)
                return

    for key in ("main", "module", "browser", "types", "typings"):
        _add(data.get(key))
    bin_val = data.get("bin")
    if isinstance(bin_val, str):
        _add(bin_val)
    elif isinstance(bin_val, dict):
        for v in bin_val.values():
            _add(v)
    exports = data.get("exports")
    if isinstance(exports, str):
        _add(exports)
    elif isinstance(exports, dict):
        for v in exports.values():
            if isinstance(v, str):
                _add(v)
            elif isinstance(v, dict):
                for nested in v.values():
                    if isinstance(nested, str):
                        _add(nested)
    return found


def _config_declared_entry_roots(root: Path, file_set: Set[str]) -> Set[str]:
    """Paths from ``indexing.entry_roots`` that exist in the tracked file set."""
    declared: list = []
    try:
        from devcouncil.app.config import load_config

        cfg = load_config(root)
        raw_declared = getattr(cfg.indexing, "entry_roots", None)
        if isinstance(raw_declared, list):
            declared = raw_declared
    except Exception:
        logger.debug("config entry_roots load failed", exc_info=True)
    if not declared:
        try:
            import yaml

            cfg_path = root / ".devcouncil" / "config.yaml"
            if cfg_path.is_file():
                payload = yaml.safe_load(cfg_path.read_text(encoding="utf-8")) or {}
                indexing = payload.get("indexing")
                if isinstance(indexing, dict):
                    yaml_roots = indexing.get("entry_roots")
                    if isinstance(yaml_roots, list):
                        declared = yaml_roots
        except Exception:
            logger.debug("yaml entry_roots load failed", exc_info=True)
    return {_norm(str(p)) for p in declared if p and _norm(str(p)) in file_set}


def entry_roots(
    root: Path,
    files: Iterable[str],
    *,
    production_only: bool = False,
) -> list[str]:
    """Config-declared + small convention set used as BFS reachability seeds.

    Seeds are pyproject/package.json targets plus ``__main__.py`` / ``manage.py``.
    Structural exemptions (routes, migrations, scripts, stories, tests) remain a
    skip-list for unwired/unreachable — they are NOT BFS seeds (that diluted
    reachability and, with caps, could truncate real config entries).

    When ``production_only`` is True, test-file seeds are excluded so reachability
    means "reachable from production code".

    Never raises. Returns a sorted list of repo-relative posix paths.
    """
    try:
        file_set = {_norm(f) for f in files}
        roots: Set[str] = set()
        roots |= _config_declared_entry_roots(root, file_set)
        roots |= _pyproject_script_targets(root, file_set)
        roots |= _package_json_entry_targets(root, file_set)

        for f in file_set:
            if production_only and is_test_path(f):
                continue
            name = Path(f).name
            if name in {"__main__.py", "manage.py"}:
                roots.add(f)

        return sorted(roots)
    except Exception:
        logger.debug("entry_roots failed", exc_info=True)
        return []


def entry_point_symbols(root: Path, files: Iterable[str]) -> Set[str]:
    """Return ``path::attr`` keys for pyproject ``module:attr`` script targets.

    Used by graph dead-code so CLI entry functions (e.g. ``pkg.b:main``) are not
    flagged merely because nothing in-repo calls them.
    """
    out: Set[str] = set()
    try:
        file_set = {_norm(f) for f in files}
        text = _read_text(root, "pyproject.toml")
        if not text:
            return out
        try:
            import tomllib
        except ImportError:  # pragma: no cover
            import tomli as tomllib  # type: ignore

        data = tomllib.loads(text)
        project = data.get("project") or {}
        entry_maps: list = []
        for key in ("scripts", "gui-scripts"):
            val = project.get(key)
            if isinstance(val, dict):
                entry_maps.append(val)
        eps = project.get("entry-points") or {}
        if isinstance(eps, dict):
            for group in eps.values():
                if isinstance(group, dict):
                    entry_maps.append(group)
        for mapping in entry_maps:
            for target in mapping.values():
                if not isinstance(target, str) or ":" not in target:
                    continue
                mod, _, attr = target.partition(":")
                mod, attr = mod.strip(), attr.strip()
                if not mod or not attr:
                    continue
                found: Set[str] = set()
                _add_module_file(mod, file_set, found)
                for path in found:
                    out.add(f"{path}::{attr}")
    except Exception:
        logger.debug("entry_point_symbols failed", exc_info=True)
    return out


_SHORT_STEM_MAX = 12


def module_tokens_for(path: str) -> Set[str]:
    """Tokens that could appear in an importlib/dynamic string for ``path``.

    Omits bare short stems (``config``, ``utils``) that over-match via suffix
    checks against unrelated modules like ``other.config``.
    """
    norm = _norm(path)
    stem = Path(norm).stem
    no_ext = norm.rsplit(".", 1)[0] if "." in Path(norm).name else norm
    dotted = no_ext.replace("/", ".")
    if dotted.startswith("src."):
        dotted = dotted[4:]
    tokens = {no_ext, dotted, norm}
    if Path(norm).name == "__init__.py":
        pkg = Path(norm).parent.as_posix().replace("/", ".")
        if pkg.startswith("src."):
            pkg = pkg[4:]
        tokens.add(pkg)
        tokens.add(Path(norm).parent.as_posix())
    # Bare stem only when long enough to be specific, or path is top-level.
    if "/" not in no_ext and len(stem) >= _SHORT_STEM_MAX:
        tokens.add(stem)
    elif "/" in no_ext and len(stem) >= _SHORT_STEM_MAX:
        # Still skip short stems; path/dotted forms above are enough.
        pass
    return {t for t in tokens if t}


def _module_forms(value: str) -> Set[str]:
    """Comparable dotted + slash forms (extensions stripped) for boundary matching."""
    v = _norm(value)
    forms = {v, v.replace("/", "."), v.replace(".", "/")}
    for ext in (".py", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"):
        if v.endswith(ext):
            base = v[: -len(ext)]
            forms.add(base)
            forms.add(base.replace("/", "."))
            forms.add(base.replace(".", "/"))
            break
    return {f for f in forms if f}


def import_spec_matches(spec: str, tokens: Set[str]) -> bool:
    """True when an import string matches ``tokens`` on a module/path boundary."""
    if not spec or not tokens:
        return False
    spec_forms = _module_forms(spec)
    for t in tokens:
        if spec_forms & _module_forms(t):
            return True
    return False


def has_allow_unwired(project_root: Path, path: str) -> bool:
    """True when ``path`` contains the ``devcouncil: allow-unwired`` marker."""
    try:
        text = (project_root / path).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return False
    return ALLOW_UNWIRED in text


def dynamic_import_keys(path: str, source: str) -> Set[str]:
    """Return normalized dynamic-import and ``getattr`` keys for one file."""
    norm = _norm(path)
    suffix = Path(norm).suffix.lower()
    if suffix not in _CODE_CONFIG_SUFFIXES:
        return set()
    specs: List[str] = [match.group(1) for match in _IMPORTLIB_RE.finditer(source)]
    specs.extend(
        spec
        for match in _DYNAMIC_IMPORT_RE.finditer(source)
        if (spec := match.group(1)) and not spec.startswith(".")
    )
    if suffix == ".toml":
        specs.extend(
            (Path(norm).parent / match.group(1)).with_suffix("").as_posix()
            for match in _HATCH_CUSTOM_HOOK_RE.finditer(source)
        )
    keys = {form for spec in specs for form in _module_forms(spec)}
    keys.update(
        f"{GETATTR_INDEX_PREFIX}{match.group(1)}"
        for match in _GETATTR_NAME_RE.finditer(source)
        if match.group(1)
    )
    return keys


def build_dynamic_import_index(
    project_root: Path,
    git_files: Optional[List[str]] = None,
) -> dict[str, Set[str]]:
    """One shared scan: normalized module form → non-test files that reference it.

    Call once per liveness/verify pass; O(repo files) instead of O(candidates × files).
    """
    index: dict[str, Set[str]] = {}
    try:
        if git_files is None:
            from devcouncil.indexing.repo_mapper import RepoMapper

            try:
                candidates = RepoMapper(project_root).get_git_files()
            except Exception:
                candidates = []
        else:
            candidates = list(git_files)

        for rel in candidates:
            norm = _norm(rel)
            if is_test_path(norm):
                continue
            path = project_root / norm
            if not path.is_file():
                continue
            if path.suffix.lower() not in _CODE_CONFIG_SUFFIXES:
                continue
            try:
                text = path.read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            for key in dynamic_import_keys(norm, text):
                index.setdefault(key, set()).add(norm)
    except Exception:
        logger.debug("build_dynamic_import_index failed", exc_info=True)
    return index


def reference_cleared(
    project_root: Path,
    target: str,
    *,
    skip_files: Optional[Set[str]] = None,
    git_files: Optional[List[str]] = None,
    dynamic_index: Optional[dict[str, Set[str]]] = None,
) -> bool:
    """True when a non-test file holds an import-shaped string reference to ``target``.

    Prefer a prebuilt ``dynamic_index`` (from :func:`build_dynamic_import_index`) so
    a liveness pass pays one repo scan. Falls back to a targeted scan when omitted.

    Scans only non-test code/config files so a dynamic import in a test does not
    clear unwired (parity with the static-import rule).
    """
    tokens = module_tokens_for(target)
    if not tokens:
        return False
    skip = {_norm(p) for p in (skip_files or set())}
    target_n = _norm(target)
    token_forms: Set[str] = set()
    for t in tokens:
        token_forms |= _module_forms(t)

    try:
        if dynamic_index is not None:
            for form in token_forms:
                for ref in dynamic_index.get(form, ()):
                    if ref in skip or ref == target_n or is_test_path(ref):
                        continue
                    return True
            return False

        if git_files is None:
            from devcouncil.indexing.repo_mapper import RepoMapper

            try:
                candidates = RepoMapper(project_root).get_git_files()
            except Exception:
                candidates = []
        else:
            candidates = list(git_files)
        for rel in candidates:
            norm = _norm(rel)
            if norm in skip or norm == target_n:
                continue
            if is_test_path(norm):
                continue
            path = project_root / norm
            if not path.is_file():
                continue
            if path.suffix.lower() not in _CODE_CONFIG_SUFFIXES:
                continue
            try:
                text = path.read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            for m in _IMPORTLIB_RE.finditer(text):
                if import_spec_matches(m.group(1), tokens):
                    return True
            for m in _DYNAMIC_IMPORT_RE.finditer(text):
                spec = m.group(1)
                if spec.startswith("."):
                    continue
                if import_spec_matches(spec, tokens):
                    return True
    except Exception:
        logger.debug("reference scan failed for %s", target, exc_info=True)
    return False


# ---------------------------------------------------------------------------
# Advisory corpus index (docs / PDF / image side graph)
# ---------------------------------------------------------------------------
# Separate from the deterministic code graph — never wired into verify gates.
# Artifacts: ``.devcouncil/corpus/graph.json`` (+ optional ``graph.html``).

CorpusNodeKind = Literal[
    "document",
    "section",
    "concept",
    "link",
    "code_ref",
    "pdf",
    "pdf_page",
    "image",
]
CorpusEdgeKind = Literal[
    "contains",
    "links_to",
    "references",
    "cites",
    "parent_of",
    "mentions",
]

_CORPUS_TEXT_EXTS = frozenset({".md", ".markdown", ".txt", ".rst"})
_CORPUS_PDF_EXTS = frozenset({".pdf"})
_CORPUS_IMAGE_EXTS = frozenset({".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg"})
_MD_HEADING_RE = re.compile(r"^(#{1,6})\s+(.+?)\s*$", re.MULTILINE)
_MD_LINK_RE = re.compile(r"\[([^\]]+)\]\(([^)]+)\)")
_WIKILINK_RE = re.compile(r"\[\[([^\]|]+)(?:\|([^\]]+))?\]\]")
_CODE_PATH_RE = re.compile(
    r"`([^`]+)`|(?:^|\s)((?:src|docs|tests)/[\w./-]+\.(?:py|ts|tsx|js|md|rst|yaml|yml))(?:\s|$)"
)
_RST_HEADING_RE = re.compile(
    r"^(?P<title>.+)\n(?P<uline>[=\-`:~^_*+#]+)\s*$",
    re.MULTILINE,
)


class CorpusNode(BaseModel):
    id: str
    kind: CorpusNodeKind
    label: str
    path: Optional[str] = None
    content: Optional[str] = None
    metadata: Dict[str, Any] = Field(default_factory=dict)


class CorpusEdge(BaseModel):
    id: str
    source: str
    target: str
    kind: CorpusEdgeKind
    metadata: Dict[str, Any] = Field(default_factory=dict)


class CorpusGraph(BaseModel):
    version: int = 1
    advisory: bool = True
    built_at: str = Field(default_factory=lambda: datetime.now(UTC).isoformat())
    source_roots: List[str] = Field(default_factory=list)
    nodes: List[CorpusNode] = Field(default_factory=list)
    edges: List[CorpusEdge] = Field(default_factory=list)


class CorpusSettings(BaseModel):
    enabled: bool = True
    paths: List[str] = Field(default_factory=lambda: ["docs", "README.md"])
    llm_enrichment: bool = False
    vision_captions: bool = False
    write_html: bool = False
    auto_refresh_on_verify: bool = True
    extensions: List[str] = Field(
        default_factory=lambda: sorted(
            _CORPUS_TEXT_EXTS | _CORPUS_PDF_EXTS | _CORPUS_IMAGE_EXTS
        )
    )


def corpus_dir(project_root: Path) -> Path:
    return project_root / ".devcouncil" / "corpus"


def corpus_graph_path(project_root: Path) -> Path:
    return corpus_dir(project_root) / "graph.json"


def corpus_html_path(project_root: Path) -> Path:
    return corpus_dir(project_root) / "graph.html"


def load_corpus_settings(project_root: Path) -> CorpusSettings:
    merged: dict = {}
    try:
        from devcouncil.app.config import load_config

        merged.update(load_config(project_root).indexing.corpus.model_dump())
    except FileNotFoundError:
        pass
    return CorpusSettings.model_validate(merged)


def load_corpus_graph(project_root: Path) -> Optional[CorpusGraph]:
    path = corpus_graph_path(project_root)
    if not path.is_file():
        return None
    return read_model_json(path, CorpusGraph)


def write_corpus_graph(project_root: Path, graph: CorpusGraph) -> Path:
    out = corpus_graph_path(project_root)
    out.parent.mkdir(parents=True, exist_ok=True)
    write_model_json(out, graph)
    return out


def _slug_id(prefix: str, label: str) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", label.lower()).strip("-") or "node"
    return f"{prefix}:{slug}"


def _edge_id(source: str, target: str, kind: str) -> str:
    return f"{source}->{kind}->{target}"


def _iter_corpus_files(
    project_root: Path,
    roots: List[str],
    extensions: List[str],
) -> Iterator[Path]:
    ext_set = {e.lower() if e.startswith(".") else f".{e.lower()}" for e in extensions}
    root = project_root.resolve()
    for rel in roots:
        target = (root / rel).resolve()
        if not str(target).startswith(str(root)):
            continue
        if target.is_file():
            if target.suffix.lower() in ext_set:
                yield target
            continue
        if not target.is_dir():
            continue
        for dirpath, dirnames, filenames in target.walk(on_error=lambda _: None):
            dirnames[:] = [n for n in dirnames if n not in IGNORED_DIR_NAMES]
            for name in filenames:
                file_path = dirpath / name
                if file_path.suffix.lower() not in ext_set:
                    continue
                rel_path = file_path.relative_to(root)
                if should_skip_path(rel_path):
                    continue
                yield file_path


def _extract_text_doc(
    rel: str,
    text: str,
    *,
    suffix: str,
) -> Tuple[List[CorpusNode], List[CorpusEdge]]:
    doc_id = f"doc:{rel}"
    nodes: List[CorpusNode] = [
        CorpusNode(id=doc_id, kind="document", label=rel, path=rel, content=text[:8000])
    ]
    edges: List[CorpusEdge] = []
    parent_stack: List[Tuple[int, str]] = [(0, doc_id)]

    if suffix in _CORPUS_TEXT_EXTS and suffix != ".rst":
        for match in _MD_HEADING_RE.finditer(text):
            level = len(match.group(1))
            title = match.group(2).strip()
            sec_id = f"section:{rel}:{level}:{title[:48]}"
            nodes.append(
                CorpusNode(
                    id=sec_id,
                    kind="section",
                    label=title,
                    path=rel,
                    metadata={"level": level},
                )
            )
            while parent_stack and parent_stack[-1][0] >= level:
                parent_stack.pop()
            parent_id = parent_stack[-1][1] if parent_stack else doc_id
            edges.append(
                CorpusEdge(
                    id=_edge_id(parent_id, sec_id, "contains"),
                    source=parent_id,
                    target=sec_id,
                    kind="contains",
                )
            )
            parent_stack.append((level, sec_id))

        for match in _MD_LINK_RE.finditer(text):
            label, href = match.group(1).strip(), match.group(2).strip()
            link_id = _slug_id(f"link:{rel}", label)
            nodes.append(
                CorpusNode(
                    id=link_id,
                    kind="link",
                    label=label,
                    path=rel,
                    metadata={"href": href},
                )
            )
            edges.append(
                CorpusEdge(
                    id=_edge_id(doc_id, link_id, "mentions"),
                    source=doc_id,
                    target=link_id,
                    kind="mentions",
                )
            )
            if href and not href.startswith(("http://", "https://", "#", "mailto:")):
                target = href.split("#", 1)[0].lstrip("./")
                edges.append(
                    CorpusEdge(
                        id=_edge_id(link_id, f"doc:{target}", "links_to"),
                        source=link_id,
                        target=f"doc:{target}",
                        kind="links_to",
                    )
                )

        for match in _WIKILINK_RE.finditer(text):
            target = match.group(1).strip()
            label = (match.group(2) or target).strip()
            link_id = _slug_id(f"wiki:{rel}", target)
            nodes.append(
                CorpusNode(id=link_id, kind="link", label=label, path=rel, metadata={"wiki": target})
            )
            edges.append(
                CorpusEdge(
                    id=_edge_id(doc_id, link_id, "mentions"),
                    source=doc_id,
                    target=link_id,
                    kind="mentions",
                )
            )

    if suffix == ".rst":
        for match in _RST_HEADING_RE.finditer(text):
            title = match.group("title").strip()
            sec_id = f"section:{rel}:rst:{title[:48]}"
            nodes.append(CorpusNode(id=sec_id, kind="section", label=title, path=rel))
            edges.append(
                CorpusEdge(
                    id=_edge_id(doc_id, sec_id, "contains"),
                    source=doc_id,
                    target=sec_id,
                    kind="contains",
                )
            )

    for match in _CODE_PATH_RE.finditer(text):
        code_path = (match.group(1) or match.group(2) or "").strip()
        if not code_path or "/" not in code_path:
            continue
        ref_id = f"code:{code_path}"
        nodes.append(
            CorpusNode(id=ref_id, kind="code_ref", label=code_path, path=rel, metadata={"ref": code_path})
        )
        edges.append(
            CorpusEdge(
                id=_edge_id(doc_id, ref_id, "references"),
                source=doc_id,
                target=ref_id,
                kind="references",
            )
        )

    return nodes, edges


def _extract_pdf(rel: str, file_path: Path) -> Tuple[List[CorpusNode], List[CorpusEdge]]:
    doc_id = f"pdf:{rel}"
    nodes: List[CorpusNode] = [
        CorpusNode(id=doc_id, kind="pdf", label=rel, path=rel, metadata={"pages": 0})
    ]
    edges: List[CorpusEdge] = []
    try:
        from pypdf import PdfReader
    except ImportError:
        logger.debug("pypdf not installed; PDF %s indexed as metadata-only", rel)
        return nodes, edges

    try:
        reader = PdfReader(str(file_path))
        nodes[0].metadata["pages"] = len(reader.pages)
        for idx, page in enumerate(reader.pages[:200]):
            try:
                page_text = page.extract_text() or ""
            except Exception:
                page_text = ""
            page_id = f"pdf-page:{rel}:{idx + 1}"
            nodes.append(
                CorpusNode(
                    id=page_id,
                    kind="pdf_page",
                    label=f"{rel} p.{idx + 1}",
                    path=rel,
                    content=page_text[:4000],
                    metadata={"page": idx + 1},
                )
            )
            edges.append(
                CorpusEdge(
                    id=_edge_id(doc_id, page_id, "contains"),
                    source=doc_id,
                    target=page_id,
                    kind="contains",
                )
            )
            for match in _MD_LINK_RE.finditer(page_text):
                href = match.group(2).strip()
                if href.startswith(("http://", "https://")):
                    cite_id = _slug_id(f"cite:{rel}:{idx}", href)
                    nodes.append(
                        CorpusNode(
                            id=cite_id,
                            kind="link",
                            label=match.group(1).strip(),
                            path=rel,
                            metadata={"href": href},
                        )
                    )
                    edges.append(
                        CorpusEdge(
                            id=_edge_id(page_id, cite_id, "cites"),
                            source=page_id,
                            target=cite_id,
                            kind="cites",
                        )
                    )
    except Exception:
        logger.debug("PDF extract failed for %s", rel, exc_info=True)
    return nodes, edges


def _extract_image(
    rel: str,
    file_path: Path,
    *,
    vision_captions: bool,
    project_root: Path,
) -> Tuple[List[CorpusNode], List[CorpusEdge]]:
    img_id = f"image:{rel}"
    stat = file_path.stat()
    meta: Dict[str, Any] = {
        "size_bytes": stat.st_size,
        "suffix": file_path.suffix.lower(),
    }
    caption: Optional[str] = None
    if vision_captions:
        try:
            from devcouncil.app.config import load_config

            cfg = load_config(project_root)
            if cfg.models.roles:
                caption = _optional_vision_caption(project_root, file_path)
        except Exception:
            logger.debug("vision caption skipped for %s", rel, exc_info=True)
    if caption:
        meta["caption"] = caption
    return (
        [CorpusNode(id=img_id, kind="image", label=rel, path=rel, content=caption, metadata=meta)],
        [],
    )


def _optional_vision_caption(project_root: Path, file_path: Path) -> Optional[str]:
    """Best-effort caption when a vision-capable model is configured (opt-in)."""
    # ModelRouter does not yet expose a standardized multimodal request API.
    # Keep the opt-in deterministic and advisory until that contract exists.
    return None


def _optional_llm_enrich(project_root: Path, graph: CorpusGraph) -> CorpusGraph:
    settings = load_corpus_settings(project_root)
    if not settings.llm_enrichment:
        return graph
    try:
        from devcouncil.app.config import load_config

        cfg = load_config(project_root)
        if not cfg.models.roles:
            return graph
    except Exception:
        return graph
    # Placeholder: deterministic graph is authoritative; LLM enrichment is optional.
    return graph


def build_corpus(
    project_root: Path,
    *,
    path: Optional[str] = None,
) -> CorpusGraph:
    """Build the advisory corpus graph under ``.devcouncil/corpus/``."""
    settings = load_corpus_settings(project_root)
    roots = [path] if path else list(settings.paths)
    nodes: List[CorpusNode] = []
    edges: List[CorpusEdge] = []
    seen_nodes: set[str] = set()
    seen_edges: set[str] = set()

    for file_path in _iter_corpus_files(project_root, roots, settings.extensions):
        rel = file_path.relative_to(project_root.resolve()).as_posix()
        suffix = file_path.suffix.lower()
        if suffix in _CORPUS_TEXT_EXTS:
            try:
                text = file_path.read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            n, e = _extract_text_doc(rel, text, suffix=suffix)
        elif suffix in _CORPUS_PDF_EXTS:
            n, e = _extract_pdf(rel, file_path)
        elif suffix in _CORPUS_IMAGE_EXTS:
            n, e = _extract_image(
                rel,
                file_path,
                vision_captions=settings.vision_captions,
                project_root=project_root,
            )
        else:
            continue
        for node in n:
            if node.id not in seen_nodes:
                seen_nodes.add(node.id)
                nodes.append(node)
        for edge in e:
            if edge.id not in seen_edges:
                seen_edges.add(edge.id)
                edges.append(edge)

    graph = CorpusGraph(source_roots=roots, nodes=nodes, edges=edges)
    graph = _optional_llm_enrich(project_root, graph)
    write_corpus_graph(project_root, graph)
    if settings.write_html:
        write_corpus_html(project_root, graph)
    return graph


def write_corpus_html(project_root: Path, graph: CorpusGraph) -> Path:
    """Self-contained advisory corpus listing (not the code-graph visualizer)."""
    rows = []
    for node in sorted(graph.nodes, key=lambda n: (n.kind, n.label)):
        rows.append(
            f"<tr><td>{node.kind}</td><td>{node.label}</td><td>{node.path or ''}</td></tr>"
        )
    html = (
        "<!DOCTYPE html><html><head><meta charset='utf-8'>"
        "<title>DevCouncil Corpus Index</title>"
        "<style>body{font-family:system-ui;margin:1.5rem}"
        "table{border-collapse:collapse;width:100%}td,th{border:1px solid #ccc;padding:.4rem}"
        "</style></head><body>"
        "<h1>DevCouncil Corpus Index (advisory)</h1>"
        f"<p>Built {graph.built_at} — {len(graph.nodes)} nodes, {len(graph.edges)} edges</p>"
        "<table><thead><tr><th>Kind</th><th>Label</th><th>Path</th></tr></thead><tbody>"
        + "".join(rows)
        + "</tbody></table></body></html>"
    )
    out = corpus_html_path(project_root)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(html, encoding="utf-8")
    return out


def query_corpus(project_root: Path, query: str, *, limit: int = 20) -> Dict[str, Any]:
    graph = load_corpus_graph(project_root)
    if graph is None:
        return {"error": "No corpus graph; run `dev corpus build` first.", "matches": []}
    needle = query.strip().lower()
    if not needle:
        return {"error": "Empty query.", "matches": []}
    scored: List[Tuple[int, CorpusNode]] = []
    for node in graph.nodes:
        hay = " ".join(
            filter(None, [node.label, node.content or "", str(node.metadata)])
        ).lower()
        if needle in hay:
            scored.append((hay.count(needle), node))
    scored.sort(key=lambda item: (-item[0], item[1].label))
    matches = [
        {
            "id": node.id,
            "kind": node.kind,
            "label": node.label,
            "path": node.path,
            "score": score,
        }
        for score, node in scored[:limit]
    ]
    return {"query": query, "matches": matches, "count": len(matches)}


def corpus_status(project_root: Path) -> Dict[str, Any]:
    settings = load_corpus_settings(project_root)
    path = corpus_graph_path(project_root)
    graph = load_corpus_graph(project_root)
    return {
        "enabled": settings.enabled,
        "graph_path": str(path.relative_to(project_root.resolve())) if path.is_file() else None,
        "built_at": graph.built_at if graph else None,
        "node_count": len(graph.nodes) if graph else 0,
        "edge_count": len(graph.edges) if graph else 0,
        "source_roots": graph.source_roots if graph else settings.paths,
        "advisory": True,
        "verify_gates": False,
    }
