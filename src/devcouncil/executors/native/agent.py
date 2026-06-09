from typing import List, Dict, Any
import asyncio
from rich.console import Console
from pydantic import BaseModel
from devcouncil.domain.task import Task
from devcouncil.domain.requirement import Requirement
from devcouncil.execution.executor import Executor, ExecutionResult
from devcouncil.llm.router import ModelRouter
from devcouncil.execution.task_runner import TaskRunner
from devcouncil.execution.context_builder import ContextBuilder
from devcouncil.execution.paths import resolve_project_path

console = Console()

class ToolCall(BaseModel):
    tool: str
    args: Dict[str, Any]

class AgentAction(BaseModel):
    thought: str
    tool_calls: List[ToolCall] = []
    finish: bool = False

class NativeAgent(Executor):
    def __init__(self, router: ModelRouter, task_runner: TaskRunner):
        self.router = router
        self.task_runner = task_runner
        self.context_builder = ContextBuilder(task_runner.project_root)

    def run_task(self, task: Task, requirements: List[Requirement]) -> ExecutionResult:
        """Run the preview native executor behind the normal synchronous executor contract."""
        return asyncio.run(self._run_task_async(task, requirements))

    async def _run_task_async(self, task: Task, requirements: List[Requirement]) -> ExecutionResult:
        console.print(f"Starting [bold]Native Executor[/bold] for task {task.id}...")
        console.print("[yellow]Native executor is preview quality; DevCouncil verification remains the completion gate.[/yellow]")
        
        # 1. Gather rich context
        context_json = self.context_builder.build_task_context(task, requirements)
        from devcouncil.planning.correction_manifest import load_latest_correction_manifest

        correction = load_latest_correction_manifest(self.task_runner.project_root, task.id)
        correction_block = ""
        if correction is not None:
            correction_block = f"\nCorrection Manifest:\n{correction.model_dump_json(indent=2)}\n"

        system_prompt = f"""
You are the DevCouncil Native Agent. Your goal is to implement the provided task.
Current Project Context:
{context_json}
{correction_block}

You have access to the following tools:
- read_file(path: str)
- list_files()
- apply_patch(patch: str)
- run_command(command: str)

Rules:
1. You can only write to files or apply patches to files listed in the task's 'planned_files'.
2. You can only run commands listed in the task's 'allowed_commands'.
3. Use 'thought' to explain your reasoning.
4. Set 'finish' to true when you believe the task is complete and verified.
"""
        messages = [{"role": "system", "content": system_prompt}]
        
        # Initial task prompt
        messages.append({"role": "user", "content": f"Begin implementing task {task.id} based on the context provided."})

        # Basic tool loop (Max 10 steps for safety in MVP)
        for step in range(10):
            action = await self.router.complete_structured(
                role="native_agent", 
                messages=messages,
                schema=AgentAction
            )
            
            console.print(f"\n[bold]Step {step+1}:[/bold] {action.thought}")
            
            if action.finish:
                console.print("[green]Native agent signaled completion.[/green]")
                return ExecutionResult(success=True, message="Agent signaled completion; pending DevCouncil verification")

            for tool_call in action.tool_calls:
                result_summary = ""
                try:
                    if tool_call.tool == "read_file":
                        path = tool_call.args["path"]
                        resolved = resolve_project_path(self.task_runner.project_root, path)
                        # Security: block reading sensitive files
                        sensitive_patterns = {".env", ".pem", ".key", "credentials", "secrets"}
                        path_lower = path.lower()
                        if any(s in path_lower for s in sensitive_patterns):
                            raise PermissionError(f"Reading sensitive file blocked: {path}")
                        content = resolved.read_text(encoding="utf-8")
                        if len(content) > 8000:
                            content = content[:8000] + "\n[truncated]"
                        result_summary = f"File content of {path}:\n{content}"
                    elif tool_call.tool == "list_files":
                        # We use the internal helper but return limited list
                        files = self.context_builder.get_structure_summary()
                        result_summary = f"Found {len(files)} files in repository."
                    elif tool_call.tool == "write_file":
                        raise PermissionError("write_file is disabled for the native executor; use apply_patch.")
                    elif tool_call.tool == "apply_patch":
                        self.task_runner.apply_patch(tool_call.args["patch"], task)
                        result_summary = "Successfully applied patch."
                    elif tool_call.tool == "run_command":
                        cmd_result = self.task_runner.run_command(tool_call.args["command"], task)
                        result_summary = f"Command finished with exit code {cmd_result.exit_code}."
                    else:
                        raise ValueError(f"Unknown tool: {tool_call.tool}")
                    
                    messages.append({"role": "user", "content": f"[Tool Result] '{tool_call.tool}': {result_summary}"})
                except Exception as e:
                    console.print(f"[red]Error executing tool {tool_call.tool}: {e}[/red]")
                    messages.append({"role": "user", "content": f"[Tool Error] '{tool_call.tool}' failed: {e}"})

        console.print("[red]Native agent reached maximum step limit.[/red]")
        return ExecutionResult(success=False, message="Reached maximum step limit")
