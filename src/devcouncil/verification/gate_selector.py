"""Map changed paths to the subset of verification gates/commands that need to run.

The full verify path (release / ``dev go``) always runs every configured gate. But
the iterative sidecar (``dev check --watch``) re-runs on every save, and re-running the
*whole* gate suite on a one-line edit is what makes it feel sluggish. This selector
narrows the work two ways:

1. **Kind relevance** — a gate keyed to a language stack (``pytest``/``mypy`` → Python,
   ``eslint``/``tsc`` → JS/TS) is skipped entirely when no changed file belongs to that
   stack. A docs-only edit runs no code gate at all.
2. **Scope narrowing** — a linter/type-checker invoked over a broad target (``.``,
   ``src``, ``src tests``) is rewritten to target only the touched files of its stack
   ("lint only touched packages"), so the tool parses a handful of files instead of the
   tree. Commands with explicit paths, shell operators, or non-narrowable tools are left
   untouched.

Each :class:`GateSpec` also carries the ``inputs`` (the changed files that determine its
result) so the content-hash cache (:mod:`devcouncil.verification.gate_cache`) can skip a
gate whose inputs are byte-for-byte unchanged since it last passed.

Pure/​deterministic and dependency-free by design: it takes changed paths + a command
map and returns specs. Nothing here shells out — running the gates stays the caller's job.
"""

from __future__ import annotations

import shlex
from dataclasses import dataclass, field
from pathlib import Path
from typing import Mapping, Sequence

from devcouncil.indexing.subsystem_map import dependents_of

# Extensions we treat as "code" for the purpose of deciding whether any code gate is
# relevant at all. Docs/config-only diffs skip the code gates.
_CODE_EXTS = frozenset({
    ".py", ".pyi", ".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs",
    ".go", ".rs", ".java", ".kt", ".rb", ".cs", ".cpp", ".cc", ".c", ".h", ".hpp",
    ".swift", ".scala", ".php",
})

_PYTHON_EXTS = frozenset({".py", ".pyi"})
_JS_TS_EXTS = frozenset({".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs"})

# Project config paths that affect tool behaviour. A config-only diff still selects the
# gates for the matching stack so ``dev check --watch`` re-runs lint/typecheck after
# rule changes instead of falling back to the evidence gate.
_CONFIG_PATHS = frozenset({
    "pyproject.toml", "ruff.toml", ".ruff.toml", "mypy.ini", "setup.cfg",
    "pyrightconfig.json", "tsconfig.json", "tsconfig.base.json",
    "package.json", "package-lock.json", "uv.lock", "poetry.lock", "Pipfile",
    ".python-version",
})

# Config basename → stack it belongs to. Names in ``_CONFIG_PATHS`` but absent here apply
# to every stack (e.g. lockfiles that can affect multiple toolchains).
_CONFIG_STACK: dict[str, str] = {
    "pyproject.toml": "python",
    "ruff.toml": "python",
    ".ruff.toml": "python",
    "mypy.ini": "python",
    "setup.cfg": "python",
    "pyrightconfig.json": "python",
    "Pipfile": "python",
    ".python-version": "python",
    "uv.lock": "python",
    "poetry.lock": "python",
    "tsconfig.json": "js_ts",
    "tsconfig.base.json": "js_ts",
    "package-lock.json": "js_ts",
}

# Tool → the stack it belongs to. A command whose resolved tool is here is only
# relevant when a changed file matches that stack. Tools not listed are treated as
# stack-agnostic (always relevant; inputs = every changed file).
_TOOL_STACK: dict[str, str] = {
    # Python
    "pytest": "python", "mypy": "python", "pyright": "python", "ruff": "python",
    "black": "python", "flake8": "python", "isort": "python", "pylint": "python",
    "pyflakes": "python", "pycodestyle": "python", "autopep8": "python", "yapf": "python",
    "bandit": "python", "unittest": "python",
    # JS / TS
    "eslint": "js_ts", "tsc": "js_ts", "prettier": "js_ts", "jest": "js_ts",
    "vitest": "js_ts", "biome": "js_ts", "standard": "js_ts", "stylelint": "js_ts",
}

_STACK_EXTS: dict[str, frozenset[str]] = {
    "python": _PYTHON_EXTS,
    "js_ts": _JS_TS_EXTS,
}

