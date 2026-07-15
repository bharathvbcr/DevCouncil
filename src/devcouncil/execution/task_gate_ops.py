"""Task gate operations shared by CLI and MCP (verify, scope, evidence, policy, run)."""

from __future__ import annotations

import asyncio
import os
import shlex
import subprocess
from pathlib import Path
from typing import Any

from devcouncil.domain.evidence import CommandResult, DiffCoverageEvidence, DiffEvidence, TestEvidence
from devcouncil.execution.hook_policy import HookPolicy
from devcouncil.execution.lease_validation import require_valid_lease
from devcouncil.integrations.mcp.util import allowed_next_tools, read_log_file, truncate_text
from devcouncil.storage.native import ShellCommandRepository, TaskLeaseRepository
from devcouncil.storage.repositories import (
    EvidenceRepository,
    GapRepository,
    RequirementRepository,
    TaskRepository,
)
from devcouncil.verification.command_evidence import (
    command_has_acceptance_evidence,
    command_is_trivial_evidence,
)
from devcouncil.verification.next_actions import split_next_actions

RECORD_COMMAND_STATUSES = frozenset({"started", "finished", "failed", "blocked"})
CLI_TIMEOUT_SECONDS = 120


def _db(project_root: Path):
    from devcouncil.cli.commands.init import initialize_project
    from devcouncil.storage.db import get_db

    if not (project_root / ".devcouncil" / "state.sqlite").is_file():
        initialize_project(project_root, quiet=True)
    return get_db(project_root)


def _build_router(project_root: Path):
    try:
        from devcouncil.app.config import get_api_key, load_config
        from devcouncil.llm.provider import create_provider, validate_model_provider
        from devcouncil.llm.router import ModelRouter

        config = load_config(project_root)
        validate_model_provider(config.models.provider)
        api_key = get_api_key(config.models.provider, project_root)
        provider = create_provider(
            config.models.provider, api_key, project_root=project_root, provider_prefs=config.provider,
        )
        role_config = {name: role.model_dump() for name, role in config.models.roles.items()}
        return ModelRouter(provider, role_config, project_root=project_root)
    except Exception:
        return None


def verify_task_payload(
    project_root: Path,
    *,
    task_id: str,
    lease_token: str,
    sandbox: str = "local",
) -> dict[str, Any]:
    if sandbox in {"docker", "nix"}:
        return {
            "ok": False,
            "code": "unsupported_sandbox",
            "reason": f"Sandbox {sandbox} is not available in this build.",
            "sandbox": sandbox,
        }
    db = _db(project_root)
    if not db:
        return {"ok": False, "error": "DevCouncil state is unavailable in this directory.", "code": "not_initialized"}

    with db.get_session() as session:
        lease_error = require_valid_lease(session, task_id, lease_token)
        if lease_error:
            return lease_error
        task_repo = TaskRepository(session)
        task = task_repo.get_by_id(task_id)
        if not task:
            return {"ok": False, "error": f"Task {task_id} not found.", "code": "not_found", "task_id": task_id}
        from devcouncil.verification.verifier import Verifier

        GapRepository(session).delete_for_task(task_id)
        EvidenceRepository(session).delete_for_task(task_id)
        verifier = Verifier(project_root, router=_build_router(project_root))
        evidence_gaps, evidence = asyncio.run(
            verifier.verify_task(task, RequirementRepository(session).get_all()),
        )
        gaps = evidence_gaps
        for gap in gaps:
            GapRepository(session).save(gap)
        for ev in evidence:
            if isinstance(ev, CommandResult):
                EvidenceRepository(session).save_command_result(task_id, ev)
            elif isinstance(ev, DiffCoverageEvidence):
                EvidenceRepository(session).save_diff_coverage_evidence(ev)
            elif isinstance(ev, DiffEvidence):
                EvidenceRepository(session).save_diff_evidence(ev)
            elif isinstance(ev, TestEvidence):
                EvidenceRepository(session).save_test_evidence(ev, task_id)
        task.status = "blocked" if any(g.blocking for g in gaps) else "verified"
        task_repo.save(task)
        blocking = [g.model_dump() for g in gaps if g.blocking]
        blocking_actions, advisory_actions = split_next_actions(gaps)
        outcome = verifier.last_outcome
        return {
            "ok": True,
            "task_id": task_id,
            "status": task.status,
            "sandbox": sandbox,
            "blocking_gaps": blocking,
            "next_actions": [a.model_dump() for a in blocking_actions],
            "advisory_actions": [a.model_dump() for a in advisory_actions],
            "allowed_next_tools": allowed_next_tools(task.status, len(blocking) > 0),
            "passed": len(blocking) == 0,
            "verification_mode": outcome.mode if outcome else "unknown",
            "compiler_active": outcome.compiler_active if outcome else False,
            "diff_empty": outcome.diff_empty if outcome else False,
            "coverage_measured": outcome.coverage_measured if outcome else False,
            "coverage_skipped_reason": outcome.coverage_skipped_reason if outcome else None,
            "difficulty": outcome.difficulty if outcome else None,
            "rigor_applied": list(outcome.rigor_applied) if outcome else [],
        }


