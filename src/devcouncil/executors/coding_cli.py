import json
import os
import queue
import shutil
import subprocess
import sys
import threading
import time
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
    resolve_cursor_agent_executable,
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
        stream_output: bool | None = None,
    ):
        self.project_root = project_root
        self.client = self._normalize_client(client)
        self.timeout_seconds = timeout_seconds
        self.spec = self._resolve_spec()
        self.profile_name = profile or self.spec.default_profile or "default"
        self.profile = load_agent_profiles(project_root).get(self.profile_name)
        self.last_run_id: str | None = None
        self.last_transcript_path: Path | None = None
        self.stream_output = self._resolve_stream_output(stream_output)

    def _normalize_client(self, client: str) -> str:
        return normalize_agent_name(client)

    def _resolve_spec(self) -> CliAgentSpec:
        spec = get_cli_agent_spec(self.project_root, self.client)
        if spec:
            return spec
        raise ValueError(f"Unsupported coding CLI client: {self.client}")

    def _resolve_stream_output(self, stream_output: bool | None) -> bool:
        if stream_output is not None:
            return stream_output
        try:
            return bool(load_config(self.project_root).execution.stream_cli_output)
        except Exception:
            return False

    def _command(self, task_id: str | None = None) -> list[str]:
        if self.client == "warp":
            base = self._warp_command()
        elif self.client == "cursor":
            base = self._cursor_command(task_id)
        else:
            base = self.spec.base_command()
        return self._apply_profile_args(base)

    # Per-CLI flag used to override the model, when the CLI accepts one. Clients
    # absent from this map simply ignore a profile ``model`` override.
    _MODEL_FLAGS: dict[str, str] = {
        "claude": "--model",
        "codex": "--model",
        "gemini": "--model",
        "cursor": "--model",
        "qwen": "--model",
        "opencode": "--model",
        "aider": "--model",
    }

    def _apply_profile_args(self, command: list[str]) -> list[str]:
        """Apply per-profile CLI overrides to the resolved command.

        Empty/None overrides reproduce today's invocation exactly (no regression):
        ``model`` rewrites/adds the model flag for CLIs that accept one,
        ``permission_mode`` is translated into the right per-CLI flag (and an
        overly-permissive baked-in flag is replaced for stricter modes), and
        ``extra_args`` are appended verbatim. Surfaced in the run manifest so
        ``dev runs show`` reveals exactly how the CLI was invoked."""
        if not self.profile:
            return command
        result = list(command)
        result = self._apply_permission_mode(result)
        result = self._apply_model_override(result)
        extra_args = list(self.profile.extra_args or [])
        if extra_args:
            result = [*result, *extra_args]
        return result

    def _apply_model_override(self, command: list[str]) -> list[str]:
        model = (self.profile.model or "").strip() if self.profile else ""
        if not model:
            return command
        flag = self._MODEL_FLAGS.get(self.client)
        if not flag:
            return command
        result = list(command)
        for index, part in enumerate(result):
            if part == flag and index + 1 < len(result):
                result[index + 1] = model
                return result
        return [*result, flag, model]

    def _apply_permission_mode(self, command: list[str]) -> list[str]:
        mode = (self.profile.permission_mode or "").strip() if self.profile else ""
        if not mode:
            return command
        if self.client == "claude":
            return self._apply_claude_permission_mode(command, mode)
        return command

    @staticmethod
    def _apply_claude_permission_mode(command: list[str], mode: str) -> list[str]:
        """Translate an abstract permission mode into Claude Code's
        ``--permission-mode`` value. ``auto`` keeps blanket auto-apply
        (``acceptEdits``); ``gated``/``ask`` drop blanket auto-apply so edits are
        gated (``default``); ``plan`` is read-only planning. An explicit native
        value (e.g. ``acceptEdits``, ``bypassPermissions``) is passed through."""
        translation = {
            "auto": "acceptEdits",
            "gated": "default",
            "ask": "default",
            "plan": "plan",
        }
        value = translation.get(mode.lower(), mode)
        result = list(command)
        for index, part in enumerate(result):
            if part == "--permission-mode" and index + 1 < len(result):
                result[index + 1] = value
                return result
        return [*result, "--permission-mode", value]

    def _cursor_command(self, task_id: str | None = None) -> list[str]:
        executable = resolve_cursor_agent_executable()
        if not executable:
            raise ValueError("cursor-agent (or agent) is not installed or not on PATH.")
        command = [
            executable,
            "--print",
            "--trust",
            "--workspace",
            str(self.project_root),
        ]
        chat_id = self._cursor_resume_chat_id(task_id)
        if chat_id:
            command.extend(["--resume", chat_id])
        command.append("Read and execute the DevCouncil task prompt at {prompt_file}.")
        return command

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
            command = self._command(task.id)
        except ValueError as exc:
            return ExecutionResult(success=False, message=str(exc))

        executable = command[0]
        if not shutil.which(executable):
            return ExecutionResult(
                success=False,
                message=f"{self.client} CLI is not installed or not on PATH.",
            )

        prompt = PromptBuilder(self.project_root).build_task_prompt(task, requirements)
        from devcouncil.planning.correction_manifest import load_latest_correction_manifest

        correction = load_latest_correction_manifest(self.project_root, task.id)
        if correction is not None:
            prompt = (
                f"# DevCouncil Correction Manifest\n\n"
                f"{correction.model_dump_json(indent=2)}\n\n"
                f"{prompt}"
            )
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

        started = time.monotonic()
        try:
            invocation, input_text = self._invocation(command, prompt, instruction_file)
            display_invocation = self._display_invocation(invocation, prompt)
            # Print the resolved command (placeholders like {prompt_file} already
            # substituted, prompt redacted) rather than the raw template.
            console.print(f"Command: [dim]{' '.join(display_invocation)}[/dim]")
            manifest_path = self._write_run_manifest(
                run_id,
                task,
                display_invocation,
                instruction_file,
                stream=self.stream_output,
            )
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
            transcript_path = (
                self.project_root / ".devcouncil" / "runs" / run_id / "transcript.txt"
                if self.stream_output
                else None
            )
            started = time.monotonic()
            result = self._run_subprocess(invocation, input_text, env, transcript_path=transcript_path)
            duration = round(time.monotonic() - started, 3)
            finished_at = datetime.now(timezone.utc).isoformat()
            self._write_log(log_prefix, result)
            if transcript_path and transcript_path.exists():
                self._append_manifest_transcript(run_id, transcript_path)
                self.last_transcript_path = transcript_path
                console.print(f"Stream transcript: [dim]{transcript_path}[/dim]")
            if result.returncode != 0:
                self._update_run_manifest(
                    run_id,
                    status="failed",
                    returncode=result.returncode,
                    stdout_preview=self._preview_lines(result.stdout),
                    stderr_preview=self._preview_lines(result.stderr),
                    finished_at=finished_at,
                    duration_seconds=duration,
                )
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
            self._update_run_manifest(
                run_id,
                status="finished",
                returncode=result.returncode,
                stdout_preview=self._preview_lines(result.stdout),
                stderr_preview=self._preview_lines(result.stderr),
                finished_at=finished_at,
                duration_seconds=duration,
            )
            TraceLogger(self.project_root).log_event(
                "agent_run_finished",
                {"agent": self.client, "profile": self.profile_name, "returncode": result.returncode},
                run_id=run_id,
                task_id=task.id,
                summary=f"{self.client} finished for {task.id}",
            )
            return ExecutionResult(success=True, message=f"{self.client} execution finished.")
        except subprocess.TimeoutExpired:
            self._update_run_manifest(
                run_id,
                status="timeout",
                finished_at=datetime.now(timezone.utc).isoformat(),
                duration_seconds=round(time.monotonic() - started, 3),
            )
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
            self._update_run_manifest(
                run_id,
                status="failed",
                returncode=None,
                stderr_preview=self._preview_lines(str(exc)),
                finished_at=datetime.now(timezone.utc).isoformat(),
                duration_seconds=round(time.monotonic() - started, 3),
            )
            return ExecutionResult(success=False, message=str(exc))

    def _resolve_invocation(self, invocation: list[str], env: dict[str, str]) -> list[str]:
        """Route Windows batch shims through the command interpreter.

        Coding CLIs installed via npm are exposed on Windows as ``.cmd``/``.bat``
        shims (e.g. ``codex.CMD``). ``CreateProcess`` (shell=False) cannot execute
        a batch file directly nor apply PATHEXT to a bare ``codex``, so the run
        fails with ``WinError 2``/``193``. When the program resolves to such a
        shim, invoke it via ``cmd /c <shim>``; ``.exe`` programs and non-Windows
        platforms are left untouched so the invocation passed to the agent is
        otherwise verbatim.
        """
        if not invocation or os.name != "nt":
            return invocation
        resolved = shutil.which(invocation[0])
        if resolved and resolved.lower().endswith((".cmd", ".bat")):
            comspec = os.environ.get("COMSPEC", "cmd.exe")
            return [comspec, "/c", resolved, *invocation[1:]]
        return invocation

    @staticmethod
    def _emit_stream_line(line: str) -> None:
        """Print a streamed agent line without letting a non-encodable character
        crash the run. Coding agents emit Unicode (e.g. ``✓``) that the
        Windows console / a redirected cp1252 stdout cannot encode; an unguarded
        ``console.print`` would raise UnicodeEncodeError and be misreported as the
        agent failing to start, even though it ran (and may have applied edits).
        """
        try:
            console.print(line, end="")
        except UnicodeEncodeError:
            encoding = getattr(sys.stdout, "encoding", None) or "utf-8"
            safe = line.encode(encoding, errors="replace").decode(encoding, errors="replace")
            console.print(safe, end="")

    def _run_subprocess(
        self,
        invocation: list[str],
        input_text: str | None,
        env: dict[str, str],
        transcript_path: Path | None = None,
    ) -> subprocess.CompletedProcess[str]:
        timeout = self._effective_timeout()
        invocation = self._resolve_invocation(invocation, env)
        if not self.stream_output:
            return subprocess.run(
                invocation,
                input=input_text,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                cwd=self.project_root,
                env=env,
                timeout=timeout,
            )

        process = subprocess.Popen(
            invocation,
            stdin=subprocess.PIPE if input_text is not None else None,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
            cwd=self.project_root,
            env=env,
        )
        stdin = process.stdin
        if input_text is not None and stdin is not None:
            # Feed stdin from a thread so a child that fills its stdout pipe
            # before consuming stdin cannot deadlock against us.
            def _feed_stdin() -> None:
                try:
                    stdin.write(input_text)
                    stdin.close()
                except (BrokenPipeError, OSError):
                    pass

            threading.Thread(target=_feed_stdin, daemon=True).start()

        stdout = process.stdout
        assert stdout is not None
        lines: queue.Queue[str | None] = queue.Queue()

        def _drain_stdout() -> None:
            try:
                for raw_line in iter(stdout.readline, ""):
                    lines.put(raw_line)
            finally:
                lines.put(None)

        reader = threading.Thread(target=_drain_stdout, daemon=True)
        reader.start()

        captured: list[str] = []
        transcript_handle = None
        if transcript_path is not None:
            transcript_path.parent.mkdir(parents=True, exist_ok=True)
            transcript_handle = transcript_path.open("w", encoding="utf-8")
        deadline = time.monotonic() + timeout
        try:
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    process.kill()
                    raise subprocess.TimeoutExpired(invocation, timeout)
                try:
                    line = lines.get(timeout=min(remaining, 1.0))
                except queue.Empty:
                    continue
                if line is None:
                    break
                self._emit_stream_line(line)
                captured.append(line)
                if transcript_handle is not None:
                    transcript_handle.write(redact_text(line))
                    transcript_handle.flush()
            process.wait()
        finally:
            if transcript_handle is not None:
                transcript_handle.close()
            reader.join(timeout=5)

        return subprocess.CompletedProcess(
            invocation,
            process.returncode if process.returncode is not None else 0,
            stdout="".join(captured),
            stderr="",
        )

    def _cursor_resume_mode(self) -> str:
        try:
            mode = (load_config(self.project_root).execution.cursor_resume_mode or "off").strip().lower()
        except Exception:
            mode = "off"
        if mode not in {"off", "project", "task"}:
            return "off"
        return mode

    def _cursor_session_path(self, task_id: str | None = None) -> Path:
        if self._cursor_resume_mode() == "task" and task_id:
            return self.project_root / ".devcouncil" / "sessions" / f"{task_id}-cursor.json"
        return self.project_root / ".devcouncil" / "integrations" / "cursor-session.json"

    def _cursor_resume_chat_id(self, task_id: str | None) -> str | None:
        mode = self._cursor_resume_mode()
        if mode == "off":
            return None
        path = self._cursor_session_path(task_id if mode == "task" else None)
        if path.exists():
            try:
                data = json.loads(path.read_text(encoding="utf-8")) or {}
            except json.JSONDecodeError:
                data = {}
            existing_chat_id = str(data.get("chat_id") or "").strip()
            if existing_chat_id:
                return existing_chat_id
        ensured_chat_id = self._ensure_cursor_chat_id()
        if not ensured_chat_id:
            return None
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({"chat_id": ensured_chat_id}, indent=2) + "\n", encoding="utf-8")
        return ensured_chat_id

    def _ensure_cursor_chat_id(self) -> str | None:
        executable = resolve_cursor_agent_executable()
        if not executable:
            return None
        try:
            result = subprocess.run(
                [executable, "create-chat"],
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                cwd=self.project_root,
                timeout=60,
            )
        except subprocess.TimeoutExpired:
            return None
        if result.returncode != 0:
            return None
        chat_id = (result.stdout or result.stderr or "").strip().splitlines()[-1].strip()
        return chat_id or None

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

    def _profile_override_summary(self) -> dict[str, object]:
        """Resolved per-profile CLI overrides recorded in the manifest so a
        supervisor can see exactly how the profile constrained the invocation."""
        if not self.profile:
            return {"extra_args": [], "permission_mode": None, "model": None}
        return {
            "extra_args": list(self.profile.extra_args or []),
            "permission_mode": self.profile.permission_mode,
            "model": self.profile.model,
        }

    def _update_run_manifest(self, run_id: str, **updates: object) -> None:
        manifest_path = self.project_root / ".devcouncil" / "runs" / run_id / "agent-run.json"
        if not manifest_path.exists():
            return
        try:
            manifest = json.loads(manifest_path.read_text(encoding="utf-8")) or {}
        except json.JSONDecodeError:
            return
        manifest.update(updates)
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

    def _preview_lines(self, value: str | None, *, limit: int = 20) -> list[str]:
        lines = redact_text(value or "").splitlines()
        return lines[:limit]

    def _append_manifest_transcript(self, run_id: str, transcript_path: Path) -> None:
        self._update_run_manifest(run_id, transcript=str(transcript_path))

    def _write_run_manifest(
        self,
        run_id: str,
        task: Task,
        invocation: list[str],
        instruction_file: Path,
        *,
        stream: bool = False,
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
            "profile_overrides": self._profile_override_summary(),
            "kind": self.spec.kind,
            "command": invocation,
            "prompt_file": str(instruction_file),
            "planned_files": [planned.model_dump() for planned in task.planned_files],
            "allowed_commands": task.allowed_commands,
            "expected_tests": task.expected_tests,
            "timestamp": datetime.now(timezone.utc).isoformat(),
            "stream": stream,
            "artifact_version": 1,
            "started_at": datetime.now(timezone.utc).isoformat(),
            "status": "running",
            "transcript": None,
            "returncode": None,
            "stdout_preview": [],
            "stderr_preview": [],
            "finished_at": None,
            "duration_seconds": None,
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
        desired = {
            "devcouncil": {
                "command": "devcouncil",
                "args": ["mcp-server"],
                "env": {"DEVCOUNCIL_PROJECT_ROOT": str(self.project_root)},
            }
        }
        should_write = not path.exists()
        if path.exists():
            try:
                existing = json.loads(path.read_text(encoding="utf-8")) or {}
            except json.JSONDecodeError:
                existing = {}
            should_write = "mcpServers" in existing and "devcouncil" not in existing
        if should_write:
            path.write_text(json.dumps(desired, indent=2) + "\n", encoding="utf-8")
        return path
