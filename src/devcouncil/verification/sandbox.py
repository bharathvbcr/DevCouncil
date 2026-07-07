"""Verification sandbox abstraction."""

from __future__ import annotations

import asyncio
import hashlib
import logging
import platform
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Literal

from pydantic import BaseModel

from devcouncil.app.config import DevCouncilConfig, load_config
from devcouncil.domain.task import Task
from devcouncil.storage.db import get_db
from devcouncil.storage.native import VerificationRunRepository
from devcouncil.verification.verifier import Verifier

logger = logging.getLogger(__name__)


class SandboxResult(BaseModel):
    sandbox: str
    status: Literal["passed", "failed", "unsupported"]
    environment: dict[str, str]
    commands: list[dict]


class VerificationSandbox:
    def run(self, task: Task, commands: list[str], requirements: list) -> SandboxResult:
        raise NotImplementedError


def _command_timeout(config: DevCouncilConfig) -> float:
    """Per-command ceiling for sandboxed verification commands.

    Reuses ``execution.command_timeout`` (same knob the task runner applies to
    the commands it executes) so one config value bounds both surfaces."""
    try:
        return float(config.execution.command_timeout)
    except Exception:
        return 300.0


def _run_sandboxed(argv: list[str], *, cwd: Path | None = None, timeout: float) -> subprocess.CompletedProcess:
    """subprocess.run with capture + timeout; a timeout surfaces as returncode 124
    (consistent with utils.proc.run_git) so callers' exit-code checks fail loudly
    instead of the sandbox hanging verification forever."""
    try:
        return subprocess.run(
            argv,
            cwd=str(cwd) if cwd is not None else None,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        logger.warning("Sandbox command timed out after %.0fs: %s", timeout, " ".join(argv))
        return subprocess.CompletedProcess(argv, returncode=124, stdout="", stderr=f"timed out after {timeout}s")


def _save_run(
    project_root: Path,
    task: Task,
    sandbox: str,
    environment: dict[str, str],
    commands: list[dict],
    status: Literal["passed", "failed", "unsupported"],
) -> None:
    log = logger.info if status == "passed" else logger.warning
    log("Sandbox %s for %s: %s (%d command(s))", sandbox, task.id, status, len(commands))
    db = get_db(project_root)
    if not db:
        return
    with db.get_session() as session:
        VerificationRunRepository(session).save(
            task.id,
            sandbox,
            environment,
            commands,
            status,
        )


class LocalSandbox(VerificationSandbox):
    def __init__(self, project_root: Path):
        self.project_root = project_root

    def run(self, task: Task, commands: list[str], requirements: list) -> SandboxResult:
        gaps, _ = asyncio.run(Verifier(self.project_root).verify_task(task, requirements))
        status: Literal["passed", "failed", "unsupported"] = (
            "failed" if any(g.blocking for g in gaps) else "passed"
        )
        env = _environment_metadata(self.project_root)
        command_results = [{"command": cmd, "status": status} for cmd in commands]
        _save_run(self.project_root, task, "local", env, command_results, status)
        return SandboxResult(sandbox="local", status=status, environment=env, commands=command_results)


class DockerSandbox(VerificationSandbox):
    def __init__(self, project_root: Path, config: DevCouncilConfig):
        self.project_root = project_root
        self.config = config

    def run(self, task: Task, commands: list[str], requirements: list) -> SandboxResult:
        if not shutil_which("docker"):
            result = SandboxResult(
                sandbox="docker",
                status="unsupported",
                environment={},
                commands=[{"reason": "docker not available"}],
            )
            _save_run(self.project_root, task, "docker", result.environment, result.commands, result.status)
            return result
        image = self.config.verification.sandbox.docker_image
        setup = self.config.verification.sandbox.docker_setup_commands
        results: list[dict] = []
        timeout = _command_timeout(self.config)
        for setup_cmd in setup:
            proc = _run_sandboxed(
                ["docker", "run", "--rm", "-v", f"{self.project_root}:/work", "-w", "/work", image, "sh", "-c", setup_cmd],
                timeout=timeout,
            )
            results.append({"command": setup_cmd, "exit_code": proc.returncode})
            if proc.returncode != 0:
                result = SandboxResult(sandbox="docker", status="failed", environment={"image": image}, commands=results)
                _save_run(self.project_root, task, "docker", result.environment, result.commands, result.status)
                return result
        for cmd in commands:
            proc = _run_sandboxed(
                ["docker", "run", "--rm", "-v", f"{self.project_root}:/work", "-w", "/work", image, "sh", "-c", cmd],
                timeout=timeout,
            )
            results.append({"command": cmd, "exit_code": proc.returncode})
            if proc.returncode != 0:
                result = SandboxResult(sandbox="docker", status="failed", environment={"image": image}, commands=results)
                _save_run(self.project_root, task, "docker", result.environment, result.commands, result.status)
                return result
        result = SandboxResult(sandbox="docker", status="passed", environment={"image": image}, commands=results)
        _save_run(self.project_root, task, "docker", result.environment, result.commands, result.status)
        return result


class NixSandbox(VerificationSandbox):
    def __init__(self, project_root: Path, config: DevCouncilConfig):
        self.project_root = project_root
        self.config = config

    def run(self, task: Task, commands: list[str], requirements: list) -> SandboxResult:
        if not (self.project_root / "flake.nix").exists() or not shutil_which("nix"):
            result = SandboxResult(
                sandbox="nix",
                status="unsupported",
                environment={},
                commands=[{"reason": "nix or flake.nix unavailable"}],
            )
            _save_run(self.project_root, task, "nix", result.environment, result.commands, result.status)
            return result
        attr = self.config.verification.sandbox.nix_flake_attr or "devShells.default"
        results: list[dict] = []
        timeout = _command_timeout(self.config)
        for cmd in commands:
            proc = _run_sandboxed(
                ["nix", "develop", f".#{attr}", "-c", "sh", "-c", cmd],
                cwd=self.project_root,
                timeout=timeout,
            )
            results.append({"command": cmd, "exit_code": proc.returncode})
            if proc.returncode != 0:
                result = SandboxResult(sandbox="nix", status="failed", environment={"attr": attr}, commands=results)
                _save_run(self.project_root, task, "nix", result.environment, result.commands, result.status)
                return result
        result = SandboxResult(sandbox="nix", status="passed", environment={"attr": attr}, commands=results)
        _save_run(self.project_root, task, "nix", result.environment, result.commands, result.status)
        return result


def shutil_which(name: str) -> str | None:
    return shutil.which(name)


def _environment_metadata(project_root: Path) -> dict[str, str]:
    env = {
        "python": sys.version.split()[0],
        "platform": platform.platform(),
    }
    try:
        uv = subprocess.check_output(["uv", "--version"], text=True, timeout=10).strip()
        env["uv"] = uv
    except (FileNotFoundError, subprocess.CalledProcessError, subprocess.TimeoutExpired, OSError):
        pass
    lock = project_root / "uv.lock"
    if lock.exists():
        env["uv_lock_hash"] = hashlib.sha256(lock.read_bytes()).hexdigest()[:16]
    return env


def get_sandbox(name: str, project_root: Path) -> VerificationSandbox:
    config = load_config(project_root)
    if name == "docker":
        return DockerSandbox(project_root, config)
    if name == "nix":
        return NixSandbox(project_root, config)
    return LocalSandbox(project_root)