def attach_committed_range_payload(project_root: Path, *, task_id: str, lease_token: str, base: str, head: str = "HEAD") -> dict[str, Any]:
    """Attach an existing committed range to a leased task's checkpoint refs."""
    db = _db(project_root)
    if not db:
        return {"ok": False, "error": "DevCouncil state is unavailable in this directory.", "code": "not_initialized"}

    with db.get_session() as session:
        lease_error = require_valid_lease(session, task_id, lease_token)
        if lease_error:
            return lease_error
        if not TaskRepository(session).get_by_id(task_id):
            return {"ok": False, "error": f"Task {task_id} not found.", "code": "not_found", "task_id": task_id}

    try:
        base_sha = subprocess.check_output(["git", "rev-parse", "--verify", f"{base}^{{commit}}"], cwd=project_root, text=True, timeout=CLI_TIMEOUT_SECONDS).strip()
        head_sha = subprocess.check_output(["git", "rev-parse", "--verify", f"{head}^{{commit}}"], cwd=project_root, text=True, timeout=CLI_TIMEOUT_SECONDS).strip()
        subprocess.run(["git", "merge-base", "--is-ancestor", base_sha, head_sha], cwd=project_root, check=True, timeout=CLI_TIMEOUT_SECONDS)
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
        return {"ok": False, "code": "invalid_commit_range", "error": "--base and --head must resolve to an ancestor commit range.", "task_id": task_id}
    if base_sha == head_sha:
        return {"ok": False, "code": "invalid_commit_range", "error": "--base must be a strict ancestor of --head.", "task_id": task_id}

    refs = {"before": f"refs/devcouncil/tasks/{task_id}/before", "after": f"refs/devcouncil/tasks/{task_id}/after"}
    subprocess.run(["git", "update-ref", refs["before"], base_sha], cwd=project_root, check=True, timeout=CLI_TIMEOUT_SECONDS)
    subprocess.run(["git", "update-ref", refs["after"], head_sha], cwd=project_root, check=True, timeout=CLI_TIMEOUT_SECONDS)
    return {"ok": True, "task_id": task_id, "base": base_sha, "head": head_sha, "range": f"{base_sha}..{head_sha}", "refs": refs}


