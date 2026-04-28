import shutil
import subprocess
import os
from pathlib import Path

from rich.console import Console

from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.execution.executor import Executor, ExecutionResult
from devcouncil.execution.prompt_builder import PromptBuilder

console = Console()


class CodingCliExecutor(Executor):
    """Execute a DevCouncil task by handing it off to an external coding CLI."""

    _ALIASES = {
        "codex-cli": "codex",
        "gemini-cli": "gemini",
        "claude-cli": "claude",
        "claude-code": "claude",
    }

    def __init__(self, project_root: Path, client: str, timeout_seconds: int = 1800):
        self.client = self._normalize_client(client)
        self.project_root = project_root
        self.timeout_seconds = timeout_seconds

    def _normalize_client(self, client: str) -> str:
        normalized = (client or "").strip().lower().replace("_", "-")
        return self._ALIASES.get(normalized, normalized)

    def _command(self) -> list[str]:
        if self.client == "codex":
            return ["codex", "exec", "-"]
        if self.client == "gemini":
            return ["gemini"]
        if self.client == "claude":
            return ["claude", "-p"]
        raise ValueError(f"Unsupported coding CLI client: {self.client}")

    def run_task(self, task: Task, requirements: list[Requirement]) -> ExecutionResult:
        try:
            command = self._command()
        except ValueError as exc:
            return ExecutionResult(success=False, message=str(exc))

        executable = command[0]
        if not shutil.which(executable):
            return ExecutionResult(
                success=False,
                message=f"{self.client} CLI is not installed or not on PATH.",
            )

        prompt = PromptBuilder(self.project_root).build_task_prompt(task, requirements)
        instruction_file = self.project_root / ".devcouncil" / f"{task.id}-{self.client}-task.md"
        instruction_file.parent.mkdir(parents=True, exist_ok=True)
        instruction_file.write_text(prompt, encoding="utf-8")

        env = {**dict(os.environ), "DEVCOUNCIL_PROJECT_ROOT": str(self.project_root)}
        log_prefix = f"{task.id}-{self.client}"

        console.print(f"Starting [bold]{self.client.upper()}[/bold] for task [bold]{task.id}[/bold]...")
        console.print(f"Task prompt: [dim]{instruction_file}[/dim]")
        console.print(f"Command: [dim]{' '.join(command)}[/dim]")

        try:
            result = subprocess.run(
                command,
                input=prompt,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                cwd=self.project_root,
                env=env,
                timeout=self.timeout_seconds,
            )
            self._write_log(log_prefix, result)
            if result.returncode != 0:
                stderr_preview = (result.stderr or result.stdout or "").strip().splitlines()[:5]
                detail = stderr_preview[0] if stderr_preview else "No diagnostics were produced."
                return ExecutionResult(
                    success=False,
                    message=f"{self.client} exited with code {result.returncode}: {detail}",
                )
            return ExecutionResult(success=True, message=f"{self.client} execution finished.")
        except subprocess.TimeoutExpired as exc:
            _ = exc
            return ExecutionResult(
                success=False,
                message=f"{self.client} execution timed out after {self.timeout_seconds}s.",
            )
        except Exception as exc:
            return ExecutionResult(success=False, message=str(exc))

    def _write_log(self, task_client: str, result: subprocess.CompletedProcess[str]) -> None:
        log_dir = self.project_root / ".devcouncil" / "logs"
        log_dir.mkdir(parents=True, exist_ok=True)
        log_path = log_dir / f"{task_client}.log"
        log_path.write_text(
            "\n".join([
                f"command_returncode={result.returncode}",
                "=== stdout ===",
                result.stdout or "",
                "=== stderr ===",
                result.stderr or "",
            ]),
            encoding="utf-8",
        )
