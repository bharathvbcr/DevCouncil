"""Shared subprocess helpers with consistent timeouts and error handling.

Every git/tool invocation in DevCouncil should go through these helpers (or at
minimum pass an explicit ``timeout=``) so a hung child process can never hang a
verification run or the CLI. Defaults are deliberately generous: they exist to
convert "hangs forever" into "fails loudly", not to race fast commands.
"""

from __future__ import annotations

import logging
import subprocess
from pathlib import Path
from typing import List, Optional, Sequence, Union

logger = logging.getLogger(__name__)

# Default ceiling for git plumbing commands (status/diff/ls-files/rev-parse).
GIT_TIMEOUT: float = 60.0
# Default ceiling for generic tool invocations when the caller has no better bound.
DEFAULT_TIMEOUT: float = 120.0

Cmd = Sequence[str]
PathLike = Union[str, Path]


def run_git(
    args: Cmd,
    cwd: PathLike,
    *,
    timeout: float = GIT_TIMEOUT,
    check: bool = False,
) -> subprocess.CompletedProcess[str]:
    """Run a git command with a timeout, capturing decoded text output.

    Never raises ``TimeoutExpired``: a timeout is logged and surfaced as a
    ``CompletedProcess`` with returncode 124 so existing ``returncode``/stdout
    checks keep working. Raises ``CalledProcessError`` only when ``check=True``
    and the command failed (mirroring ``subprocess.run``).
    """
    cmd: List[str] = ["git", *args] if args and args[0] != "git" else list(args)
    try:
        return subprocess.run(
            cmd,
            cwd=str(cwd),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
            check=check,
        )
    except subprocess.TimeoutExpired:
        logger.warning("git command timed out after %.0fs: %s (cwd=%s)", timeout, " ".join(cmd), cwd)
        return subprocess.CompletedProcess(cmd, returncode=124, stdout="", stderr=f"timed out after {timeout}s")


def git_repo_state(cwd: PathLike, *, timeout: float = GIT_TIMEOUT) -> tuple[Optional[bool], str]:
    """Is *cwd* inside a git work tree? ``(True|False|None, reason)``.

    Tri-state on purpose. ``git rev-parse --is-inside-work-tree`` has three
    distinct outcomes and a boolean can only carry two:

    * ``(True, "")``  — git answered ``true``.
    * ``(False, why)`` — git answered, and the answer is no (exit 128, or
      ``false`` inside a bare repository's ``.git`` directory).
    * ``(None, why)`` — the probe could not run: it timed out (``run_git``
      surfaces that as returncode 124), git is not installed, or the directory
      could not be entered. Nothing was determined.

    Collapsing the third case into ``False`` is what made a 60-second timeout
    indistinguishable from an absent repository, so a caller was told "this is
    not a git repository" about a repository that is one. A check that could not
    run must never report what a check that ran and answered reports.
    """
    try:
        result = run_git(["rev-parse", "--is-inside-work-tree"], cwd=cwd, timeout=timeout)
    except OSError as exc:
        return None, f"could not run git: {exc}"
    if result.returncode == 124:
        return None, (result.stderr or f"git rev-parse timed out after {timeout}s").strip()
    stdout = (result.stdout or "").strip()
    if result.returncode == 0:
        if stdout == "true":
            return True, ""
        return False, f"git reports the work tree as {stdout or 'unset'}"
    detail = (result.stderr or result.stdout or "").strip()
    if "not a git repository" in detail.lower():
        return False, detail
    # A non-zero exit that is not the "no repository here" message is a git
    # failure, not an answer: reporting it as "not a repository" would be the
    # same conflation one line up.
    return None, detail or f"git rev-parse exited {result.returncode}"


def git_output(
    args: Cmd,
    cwd: PathLike,
    *,
    timeout: float = GIT_TIMEOUT,
    default: Optional[str] = None,
) -> str:
    """Return stdout of a git command, or ``default`` on any failure.

    When ``default`` is None a failure raises ``CalledProcessError`` /
    ``TimeoutExpired`` for the caller's existing except paths; otherwise the
    failure is logged at debug level and ``default`` is returned.
    """
    cmd: List[str] = ["git", *args] if args and args[0] != "git" else list(args)
    try:
        result = subprocess.run(
            cmd,
            cwd=str(cwd),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
        )
        if result.returncode != 0:
            raise subprocess.CalledProcessError(result.returncode, cmd, result.stdout, result.stderr)
        return result.stdout
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired, OSError) as e:
        if default is not None:
            logger.debug("git command failed (%s): %s (cwd=%s)", e, " ".join(cmd), cwd)
            return default
        raise
