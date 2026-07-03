import logging
import subprocess
from pathlib import Path
from rich.console import Console
from devcouncil.domain.task import Task
from devcouncil.domain.requirement import Requirement
from devcouncil.execution.executor import Executor, ExecutionResult
from devcouncil.execution.prompt_builder import PromptBuilder
from devcouncil.telemetry.stages import log_step

console = Console()
logger = logging.getLogger(__name__)

class OpenHandsExecutor(Executor):
    def __init__(self, project_root: Path):
        self.project_root = project_root

    def run_task(self, task: Task, requirements: list[Requirement]) -> ExecutionResult:
        builder = PromptBuilder(self.project_root)
        from devcouncil.planning.correction_manifest import repair_prompt_prefix

        task_prompt = builder.build_task_prompt(task, requirements)
        prefix = repair_prompt_prefix(self.project_root, task.id)
        if prefix:
            task_prompt = f"{prefix}{task_prompt}"
        
        logger.info("OpenHands starting for %s", task.id)
        log_step(
            "executor/openhands: starting task %s" % task.id,
            project_root=self.project_root,
            task_id=task.id,
        )
        console.print(f"Starting [bold]OpenHands[/bold] for task {task.id}...")
        
        # OpenHands often expects a workspace mount and an instruction.
        # Keep the full prompt out of argv so Windows command-line limits and
        # terminal logs do not become part of the execution boundary.
        # Reference: https://github.com/All-Hands-AI/OpenHands
        instruction_file = self.project_root / ".devcouncil" / f"{task.id}-openhands-task.md"
        instruction_file.parent.mkdir(parents=True, exist_ok=True)
        instruction_file.write_text(task_prompt, encoding="utf-8")
        
        cmd = [
            "openhands", "run",
            "--workspace-base", str(self.project_root),
            "--task-file", str(instruction_file),
            "--headless"
        ]
        
        console.print(f"Command: [dim]{' '.join(cmd)}[/dim]")
        console.print("[yellow]Note: OpenHands must be installed and configured in the environment.[/yellow]")
        
        try:
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                cwd=self.project_root,
                timeout=1800,
            )
            self._write_log(task.id, result)
            if result.returncode != 0:
                logger.error("OpenHands exited %s for %s", result.returncode, task.id)
                log_step(
                    "executor/openhands: finished task %s" % task.id,
                    project_root=self.project_root,
                    task_id=task.id,
                    returncode=result.returncode,
                    success=False,
                )
                console.print(f"[red]OpenHands exited with {result.returncode}.[/red]")
                return ExecutionResult(success=False, message=f"Exited with code {result.returncode}")
            logger.info("OpenHands finished for %s", task.id)
            log_step(
                "executor/openhands: finished task %s" % task.id,
                project_root=self.project_root,
                task_id=task.id,
                returncode=0,
                success=True,
            )
            return ExecutionResult(success=True, message="Completed successfully")
        except Exception as e:
            logger.exception("OpenHands error for %s: %s", task.id, e)
            log_step(
                "executor/openhands: finished task %s" % task.id,
                project_root=self.project_root,
                task_id=task.id,
                success=False,
                error=str(e),
            )
            console.print(f"[red]Error running OpenHands: {e}[/red]")
            return ExecutionResult(success=False, message=str(e))

    def _write_log(self, task_id: str, result: subprocess.CompletedProcess[str]) -> None:
        log_dir = self.project_root / ".devcouncil" / "logs"
        log_dir.mkdir(parents=True, exist_ok=True)
        log_path = log_dir / f"{task_id}-openhands.log"
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