# Tools whose positional path arguments can be safely narrowed to specific files
# without changing their meaning. ``tsc`` is intentionally excluded: passing files to
# ``tsc`` disables the project's tsconfig, silently changing the check.
_NARROWABLE_TOOLS = frozenset({
    "ruff", "black", "flake8", "isort", "pylint", "pyflakes", "pycodestyle",
    "autopep8", "yapf", "bandit", "mypy", "pyright", "eslint", "prettier", "stylelint",
})

# Broad positional targets a narrowable linter is commonly pointed at. When a command's
# only path args are drawn from this set, they are replaced with the touched files.
_BROAD_TARGETS = frozenset({".", "./", "src", "src/", "tests", "tests/", "test", "test/", "lib", "lib/", "app", "app/"})

# Command-runner wrappers to peel off before resolving the underlying tool.
_WRAPPERS = frozenset({"npx", "poetry", "uv", "pdm", "hatch", "rye", "pipenv"})

# Subcommand tokens that are part of the invocation, not a path argument, for tools
# that take one (``ruff check .`` → ``check`` is the subcommand). Kept verbatim when a
# command is narrowed so the meaning is preserved.
_TOOL_SUBCOMMANDS: dict[str, frozenset[str]] = {
    "ruff": frozenset({"check", "format"}),
}


@dataclass(frozen=True)
class GateSpec:
    """One unit of verification work selected for the current changed-file set.

    ``name`` is a stable identifier used as the cache key. ``kind`` is the gate family
    (``lint``/``typecheck``/``test``). ``command`` is the (possibly narrowed) shell
    command to run. ``inputs`` are the changed files whose content determines the
    result — the content-hash cache keys on them plus the command string.
    """

    name: str
    kind: str
    command: str
    inputs: tuple[str, ...] = ()
    stack: str | None = None
    narrowed: bool = False
    reason: str = ""


@dataclass
class GateSelection:
    """Result of :func:`select_gates`: the gates to run and the ones skipped (with why)."""

    gates: list[GateSpec] = field(default_factory=list)
    skipped: list[tuple[str, str]] = field(default_factory=list)  # (command, reason)

    @property
    def commands(self) -> list[str]:
        return [g.command for g in self.gates]


def _norm(path: str) -> str:
    return path.replace("\\", "/")


def _ext(path: str) -> str:
    return Path(path).suffix.lower()


def _resolve_tool(tokens: Sequence[str]) -> tuple[str | None, int]:
    """Return (tool_name, index_of_first_positional_after_tool) for a command's tokens.

    Peels ``python -m``, ``npm run``, and runner wrappers (``poetry run`` etc.) so the
    stack can be inferred from the real tool. ``tool`` is lower-cased and stripped of
    any path prefix / version pin. Index points just past the tool token.
    """
    i = 0
    n = len(tokens)
    while i < n:
        tok = tokens[i]
        if tok in {"python", "python3", "py"} and i + 1 < n and tokens[i + 1] == "-m":
            i += 2
            continue
        if tok in {"npm", "pnpm", "yarn"}:
            # `npm run lint` / `npm test` — the script name is not the tool; treat as
            # agnostic-but-named by the manager. Return the manager so callers don't
            # try to narrow it.
            return tok, n
        if tok in _WRAPPERS:
            i += 1
            if i < n and tokens[i] == "run":
                i += 1
            continue
        break
    if i >= n:
        return None, n
    raw = tokens[i]
    tool = raw.replace("\\", "/").split("/")[-1].split("==")[0].lower()
    return tool, i + 1


def _stack_for_tool(tool: str | None) -> str | None:
    if tool is None:
        return None
    return _TOOL_STACK.get(tool)


def _is_config_path(path: str) -> bool:
    return Path(path).name in _CONFIG_PATHS


def _config_paths_for_stack(changed: Sequence[str], stack: str | None) -> list[str]:
    """Config files in ``changed`` that apply to ``stack`` (or any stack when unknown)."""
    result: list[str] = []
    for path in changed:
        if not _is_config_path(path):
            continue
        name = Path(path).name
        cfg_stack = _CONFIG_STACK.get(name)
        if stack is None or cfg_stack is None or cfg_stack == stack:
            result.append(path)
    return result


def _files_for_stack(changed: Sequence[str], stack: str | None) -> list[str]:
    if stack is None:
        return list(changed)
    exts = _STACK_EXTS.get(stack, frozenset())
    return [p for p in changed if _ext(p) in exts]


def _relevant_files(changed: Sequence[str], stack: str | None) -> list[str]:
    """Changed files that make a stack gate worth running (code and/or config)."""
    code = _files_for_stack(changed, stack)
    config = _config_paths_for_stack(changed, stack)
    if code:
        return sorted(set(code) | set(config))
    return sorted(config)