def update_task_scope_payload(
    project_root: Path,
    *,
    task_id: str,
    lease_token: str,
    expected_tests: list[str] | None = None,
    allowed_commands: list[str] | None = None,
    planned_files: list[str] | None = None,
) -> dict[str, Any]:
    db = _db(project_root)
    if not db:
        return {"ok": False, "error": "DevCouncil state is unavailable in this directory.", "code": "not_initialized"}

    expected_tests = expected_tests or []
    allowed_commands = allowed_commands or []
    planned_files = planned_files or []
    with db.get_session() as session:
        lease_error = require_valid_lease(session, task_id, lease_token)
        if lease_error:
            return lease_error
        task_repo = TaskRepository(session)
        task = task_repo.get_by_id(task_id)
        if not task:
            return {"ok": False, "error": f"Task {task_id} not found.", "code": "not_found", "task_id": task_id}
        rejected_commands: list[str] = []
        for cmd in allowed_commands:
            if cmd in task.allowed_commands:
                continue
            if command_is_trivial_evidence(cmd):
                rejected_commands.append(cmd)
                continue
            task.allowed_commands.append(cmd)
            if cmd not in task.agent_appended_allowed_commands:
                task.agent_appended_allowed_commands.append(cmd)
        rejected_tests: list[str] = []
        for test in expected_tests:
            if test in task.expected_tests:
                continue
            if not command_has_acceptance_evidence(test):
                rejected_tests.append(test)
                continue
            task.expected_tests.append(test)
            if test not in task.agent_appended_expected_tests:
                task.agent_appended_expected_tests.append(test)
        rejected_planned_files: list[str] = []
        if planned_files:
            from fnmatch import fnmatch

            from devcouncil.domain.task import PlannedFile
            from devcouncil.execution.policy_engine import (
                SECRET_PATH_PATTERNS,
                TaskPolicyEngine,
            )

            engine = TaskPolicyEngine(project_root)
            existing = {pf.path.replace("\\", "/") for pf in task.planned_files}
            for raw in planned_files:
                path = raw.replace("\\", "/")
                if path.startswith("./"):
                    path = path[2:]
                if not path or path in existing:
                    continue
                normalized = engine._normalize_path(path)
                if any(fnmatch(normalized, pat) for pat in SECRET_PATH_PATTERNS):
                    rejected_planned_files.append(raw)
                    continue
                if engine._matches_restricted(normalized):
                    rejected_planned_files.append(raw)
                    continue
                # Modify-op only: agents may append an existing caller to wire a new
                # file, not authorize creates/deletes through this surface.
                if not (project_root / normalized).is_file():
                    rejected_planned_files.append(raw)
                    continue
                task.planned_files.append(PlannedFile(
                    path=normalized,
                    reason="agent-appended scope (wire caller)",
                    allowed_change="modify",
                ))
                existing.add(normalized)
                if normalized not in task.agent_appended_planned_files:
                    task.agent_appended_planned_files.append(normalized)
        task_repo.save(task)
        return {
            "ok": True,
            "task_id": task_id,
            "allowed_commands": task.allowed_commands,
            "expected_tests": task.expected_tests,
            "planned_files": [pf.model_dump() for pf in task.planned_files],
            "agent_appended_planned_files": list(task.agent_appended_planned_files),
            "rejected_expected_tests": rejected_tests,
            "rejected_allowed_commands": rejected_commands,
            "rejected_planned_files": rejected_planned_files,
        }


def append_evidence_payload(
    project_root: Path,
    *,
    task_id: str,
    lease_token: str,
    command: str,
    summary: str,
    exit_code: int = 0,
) -> dict[str, Any]:
    db = _db(project_root)
    if not db:
        return {"ok": False, "error": "DevCouncil state is unavailable in this directory.", "code": "not_initialized"}

    with db.get_session() as session:
        lease_error = require_valid_lease(session, task_id, lease_token)
        if lease_error:
            return lease_error
        EvidenceRepository(session).save_command_result(
            task_id,
            CommandResult(
                command=command,
                exit_code=exit_code,
                stdout_path="",
                stderr_path="",
                summary=summary,
            ),
        )
        return {"ok": True, "task_id": task_id, "recorded": True}


