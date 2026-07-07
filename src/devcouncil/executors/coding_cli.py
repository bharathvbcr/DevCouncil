import json
import logging
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
from typing import Callable, Optional

from rich.console import Console

from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.app.config import DevCouncilConfig, load_config
from devcouncil.utils.json_persist import read_json, write_json
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
from devcouncil.telemetry.stages import log_step
from devcouncil.telemetry.logging_setup import run_log
from devcouncil.utils.redaction import redact_text

console = Console()
logger = logging.getLogger(__name__)

# DevCouncil-managed scaffolding `dev` writes into a workspace itself (agent guides, the
# managed .gitignore). The pre-verify scope gate must never revert these — they are not
# task work, and reverting them would undo `dev`'s own setup. .devcouncil/* is handled
# by a prefix check at the call site.
_SCAFFOLDING_PATHS = frozenset({"AGENTS.md", "AGENTS.json", "CLAUDE.md", ".gitignore"})


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
        # Load the project config once per executor instance. The same handle is
        # reused by _resolve_stream_output, _cursor_resume_mode and the Warp
        # command builder, which would otherwise each re-parse config.yaml.
        self._config: Optional[DevCouncilConfig]
        try:
            self._config = load_config(project_root)
        except Exception:
            self._config = None
        self.client = self._normalize_client(client)
        self.timeout_seconds = timeout_seconds
        self.spec = self._resolve_spec()
        self.profile_name = profile or self.spec.default_profile or "default"
        self.profile = load_agent_profiles(project_root).get(self.profile_name)
        self.last_run_id: str | None = None
        self.last_transcript_path: Path | None = None
        # Claude Code session id for the most recent run (assigned by DevCouncil via
        # --session-id, or reused via --resume). Lets callers/live-review locate the
        # native ~/.claude transcript for a headless run and resume it for repairs.
        self.last_agent_session_id: str | None = None
        self._pending_claude_session: tuple[Path, str] | None = None
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
            if self._config is None:
                return False
            return bool(self._config.execution.stream_cli_output)
        except Exception:
            return False

    def _command(self, task_id: str | None = None) -> list[str]:
        if self.client == "warp":
            base = self._warp_command()
        elif self.client == "cursor":
            base = self._cursor_command(task_id)
        elif self.client == "claude":
            base = self._claude_command(task_id)
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
        # NOTE: extra_args are NOT appended here. For argument/prompt-file CLIs the prompt
        # (and sometimes its flag, e.g. warp --prompt / aider --message) is appended last
        # by _invocation; appending extra_args at the tail here would slot them between the
        # prompt flag and its value. _invocation places them correctly instead.
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
            if self._config is None:
                data = {}
            else:
                data = self._config.integrations.warp.model_dump()
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
        logger.info("coding_cli.run_task: client=%s profile=%s task=%s", self.client, self.profile_name, task.id)
        log_step(
            f"executor/{self.client}: starting task {task.id}",
            project_root=self.project_root,
            task_id=task.id,
            profile=self.profile_name,
        )
        if self.profile is None:
            logger.error("Unknown agent profile %r for %s; cannot start.", self.profile_name, self.client)
            return ExecutionResult(
                success=False,
                message=f"Unknown agent profile '{self.profile_name}' for {self.client}.",
            )
        if self.spec.input_mode not in VALID_INPUT_MODES:
            logger.error("Invalid input_mode %r for %s; cannot start.", self.spec.input_mode, self.client)
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
            logger.error("Failed to build %s command for %s: %s", self.client, task.id, exc)
            return ExecutionResult(success=False, message=str(exc))

        executable = command[0]
        if not shutil.which(executable):
            logger.error("%s CLI executable %r not found on PATH.", self.client, executable)
            return ExecutionResult(
                success=False,
                message=f"{self.client} CLI is not installed or not on PATH.",
            )

        prompt = PromptBuilder(self.project_root).build_task_prompt(task, requirements)
        from devcouncil.planning.correction_manifest import repair_prompt_prefix

        prefix = repair_prompt_prefix(self.project_root, task.id)
        if prefix:
            prompt = f"{prefix}{prompt}"
        prompt = self._apply_profile_prompt(prompt)
        instruction_file = self.project_root / ".devcouncil" / f"{task.id}-{self.client}-task.md"
        instruction_file.parent.mkdir(parents=True, exist_ok=True)
        instruction_file.write_text(prompt, encoding="utf-8")

        env = self._build_env()
        log_prefix = f"{task.id}-{self.client}"
        run_id = str(uuid.uuid4())
        self.last_run_id = run_id

        console.print(f"Starting [bold]{self.client.upper()}[/bold] for task [bold]{task.id}[/bold]...")
        console.print(f"Task prompt: [dim]{instruction_file}[/dim]")

        # Isolate this run's full DEBUG trail in its own run-dir log, on top of the
        # always-on shared devcouncil.log, so an agent run can be inspected end-to-end
        # without grepping across unrelated activity / log rotations.
        run_log_cm = run_log(self.project_root / ".devcouncil" / "runs" / run_id / "run.log")
        run_log_cm.__enter__()

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
            # Persist the Claude session id now (before the subprocess), so a crashed or
            # timed-out run still leaves a resumable session, and record it on the manifest.
            if self.client == "claude":
                self._persist_claude_session()
                if self.last_agent_session_id:
                    self._update_run_manifest(run_id, agent_session_id=self.last_agent_session_id)
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
            logger.info("Launching %s subprocess for %s (timeout=%ss)", self.client, task.id, self._effective_timeout())
            # Render Claude's NDJSON stream as readable lines while it runs (raw is still
            # captured to the transcript); other clients print their output verbatim.
            display_transform = (
                self._render_claude_stream_event
                if (self.client == "claude" and self.stream_output)
                else None
            )
            result = self._run_subprocess(
                invocation, input_text, env,
                transcript_path=transcript_path,
                display_transform=display_transform,
            )
            # Transient-failure retry: a CLI that died on a network/provider fault
            # ("API Error: Connection closed mid-response", 429/5xx, overloaded) says
            # nothing about the task — without a retry the failure ends the task
            # `blocked`, burns a repair attempt on a non-code problem, and surfaces
            # as a false negative (seen directly in the benchmark). Only failures
            # whose output matches a known-transient signature are retried, with a
            # short backoff; genuine agent errors still fail immediately.
            retries = 0
            retry_limit = self._transient_retry_limit()
            while result.returncode != 0 and retries < retry_limit:
                reason = self._transient_failure_reason(result)
                if reason is None:
                    break
                retries += 1
                delay = min(30.0, 5.0 * retries)
                logger.warning(
                    "%s failed with a transient error for %s (%s); retrying %d/%d in %.0fs",
                    self.client, task.id, reason, retries, retry_limit, delay,
                )
                TraceLogger(self.project_root).log_event(
                    "agent_run_transient_retry",
                    {
                        "agent": self.client,
                        "profile": self.profile_name,
                        "returncode": result.returncode,
                        "reason": reason,
                        "attempt": retries,
                        "limit": retry_limit,
                    },
                    run_id=run_id,
                    task_id=task.id,
                    summary=f"Transient {self.client} failure ({reason}); retry {retries}/{retry_limit}",
                )
                time.sleep(delay)
                result = self._run_subprocess(
                    invocation, input_text, env,
                    transcript_path=transcript_path,
                    display_transform=display_transform,
                )
            duration = round(time.monotonic() - started, 3)
            logger.info("%s subprocess for %s exited %s in %.2fs", self.client, task.id, result.returncode, duration)
            finished_at = datetime.now(timezone.utc).isoformat()
            self._write_log(log_prefix, result)
            # Capture Claude's structured result onto the manifest. Non-stream: parse the one
            # JSON blob and swap stdout for the human ``result`` text so previews stay readable.
            # Stream: harvest the terminal result event from the captured NDJSON for telemetry.
            if self.client == "claude":
                if self.stream_output:
                    self._capture_claude_stream_json(run_id, result)
                else:
                    result = self._capture_claude_json(run_id, result)
                self._mirror_claude_transcript()
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
                logger.error("%s exited %s for %s: %s", self.client, result.returncode, task.id, detail)
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
            log_step(
                f"executor/{self.client}: finished task {task.id}",
                project_root=self.project_root,
                task_id=task.id,
                returncode=result.returncode,
                duration_s=round(duration, 2),
                trace=True,
            )
            # Opt-in pre-verify scope gate: this CLI subprocess wrote directly to disk with
            # no per-write hook, so revert any out-of-scope change now (before it reaches the
            # verify gate or a commit) rather than only flagging it as orphan_diff post-verify.
            if self._scope_enforcement_enabled():
                reverted = self._enforce_file_scope(task)
                if reverted:
                    files = ", ".join(path for path, _ in reverted)
                    TraceLogger(self.project_root).log_event(
                        "agent_scope_violation_reverted",
                        {"agent": self.client, "task_id": task.id,
                         "reverted": [{"path": p, "reason": r} for p, r in reverted]},
                        run_id=run_id,
                        task_id=task.id,
                        summary=f"Reverted {len(reverted)} out-of-scope change(s) by {self.client}",
                    )
                    return ExecutionResult(
                        success=False,
                        message=(
                            f"Reverted {len(reverted)} out-of-scope file change(s) the task did not "
                            f"authorize: {files}. Re-run keeping edits within the task's allowed files."
                        ),
                    )
            return ExecutionResult(success=True, message=f"{self.client} execution finished.")
        except subprocess.TimeoutExpired:
            logger.error("%s timed out after %ss for %s", self.client, self._effective_timeout(), task.id)
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
            logger.exception("%s execution raised for %s: %s", self.client, task.id, exc)
            self._update_run_manifest(
                run_id,
                status="failed",
                returncode=None,
                stderr_preview=self._preview_lines(str(exc)),
                finished_at=datetime.now(timezone.utc).isoformat(),
                duration_seconds=round(time.monotonic() - started, 3),
            )
            return ExecutionResult(success=False, message=str(exc))
        finally:
            run_log_cm.__exit__(None, None, None)

    def _build_env(self) -> dict[str, str]:
        """Environment for the agent subprocess.

        Layering (later wins): parent process env → the agent spec's ``env`` (custom
        agents) → the profile's ``env`` overrides → DevCouncil's own variables. The
        profile layer is what lets a profile redirect the Claude Code harness at an
        alternative Anthropic-compatible endpoint (``ANTHROPIC_BASE_URL`` /
        ``ANTHROPIC_AUTH_TOKEN`` / ``ANTHROPIC_MODEL``, e.g. a local LiteLLM proxy in
        front of Ollama or OpenRouter) — the same provider-redirection trick DevPrism's
        loopback proxy uses — without editing the base spec. DevCouncil's variables are
        applied last so no profile can mask them."""
        profile_env = dict(self.profile.env) if (self.profile and self.profile.env) else {}
        return {
            **dict(os.environ),
            **self.spec.env,
            **profile_env,
            "DEVCOUNCIL_PROJECT_ROOT": str(self.project_root),
            "DEVCOUNCIL_AGENT_PROFILE": self.profile_name,
        }

    def _scope_enforcement_enabled(self) -> bool:
        try:
            from devcouncil.app.config import load_config
            return bool(load_config(self.project_root).execution.enforce_file_scope_pre_verify)
        except Exception:
            return False

    # Output signatures of failures caused by the NETWORK/PROVIDER, not the task.
    # Matched case-insensitively against stderr plus the tail of stdout. Deliberately
    # phrase-based (no bare status-code numbers) so code/diff content in stdout cannot
    # spuriously classify a genuine agent failure as transient.
    _TRANSIENT_FAILURE_MARKERS = (
        "connection closed",
        "connection reset",
        "connection refused",
        "connection error",
        "connection aborted",
        "econnreset",
        "econnrefused",
        "etimedout",
        "socket hang up",
        "mid-response",
        "network error",
        "fetch failed",
        "temporarily unavailable",
        "service unavailable",
        "internal server error",
        "bad gateway",
        "gateway timeout",
        "overloaded",
        "rate limit",
        "too many requests",
        "request timed out",
        "timeout awaiting",
        "tls handshake",
        "dns",
    )

    def _transient_failure_reason(self, result: "subprocess.CompletedProcess[str]") -> str | None:
        """The matched transient marker when a failed run looks network/provider-caused,
        else None. Scans stderr fully and only the TAIL of stdout (where CLIs print
        their final error) so large code output cannot trigger a false match."""
        haystack = f"{result.stderr or ''}\n{(result.stdout or '')[-4000:]}".lower()
        for marker in self._TRANSIENT_FAILURE_MARKERS:
            if marker in haystack:
                return marker
        return None

    def _transient_retry_limit(self) -> int:
        """Max transient-failure retries from ``execution.transient_retry_attempts``.

        Defaults conservatively (2) when config is unavailable; 0 disables retry."""
        try:
            from devcouncil.app.config import load_config

            return max(0, int(load_config(self.project_root).execution.transient_retry_attempts))
        except Exception:
            return 2

    def _enforce_file_scope(self, task: Task) -> list[tuple[str, str]]:
        """Revert any file this task's subprocess changed that the task does not authorize.

        Uses the task's net changed files (baseline/snapshot subtracted, DevCouncil-managed
        paths already filtered) and the same policy the hook path enforces. Returns the list
        of ``(path, reason)`` reverted; empty when every change was in scope."""
        try:
            from devcouncil.execution.policy_engine import TaskPolicyEngine
            from devcouncil.verification.verifier import Verifier

            changed = Verifier(self.project_root).get_task_changed_files(task.id)
        except Exception as exc:
            logger.exception(
                "Scope enforcement failed for %s: could not load changed files",
                task.id,
            )
            raise RuntimeError(
                f"Scope enforcement failed for {task.id}: could not determine changed files ({exc})."
            ) from exc
        engine = TaskPolicyEngine(self.project_root)
        reverted: list[tuple[str, str]] = []
        for path in changed:
            # Never touch DevCouncil-managed scaffolding even if it surfaces in the diff
            # (e.g. baseline snapshots failed to load): `dev` owns these files, not the task.
            if path in _SCAFFOLDING_PATHS or path.startswith(".devcouncil/"):
                continue
            try:
                decision = engine.evaluate_file_change(path, task, "write")
            except Exception:
                logger.warning(
                    "Scope enforcement skipped policy check for %s on %s",
                    task.id,
                    path,
                    exc_info=True,
                )
                continue
            if decision.action == "deny" and self._revert_path(path):
                reverted.append((path, decision.reason))
        return reverted

    _GIT_REVERT_TIMEOUT_SECONDS = 120

    def _revert_path(self, rel_path: str) -> bool:
        """Undo an out-of-scope change. A file that exists in HEAD is restored to HEAD; a
        file the task newly added (absent from HEAD, or no HEAD at all) is unstaged and
        deleted. Best-effort — a failed revert returns False so the path is not reported as
        cleanly gated (and the caller does not claim it was reverted)."""
        timeout = self._GIT_REVERT_TIMEOUT_SECONDS
        try:
            in_head = subprocess.run(
                ["git", "cat-file", "-e", f"HEAD:{rel_path}"],
                cwd=self.project_root, capture_output=True, text=True,
                timeout=timeout,
            ).returncode == 0
            if in_head:
                return subprocess.run(
                    ["git", "checkout", "HEAD", "--", rel_path],
                    cwd=self.project_root, capture_output=True, text=True,
                    timeout=timeout,
                ).returncode == 0
            # New file (incl. the no-HEAD case): unstage if staged, then remove the working
            # copy so it cannot be committed by the next repair attempt.
            subprocess.run(
                ["git", "rm", "-f", "--cached", "--ignore-unmatch", rel_path],
                cwd=self.project_root, capture_output=True, text=True,
                timeout=timeout,
            )
            full = self.project_root / rel_path
            if full.is_file():
                full.unlink()
            return True
        except Exception:
            return False

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
        # Resolve against the PATH the child will actually run with (which includes any
        # per-agent env overrides), not the parent process PATH — otherwise shim
        # detection and execution can disagree on which executable runs.
        resolved = shutil.which(invocation[0], path=env.get("PATH"))
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
        display_transform: Callable[[str], str | None] | None = None,
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
                    # Reap the killed child so it doesn't linger as a zombie, and close
                    # stdin so the feeder thread unblocks. Bounded wait — kill() already
                    # signalled it.
                    try:
                        if process.stdin is not None:
                            process.stdin.close()
                    except OSError:
                        pass
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        pass
                    raise subprocess.TimeoutExpired(invocation, timeout)
                try:
                    line = lines.get(timeout=min(remaining, 1.0))
                except queue.Empty:
                    continue
                if line is None:
                    break
                # Display may be transformed (e.g. Claude NDJSON → a readable line, or
                # suppressed for noise), but capture and transcript always keep the raw
                # bytes so post-run parsing and forensics see the full stream.
                display: str | None = line
                if display_transform is not None:
                    try:
                        display = display_transform(line)
                    except Exception:
                        display = line
                if display:
                    self._emit_stream_line(display)
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
            if self._config is None:
                mode = "off"
            else:
                mode = (self._config.execution.cursor_resume_mode or "off").strip().lower()
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
                data = read_json(path) or {}
            except json.JSONDecodeError:
                data = {}
            existing_chat_id = str(data.get("chat_id") or "").strip()
            if existing_chat_id:
                return existing_chat_id
        ensured_chat_id = self._ensure_cursor_chat_id()
        if not ensured_chat_id:
            return None
        path.parent.mkdir(parents=True, exist_ok=True)
        write_json(path, {"chat_id": ensured_chat_id})
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

    def _claude_command(self, task_id: str | None = None) -> list[str]:
        """Build Claude Code's headless command with a stable session identity.

        A fresh task run is pinned to a new UUID via ``--session-id`` so DevCouncil
        knows, up front, which native ``~/.claude`` transcript the run will write —
        making headless evidence and live review locatable without scraping stdout.
        A re-run of the SAME task (e.g. a repair iteration) instead ``--resume``\\s the
        prior session so the model keeps the context of what it already tried, rather
        than restarting cold from a re-prepended correction manifest. The chosen id is
        stashed in ``_pending_claude_session`` and persisted once the run launches."""
        base = list(self.spec.base_command())
        # Capture Claude's structured result either way. Non-stream: one JSON blob parsed
        # after exit. Stream: NDJSON events rendered live (readable lines, not raw JSON) and
        # the terminal result event harvested for telemetry. ``--verbose`` is required by
        # Claude for stream-json.
        if self.stream_output:
            base = [*base, "--output-format", "stream-json", "--verbose"]
        else:
            base = [*base, "--output-format", "json"]
        session_path = self._claude_session_path(task_id)
        prior = self._read_claude_session_id(session_path)
        if prior:
            self._pending_claude_session = (session_path, prior)
            return [*base, "--resume", prior]
        new_id = str(uuid.uuid4())
        self._pending_claude_session = (session_path, new_id)
        return [*base, "--session-id", new_id]

    def _claude_session_path(self, task_id: str | None) -> Path:
        name = f"{task_id}-claude.json" if task_id else "claude-session.json"
        return self.project_root / ".devcouncil" / "sessions" / name

    @staticmethod
    def _read_claude_session_id(path: Path) -> str | None:
        if not path.exists():
            return None
        try:
            data = read_json(path) or {}
        except json.JSONDecodeError:
            return None
        session_id = str(data.get("session_id") or "").strip()
        return session_id or None

    def _persist_claude_session(self) -> None:
        """Record the run's Claude session id so a later re-run of the task resumes it.

        Persisted after the subprocess launches (not on exit) so even a crashed or
        timed-out run leaves a resumable session behind. Also exposes the id via
        ``last_agent_session_id`` for the caller and the run manifest."""
        pending = self._pending_claude_session
        if not pending:
            return
        path, session_id = pending
        self.last_agent_session_id = session_id
        try:
            path.parent.mkdir(parents=True, exist_ok=True)
            write_json(path, {"session_id": session_id})
        except OSError:
            # A non-persisted session only costs a cold repair next time; never fail the run.
            pass

    def _mirror_claude_transcript(self) -> None:
        """Best-effort copy of Claude's native JSONL into the project for live review."""
        session_id = self.last_agent_session_id
        if not session_id:
            return
        try:
            from devcouncil.live.transcripts import mirror_claude_transcript

            mirror_claude_transcript(self.project_root, session_id)
        except Exception as exc:
            logger.debug("Claude transcript mirror failed: %s", exc)

    def _capture_claude_json(
        self, run_id: str, result: subprocess.CompletedProcess[str]
    ) -> subprocess.CompletedProcess[str]:
        """Parse Claude Code's ``--output-format json`` result blob.

        Records the reported session id, cost, and token usage on the run manifest, then
        returns a copy of ``result`` with stdout replaced by the human-readable ``result``
        text so downstream logs/previews/diagnostics read like the old text mode. On any
        parse failure the original ``result`` is returned unchanged — a telemetry miss must
        never turn a successful run into a reported failure."""
        raw = (result.stdout or "").strip()
        if not raw:
            return result
        payload: dict | None = None
        # Tolerate any leading noise: scan lines from the last back for a JSON object.
        for line in reversed(raw.splitlines()):
            line = line.strip()
            if not line:
                continue
            try:
                candidate = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(candidate, dict):
                payload = candidate
                break
        if payload is None:
            try:
                loaded = json.loads(raw)
            except json.JSONDecodeError:
                return result
            payload = loaded if isinstance(loaded, dict) else None
        if payload is None:
            return result

        self._record_claude_result_meta(run_id, payload)
        text = payload.get("result")
        if isinstance(text, str) and text.strip():
            return subprocess.CompletedProcess(
                result.args, result.returncode, stdout=text, stderr=result.stderr
            )
        return result

    def _record_claude_result_meta(self, run_id: str, payload: dict) -> None:
        """Record session id, cost, and token usage from a Claude ``result`` payload.

        Shared by the non-stream JSON path and the stream-json path; both surface the same
        terminal ``result`` object, just delivered as one blob vs. the last NDJSON line."""
        usage = payload.get("usage") if isinstance(payload.get("usage"), dict) else {}
        session_id = str(payload.get("session_id") or "").strip() or None
        if session_id:
            self.last_agent_session_id = session_id
            self._update_run_manifest(run_id, agent_session_id=session_id)
            # Keep the persisted resume pointer authoritative: if Claude reported a session
            # id different from the one we assigned, a later --resume must target the real
            # one, so rewrite the session file to what actually ran.
            pending = self._pending_claude_session
            if pending is not None and pending[1] != session_id:
                self._pending_claude_session = (pending[0], session_id)
                self._persist_claude_session()
        meta = {
            "session_id": session_id,
            "total_cost_usd": payload.get("total_cost_usd"),
            "num_turns": payload.get("num_turns"),
            "is_error": payload.get("is_error"),
            "input_tokens": usage.get("input_tokens"),
            "output_tokens": usage.get("output_tokens"),
            "cache_read_input_tokens": usage.get("cache_read_input_tokens"),
        }
        self._update_run_manifest(run_id, agent_result={k: v for k, v in meta.items() if v is not None})

    def _capture_claude_stream_json(self, run_id: str, result: subprocess.CompletedProcess[str]) -> None:
        """Record telemetry from a streamed NDJSON run by finding the terminal result event.

        The streaming path already rendered readable lines live; here we only harvest the
        last ``result`` event for the manifest. Best-effort — never raises into the run."""
        payload: dict | None = None
        for line in (result.stdout or "").splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(event, dict) and event.get("type") == "result":
                payload = event  # keep scanning; the last result event wins
        if payload is not None:
            self._record_claude_result_meta(run_id, payload)

    @staticmethod
    def _render_claude_stream_event(line: str) -> str | None:
        """Turn one Claude stream-json NDJSON line into a concise console line.

        Returns the text to display, or None to suppress the event (system/thinking/rate-
        limit noise). Non-JSON lines pass through verbatim so nothing is silently dropped."""
        stripped = line.strip()
        if not stripped:
            return None
        try:
            event = json.loads(stripped)
        except json.JSONDecodeError:
            return line
        if not isinstance(event, dict):
            return line
        etype = event.get("type")
        if etype == "assistant":
            message = event.get("message") if isinstance(event.get("message"), dict) else {}
            parts: list[str] = []
            for block in (message.get("content") or []):
                if not isinstance(block, dict):
                    continue
                btype = block.get("type")
                if btype == "text" and str(block.get("text") or "").strip():
                    parts.append(str(block["text"]).strip())
                elif btype == "tool_use":
                    name = str(block.get("name") or "tool")
                    tool_input = block.get("input") if isinstance(block.get("input"), dict) else {}
                    target = tool_input.get("file_path") or tool_input.get("path") or tool_input.get("command") or ""
                    parts.append(f"→ {name} {str(target)[:80]}".rstrip())
            text = "\n".join(p for p in parts if p)
            return text + "\n" if text else None
        if etype == "result":
            bits: list[str] = []
            if event.get("num_turns") is not None:
                bits.append(f"{event['num_turns']} turns")
            cost = event.get("total_cost_usd")
            if isinstance(cost, (int, float)):
                bits.append(f"${cost:.4f}")
            return ("✓ " + ", ".join(bits) + "\n") if bits else None
        # system / user / tool_result / rate_limit_event: keep the console quiet.
        return None

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
        extra = list(self.profile.extra_args) if (self.profile and self.profile.extra_args) else []

        def _place(base: list[str]) -> list[str]:
            """Insert profile extra_args after the base flags but before a trailing prompt
            flag (the last token of a baked-in prompt-flag CLI like warp ``--prompt`` /
            aider ``--message``), so that flag still binds to the prompt appended after it."""
            if extra and base and base[-1].startswith("-"):
                return [*base[:-1], *extra, base[-1]]
            return [*base, *extra]

        if mode == "stdin":
            return _place(resolved), prompt
        if mode == "argument":
            if any("{prompt}" in part for part in resolved):
                return [part.replace("{prompt}", prompt) for part in resolved] + extra, None
            prompt_arg = self.spec.prompt_arg
            if prompt_arg:
                return [*resolved, *extra, prompt_arg, prompt], None
            return [*_place(resolved), prompt], None
        if mode == "prompt-file":
            if "{prompt_file}" in " ".join(command):
                return resolved + extra, None
            prompt_arg = self.spec.prompt_arg
            if prompt_arg:
                return [*resolved, *extra, prompt_arg, str(instruction_file)], None
            return [*_place(resolved), str(instruction_file)], None
        return _place(resolved), prompt

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
            return {"extra_args": [], "permission_mode": None, "model": None, "env_keys": []}
        return {
            "extra_args": list(self.profile.extra_args or []),
            "permission_mode": self.profile.permission_mode,
            "model": self.profile.model,
            # KEY NAMES only — profile env values may carry provider tokens and must
            # never land in the manifest.
            "env_keys": sorted((self.profile.env or {}).keys()),
        }

    def _update_run_manifest(self, run_id: str, **updates: object) -> None:
        manifest_path = self.project_root / ".devcouncil" / "runs" / run_id / "agent-run.json"
        if not manifest_path.exists():
            return
        try:
            manifest = read_json(manifest_path) or {}
        except json.JSONDecodeError:
            return
        manifest.update(updates)
        write_json(manifest_path, manifest)

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
        write_json(manifest_path, manifest)
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
                existing = read_json(path) or {}
            except json.JSONDecodeError:
                existing = {}
            should_write = "mcpServers" in existing and "devcouncil" not in existing
        if should_write:
            write_json(path, desired)
        return path
