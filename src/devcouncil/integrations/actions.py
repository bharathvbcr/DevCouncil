from __future__ import annotations

import json
import logging
import shutil
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from devcouncil.integrations.check import build_integration_check_report
from devcouncil.executors.agent_registry import GEMINI_DEPRECATION_MESSAGE

logger = logging.getLogger(__name__)

VALID_INTEGRATION_TARGETS = {
    "all",
    "hooks",
    "codex",
    "gemini",
    "claude",
    "cursor",
    "grok",
    "opencode",
    "antigravity",
    "agy",
    "warp",
    "aider",
}

TARGET_ALIASES = {
    "agy": "antigravity",
}


@dataclass(frozen=True)
class IntegrationActionReport:
    target: str
    ok: bool
    results: list[dict[str, Any]]
    warnings: list[str]
    check: dict[str, Any]

    def as_dict(self) -> dict[str, Any]:
        return {
            "target": self.target,
            "ok": self.ok,
            "results": self.results,
            "warnings": self.warnings,
            "check": self.check,
        }

    def to_json(self, *, indent: int | None = 2) -> str:
        return json.dumps(self.as_dict(), indent=indent)


def normalize_apply_target(target: str) -> str:
    normalized = (target or "").strip().lower().replace("_", "-")
    normalized = TARGET_ALIASES.get(normalized, normalized)
    if normalized not in VALID_INTEGRATION_TARGETS:
        allowed = ", ".join(sorted(VALID_INTEGRATION_TARGETS))
        raise ValueError(f"Unsupported integration target '{target}'. Expected one of: {allowed}.")
    return normalized


def apply_integration_target(
    project_root: Path,
    target: str,
    *,
    include_hooks: bool = True,
    strict: bool = False,
    gemini_scope: str = "project",
    claude_scope: str = "local",
    claude_write_gate: bool = False,
) -> IntegrationActionReport:
    from devcouncil.cli.commands import integrate

    root = project_root.expanduser().resolve()
    normalized = normalize_apply_target(target)
    logger.info("Applying integration target: %s (root=%s)", normalized, root)
    warnings: list[str] = []
    results: list[dict[str, Any]] = []

    def add_result(name: str, ok: bool, path: Path | None = None, message: str = "") -> None:
        (logger.info if ok else logger.warning)("Integration %s: %s — %s", name, "ok" if ok else "FAILED", message)
        results.append({
            "target": name,
            "ok": ok,
            "path": str(path.relative_to(root)) if path and path.is_relative_to(root) else (str(path) if path else ""),
            "message": message,
        })

    def apply_first_party(name: str) -> None:
        command_builders = {
            "codex": integrate._codex_command,
            "gemini": lambda project: integrate._gemini_command(project, gemini_scope),
            "claude": lambda project: integrate._claude_command(project, claude_scope),
        }
        command = command_builders[name](root)
        if not shutil.which(command[0]):
            warning = f"{name} CLI not found on PATH; skipped CLI MCP registration."
            warnings.append(warning)
            integrate.console.print(f"[yellow]{name} CLI not found on PATH. Skipping optional integration.[/yellow]")
            add_result(name, True, None, "CLI not found; skipped optional global/client registration.")
            return
        code = integrate._run(command)
        add_result(name, code == 0, None, "MCP registration command exited " + str(code))

    def apply_project_file(name: str) -> None:
        if name == "grok":
            ok = integrate._configure_grok(root, apply=True)
            path = integrate._grok_config_path(root)
            add_result(name, ok, path if path.exists() else None, "Grok MCP integration configured." if ok else "Grok MCP integration failed.")
            return
        writers = {
            "cursor": integrate._write_cursor_config,
            "opencode": integrate._write_opencode_config,
            "antigravity": integrate._write_antigravity_mcp_config,
            "warp": integrate._write_warp_mcp_config,
        }
        recorders = {
            "cursor": integrate._record_cursor_config,
            "opencode": integrate._record_opencode_config,
            "antigravity": integrate._record_antigravity_config,
            "warp": integrate._record_warp_config,
        }
        path = writers[name](root)
        recorders[name](root)
        add_result(name, True, path, "Project integration config written.")

    def apply_aider() -> None:
        integrate._record_aider_config(root)
        add_result("aider", True, root / ".devcouncil" / "config.yaml", "Aider executor enabled.")

    def apply_hooks() -> None:
        integrate._configure_native_hooks(root, "all", apply=True, claude_write_gate=claude_write_gate)
        integrate._install_git_map_hooks(root, apply=True)
        add_result("hooks", True, None, "Native hook files configured (incl. git map --if-stale).")

    def apply_claude_assets() -> None:
        try:
            written = integrate._install_claude_assets(root)
        except (ValueError, FileNotFoundError, OSError) as exc:
            add_result("claude-assets", False, None, f"Claude asset setup failed: {exc}")
            return
        integrate._record_claude_config(
            root,
            scope=claude_scope,
            write_gate=claude_write_gate,
        )
        add_result("claude-assets", True, None, f"Claude assets installed ({len(written)} file(s)).")

    if normalized == "all":
        for name in ("claude", "codex"):
            apply_first_party(name)
        # Batch the per-tool config.yaml record updates (project files, aider,
        # and native hooks) into one load/save cycle. _batched_raw_config is
        # re-entrant, so apply_hooks()'s own batching participates in this one.
        with integrate._batched_raw_config(root):
            for name in ("cursor", "grok", "opencode", "antigravity", "warp"):
                apply_project_file(name)
            apply_aider()
            if include_hooks:
                apply_hooks()
            # The static Claude Code asset surface (slash commands, subagents, output
            # style, statusline, permissions, skills) — installed regardless of whether
            # the claude CLI is on PATH, since these are plain files.
            apply_claude_assets()
    elif normalized in {"codex", "gemini", "claude"}:
        if normalized == "gemini":
            warnings.append(GEMINI_DEPRECATION_MESSAGE)
            integrate.console.print(f"[yellow]{GEMINI_DEPRECATION_MESSAGE}[/yellow]")
        apply_first_party(normalized)
        if include_hooks and normalized == "codex":
            integrate._configure_native_hooks(root, "codex", apply=True)
            add_result("codex-hooks", True, root / ".codex" / "hooks.json", "Codex native hooks configured; review trust with /hooks.")
        elif include_hooks and normalized == "claude":
            integrate._install_claude_hooks(root, write_gate=claude_write_gate)
            apply_claude_assets()
            add_result("claude-hooks", True, root / ".claude" / "settings.local.json", "Claude native hooks configured.")
    elif normalized in {"cursor", "grok", "opencode", "antigravity", "warp"}:
        apply_project_file(normalized)
    elif normalized == "aider":
        apply_aider()
    elif normalized == "hooks":
        apply_hooks()

    check = build_integration_check_report(root, strict=strict).as_dict()
    ok = all(item["ok"] for item in results) and (not strict or bool(check["ok"]))
    return IntegrationActionReport(normalized, ok, results, warnings, check)
