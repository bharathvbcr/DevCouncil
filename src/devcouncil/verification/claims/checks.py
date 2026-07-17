"""Independent re-execution of configured commands and git/fs cross-checks.

Commands come from DevCouncil ``config.commands`` (test/lint/typecheck), never
from claim text — except COMMAND_SUCCEEDED matching a configured command.
Timeouts yield UNVERIFIABLE (never block).
"""

from __future__ import annotations

import shlex
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

from devcouncil.verification.claims.models import Assertion, CheckResult, Kind, Status

TAIL_LINES = 50


@dataclass
class ClaimCheckBudget:
    """Timeouts for claim command re-runs."""

    per_command_timeout: int = 90
    total_timeout: int = 120


@dataclass
class CommandOutcome:
    exit_code: int | None
    output_tail: str
    duration: float
    timed_out: bool


def _tail(*chunks: str | bytes | None) -> str:
    parts: list[str] = []
    for chunk in chunks:
        if chunk is None:
            continue
        if isinstance(chunk, bytes):
            chunk = chunk.decode("utf-8", errors="replace")
        if chunk.strip():
            parts.append(chunk)
    combined = "\n".join(parts)
    lines = combined.splitlines()
    return "\n".join(lines[-TAIL_LINES:])


def _argv_to_shell(cmd: list[str] | str) -> str:
    if isinstance(cmd, str):
        return cmd
    return shlex.join(cmd)


def run_command(cmd: str, cwd: Path, timeout: int) -> CommandOutcome:
    try:
        argv = shlex.split(cmd)
    except ValueError:
        argv = []
    if argv and argv[0] in {"python", "python3"} and shutil.which(argv[0]) is None:
        # Configs commonly use the portable `python` spelling even when macOS
        # exposes only the interpreter running DevCouncil. Preserve the rest of
        # the shell command while resolving that launcher deterministically.
        cmd = f"{shlex.quote(sys.executable)}{cmd[len(argv[0]):]}"
    start = time.monotonic()
    try:
        proc = subprocess.run(
            cmd,
            shell=True,
            cwd=str(cwd),
            capture_output=True,
            text=True,
            errors="replace",
            timeout=timeout,
        )
        return CommandOutcome(
            exit_code=proc.returncode,
            output_tail=_tail(proc.stdout, proc.stderr),
            duration=time.monotonic() - start,
            timed_out=False,
        )
    except subprocess.TimeoutExpired as exc:
        return CommandOutcome(
            exit_code=None,
            output_tail=_tail(exc.stdout, exc.stderr),
            duration=time.monotonic() - start,
            timed_out=True,
        )
    except OSError as exc:
        return CommandOutcome(
            exit_code=None,
            output_tail=f"failed to launch command: {exc}",
            duration=time.monotonic() - start,
            timed_out=False,
        )


