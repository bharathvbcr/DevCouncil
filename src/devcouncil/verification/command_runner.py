"""Run verification evidence commands with sanitized env and logging.

Extracted from ``verifier.py`` so orchestration stays focused on gate logic.
"""

from __future__ import annotations

import hashlib
import logging
import shlex
import shutil
import subprocess
from pathlib import Path
from typing import List

from devcouncil.domain.evidence import CommandResult
from devcouncil.utils.redaction import redact_string
from devcouncil.utils.subprocess_env import clean_subprocess_env

logger = logging.getLogger(__name__)


def split_command(command: str) -> List[str]:
    # Use POSIX splitting so quotes are interpreted, not preserved. With
    # posix=False, `python -c "assert x"` keeps the surrounding quotes, so the
    # interpreter receives the literal string `"assert x"` and treats it as a
    # no-op string expression that exits 0 — every quoted-argument evidence
    # command would then silently "pass" without running, producing false
    # verification. posix=True strips the quotes correctly; planner-generated
    # commands use forward-slash paths, which the interpreter accepts on Windows.
    return shlex.split(command, posix=True)


def summarize_stream(content: str, budget: int = 360) -> str:
    """Condense a command's stdout/stderr for the evidence summary."""
    if not content or not content.strip():
        return "(empty)"
    lines = [ln.rstrip() for ln in content.splitlines() if ln.strip()]
    markers = ("error", "exception", "assert", "traceback", "failed", "not found", "no module named")
    salient = ""
    for ln in reversed(lines):
        low = ln.lower()
        if any(m in low for m in markers):
            salient = ln.strip()
            break
    if not salient:
        salient = lines[-1].strip()
    salient = salient[:240]
    tail = content.strip()[-budget:]
    summary = f"{salient} | {tail}" if salient not in tail[: len(salient) + 5] else tail
    return summary[: budget + len(salient) + 8]


def save_command_log(
    project_root: Path,
    label: str,
    command: str,
    stream: str,
    content: str,
) -> str:
    """Save command output to a log file and return the path."""
    log_dir = project_root / ".devcouncil" / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)
    cmd_hash = hashlib.sha256(command.encode()).hexdigest()[:8]
    filename = f"{label}-{cmd_hash}-{stream}.log"
    log_path = log_dir / filename
    log_path.write_text(redact_string(content), encoding="utf-8")
    return str(log_path)


def run_verification_command(
    project_root: Path,
    command: str,
    *,
    task_id: str = "verify",
    timeout: int = 300,
) -> CommandResult:
    """Execute one evidence command in the target repo with a cleaned environment."""
    env = clean_subprocess_env()
    try:
        argv = split_command(command)
    except ValueError as e:
        return CommandResult(
            command=command,
            exit_code=-1,
            stdout_path="",
            stderr_path="",
            summary=f"Failed to run command: unparseable shell syntax ({e})",
        )
    if argv:
        resolved = shutil.which(argv[0], path=env.get("PATH"))
        if resolved:
            argv = [resolved, *argv[1:]]

    try:
        result = subprocess.run(
            argv,
            shell=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            cwd=project_root,
            timeout=timeout,
            env=env,
        )
        stdout = result.stdout or ""
        stderr = result.stderr or ""
        stdout_path = save_command_log(project_root, task_id, command, "stdout", stdout)
        stderr_path = save_command_log(project_root, task_id, command, "stderr", stderr)
        stdout_summary = redact_string(summarize_stream(stdout))
        stderr_summary = redact_string(summarize_stream(stderr))
        return CommandResult(
            command=command,
            exit_code=result.returncode,
            stdout_path=stdout_path,
            stderr_path=stderr_path,
            summary=(
                f"Exit code {result.returncode}. "
                f"stderr: {stderr_summary}. "
                f"stdout: {stdout_summary}"
            ),
        )
    except Exception as e:
        logger.debug("Command run failed for %r: %s", command, e)
        return CommandResult(
            command=command,
            exit_code=-1,
            stdout_path="",
            stderr_path="",
            summary=f"Failed to run command: {e}",
        )
