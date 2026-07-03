"""Shared helpers for deciding whether a command qualifies as acceptance evidence."""

from __future__ import annotations

import re
import shlex

# Executables (basename of argv[0]) that ARE test/check runners.
_RUNNER_EXECUTABLES = frozenset({
    "pytest", "py.test", "vitest", "jest", "mocha", "ava", "tap",
    "unittest", "tox", "nox", "behave", "rspec", "minitest", "phpunit",
    "mypy", "pyright", "tsc",
})

# (argv0 basename, first subcommand) pairs that are runners.
_RUNNER_SUBCOMMANDS = frozenset({
    ("cargo", "test"), ("go", "test"), ("mvn", "test"), ("gradle", "test"),
    ("gradlew", "test"), ("make", "test"), ("make", "check"), ("ruff", "check"),
    ("swift", "test"), ("dotnet", "test"), ("mix", "test"), ("stack", "test"),
})

# Package managers whose ``test`` / ``run <script>`` invocations count.
_PKG_MANAGERS = frozenset({"npm", "yarn", "pnpm", "bun"})
_SCRIPT_KEYWORDS = ("test", "typecheck", "type-check", "vitest", "jest", "check")

# Interpreters whose inline payloads (-c / -e / --eval) can carry assertions.
_INLINE_INTERPRETERS = frozenset({"python", "python3", "python2", "py", "node", "deno", "bun"})
_INLINE_FLAGS = frozenset({"-c", "-e", "--eval"})
# Assertion-ish content inside an inline payload. ``print(...)`` alone is not
# evidence; an assert / raise / expect() is.
_INLINE_EVIDENCE_RE = re.compile(
    r"(\bassert\b|\braise\s+\w+|\bexpect\s*\(|\.to(?:Be|Equal|Throw)\b|\bthrow\s)",
    re.IGNORECASE,
)

# A single argument token that references a test file/directory. Path-like — a
# quoted sentence containing the word "test" has spaces and cannot match.
_TEST_PATH_RE = re.compile(
    r"(^|/)(tests?(/|$)|test_[^/\s]*$|[^/\s]*(?:_tests?|\.test|\.spec)\.[a-z]+$)",
    re.IGNORECASE,
)


def _split(command: str) -> list[str]:
    text = (command or "").strip()
    if not text:
        return []
    try:
        return shlex.split(text)
    except ValueError:
        return text.split()


def command_has_acceptance_evidence(command: str) -> bool:
    """True when ``command``'s actual INVOCATION can produce behavioral evidence.

    STRICT, structure-aware allowlist — use for UNTRUSTED input (an agent appending
    expected_tests via ``update_task_scope``). The runner must be the executable, a
    subcommand, a package-manager script, an inline interpreter assertion, or a
    test-file path argument — never just a keyword substring anywhere in the line
    (``echo "tests assert ok"`` must not qualify). For trusted planner/config
    commands at verify time use :func:`command_is_trivial_evidence` instead, whose
    deny-list does not punish legitimate behavioral commands that lack a keyword
    (``make check``, ``./run_smoke.sh``, ``python scripts/validate.py``)."""
    tokens = _split(command)
    if not tokens:
        return False
    base = tokens[0].rsplit("/", 1)[-1].lower()
    if base in _TRIVIAL_EXECUTABLES:
        return False
    if base in _RUNNER_EXECUTABLES:
        return True

    args = tokens[1:]
    lowered_args = [a.lower() for a in args]
    first_arg = lowered_args[0] if lowered_args else ""

    # Subcommand runners: `cargo test`, `go test ./...`, `ruff check .`, ...
    if (base, first_arg) in _RUNNER_SUBCOMMANDS:
        return True

    # Package-manager scripts: `npm test`, `npm run typecheck`, `yarn vitest`.
    if base in _PKG_MANAGERS and lowered_args:
        script = lowered_args[1] if first_arg == "run" and len(lowered_args) > 1 else first_arg
        if any(keyword in script for keyword in _SCRIPT_KEYWORDS):
            return True

    # Interpreters: `python -m pytest`, `python -c "assert ..."`, `node -e "expect(...)"`.
    if base in _INLINE_INTERPRETERS:
        for i, arg in enumerate(lowered_args):
            if arg == "-m" and i + 1 < len(lowered_args):
                module = lowered_args[i + 1]
                if module in _RUNNER_EXECUTABLES or module == "unittest":
                    return True
            if arg in _INLINE_FLAGS and i + 1 < len(args) and _INLINE_EVIDENCE_RE.search(args[i + 1]):
                return True

    # A test-file/dir reference as the executable or an argument: `./run_tests.sh`,
    # `python tests/test_x.py`, `bash tests/run.sh`.
    for token in tokens:
        if " " not in token and _TEST_PATH_RE.search(token):
            return True

    return False