def _git_output(cwd: Path, *args: str) -> str | None:
    try:
        proc = subprocess.run(
            ["git", *args],
            cwd=str(cwd),
            capture_output=True,
            text=True,
            errors="replace",
            timeout=15,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    return proc.stdout if proc.returncode == 0 else None


def git_toplevel(cwd: Path) -> Path | None:
    out = _git_output(cwd, "rev-parse", "--show-toplevel")
    return Path(out.strip()) if out and out.strip() else None


def last_commit_paths(cwd: Path) -> set[str]:
    out = _git_output(cwd, "log", "-1", "--name-only", "--pretty=format:")
    if not out:
        return set()
    return {line.strip().strip('"') for line in out.splitlines() if line.strip()}


def git_changed_files(cwd: Path) -> dict[str, str] | None:
    try:
        proc = subprocess.run(
            ["git", "status", "--porcelain", "-uall"],
            cwd=str(cwd),
            capture_output=True,
            text=True,
            errors="replace",
            timeout=15,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if proc.returncode != 0:
        return None
    changed: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        if len(line) < 4:
            continue
        status, path = line[:2], line[3:]
        if " -> " in path:
            path = path.split(" -> ", 1)[1]
        changed[path.strip().strip('"')] = status
    return changed


def _normalize(path: str) -> str:
    return path.replace("\\", "/").strip("/").lower()


def _in_changed(rel_path: str, changed: dict[str, str]) -> bool:
    target = _normalize(rel_path)
    return any(_normalize(p) == target for p in changed)


class _Runner:
    def __init__(self, budget: ClaimCheckBudget, cwd: Path):
        self.budget = budget
        self.cwd = cwd
        self.deadline = time.monotonic() + budget.total_timeout
        self.cache: dict[str, CommandOutcome] = {}

    def run(self, cmd: str) -> CommandOutcome | None:
        if cmd in self.cache:
            return self.cache[cmd]
        remaining = self.deadline - time.monotonic()
        if remaining <= 0:
            return None
        outcome = run_command(
            cmd, self.cwd, timeout=int(min(self.budget.per_command_timeout, max(remaining, 1)))
        )
        self.cache[cmd] = outcome
        return outcome


_NOT_FOUND_MARKERS = (
    "is not recognized as an internal or external command",
    "is not recognized as a name of a cmdlet",
    "command not found",
    "commandnotfoundexception",
    "no such file or directory",
    "failed to launch command",
)


def _command_not_found(outcome: CommandOutcome) -> bool:
    if outcome.exit_code in (127, 9009):
        return True
    tail = outcome.output_tail.lower()
    return outcome.exit_code != 0 and any(marker in tail for marker in _NOT_FOUND_MARKERS)


def _command_result(assertion: Assertion, cmd: str | None, label: str, runner: _Runner) -> CheckResult:
    if not cmd:
        return CheckResult(assertion, Status.UNVERIFIABLE, f"no {label} command configured")
    outcome = runner.run(cmd)
    if outcome is None:
        return CheckResult(assertion, Status.SKIPPED, "verification budget exhausted")
    if outcome.timed_out:
        return CheckResult(
            assertion, Status.UNVERIFIABLE, f"`{cmd}` timed out after {outcome.duration:.0f}s"
        )
    if _command_not_found(outcome):
        return CheckResult(
            assertion,
            Status.UNVERIFIABLE,
            f"`{cmd}` could not be launched (command not found) — fix the configured {label} command",
        )
    if outcome.exit_code == 0:
        return CheckResult(assertion, Status.PASS, f"`{cmd}` exited 0 in {outcome.duration:.1f}s")
    return CheckResult(
        assertion,
        Status.FAIL,
        f"`{cmd}` exited with exit code {outcome.exit_code}.\nOutput tail:\n{outcome.output_tail}",
    )


def _resolve_in_repo(target: str, cwd: Path) -> Path | None:
    candidate = Path(target)
    if not candidate.is_absolute():
        candidate = cwd / candidate
    try:
        resolved = candidate.resolve()
        if not resolved.is_relative_to(cwd.resolve()):
            return None
        return resolved
    except OSError:
        return None


@dataclass
class _GitInfo:
    toplevel: Path
    changed: dict[str, str]
    last_commit: set[str]


def _git_info(cwd: Path) -> _GitInfo | None:
    toplevel = git_toplevel(cwd)
    if toplevel is None:
        return None
    changed = git_changed_files(cwd) or {}
    return _GitInfo(toplevel=toplevel, changed=changed, last_commit=last_commit_paths(cwd))


def _find_unique_basename(name: str, cwd: Path, git: _GitInfo | None) -> Path | None:
    if git is None:
        return None
    tracked = _git_output(cwd, "ls-files")
    pool = set(tracked.splitlines() if tracked else []) | set(git.changed) | git.last_commit
    matches = {p for p in pool if p.strip() and _normalize(p).rsplit("/", 1)[-1] == _normalize(name)}
    if len(matches) != 1:
        return None
    return git.toplevel / matches.pop()


def _changed_status(rel_path: str, changed: dict[str, str]) -> str:
    target = _normalize(rel_path)
    for path, status in changed.items():
        if _normalize(path) == target:
            return status.strip() or status
    return "?"


def _file_result(assertion: Assertion, cwd: Path, git: _GitInfo | None) -> CheckResult:
    target = assertion.target or ""
    resolved = _resolve_in_repo(target, cwd)
    if resolved is None:
        return CheckResult(
            assertion, Status.UNVERIFIABLE, f"claimed path `{target}` is outside the working directory"
        )

    if not resolved.exists() and "/" not in target and "\\" not in target:
        alt = _find_unique_basename(target, cwd, git)
        if alt is not None and alt.exists():
            resolved = alt.resolve()

    if not resolved.exists():
        return CheckResult(assertion, Status.FAIL, f"claimed file `{target}` does not exist")

    if git is None:
        return CheckResult(assertion, Status.PASS, f"`{target}` exists")

    try:
        rel = resolved.relative_to(git.toplevel.resolve()).as_posix()
    except ValueError:
        return CheckResult(assertion, Status.PASS, f"`{target}` exists (outside the git repo)")

    if _in_changed(rel, git.changed):
        return CheckResult(
            assertion,
            Status.PASS,
            f"`{target}` exists (git status: {_changed_status(rel, git.changed)})",
        )
    if any(_normalize(p) == _normalize(rel) for p in git.last_commit):
        return CheckResult(
            assertion, Status.PASS, f"`{target}` exists (touched by the most recent commit)"
        )
    return CheckResult(
        assertion,
        Status.UNVERIFIABLE,
        f"`{target}` exists but git shows no recent change to it; cannot confirm it was "
        "created/modified in this session",
    )


@dataclass
class ResolvedCommands:
    test: str | None = None
    build: str | None = None  # typecheck in DevCouncil
    lint: str | None = None


def resolve_commands_from_config(commands_cfg: object) -> ResolvedCommands:
    """Map DevCouncil CommandsConfig lists to shell command strings."""
    def first(attr: str) -> str | None:
        value = getattr(commands_cfg, attr, None) or []
        if isinstance(value, str) and value.strip():
            return value.strip()
        if isinstance(value, list) and value:
            first_cmd = value[0]
            if isinstance(first_cmd, list):
                return _argv_to_shell(first_cmd)
            if isinstance(first_cmd, str) and first_cmd.strip():
                return first_cmd.strip()
        return None

    return ResolvedCommands(test=first("test"), build=first("typecheck"), lint=first("lint"))


def _matching_config_command(target: str, resolved: ResolvedCommands) -> str | None:
    wanted = " ".join(target.split()).lower()
    for cmd in (resolved.test, resolved.build, resolved.lint):
        if cmd and wanted == " ".join(cmd.split()).lower():
            return cmd
    return None


def execute_checks(
    assertions: list[Assertion],
    *,
    cwd: Path,
    commands: ResolvedCommands,
    budget: ClaimCheckBudget | None = None,
) -> list[CheckResult]:
    """Independently verify each assertion against the real environment."""
    runner = _Runner(budget or ClaimCheckBudget(), cwd)
    git = _git_info(cwd)
    results: list[CheckResult] = []

    for assertion in assertions:
        kind = assertion.kind
        if kind is Kind.TESTS_PASS:
            results.append(_command_result(assertion, commands.test, "test", runner))
        elif kind is Kind.BUILD_SUCCEEDS:
            results.append(_command_result(assertion, commands.build, "typecheck/build", runner))
        elif kind is Kind.LINT_CLEAN:
            results.append(_command_result(assertion, commands.lint, "lint", runner))
        elif kind in (Kind.FILE_CREATED, Kind.FILE_UPDATED):
            results.append(_file_result(assertion, cwd, git))
        elif kind is Kind.COMMAND_SUCCEEDED:
            cmd = _matching_config_command(assertion.target or "", commands)
            if cmd is None:
                results.append(
                    CheckResult(
                        assertion,
                        Status.UNVERIFIABLE,
                        f"claimed command `{assertion.target}` does not match any configured "
                        "command; not re-running arbitrary commands",
                    )
                )
            else:
                results.append(_command_result(assertion, cmd, "matched", runner))
        elif kind is Kind.GENERIC_DONE:
            if commands.test:
                results.append(_command_result(assertion, commands.test, "test", runner))
            else:
                results.append(
                    CheckResult(
                        assertion,
                        Status.UNVERIFIABLE,
                        "generic completion claim with no configured checks",
                    )
                )
        else:
            results.append(CheckResult(assertion, Status.UNVERIFIABLE, f"no checker for kind {kind}"))

    return results