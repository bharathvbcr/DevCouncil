import subprocess
from pathlib import Path
from rich.console import Console
from devcouncil.domain.task import Task
from devcouncil.domain.requirement import Requirement
from devcouncil.execution.executor import Executor, ExecutionResult
from devcouncil.execution.prompt_builder import PromptBuilder

console = Console()

class MiniSWEExecutor(Executor):
    def __init__(self, project_root: Path):
        self.project_root = project_root

    def run_task(self, task: Task, requirements: list[Requirement]) -> ExecutionResult:
        builder = PromptBuilder()
        task_prompt = builder.build_task_prompt(task, requirements)
        
        # Write temporary instruction file for mini-SWE-agent
        instruction_file = self.project_root / ".devcouncil" / "task_instruction.md"
        instruction_file.write_text(task_prompt)
        
        console.print(f"Starting [bold]mini-SWE-agent[/bold] for task {task.id}...")
        
        # In a real implementation, we'd invoke the agent CLI
        # For now, we simulate the command call
        cmd = [
            "python", "-m", "mini_swe_agent.main", 
            "--instruction-file", str(instruction_file),
            "--repo-path", str(self.project_root)
        ]
        
        console.print(f"Command: [dim]{' '.join(cmd)}[/dim]")
        
        # Since I might not have mini_swe_agent installed here, 
        # I'll just explain what it would do.
        console.print("[yellow]Note: mini_swe_agent must be installed in the environment.[/yellow]")
        
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
                console.print(f"[red]mini-SWE-agent exited with {result.returncode}.[/red]")
                return ExecutionResult(success=False, message='Execution failed')
            return ExecutionResult(success=True, message='Execution successful')
        except Exception as e:
            console.print(f"[red]Error running mini-SWE-agent: {e}[/red]")
            return ExecutionResult(success=False, message='Execution failed')

    def _write_log(self, task_id: str, result: subprocess.CompletedProcess[str]) -> None:
        log_dir = self.project_root / ".devcouncil" / "logs"
        log_dir.mkdir(parents=True, exist_ok=True)
        log_path = log_dir / f"{task_id}-mini-swe.log"
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
