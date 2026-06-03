import json
import os
import shutil
import subprocess
import uuid
from datetime import datetime, timezone
from pathlib import Path

from rich.console import Console

from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.app.config import load_config
from devcouncil.execution.executor import Executor, ExecutionResult
from devcouncil.execution.prompt_builder import PromptBuilder
from devcouncil.executors.agent_registry import (
    VALID_INPUT_MODES,
    CliAgentSpec,
    get_cli_agent_spec,
    load_agent_profiles,
    normalize_agent_name,
)
from devcouncil.repo.gitignore import ensure_gitignore
from devcouncil.telemetry.traces import TraceLogger
from devcouncil.utils.redaction import redact_text

console = Console()


class CodingCliExecutor(Executor):
    """Execute a DevCouncil task by handing it off to an external coding CLI."""

    def __init__(
        self,
        project_root: Path,
        client: str,
        timeout_seconds: int = 1800,
        profile: str | None = None,
    ):
        self.project_root = project_root
        self.client = self._normalize_client(client)
        self.timeout_seconds = timeout_seconds
        self.spec = self._resolve_spec()
        self.profile_name = profile or self.spec.default_profile or "default"
        self.profile = load_agent_profiles(project_root).get(self.profile_name)
        self.last_run_id: str | None = None

    def _normalize_client(self, client: str) -> str:
        return normalize_agent_name(client)

    def _resolve_spec(self) -> CliAgentSpec:
        spec = get_cli_agent_spec(self.project_root, self.client)
        if spec:
            return spec
        raise ValueError(f"Unsupported coding CLI client: {self.client}")

    def _command(self) -> list[str]:
        if self.client == "warp":
            return self._warp_command()
        return self.spec.base_command()

    def _warp_command(self) -> list[str]:
        config = self._load_warp_config()
        command = config.get("command", "oz")
        mode = config.get("run_mode", "local")
        subcommand = "run-cloud" if mode == "cloud" else "run"
        mcp_path = self._ensure_warp_mcp_config(config)
        args = [command, "agent", subcommand, "--name", "devcouncil-task", "--mcp", str(mcp_path)]
        if subcommand == "run":
            args.extend(["--cwd", str(self.project_root)])
        if profile := config.get("profile"):
            if subcommand == "run":
                args.extend(["--profile", str(profile)])
        if model := config.get("model"):
            args.extend(["--model", str(model)])
        if environment := config.get("environment"):
            args.extend(["--environment", str(environment)])
        for share in config.get("share", []):
            args.extend(["--share", str(share)])
        args.append("--prompt")
        return args

    def _load_warp_config(self) -> dict:
        try:
            warp = load_config(self.project_root).integrations.warp
            data = warp.model_dump()
        except Exception:
            data = {}
        if command := os.environ.get("DEVCOUNCIL_WARP_COMMAND"):
            data["command"] = command
        if mode := os.environ.get("DEVCOUNCIL_WARP_RUN_MODE"):
            data["run_mode"] = mode
        if profile := os.environ.get("DEVCOUNCIL_WARP_PROFILE"):
            data["profile"] = profile
        if model := os.environ.get("DEVCOUNCIL_WARP_MODEL"):
            data["model"] = model
        if environment := os.environ.get("DEVCOUNCIL_WARP_ENVIRONMENT"):
            data["environment"] = environment
        return data

    def run_task(self, task: Task, requirements: list[Requirement]) -> ExecutionResult:
        if self.profile is None:
            return ExecutionResult(
                success=False,
                message=f"Unknown agent profile '{self.profile_name}' for {self.client}.",
            )
        if self.spec.input_mode not in VALID_INPUT_MODES:
            return ExecutionResult(
                success=False,
                message=(
                    f"Invalid input_mode '{self.spec.input_mode}' for {self.client}. "
                    "Use one of: argument, prompt-file, stdin."
                ),
            )

        ensure_gitignore(self.project_root)

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
        prompt = self._apply_profile_prompt(prompt)
        instruction_file = self.project_root / ".devcouncil" / f"{task.id}-{self.client}-task.md"
        instruction_file.parent.mkdir(parents=True, exist_ok=True)
        instruction_file.write_text(prompt, encoding="utf-8")

        custom_env = self.spec.env
        env = {**dict(os.environ), **custom_env, "DEVCOUNCIL_PROJECT_ROOT": str(self.project_root)}
        env["DEVCOUNCIL_AGENT_PROFILE"] = self.profile_name
        log_prefix = f"{task.id}-{self.client}"
        run_id = str(uuid.uuid4())
        self.last_run_id = run_id

        console.print(f"Starting [bold]{self.client.upper()}[/bold] for task [bold]{task.id}[/bold]...")
        console.print(f"Task prompt: [dim]{instruction_file}[/dim]")
        console.print(f"Command: [dim]{' '.join(command)}[/dim]")

        try:
            invocation, input_text = self._invocation(command, prompt, instruction_file)
            display_invocation = self._display_invocation(invocation, prompt)
            manifest_path = self._write_run_manifest(run_id, task, display_invocation, instruction_file)
            TraceLogger(self.project_root).log_event(
                "agent_run_started",
                {
                    "agent": self.client,
                    "profile": self.profile_name,
                    "command": display_invocation,
                    "prompt_file": str(instruction_file),
                    "manifest": str(manifest_path),
                },
                run_id=run_id,
                task_id=task.id,
                summary=f"Started {self.client} for {task.id}",
            )
            result = subprocess.run(
                invocation,
                input=input_text,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                cwd=self.project_root,
                env=env,
                timeout=self._effective_timeout(),
            )
            self._write_log(log_prefix, result)
            if result.returncode != 0:
                stderr_preview = (result.stderr or result.stdout or "").strip().splitlines()[:5]
                detail = redact_text(stderr_preview[0]) if stderr_preview else "No diagnostics were produced."
                TraceLogger(self.project_root).log_event(
                    "agent_run_failed",
                    {"agent": self.client, "profile": self.profile_name, "returncode": result.returncode, "detail": detail},
                    run_id=run_id,
                    task_id=task.id,
                    summary=f"{self.client} exited with code {result.returncode}",
                )
                return ExecutionResult(
                    success=False,
                    message=f"{self.client} exited with code {result.returncode}: {detail}",
                )
            TraceLogger(self.project_root).log_event(
                "agent_run_finished",
                {"agent": self.client, "profile": self.profile_name, "returncode": result.returncode},
                run_id=run_id,
                task_id=task.id,
                summary=f"{self.client} finished for {task.id}",
            )
            return ExecutionResult(success=True, message=f"{self.client} execution finished.")
        except subprocess.TimeoutExpired as exc:
            _ = exc
            TraceLogger(self.project_root).log_event(
                "agent_run_failed",
                {"agent": self.client, "profile": self.profile_name, "timeout_seconds": self._effective_timeout()},
                run_id=run_id,
                task_id=task.id,
                summary=f"{self.client} timed out for {task.id}",
            )
            return ExecutionResult(
                success=False,
                message=f"{self.client} execution timed out after {self._effective_timeout()}s.",
            )
        except Exception as exc:
            return ExecutionResult(success=False, message=str(exc))

    def _effective_timeout(self) -> int:
        if self.profile and self.profile.timeout_seconds:
            return int(self.profile.timeout_seconds)
        return int(self.spec.timeout_seconds or self.timeout_seconds)

    def _invocation(self, command: list[str], prompt: str, instruction_file: Path) -> tuple[list[str], str | None]:
        mode = self.spec.input_mode
        resolved = [
            part.replace("{prompt_file}", str(instruction_file)).replace("{project_root}", str(self.project_root))
            for part in command
        ]
        if mode == "stdin":
            return resolved, prompt
        if mode == "argument":
            if any("{prompt}" in part for part in resolved):
                return [part.replace("{prompt}", prompt) for part in resolved], None
            prompt_arg = self.spec.prompt_arg
            return [*resolved, *(([prompt_arg] if prompt_arg else [])), prompt], None
        if mode == "prompt-file":
            if "{prompt_file}" in " ".join(command):
                return resolved, None
            prompt_arg = self.spec.prompt_arg
            return [*resolved, *(([prompt_arg] if prompt_arg else [])), str(instruction_file)], None
        return resolved, prompt

    def _display_invocation(self, invocation: list[str], prompt: str) -> list[str]:
        return [part.replace(prompt, "<task prompt>") for part in invocation]

    def _apply_profile_prompt(self, prompt: str) -> str:
        if not self.profile:
            return prompt
        additions = []
        if self.profile.prompt_preamble:
            additions.append(self.profile.prompt_preamble)
        if self.profile.require_explicit_confirmation:
            additions.append("Ask for confirmation before any high-risk, out-of-scope, or destructive action.")
        if not additions:
            return prompt
        return "\n\n".join(["# DevCouncil Agent Profile", *additions, prompt])

    def _write_run_manifest(
        self,
        run_id: str,
        task: Task,
        invocation: list[str],
        instruction_file: Path,
    ) -> Path:
        run_dir = self.project_root / ".devcouncil" / "runs" / run_id
        run_dir.mkdir(parents=True, exist_ok=True)
        manifest_path = run_dir / "agent-run.json"
        manifest = {
            "run_id": run_id,
            "task_id": task.id,
            "agent": self.client,
            "display_name": self.spec.label,
            "profile": self.profile_name,
            "kind": self.spec.kind,
            "command": invocation,
            "prompt_file": str(instruction_file),
            "planned_files": [planned.model_dump() for planned in task.planned_files],
            "allowed_commands": task.allowed_commands,
            "expected_tests": task.expected_tests,
            "timestamp": datetime.now(timezone.utc).isoformat(),
        }
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        return manifest_path

    def _write_log(self, task_client: str, result: subprocess.CompletedProcess[str]) -> None:
        log_dir = self.project_root / ".devcouncil" / "logs"
        log_dir.mkdir(parents=True, exist_ok=True)
        log_path = log_dir / f"{task_client}.log"
        log_path.write_text(
            "\n".join([
                f"command_returncode={result.returncode}",
                "=== stdout ===",
                redact_text(result.stdout or ""),
                "=== stderr ===",
                redact_text(result.stderr or ""),
            ]),
            encoding="utf-8",
        )

    def _ensure_warp_mcp_config(self, config: dict | None = None) -> Path:
        configured_path = (config or {}).get("mcp_config_path") or ".devcouncil/integrations/warp-mcp.json"
        path = Path(configured_path).expanduser()
        if not path.is_absolute():
            path = self.project_root / path
        path.parent.mkdir(parents=True, exist_ok=True)
        if not path.exists():
            import json

            path.write_text(
                json.dumps(
                    {
                        "mcpServers": {
                            "devcouncil": {
                                "command": "devcouncil",
                                "args": ["mcp-server"],
                                "env": {"DEVCOUNCIL_PROJECT_ROOT": str(self.project_root)},
                                "working_directory": str(self.project_root),
                            }
                        }
                    },
                    indent=2,
                )
                + "\n",
                encoding="utf-8",
            )
        return path
