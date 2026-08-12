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
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Dict, Iterable, Iterator, List, Literal, Optional, Set, Tuple

from pydantic import BaseModel, Field

from devcouncil.indexing.walk import IGNORED_DIR_NAMES, should_skip_path
from devcouncil.utils.json_persist import read_model_json, write_model_json


@dataclass
class DiscoveryReport:
    sources_attempted: List[str] = field(default_factory=list)
    sources_yielded: Dict[str, int] = field(default_factory=dict)
    errors: List[Dict[str, str]] = field(default_factory=list)


logger = logging.getLogger(__name__)

# Python + JS/TS always. Go and Rust are included only when tree-sitter edges
# are available (see is_liveness_code_file).
_LIVENESS_EXTS = {".py", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"}
_GO_LIVENESS_EXT = ".go"
_RUST_LIVENESS_EXT = ".rs"
_TEST_DIR_NAMES = {"tests", "test", "__tests__", "spec", "androidtest"}
_SCRIPT_DIR_NAMES = {"scripts", "bin", "benchmarks"}
ALLOW_UNWIRED = "devcouncil: allow-unwired"
_IMPORTLIB_RE = re.compile(
    r"""(?:importlib(?:\.import_module)?|__import__)\s*\(\s*['"]([^'"]+)['"]"""
)
_DYNAMIC_IMPORT_RE = re.compile(r"""import\s*\(\s*['"]([^'"]+)['"]\s*\)""")
# Vite/webpack ``new Worker(new URL("./x", import.meta.url))`` (and bare URL).
_WORKER_URL_RE = re.compile(
    r"""(?:new\s+(?:Worker|SharedWorker)\s*\(\s*)?"""
    r"""new\s+URL\s*\(\s*['"]([^'"]+)['"]\s*,\s*import\.meta\.url"""
)
# ``python -m pkg.mod`` and argv forms like ``"-m", "pkg.mod"``.
_PYTHON_DASH_M_RE = re.compile(
    r"""(?:^|[^\w-])(?:-m|--module)(?:\s+|\s*,\s*)['"]([A-Za-z_][\w.]*)['"]"""
    r"""|['"](?:-m|--module)['"]\s*,\s*['"]([A-Za-z_][\w.]*)['"]"""
)
# ``importlib.resources.files("pkg.sub")`` package-resource loads.
_PACKAGE_RESOURCES_RE = re.compile(
    r"""(?:resources\.)?files\s*\(\s*['"]([A-Za-z_][\w.]*)['"]"""
)
# Bundled asset basenames referenced as string constants (plugins, images, …).
_BUNDLED_ASSET_RE = re.compile(
    r"""['"]([A-Za-z_][\w.-]*\.(?:mjs|cjs|js|css|svg|png|jpe?g|webp|html))['"]"""
)
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
    "receiver", "api_view", "action", "subscriber", "listener", "on_event",
    "dramatiq.", "huey.",
)

# Bumped when dead-symbol / token-scan semantics change so ratchet baselines
# skip stale symbol diffs instead of firing false stranded_code regressions.
# v4 = confidence hardening: generated-file / side-effect-carrier / entry-script
# / barrel-init exemptions, launcher-file references, globals()/hasattr dynamic
# evidence, and Go/Rust liveness gated on tree-sitter availability.
LIVENESS_SCAN_VERSION = 4

_VENDOR_DIR_NAMES = frozenset({"vendor", "vendored", "node_modules"})


def _norm(path: str) -> str:
    """Normalize to posix and strip leading ``./`` only (never ``lstrip('./')``)."""
    s = str(path).replace("\\", "/")
    while s.startswith("./"):
        s = s[2:]
    return s