def get_evidence_payload(
    project_root: Path,
    *,
    task_id: str,
    command_filter: str | None = None,
    limit: int = 20,
) -> dict[str, Any]:
    db = _db(project_root)
    if not db:
        return {"ok": False, "error": "DevCouncil state is unavailable in this directory.", "code": "not_initialized"}

    with db.get_session() as session:
        results = EvidenceRepository(session).get_command_results_for_task(task_id)
    evidence_rows: list[dict[str, object]] = []
    for result in results:
        if command_filter and command_filter not in result.command:
            continue
        stdout, stdout_truncated = truncate_text(read_log_file(result.stdout_path))
        stderr, stderr_truncated = truncate_text(read_log_file(result.stderr_path))
        evidence_rows.append({
            "command": result.command,
            "exit_code": result.exit_code,
            "summary": result.summary,
            "stdout": stdout,
            "stderr": stderr,
            "truncated": stdout_truncated or stderr_truncated,
        })
        if len(evidence_rows) >= limit:
            break
    return {"ok": True, "task_id": task_id, "evidence": evidence_rows}


def policy_check_write_payload(
    project_root: Path,
    *,
    path: str,
    task_id: str | None = None,
) -> dict[str, Any]:
    db = _db(project_root)
    if not db:
        return {"ok": False, "error": "DevCouncil state is unavailable in this directory.", "code": "not_initialized"}

    with db.get_session() as session:
        task_repo = TaskRepository(session)
        if task_id:
            task = task_repo.get_by_id(task_id)
        else:
            running = [task for task in task_repo.get_all() if task.status == "running"]
            task = running[0] if running else None
        decision = HookPolicy(project_root=project_root).evaluate_file_write(path, task)
        return {
            "action": decision.action,
            "allowed": decision.allowed,
            "reason": decision.reason,
            "target": decision.target,
            "task_id": task.id if task else None,
        }


def record_command_payload(
    project_root: Path,
    *,
    task_id: str,
    lease_token: str,
    command: str,
    status: str,
    exit_code: int | None = None,
    reason: str = "",
) -> dict[str, Any]:
    if status not in RECORD_COMMAND_STATUSES:
        return {
            "ok": False,
            "error": f"status must be one of {sorted(RECORD_COMMAND_STATUSES)}",
            "code": "invalid_arguments",
            "argument": "status",
        }
    db = _db(project_root)
    if not db:
        return {"ok": False, "error": "DevCouncil state is unavailable in this directory.", "code": "not_initialized"}

    with db.get_session() as session:
        lease_error = require_valid_lease(session, task_id, lease_token)
        if lease_error:
            return lease_error
        ShellCommandRepository(session).record(
            task_id,
            command,
            status,
            exit_code=exit_code if isinstance(exit_code, int) else None,
            reason=reason,
        )
        return {"ok": True, "task_id": task_id, "recorded": True}


def next_task_payload(
    project_root: Path,
    *,
    status_filter: str | None = None,
    client_id: str | None = None,
) -> dict[str, Any]:
    del client_id  # reserved for future fleet routing
    db = _db(project_root)
    if not db:
        return {"ok": False, "error": "DevCouncil state is unavailable in this directory.", "code": "not_initialized"}

    with db.get_session() as session:
        tasks = TaskRepository(session).get_all()
        leased_task_ids = {
            lease.task_id
            for lease, expired in TaskLeaseRepository(session).list_leases(active_only=True)
            if not expired
        }
        blocking_by_task: dict[str, int] = {}
        for gap in GapRepository(session).get_all():
            if gap.blocking and gap.task_id:
                blocking_by_task[gap.task_id] = blocking_by_task.get(gap.task_id, 0) + 1
    done_ids = {t.id for t in tasks if t.status in {"verified", "done"}}
    wanted_statuses = {status_filter} if status_filter else {"planned", "ready"}
    candidates = []
    for task in tasks:
        if task.status not in wanted_statuses:
            continue
        if task.id in leased_task_ids:
            continue
        if any(dep not in done_ids for dep in task.depends_on):
            continue
        candidates.append(task)
    if not candidates:
        return {
            "ok": True,
            "task": None,
            "reason": "No unblocked, unleased task is available.",
        }
    candidates.sort(key=lambda t: (len(t.depends_on), t.id))
    chosen = candidates[0]
    blocking_count = blocking_by_task.get(chosen.id, 0)
    return {
        "ok": True,
        "task": chosen.model_dump(),
        "blocking_gap_count": blocking_count,
        "ready_to_checkout": blocking_count == 0,
        "allowed_next_tools": allowed_next_tools(chosen.status, blocking_count > 0),
    }