def _expand_narrow_targets(
    code_files: Sequence[str],
    stack: str | None,
    repo_map: Mapping | None,
) -> list[str]:
    """Add direct import dependents so narrowed type-checkers catch cross-file errors."""
    if not code_files or not repo_map:
        return list(code_files)
    exts = _STACK_EXTS.get(stack or "", frozenset())
    expanded = set(code_files)
    for path in code_files:
        for dependent in dependents_of(path, repo_map):
            if _ext(dependent) in exts:
                expanded.add(_norm(dependent))
    return sorted(expanded)


def _narrow_command(command: str, tool: str, tool_idx: int, targets: Sequence[str]) -> str | None:
    """Rewrite ``command`` so its broad positional path args become ``targets``.

    Returns the narrowed command string, or ``None`` when narrowing is unsafe (shell
    operators present, no broad target found, or the command already names specific
    paths). Flags/options are preserved; only bare broad targets are replaced.
    """
    if any(op in command for op in ("&&", "||", "|", ";", ">", "<", "$(", "`")):
        return None
    try:
        tokens = shlex.split(command)
    except ValueError:
        return None
    head = tokens[:tool_idx]
    rest = tokens[tool_idx:]
    subcommands = _TOOL_SUBCOMMANDS.get(tool, frozenset())
    # ``prefix`` keeps flags and the tool's subcommand verbatim; ``positionals`` are the
    # path-like args we consider replacing.
    prefix: list[str] = []
    positionals: list[str] = []
    for tok in rest:
        if tok.startswith("-"):
            prefix.append(tok)
        elif not positionals and tok in subcommands:
            prefix.append(tok)
        else:
            positionals.append(tok)
    # Only narrow when every positional is a broad target. If the command already names
    # a specific file/dir (or a flag value we don't understand), respect it — the author
    # scoped it deliberately.
    if not positionals or any(p not in _BROAD_TARGETS for p in positionals):
        return None
    narrowed = head + prefix + list(targets)
    return " ".join(shlex.quote(t) if (" " in t) else t for t in narrowed)


def select_gates(
    changed_files: Sequence[str],
    commands: Mapping[str, Sequence[str]],
    *,
    narrow: bool = True,
    repo_map: Mapping | None = None,
) -> GateSelection:
    """Select the gates to run for ``changed_files`` from a ``kind -> [command]`` map.

    ``commands`` keys are gate kinds (``test``/``lint``/``typecheck``); each maps to the
    configured command strings. A command is dropped when it targets a stack none of the
    changed files belong to, and narrowed to the touched files when it is a broad-target
    linter/type-checker and ``narrow`` is set. Order of kinds and commands is preserved.
    """
    changed = [_norm(p) for p in changed_files if p and p.strip()]
    selection = GateSelection()
    if not changed:
        return selection

    seen_names: set[str] = set()
    for kind in ("lint", "typecheck", "test"):
        for command in commands.get(kind, []) or []:
            command = command.strip()
            if not command:
                continue
            try:
                tokens = shlex.split(command)
            except ValueError:
                tokens = command.split()
            tool, tool_idx = _resolve_tool(tokens)
            stack = _stack_for_tool(tool)
            relevant = _relevant_files(changed, stack) if stack is not None else list(changed)
            if stack is not None and not relevant:
                selection.skipped.append(
                    (command, f"no changed file in the '{stack}' stack")
                )
                continue
            final_command = command
            narrowed = False
            input_files = list(relevant if stack is not None else changed)
            if narrow and tool in _NARROWABLE_TOOLS and stack is not None:
                code_targets = _files_for_stack(relevant, stack)
                narrow_targets = code_targets
                if tool in {"mypy", "pyright"} and code_targets:
                    narrow_targets = _expand_narrow_targets(code_targets, stack, repo_map)
                if narrow_targets:
                    candidate = _narrow_command(command, tool, tool_idx, narrow_targets)
                    if candidate is not None:
                        final_command = candidate
                        narrowed = True
                        config_in_relevant = [p for p in relevant if _is_config_path(p)]
                        input_files = sorted(set(narrow_targets) | set(config_in_relevant))
            inputs = tuple(sorted(input_files))
            name = f"{kind}:{final_command}"
            if name in seen_names:
                continue
            seen_names.add(name)
            selection.gates.append(GateSpec(
                name=name,
                kind=kind,
                command=final_command,
                inputs=inputs,
                stack=stack,
                narrowed=narrowed,
                reason="narrowed to touched files" if narrowed else "",
            ))
    return selection