# Executables whose success proves nothing about the code under review: pure
# printing, filesystem/system queries, and no-ops. First-token match (after
# common wrappers) so e.g. ``echo done`` or ``date`` can never coarse-prove an
# acceptance criterion, while ``make check`` / ``./run_smoke.sh`` still can.
_TRIVIAL_EXECUTABLES = frozenset({
    "echo", "printf", "true", "false", ":", "exit", "sleep", "ls", "dir", "cat",
    "head", "tail", "wc", "pwd", "cd", "which", "whereis", "whoami", "date",
    "env", "printenv", "type", "touch", "mkdir", "cp", "mv", "hostname", "uname",
})

# Wrapper tokens to skip before resolving the effective executable.
_WRAPPER_TOKENS = frozenset({"sudo", "time", "nice", "nohup", "uv", "poetry", "pdm", "hatch", "rye", "npx"})

# Version/help style flags: a command that only reports its own identity.
_IDENTITY_FLAGS = frozenset({"--version", "-v", "-version", "--help", "-h", "-help", "version", "--about"})


def command_is_trivial_evidence(command: str) -> bool:
    """True when ``command`` cannot possibly prove behavior — deny-list, not allowlist.

    Use at VERIFY time to filter which passing planner/config commands may
    coarse-prove an acceptance criterion. Deliberately permissive: a behavioral
    command without a test keyword (``make check``, ``python scripts/validate.py``,
    ``./run_smoke.sh``) is NOT trivial and keeps its evidential value; only commands
    that provably exercise nothing are rejected:

    * empty commands;
    * pure printing / system queries (``echo``, ``ls``, ``date``, ...);
    * identity checks (``python --version``, ``node --help``);
    * ``git`` state queries (repo state is not behavior);
    * interpreter one-liners that neither import nor assert nor raise
      (``python -c "print('ok')"``).
    """
    lowered = (command or "").strip().lower()
    if not lowered:
        return True
    if command_has_acceptance_evidence(lowered):
        return False

    tokens = lowered.split()
    index = 0
    while index < len(tokens) and tokens[index] in _WRAPPER_TOKENS:
        index += 1
        if index < len(tokens) and tokens[index] == "run":
            index += 1
    if index >= len(tokens):
        return True
    executable = tokens[index].rsplit("/", 1)[-1]

    if executable in _TRIVIAL_EXECUTABLES:
        return True
    # `git status` / `git diff` / `git log` etc.: repository state, not behavior.
    if executable == "git":
        return True
    # Bare identity check: every remaining argument is a version/help flag.
    rest = tokens[index + 1:]
    if rest and all(tok in _IDENTITY_FLAGS for tok in rest):
        return True
    # Interpreter one-liner that neither imports the code, asserts, raises, nor
    # calls anything that could fail meaningfully: `python -c "print('ok')"`.
    if executable.startswith(("python", "node", "ruby", "perl")) and (" -c " in f" {lowered} " or " -e " in f" {lowered} "):
        if not any(marker in lowered for marker in ("import", "require", "assert", "raise", "exit(", "sys.exit")):
            return True
    return False