def run_command_payload(
    project_root: Path,
    *,
    task_id: str,
    lease_token: str,
    command: str,
) -> dict[str, Any]:
    from devcouncil.utils.subprocess_env import clean_subprocess_env

    normalized = " ".join(command.split())
    db = _db(project_root)
    if not db:
        return {"ok": False, "error": "DevCouncil state is unavailable in this directory.", "code": "not_initialized"}

    with db.get_session() as session:
        lease_error = require_valid_lease(session, task_id, lease_token)
        if lease_error:
            return lease_error
        task = TaskRepository(session).get_by_id(task_id)
        if not task:
            return {"ok": False, "error": f"Task {task_id} not found.", "code": "not_found", "task_id": task_id}
        from devcouncil.execution.policy_engine import TaskPolicyEngine

        policy_decision = TaskPolicyEngine(project_root).evaluate_command(normalized, task)
        if policy_decision.action == "deny":
            ShellCommandRepository(session).record(
                task_id, normalized, "blocked", reason=policy_decision.reason,
            )
            return {
                "ok": False,
                "error": policy_decision.reason or "Command is not in the task allowlist.",
                "code": "command_not_allowed",
                "task_id": task_id,
                "command": normalized,
            }
        try:
            args = shlex.split(normalized, posix=(os.name != "nt"))
            completed = subprocess.run(
                args,
                cwd=project_root,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                env=clean_subprocess_env(),
                timeout=CLI_TIMEOUT_SECONDS,
            )
            exit_code = completed.returncode
            stdout, stdout_truncated = truncate_text(completed.stdout)
            stderr, stderr_truncated = truncate_text(completed.stderr)
            timed_out = False
        except subprocess.TimeoutExpired as exc:
            exit_code = None
            stdout, stdout_truncated = truncate_text(exc.output)
            stderr, stderr_truncated = truncate_text(exc.stderr)
            timed_out = True
        except (FileNotFoundError, OSError, ValueError) as exc:
            ShellCommandRepository(session).record(
                task_id, normalized, "failed", reason=str(exc),
            )
            return {
                "ok": False,
                "error": f"Could not run command: {exc}",
                "code": "run_failed",
                "task_id": task_id,
            }
        ShellCommandRepository(session).record(
            task_id,
            normalized,
            "finished" if exit_code == 0 else "failed",
            exit_code=exit_code,
        )
        return {
            "ok": exit_code == 0,
            "task_id": task_id,
            "exit_code": exit_code,
            "stdout": stdout,
            "stderr": stderr,
            "truncated": stdout_truncated or stderr_truncated,
            "timed_out": timed_out,
        }


def handoff_agent_payload(
    project_root: Path,
    *,
    task_id: str,
    lease_token: str,
    from_agent: str,
    to_agent: str,
    instruction: str = "",
) -> dict[str, Any]:
    db = _db(project_root)
    if not db:
        return {"ok": False, "error": "DevCouncil state is unavailable in this directory.", "code": "not_initialized"}

    with db.get_session() as session:
        lease_error = require_valid_lease(session, task_id, lease_token)
        if lease_error:
            return lease_error
    try:
        from devcouncil.execution.handoff import HandoffService

        manifest, handoff_path, run_id = HandoffService(project_root).create(
            task_id,
            from_agent,
            to_agent,
            instruction=instruction,
        )
        return {
            "ok": True,
            "task_id": task_id,
            "manifest_path": str(handoff_path),
            "run_id": run_id,
            "manifest": manifest.model_dump(),
        }
    except ValueError as exc:
        return {"ok": False, "error": str(exc), "code": "handoff_failed", "task_id": task_id}
