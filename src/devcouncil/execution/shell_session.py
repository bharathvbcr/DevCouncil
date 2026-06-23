"""Guarded shell command session for DevCouncil tasks."""

from __future__ import annotations

import os
import shlex
import shutil
import subprocess
import uuid
from pathlib import Path

from rich.console import Console

from devcouncil.domain.evidence import CommandResult
from devcouncil.domain.task import Task
from devcouncil.execution.checkpoints import CheckpointService
from devcouncil.execution.policy_engine import TaskPolicyEngine
from devcouncil.storage.db import get_db
from devcouncil.storage.native import ShellCommandRepository, ShellSessionRepository, TaskLeaseRepository
from devcouncil.storage.repositories import EvidenceRepository, TaskRepository
from devcouncil.telemetry.traces import TraceLogger


class ShellBackend:
    def run_command(self, command: str, cwd: Path, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
        raise NotImplementedError


class CommandLoopBackend(ShellBackend):
    def run_command(self, command: str, cwd: Path, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
        args = shlex.split(command, posix=(os.name != "nt"))
        return subprocess.run(
            args,
            cwd=cwd,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            env=env,
        )


class ShellWrappedBackend(ShellBackend):
    """Runs each command through an explicit shell (pwsh/bash/zsh/...)."""

    _LAUNCHERS = {
        "pwsh": ["pwsh", "-NoProfile", "-Command"],
        "powershell": ["powershell", "-NoProfile", "-Command"],
        "bash": ["bash", "-lc"],
        "zsh": ["zsh", "-lc"],
        "sh": ["sh", "-lc"],
    }

    def __init__(self, shell: str):
        launcher = self._LAUNCHERS.get(shell)
        if launcher is None:
            supported = ", ".join(sorted(self._LAUNCHERS))
            raise ValueError(f"Unknown shell backend '{shell}'. Use auto or one of: {supported}.")
        if not shutil.which(launcher[0]):
            raise ValueError(f"Shell '{launcher[0]}' is not installed or not on PATH.")
        self.launcher = launcher

    def run_command(self, command: str, cwd: Path, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [*self.launcher, command],
            cwd=cwd,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            env=env,
        )


_EVIDENCE_COMMAND_HINTS = ("pytest", "ruff", "mypy", "npm test", "npm run lint", "npm run typecheck")

console = Console()


class GuardedShellSession:
    def __init__(self, project_root: Path, task: Task, *, shell: str = "auto"):
        self.project_root = project_root.resolve()
        self.task = task
        self.shell = shell
        self.policy = TaskPolicyEngine(self.project_root)
        self.backend: ShellBackend = (
            CommandLoopBackend() if shell in {"auto", "loop"} else ShellWrappedBackend(shell)
        )
        self.lease_token: str | None = None
        self.session_id: str | None = None
        self.log_dir = self.project_root / ".devcouncil" / "logs"
        self.log_dir.mkdir(parents=True, exist_ok=True)

    def start(self, *, force: bool = False) -> None:
        db = get_db(self.project_root)
        if not db:
            raise RuntimeError("DevCouncil not initialized.")
        with db.get_session() as session:
            lease = TaskLeaseRepository(session).acquire(
                self.task.id,
                owner="dev shell",
                agent=self.shell,
                force=force,
            )
            self.lease_token = lease.lease_token
            shell_session = ShellSessionRepository(session).start(
                self.task.id,
                self.shell,
                str(self.project_root),
                lease_id=lease.id,
            )
            self.session_id = shell_session.id
            self.task.status = "running"
            TaskRepository(session).save(self.task)
        CheckpointService(self.project_root).create_before(self.task.id)

    def finish(self) -> None:
        db = get_db(self.project_root)
        if not db or not self.lease_token:
            return
        with db.get_session() as session:
            if self.session_id:
                ShellSessionRepository(session).finish(self.session_id, "finished")
            TaskLeaseRepository(session).release(self.task.id, self.lease_token)

    def run_one(self, command: str) -> int:
        normalized = " ".join(command.split())
        decision = self.policy.evaluate_command(normalized, self.task)
        log_id = uuid.uuid4().hex[:8]
        stdout_path = self.log_dir / f"{self.task.id}-{log_id}.stdout.log"
        stderr_path = self.log_dir / f"{self.task.id}-{log_id}.stderr.log"

        if decision.action == "deny":
            self._record_command(normalized, "denied", reason=decision.reason)
            TraceLogger(self.project_root).log_event(
                "shell_command_denied",
                {"command": normalized, "reason": decision.reason},
                task_id=self.task.id,
            )
            # Tell the user *why* — a silent non-zero exit is unactionable.
            console.print(
                f"[red]Command denied for {self.task.id}:[/red] {decision.reason or 'not permitted by task policy.'}"
            )
            console.print(
                "[dim]Add it to the task's allowed_commands, or run it outside DevCouncil.[/dim]"
            )
            return 1

        try:
            result = self.backend.run_command(normalized, self.project_root)
        except (NotImplementedError, FileNotFoundError, OSError) as exc:
            self._record_command(normalized, "denied", reason=str(exc))
            console.print(f"[red]Could not run '{normalized}':[/red] {exc}")
            return 1

        stdout_path.write_text(result.stdout or "", encoding="utf-8")
        stderr_path.write_text(result.stderr or "", encoding="utf-8")
        # Echo the command output so the guarded shell is actually usable.
        if result.stdout:
            console.print(result.stdout, end="", markup=False, highlight=False)
        if result.stderr:
            console.print(result.stderr, end="", markup=False, highlight=False, style="dim")
        status = "finished" if result.returncode == 0 else "failed"
        self._record_command(
            normalized,
            status,
            exit_code=result.returncode,
            stdout_path=str(stdout_path),
            stderr_path=str(stderr_path),
        )
        if any(hint in normalized for hint in _EVIDENCE_COMMAND_HINTS):
            self._save_command_evidence(normalized, result.returncode, result.stdout or result.stderr or "")
        TraceLogger(self.project_root).log_event(
            "shell_command_finished",
            {"command": normalized, "exit_code": result.returncode},
            task_id=self.task.id,
        )
        return result.returncode

    def _record_command(
        self,
        command: str,
        status: str,
        *,
        exit_code: int | None = None,
        reason: str = "",
        stdout_path: str = "",
        stderr_path: str = "",
    ) -> None:
        db = get_db(self.project_root)
        if not db:
            return
        with db.get_session() as session:
            ShellCommandRepository(session).record(
                self.task.id,
                command,
                status,
                session_id=self.session_id,
                exit_code=exit_code,
                reason=reason,
                stdout_path=stdout_path,
                stderr_path=stderr_path,
            )

    def _save_command_evidence(self, command: str, exit_code: int, summary: str) -> None:
        db = get_db(self.project_root)
        if not db:
            return
        with db.get_session() as session:
            EvidenceRepository(session).save_command_result(
                self.task.id,
                CommandResult(
                    command=command,
                    exit_code=exit_code,
                    stdout_path="",
                    stderr_path="",
                    summary=summary[:500],
                ),
            )