def is_test_path(path: str) -> bool:
    """True when path looks like a test file by common conventions."""
    norm = _norm(path)
    norm_l = norm.lower()
    name = norm.rsplit("/", 1)[-1]
    name_l = name.lower()
    parts = norm_l.split("/")
    in_test_dir = any(p in _TEST_DIR_NAMES for p in parts[:-1])
    # Android / JVM: src/test/, src/androidTest/
    if "/src/test/" in f"/{norm_l}/" or "/src/androidtest/" in f"/{norm_l}/":
        in_test_dir = True
    looks_like_test = (
        name_l.startswith("test_")
        or name_l == "conftest.py"
        or any(
            name_l.endswith(suffix)
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
        # Swift / Kotlin naming: FooTests.swift, FooTest.kt (case-sensitive suffix).
        or name.endswith("Tests.swift")
        or name.endswith("Test.kt")
        or name.endswith("Tests.kt")
    )
    return looks_like_test or (in_test_dir and not name.startswith("."))


def is_liveness_code_file(path: str) -> bool:
    """True for languages with reliable file-level import edges.

    Go and Rust are included only when the optional tree-sitter layer is
    available: Go same-package wiring depends on call edges and Rust on
    ``mod``/``use`` edges. Without tree-sitter those checks cannot run, and a
    check that could not run must not produce unwired/unreachable verdicts —
    the whole language drops out of file liveness (fail closed).
    """
    suffix = Path(_norm(path)).suffix.lower()
    if suffix in {_GO_LIVENESS_EXT, _RUST_LIVENESS_EXT}:
        try:
            from devcouncil.indexing.ts_imports import tree_sitter_available

            return tree_sitter_available()
        except Exception:
            return False
    return suffix in _LIVENESS_EXTS


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
# globals()["name"] / vars()["name"] registry dispatch and hasattr(obj, "name")
# capability probes are dynamic-usage evidence just like getattr — they feed the
# same index (GETATTR_INDEX_PREFIX) so named symbols are seeded live.
_GLOBALS_NAME_RE = re.compile(
    r"""(?:globals|vars)\s*\(\s*\)\s*\[\s*['"]([A-Za-z_][A-Za-z0-9_]*)['"]"""
)
_HASATTR_NAME_RE = re.compile(
    r"""hasattr\s*\(\s*[^,]+,\s*['"]([A-Za-z_][A-Za-z0-9_]*)['"]"""
)


# ---------------------------------------------------------------------------
# Generated files, side-effect carriers, entry-shaped scripts, barrel inits
# ---------------------------------------------------------------------------
# Content-based liveness exemptions shared by ``file_liveness`` (map), the
# incremental shard path, and the verify ``unwired_file`` gate. Fail closed:
# these only ever *remove* files from unwired/unreachable candidate lists —
# they never mark anything else more dead.

# Path patterns of well-known code generators (protoc, grpc, kubebuilder).
_GENERATED_PATH_RE = re.compile(
    r"(?:^|/)(?:"
    r"[^/]*_pb2(?:_grpc)?\.pyi?"  # protoc / grpcio-tools Python stubs
    r"|[^/]*\.pb(?:\.gw)?\.go"  # protoc-gen-go / grpc-gateway
    r"|[^/]*_pb\.(?:js|ts|d\.ts)"  # protobuf JS/TS stubs
    r"|zz_generated[^/]*\.go"  # kubebuilder deepcopy et al.
    r")$"
)
_GENERATED_HEADER_MARKERS = (
    "do not edit",
    "code generated",
    "@generated",
    "auto-generated",
    "autogenerated",
    "automatically generated",
    "generated by",
)
_GENERATED_SNIFF_LINES = 5
_COMMENT_LINE_PREFIXES = ("#", "//", "/*", "*", "--", "<!--", ";", "'")

_PY_MAIN_GUARD_RE = re.compile(r"""(?m)^if\s+__name__\s*==\s*['"]__main__['"]""")
_GO_INIT_FUNC_RE = re.compile(r"(?m)^func\s+init\s*\(")
_GO_EMBED_MARKER = "//go:embed"


def is_generated_path(path: str) -> bool:
    """True when the basename matches a well-known code-generator pattern."""
    return bool(_GENERATED_PATH_RE.search(_norm(path).lower()))


def source_has_generated_header(source: str) -> bool:
    """True when a file-head comment line carries a generated-code marker.

    Only the first few lines count, and only comment/blank lines — a body
    mention of "generated" must not exempt a handwritten file.
    """
    for line in source.splitlines()[:_GENERATED_SNIFF_LINES]:
        stripped = line.strip()
        if not stripped:
            continue
        if not stripped.startswith(_COMMENT_LINE_PREFIXES):
            # First real code line ends the header window.
            return False
        lowered = stripped.lower()
        if any(marker in lowered for marker in _GENERATED_HEADER_MARKERS):
            return True
    return False


def _is_reexport_only_init_source(source: str) -> bool:
    """True when an ``__init__.py`` body is only re-exports / ``__all__`` / constants.

    Such files are package markers: their liveness follows their package members
    (importing ``pkg.mod`` executes ``pkg/__init__.py``), so flagging the barrel
    itself as unwired is noise. A body with defs/classes/control flow is real
    behavior and stays flaggable.
    """
    try:
        tree = ast.parse(source)
    except (SyntaxError, ValueError):
        return False
    for stmt in tree.body:
        if isinstance(stmt, (ast.Import, ast.ImportFrom, ast.Pass)):
            continue
        if isinstance(stmt, ast.Expr) and isinstance(stmt.value, ast.Constant):
            continue  # docstring / bare literal
        if isinstance(stmt, (ast.Assign, ast.AnnAssign)):
            value = stmt.value
            if value is None or isinstance(
                value, (ast.Constant, ast.List, ast.Tuple, ast.Name, ast.Attribute)
            ):
                continue
        return False
    return True


def content_liveness_exemption(
    project_root: Path,
    path: str,
    *,
    source: Optional[str] = None,
) -> Optional[str]:
    """Reason ``path`` is wired-by-content, or None.

    Reasons: ``"generated"`` (generator path pattern or file-head marker),
    ``"side_effect"`` (Go ``//go:embed`` carrier / ``func init()`` loader),
    ``"entry_script"`` (Python ``__main__`` guard — launched by external
    process managers), ``"reexport_init"`` (barrel/package-marker init).
    Never raises — degrades to None.
    """
    try:
        norm = _norm(path)
        if is_generated_path(norm):
            return "generated"
        suffix = Path(norm).suffix.lower()
        if suffix not in (_LIVENESS_EXTS | {_GO_LIVENESS_EXT, _RUST_LIVENESS_EXT}):
            return None
        if source is None:
            try:
                source = (project_root / norm).read_text(
                    encoding="utf-8", errors="replace"
                )
            except OSError:
                return None
        if source_has_generated_header(source):
            return "generated"
        if suffix == ".py":
            if _PY_MAIN_GUARD_RE.search(source):
                return "entry_script"
            if norm.endswith("__init__.py") and _is_reexport_only_init_source(source):
                return "reexport_init"
        elif suffix == _GO_LIVENESS_EXT:
            if _GO_EMBED_MARKER in source or _GO_INIT_FUNC_RE.search(source):
                return "side_effect"
        return None
    except Exception:
        logger.debug("content_liveness_exemption failed for %s", path, exc_info=True)
        return None


# ---------------------------------------------------------------------------
# Launcher files (Dockerfile / Procfile / compose / Makefile / package.json
# scripts) — external process managers reference modules by path or module
# string; those references wire the target exactly like a dynamic import.
# ---------------------------------------------------------------------------

_LAUNCHER_BASENAME_RE = re.compile(
    r"^(?:"
    r"dockerfile(?:\..+)?|.+\.dockerfile"
    r"|procfile(?:\..+)?"
    r"|makefile|gnumakefile|justfile"
    r"|docker-compose[^/]*\.ya?ml|compose\.ya?ml"
    r")$",
    re.IGNORECASE,
)
_LAUNCHER_PATH_REF_RE = re.compile(
    r"(?<![\w.-])((?:[\w.-]+/)*[\w-]+\.(?:py|mjs|cjs|jsx?|tsx?|sh|go|rb))(?![\w-])"
)
# Unquoted ``python -m pkg.mod`` (quoted argv forms handled by _PYTHON_DASH_M_RE).
_LAUNCHER_DASH_M_RE = re.compile(
    r"""(?:^|[\s"'=,\[])(?:-m|--module)["'\s,]+([A-Za-z_][\w.]*)"""
)
# uvicorn/gunicorn-style ``module.path:attr`` application targets.
_LAUNCHER_MODULE_ATTR_RE = re.compile(
    r"(?<![\w.:/@-])([A-Za-z_][\w.]*):([A-Za-z_]\w*)(?![\w:])"
)


def is_launcher_file(path: str) -> bool:
    """True for process-manager / build-runner files scanned for module refs."""
    name = _norm(path).rsplit("/", 1)[-1]
    return bool(_LAUNCHER_BASENAME_RE.match(name))


def _launcher_dir_resolved(base_dir: str, spec: str) -> str:
    joined = f"{base_dir}/{spec}" if base_dir else spec
    return _normalize_rel_path(joined)


def launcher_reference_keys(path: str, source: str) -> Set[str]:
    """Normalized module-form keys for path/module references in a launcher file.

    Emits both raw specs and specs resolved relative to the launcher's own
    directory (a Dockerfile's ``CMD ["python", "runner.py"]`` runs next to the
    file). Bare ``module:attr`` targets resolve relative to the launcher dir
    only, so ``postgres:latest`` image tags cannot clear unrelated modules
    unless a same-named sibling module actually exists.
    """
    norm = _norm(path)
    base_dir = norm.rsplit("/", 1)[0] if "/" in norm else ""
    specs: List[str] = []
    for match in _LAUNCHER_PATH_REF_RE.finditer(source):
        spec = _normalize_rel_path(match.group(1))
        if not spec:
            continue
        specs.append(spec)
        specs.append(_launcher_dir_resolved(base_dir, spec))
    for match in _LAUNCHER_DASH_M_RE.finditer(source):
        module = match.group(1)
        specs.append(module)
        specs.append(_launcher_dir_resolved(base_dir, module.replace(".", "/")))
    for match in _PYTHON_DASH_M_RE.finditer(source):
        module = match.group(1) or match.group(2)
        if module:
            specs.append(module)
            specs.append(_launcher_dir_resolved(base_dir, module.replace(".", "/")))
    for match in _LAUNCHER_MODULE_ATTR_RE.finditer(source):
        module = match.group(1)
        resolved = _launcher_dir_resolved(base_dir, module.replace(".", "/"))
        if "." in module:
            # Dotted target (pkg.app:main) is unambiguous — keep the raw form.
            specs.append(module)
        specs.append(resolved)
    return {form for spec in specs for form in _module_forms(spec)}


def _package_json_script_keys(path: str, source: str) -> Set[str]:
    """Launcher-style keys from a ``package.json`` ``scripts`` table."""
    try:
        data = json.loads(source)
    except Exception:
        return set()
    scripts = data.get("scripts") if isinstance(data, dict) else None
    if not isinstance(scripts, dict):
        return set()
    text = "\n".join(str(v) for v in scripts.values() if isinstance(v, str))
    if not text:
        return set()
    return launcher_reference_keys(path, text)

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
        base = dec.split("(", 1)[0].lstrip("@").strip().lower()
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
        # Cargo build scripts are invoked by rustc, not imported by app code.
        if name == "build.rs":
            return True
        # Bundler / test-runner configs are tooling entrypoints, not product modules.
        # Exempt from unwired/unreachable — do NOT seed BFS from them.
        if _TOOLING_CONFIG_RE.search(norm):
            return True
        if ".config." in name and suffix in {
            ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".mts", ".cts",
        }:
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


def _cargo_toml_script_targets(root: Path, file_set: Set[str]) -> Set[str]:
    """Discover entry points declared in Cargo.toml manifests."""
    found: Set[str] = set()
    manifests = [p for p in file_set if p.rsplit("/", 1)[-1] == "Cargo.toml"]
    if not manifests and (root / "Cargo.toml").is_file():
        manifests = ["Cargo.toml"]
    for manifest in manifests:
        text = _read_text(root, manifest)
        if not text:
            continue
        try:
            import tomllib
            data = tomllib.loads(text)
        except Exception:
            continue
        prefix = manifest[: -len("Cargo.toml")]
        
        bins = data.get("bin") or []
        if isinstance(bins, list):
            for b in bins:
                if isinstance(b, dict):
                    bpath = b.get("path")
                    if isinstance(bpath, str):
                        cand = _norm(f"{prefix}{bpath}")
                        if cand in file_set:
                            found.add(cand)
        for default_cand in [f"{prefix}src/main.rs", f"{prefix}src/lib.rs"]:
            cand = _norm(default_cand)
            if cand in file_set:
                found.add(cand)
    return found


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
    prefix_dir = f"{parts}/"
    suffix_dir = f"/{parts}/"
    for f in sorted(file_set):
        if f.endswith(suffix) or f.endswith(suffix_init) or f == f"{parts}.py":
            out.add(f)
            return
        if (f.startswith(prefix_dir) or suffix_dir in f) and f.endswith(".py"):
            out.add(f)
            return


# Workspace manifests scanned for entry targets (sorted; bounded for determinism
# and to keep monorepos with generated packages from ballooning map time).
_PACKAGE_MANIFEST_CAP = 200


def _package_json_entry_targets(root: Path, file_set: Set[str]) -> Set[str]:
    """Entry targets from the root ``package.json`` and workspace manifests.

    Monorepos often have no root ``main``/``bin``/``exports`` at all — the real
    entries live in ``frontend/package.json`` etc., so every tracked manifest is
    scanned (capped) with its targets resolved relative to the manifest's dir.
    """
    manifests = sorted(
        (p for p in file_set if p.rsplit("/", 1)[-1] == "package.json"),
        key=lambda p: (p.count("/"), p),
    )[:_PACKAGE_MANIFEST_CAP]
    if not manifests and (root / "package.json").is_file():
        manifests = ["package.json"]
    found: Set[str] = set()
    for manifest in manifests:
        _package_json_entry_targets_for(root, manifest, file_set, found)
    return found


def _package_json_entry_targets_for(
    root: Path, manifest: str, file_set: Set[str], found: Set[str]
) -> None:
    text = _read_text(root, manifest)
    if not text:
        return
    try:
        data = json.loads(text)
    except Exception:
        return
    prefix = manifest[: -len("package.json")]  # "" at root, "frontend/" nested

    def _add(candidate: object) -> None:
        if not isinstance(candidate, str):
            return
        rel = _norm(f"{prefix}{_norm(candidate)}")
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


# Convention-based mains. JS/TS and Rust paths are specific enough to seed by
# name; ``main.go`` and Python service mains collide with helpers often enough
# that a bounded content sniff gates them.
_JS_MAIN_SEED_RE = re.compile(
    r"(?:^|/)"
    r"(?:"
    r"src/(?:index|main)"
    r"|App"
    r"|(?:server|api|backend|worker|functions|lambda)/index"
    r")"
    r"\.(?:ts|tsx|js|jsx|mjs)$"
)
_RUST_MAIN_SEED_RE = re.compile(r"(?:^|/)src/(?:main\.rs|lib\.rs|bin/[^/]+\.rs)$")
_SWIFT_AT_MAIN_RE = re.compile(r"@main\b")
_SWIFTPM_EXECUTABLE_RE = re.compile(
    r"""\.executableTarget\s*\(\s*name\s*:\s*["']([^"']+)["']"""
    r"""|executableTarget\s*\(\s*name\s*:\s*["']([^"']+)["']"""
)
_TOOLING_CONFIG_RE = re.compile(
    r"(?:^|/)"
    r"(?:"
    r"vite\.config\.[cm]?[jt]s"
    r"|vitest\.config\.[cm]?[jt]s"
    r"|webpack\.config\.[cm]?[jt]s"
    r"|rollup\.config\.[cm]?[jt]s"
    r"|esbuild\.config\.[cm]?[jt]s"
    r"|next\.config\.[cm]?[jt]s"
    r"|astro\.config\.[cm]?[jt]s"
    r"|nuxt\.config\.[cm]?[jt]s"
    r"|playwright\.config\.[cm]?[jt]s"
    r"|tailwind\.config\.[cm]?[jt]s"
    r"|postcss\.config\.[cm]?[jt]s"
    r"|eslint\.config\.[cm]?[jt]s"
    r")"
    r"$"
)
_PY_MAIN_SEED_NAMES = {"main.py", "app.py", "wsgi.py", "asgi.py"}
_C_MAIN_SEED_SUFFIXES = (".c", ".cc", ".cpp", ".cxx")
_C_MAIN_RE = re.compile(r"\b(?:int|void)\s+main\s*\(")
# "__main__" covers both quote styles of the run guard.
_PY_MAIN_SEED_MARKERS = ("__main__", "FastAPI(", "Flask(", "uvicorn.run(")
_MAIN_SEED_SNIFF_CAP = 512
_JS_RESOLVE_EXTS = (".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs")
_JS_SUFFIXES = frozenset(_JS_RESOLVE_EXTS)


def _normalize_rel_path(target: str) -> str:
    parts: List[str] = []
    for comp in target.replace("\\", "/").split("/"):
        if comp in ("", "."):
            continue
        if comp == "..":
            if parts:
                parts.pop()
            continue
        parts.append(comp)
    return "/".join(parts)


def _probe_js_path(norm: str, file_set: Set[str]) -> Optional[str]:
    """Resolve extensionless / ``.js``-suffixed specs against ``file_set``."""
    if not norm:
        return None
    candidates = [norm]
    candidates += [f"{norm}{ext}" for ext in _JS_RESOLVE_EXTS]
    candidates += [f"{norm}/index{ext}" for ext in _JS_RESOLVE_EXTS]
    suffix = Path(norm).suffix.lower()
    if suffix in _JS_SUFFIXES:
        stem = norm[: -len(suffix)]
        candidates += [f"{stem}{ext}" for ext in _JS_RESOLVE_EXTS]
        candidates += [f"{stem}/index{ext}" for ext in _JS_RESOLVE_EXTS]
    for cand in candidates:
        if cand in file_set:
            return cand
    return None


def _expand_roots_via_dynamic_imports(
    root: Path, file_set: Set[str], seeds: Set[str]
) -> Set[str]:
    """One-BFS expansion: dynamic ``import(...)`` / worker URL targets become roots.

    Covers Vite/CRA ``main.tsx → import('./App')``, alias ``import('@/x')``, and
    ``new Worker(new URL('./worker.ts', import.meta.url))`` without treating every
    dynamic import in the repo as a seed.
    """
    from devcouncil.indexing.repo_mapper import RepoMapper

    mapper = RepoMapper(root)
    mapper._last_file_set = file_set
    expanded = set(seeds)
    queue = list(seeds)
    seen = set(seeds)
    while queue:
        cur = queue.pop()
        text = _read_text(root, cur)
        if not text:
            continue
        specs: List[str] = [match.group(1) for match in _DYNAMIC_IMPORT_RE.finditer(text)]
        specs.extend(match.group(1) for match in _WORKER_URL_RE.finditer(text))
        for spec in specs:
            if not spec:
                continue
            hit: Optional[str] = None
            if spec.startswith("."):
                try:
                    joined = (Path(cur).parent / spec).as_posix()
                except Exception:
                    continue
                hit = _probe_js_path(_normalize_rel_path(joined), file_set)
            else:
                try:
                    hit = mapper._resolve_js_spec(cur, spec, file_set)
                except Exception:
                    hit = None
            if hit and hit not in seen:
                seen.add(hit)
                expanded.add(hit)
                queue.append(hit)
        if cur.endswith(".py"):
            py_specs = [match.group(1) for match in _IMPORTLIB_RE.finditer(text)]
            for mod in py_specs:
                py_hits: Set[str] = set()
                _add_module_file(mod, file_set, py_hits)
                for hit in py_hits:
                    if hit and hit not in seen:
                        seen.add(hit)
                        expanded.add(hit)
                        queue.append(hit)
    return expanded


def _conventional_main_seeds(root: Path, file_set: Set[str]) -> Set[str]:
    """Language-convention entry mains: Go/Rust/C/C++ binaries, Python service
    mains, JS/TS ``src/index``/``src/main``/``App``/service ``index`` modules,
    Rust ``main.rs`` / ``lib.rs``, Swift ``main.swift`` / ``@main``, and
    Android ``MainActivity``.

    Tooling configs (Vite/PostCSS/…) are structural exemptions only — never BFS
    seeds (they do not import product modules).
    """
    found: Set[str] = set()
    sniffed = 0
    for f in sorted(file_set):
        if _JS_MAIN_SEED_RE.search(f) or _RUST_MAIN_SEED_RE.search(f):
            found.add(f)
            continue
        name = f.rsplit("/", 1)[-1]
        if name == "main.swift":
            found.add(f)
            continue
        if name in {"MainActivity.kt", "MainActivity.java"}:
            found.add(f)
            continue
        if name.endswith(".swift"):
            if sniffed >= _MAIN_SEED_SNIFF_CAP:
                continue
            sniffed += 1
            if _SWIFT_AT_MAIN_RE.search(_read_text(root, f)):
                found.add(f)
            continue
        if name == "main.go" or (f.endswith(".go") and any(d in f for d in ("cmd/", "bin/"))):
            if sniffed >= _MAIN_SEED_SNIFF_CAP:
                continue
            sniffed += 1
            text = _read_text(root, f)
            if "package main" in text and "func main(" in text:
                found.add(f)
        elif name in _PY_MAIN_SEED_NAMES:
            if sniffed >= _MAIN_SEED_SNIFF_CAP:
                continue
            sniffed += 1
            text = _read_text(root, f)
            if any(marker in text for marker in _PY_MAIN_SEED_MARKERS):
                found.add(f)
        elif name.endswith(_C_MAIN_SEED_SUFFIXES):
            # C/C++ binaries: a file defining main() is a conventional entry,
            # same as ``package main`` in Go.
            if sniffed >= _MAIN_SEED_SNIFF_CAP:
                continue
            sniffed += 1
            if _C_MAIN_RE.search(_read_text(root, f)):
                found.add(f)
    return found


def _swiftpm_package_targets(root: Path, file_set: Set[str]) -> Set[str]:
    """Entry seeds from SwiftPM ``Package.swift`` executable targets.

    Resolves ``.executableTarget(name: "App")`` to conventional
    ``Sources/<Name>/main.swift`` / ``Sources/<Name>/<Name>.swift`` paths when
    present (Cargo ``src/main.rs`` analogue).
    """
    found: Set[str] = set()
    manifests = sorted(
        (p for p in file_set if p.rsplit("/", 1)[-1] == "Package.swift"),
        key=lambda p: (p.count("/"), p),
    )[:_PACKAGE_MANIFEST_CAP]
    if not manifests and (root / "Package.swift").is_file():
        manifests = ["Package.swift"]
    for manifest in manifests:
        text = _read_text(root, manifest)
        if not text:
            continue
        prefix = manifest[: -len("Package.swift")]
        names: List[str] = []
        for match in _SWIFTPM_EXECUTABLE_RE.finditer(text):
            names.append(match.group(1) or match.group(2))
        for target_name in names:
            if not target_name:
                continue
            for cand in (
                f"{prefix}Sources/{target_name}/main.swift",
                f"{prefix}Sources/{target_name}/{target_name}.swift",
            ):
                cand_n = _norm(cand)
                if cand_n in file_set:
                    found.add(cand_n)
        # Also seed Package.swift itself as a structural marker when present.
        if manifest in file_set:
            found.add(_norm(manifest))
    return found


def _android_manifest_seeds(root: Path, file_set: Set[str]) -> Set[str]:
    """AndroidManifest.xml as structural entry seed (+ MainActivity when present)."""
    found: Set[str] = set()
    for path in file_set:
        name = path.rsplit("/", 1)[-1]
        if name == "AndroidManifest.xml":
            found.add(_norm(path))
        elif name in {"MainActivity.kt", "MainActivity.java"}:
            found.add(_norm(path))
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


def entry_roots_with_report(
    root: Path,
    files: Iterable[str],
    *,
    production_only: bool = False,
) -> Tuple[List[str], DiscoveryReport]:
    """Discover entry roots and return them alongside a DiscoveryReport with diagnostic metadata."""
    report = DiscoveryReport()
    try:
        file_set = {_norm(f) for f in files}
        roots: Set[str] = set()

        report.sources_attempted.append("config")
        try:
            cfg_roots = _config_declared_entry_roots(root, file_set)
            report.sources_yielded["config"] = len(cfg_roots)
            roots |= cfg_roots
        except Exception as exc:
            report.errors.append({"source": "config", "error": str(exc)})

        report.sources_attempted.append("pyproject")
        try:
            pyp_roots = _pyproject_script_targets(root, file_set)
            report.sources_yielded["pyproject"] = len(pyp_roots)
            roots |= pyp_roots
        except Exception as exc:
            report.errors.append({"source": "pyproject", "error": str(exc)})

        report.sources_attempted.append("cargo")
        try:
            cargo_roots = _cargo_toml_script_targets(root, file_set)
            report.sources_yielded["cargo"] = len(cargo_roots)
            roots |= cargo_roots
        except Exception as exc:
            report.errors.append({"source": "cargo", "error": str(exc)})

        report.sources_attempted.append("swiftpm")
        try:
            spm_roots = _swiftpm_package_targets(root, file_set)
            report.sources_yielded["swiftpm"] = len(spm_roots)
            roots |= spm_roots
        except Exception as exc:
            report.errors.append({"source": "swiftpm", "error": str(exc)})

        report.sources_attempted.append("android")
        try:
            android_roots = _android_manifest_seeds(root, file_set)
            report.sources_yielded["android"] = len(android_roots)
            roots |= android_roots
        except Exception as exc:
            report.errors.append({"source": "android", "error": str(exc)})

        report.sources_attempted.append("package_json")
        try:
            pj_roots = _package_json_entry_targets(root, file_set)
            report.sources_yielded["package_json"] = len(pj_roots)
            roots |= pj_roots
        except Exception as exc:
            report.errors.append({"source": "package_json", "error": str(exc)})

        report.sources_attempted.append("conventional")
        try:
            conv_roots = set()
            for f in _conventional_main_seeds(root, file_set):
                if production_only and is_test_path(f):
                    continue
                conv_roots.add(f)
            report.sources_yielded["conventional"] = len(conv_roots)
            roots |= conv_roots
        except Exception as exc:
            report.errors.append({"source": "conventional", "error": str(exc)})

        report.sources_attempted.append("scripts")
        try:
            script_roots = set()
            for f in file_set:
                if production_only and is_test_path(f):
                    continue
                name = Path(f).name
                if name in {"__main__.py", "manage.py"}:
                    script_roots.add(f)
            report.sources_yielded["scripts"] = len(script_roots)
            roots |= script_roots
        except Exception as exc:
            report.errors.append({"source": "scripts", "error": str(exc)})

        report.sources_attempted.append("dynamic")
        try:
            expanded = _expand_roots_via_dynamic_imports(root, file_set, roots)
            report.sources_yielded["dynamic"] = max(0, len(expanded) - len(roots))
            roots = expanded
        except Exception as exc:
            report.errors.append({"source": "dynamic", "error": str(exc)})

        if production_only:
            roots = {r for r in roots if not is_test_path(r)}

        return sorted(roots), report
    except Exception as exc:
        logger.debug("entry_roots_with_report failed", exc_info=True)
        report.errors.append({"source": "entry_roots", "error": str(exc)})
        return [], report


def entry_roots(
    root: Path,
    files: Iterable[str],
    *,
    production_only: bool = False,
) -> list[str]:
    """Config-declared + small convention set used as BFS reachability seeds.

    Seeds are pyproject targets, package.json targets (root + workspace
    manifests), language-convention mains (``main.go`` binaries, Python service
    mains, JS/TS ``src/index``/``src/main``/``App``/service indexes, Rust
    ``src/main.rs``/``lib.rs``), relative dynamic-import expansions from those
    seeds, plus ``__main__.py`` / ``manage.py``. Structural exemptions (routes,
    migrations, scripts, stories, tests, tooling configs, ``build.rs``) remain a
    skip-list for unwired/unreachable — they are NOT BFS seeds (that diluted
    reachability and, with caps, could truncate real config entries).

    When ``production_only`` is True, test-file seeds are excluded so reachability
    means "reachable from production code".

    Never raises. Returns a sorted list of repo-relative posix paths.
    """
    roots, _ = entry_roots_with_report(root, files, production_only=production_only)
    return roots


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
    # Bundled non-Python assets are often referenced by basename only.
    name = Path(norm).name
    if Path(norm).suffix.lower() in {
        ".mjs",
        ".cjs",
        ".js",
        ".css",
        ".svg",
        ".png",
        ".jpg",
        ".jpeg",
        ".webp",
        ".html",
    }:
        tokens.add(name)
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
    """Return normalized dynamic-import / ``getattr`` / launcher keys for one file."""
    norm = _norm(path)
    suffix = Path(norm).suffix.lower()
    launcher = is_launcher_file(norm)
    if suffix not in _CODE_CONFIG_SUFFIXES and not launcher:
        return set()
    extra_keys: Set[str] = set()
    if launcher:
        extra_keys |= launcher_reference_keys(norm, source)
    if Path(norm).name == "package.json":
        extra_keys |= _package_json_script_keys(norm, source)
    if suffix not in _CODE_CONFIG_SUFFIXES:
        return extra_keys
    specs: List[str] = [match.group(1) for match in _IMPORTLIB_RE.finditer(source)]
    for match in _DYNAMIC_IMPORT_RE.finditer(source):
        spec = match.group(1)
        if not spec:
            continue
        if spec.startswith("."):
            # Relative dynamic import — resolve against this file so
            # ``import('./App')`` clears ``App.tsx`` via reference_cleared.
            try:
                joined = (Path(norm).parent / spec).as_posix()
            except Exception:
                continue
            resolved = _normalize_rel_path(joined)
            hit_stem = resolved
            for ext in _JS_RESOLVE_EXTS:
                if resolved.endswith(ext):
                    hit_stem = resolved[: -len(ext)]
                    break
            specs.append(resolved)
            specs.append(hit_stem)
        else:
            specs.append(spec)
    for match in _WORKER_URL_RE.finditer(source):
        spec = match.group(1)
        if not spec:
            continue
        if spec.startswith("."):
            try:
                joined = (Path(norm).parent / spec).as_posix()
            except Exception:
                continue
            resolved = _normalize_rel_path(joined)
            hit_stem = resolved
            for ext in _JS_RESOLVE_EXTS:
                if resolved.endswith(ext):
                    hit_stem = resolved[: -len(ext)]
                    break
            specs.append(resolved)
            specs.append(hit_stem)
        else:
            specs.append(spec)
    for match in _PYTHON_DASH_M_RE.finditer(source):
        spec = match.group(1) or match.group(2)
        if spec:
            specs.append(spec)
    specs.extend(_PACKAGE_RESOURCES_RE.findall(source))
    specs.extend(_BUNDLED_ASSET_RE.findall(source))
    if suffix == ".toml":
        specs.extend(
            (Path(norm).parent / match.group(1)).with_suffix("").as_posix()
            for match in _HATCH_CUSTOM_HOOK_RE.finditer(source)
        )
    keys = {form for spec in specs for form in _module_forms(spec)}
    keys |= extra_keys
    for name_re in (_GETATTR_NAME_RE, _GLOBALS_NAME_RE, _HASATTR_NAME_RE):
        keys.update(
            f"{GETATTR_INDEX_PREFIX}{match.group(1)}"
            for match in name_re.finditer(source)
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
            if (
                path.suffix.lower() not in _CODE_CONFIG_SUFFIXES
                and not is_launcher_file(norm)
            ):
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
            if (
                path.suffix.lower() not in _CODE_CONFIG_SUFFIXES
                and not is_launcher_file(norm)
            ):
                continue
            try:
                text = path.read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            # Same key extraction as build_dynamic_import_index — one canonical
            # owner (dynamic_import_keys) so the prebuilt-index path and this
            # targeted fallback cannot disagree on what counts as a reference.
            if dynamic_import_keys(norm, text) & token_forms:
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
